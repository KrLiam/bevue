//! `<query>` props: lists of the entities matching a query, with their registered components.
//!
//! ```html
//! <props>
//!   <query name="enemies" data="Name, Health, Target?" with="Enemy" without="Dead" order-by="health.current desc"/>
//! </props>
//! <div v-for="e in enemies" :key="e.entity">{{ e.name }}: {{ e.health.current }}</div>
//! ```
//!
//! Each row is a struct holding `entity` and the components in `data`, by their `Props::name`.
//! Rows are kept up to date from Bevy's change ticks: each frame, only the components of matching
//! entities that changed since the last sync are diffed into their row, and only the parts the
//! template reads (see [`ReadMask`]). Entities that start matching are appended (or inserted at
//! their sorted position), entities that stop matching are removed.

use std::{cmp::Ordering, sync::Mutex};

use bevy::{
    ecs::{
        change_detection::{DetectChangesMut, Tick},
        component::Component,
        entity::{Entity, EntityHashMap, EntityHashSet},
        query::{QueryBuilder, QueryState, With},
        world::{FilteredEntityRef, World},
    },
    log::warn,
};

use crate::{
    core::{
        mask::ReadMask,
        ops,
        path::{PathSegment, PropPath},
        template::{QueryDeclaration, SortKey},
        value::Value,
    },
    runtime::{
        props::{BevueProps, PropsReadMask, PropsRegistration, PropsTypeRegistry},
        render::{restore_props, take_props},
    },
};

/// The field names of rows: `entity` followed by the component names. Interned, so rows of
/// queries with the same components share them and remounting a query does not leak.
fn row_schema(names: Vec<&'static str>) -> &'static [&'static str] {
    static SCHEMAS: Mutex<Vec<&'static [&'static str]>> = Mutex::new(Vec::new());
    let mut schemas = SCHEMAS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(schema) = schemas.iter().find(|schema| **schema == names.as_slice()) {
        return schema;
    }
    let schema: &'static [&'static str] = Box::leak(names.into_boxed_slice());
    schemas.push(schema);
    schema
}

/// A `<query>` mounted on a template entity.
pub struct MountedQuery {
    key: PropPath,
    /// The field names of a row.
    names: &'static [&'static str],
    /// The components of a row, and whether each is optional.
    data: Vec<(PropsRegistration, bool)>,
    state: QueryState<FilteredEntityRef<'static, 'static>>,
    order_by: Vec<SortKey>,
    /// The entity of each row, in list order.
    rows: Vec<Entity>,
    last_run: Tick,
}

/// The queries the template rendered on this entity declares, replaced on each render.
#[derive(Component, Default)]
pub struct TemplateQueries(Vec<MountedQuery>);

impl MountedQuery {
    /// Builds the query of `declaration`. Returns `None`, with a warning, if a type is not
    /// registered with `register_props`.
    fn new(world: &mut World, entity: Entity, declaration: &QueryDeclaration) -> Option<Self> {
        let key = match PropPath::parse(&declaration.name) {
            Ok(key) => key,
            Err(error) => {
                warn!("cannot mount query '{}' on {entity}: {error}", declaration.name);
                return None;
            }
        };
        let registry = world.get_resource::<PropsTypeRegistry>();
        let lookup = |type_path: &str| {
            let registration = registry.and_then(|r| r.get(type_path)).copied();
            if registration.is_none() {
                warn!(
                    "query '{}' on {entity} uses unknown type '{type_path}'; register it with `app.register_props`",
                    declaration.name
                );
            }
            registration
        };
        let data: Vec<(PropsRegistration, bool)> = declaration
            .data
            .iter()
            .map(|c| lookup(&c.type_path).map(|r| (r, c.optional)))
            .collect::<Option<_>>()?;
        let with: Vec<PropsRegistration> = declaration.with.iter().map(|t| lookup(t)).collect::<Option<_>>()?;
        let without: Vec<PropsRegistration> = declaration.without.iter().map(|t| lookup(t)).collect::<Option<_>>()?;

        let mut builder = QueryBuilder::<FilteredEntityRef>::new(world);
        for (registration, optional) in &data {
            let id = registration.component_id;
            if *optional {
                builder.optional(|b| {
                    b.ref_id(id);
                });
            } else {
                builder.ref_id(id);
            }
        }
        for registration in &with {
            builder.with_id(registration.component_id);
        }
        for registration in &without {
            builder.without_id(registration.component_id);
        }
        let state = builder.build();

        let names = row_schema(std::iter::once("entity").chain(data.iter().map(|(r, _)| r.name)).collect());
        Some(Self {
            key,
            names,
            data,
            state,
            order_by: declaration.order_by.clone(),
            rows: Vec::new(),
            last_run: Tick::default(),
        })
    }

    /// Updates the rows in `props` from the components changed since the last sync. `mask` is
    /// what the template reads of the whole props; without it, everything is synced.
    fn sync(&mut self, world: &World, props: &mut Value, mask: Option<&ReadMask>, this_run: Tick) {
        let last_run = std::mem::replace(&mut self.last_run, this_run);
        // What the template reads of a row.
        let row_mask = match mask {
            None => Some(ReadMask::all_ref()),
            Some(mask) => match mask.at(&self.key) {
                Some(list) => list.elements(),
                // The template does not read this query.
                None => return,
            },
        };
        let MountedQuery { key, names, data, state, order_by, rows, .. } = self;
        let field_masks: Vec<Option<&ReadMask>> =
            data.iter().map(|(registration, _)| row_mask.and_then(|m| m.field(registration.name))).collect();
        let row_value = |entity_ref: &FilteredEntityRef| {
            let mut fields = Vec::with_capacity(names.len());
            fields.push(Value::entity(entity_ref.id()));
            for ((registration, _), mask) in data.iter().zip(&field_masks) {
                let value = match (mask, (registration.read_filtered)(entity_ref)) {
                    (Some(mask), Some(component)) => component.to_value_masked(mask),
                    _ => Value::null(),
                };
                fields.push(value);
            }
            Value::structure(names, fields)
        };

        props.update_at(key, |list| {
            if list.as_list().is_none() {
                list.set(Value::list(Vec::new()));
                rows.clear();
            }
            let index: EntityHashMap<usize> = rows.iter().enumerate().map(|(i, e)| (*e, i)).collect();
            let mut seen = EntityHashSet::default();
            let mut added = Vec::new();
            let mut rows_changed = false;

            for entity_ref in state.iter(world) {
                let entity = entity_ref.id();
                seen.insert(entity);
                let Some(&row) = index.get(&entity) else {
                    added.push((entity, row_value(&entity_ref)));
                    continue;
                };
                for (field, ((registration, _), mask)) in data.iter().zip(&field_masks).enumerate() {
                    let Some(mask) = mask else { continue };
                    let ticks = entity_ref.get_change_ticks_by_id(registration.component_id);
                    if ticks.is_some_and(|t| !t.is_changed(last_run, this_run)) {
                        continue;
                    }
                    // Changed, inserted, or a missing optional component.
                    let component = (registration.read_filtered)(&entity_ref);
                    list.update_at(&[PathSegment::Index(row)], |row| {
                        row.update_field(field + 1, |value| match component {
                            Some(component) => component.sync_value_masked(value, mask),
                            None => {
                                value.set(Value::null());
                            }
                        });
                        rows_changed |= row.is_dirty();
                    });
                }
            }

            // Entities that stopped matching.
            if seen.len() < rows.len() + added.len() {
                for i in (0..rows.len()).rev() {
                    if !seen.contains(&rows[i]) {
                        list.remove_at(i);
                        rows.remove(i);
                    }
                }
            }

            // Entities that started matching.
            for (entity, value) in added {
                let position = if order_by.is_empty() {
                    rows.len()
                } else {
                    let elements = list.as_list().unwrap();
                    elements.partition_point(|row| compare_rows(row, &value, order_by) != Ordering::Greater)
                };
                list.insert_at(position, value);
                rows.insert(position, entity);
            }

            // Rows whose sort keys changed move to their new position.
            if rows_changed && !order_by.is_empty() {
                let elements = list.as_list().unwrap();
                let sorted = elements.windows(2).all(|w| compare_rows(&w[0], &w[1], order_by) != Ordering::Greater);
                if !sorted {
                    let mut order: Vec<usize> = (0..elements.len()).collect();
                    order.sort_by(|a, b| compare_rows(&elements[*a], &elements[*b], order_by));
                    list.permute(&order);
                    let old = std::mem::take(rows);
                    *rows = order.iter().map(|i| old[*i]).collect();
                }
            }
        });
    }
}

/// Orders rows by `keys`. Numbers and strings are ordered by each key's direction; anything else
/// (null, mismatched types) comes last.
fn compare_rows(a: &Value, b: &Value, keys: &[SortKey]) -> Ordering {
    static NULL: Value = Value::NULL;
    for key in keys {
        let (x, y) = (a.at(&key.path).unwrap_or(&NULL), b.at(&key.path).unwrap_or(&NULL));
        let ordered = |v: &Value| ops::compare(v, v).is_some();
        let ordering = match ops::compare(x, y) {
            Some(ordering) if key.descending => ordering.reverse(),
            Some(ordering) => ordering,
            None => ordered(y).cmp(&ordered(x)),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Mounts the queries a template declares on its entity, replacing the ones of its previous
/// render, and fills their lists right away so the render shows them.
pub(crate) fn mount_queries(world: &mut World, entity: Entity, declarations: &[QueryDeclaration]) {
    if declarations.is_empty() {
        if let Ok(mut target) = world.get_entity_mut(entity) {
            target.remove::<TemplateQueries>();
        }
        return;
    }
    let mut mounted: Vec<MountedQuery> =
        declarations.iter().filter_map(|declaration| MountedQuery::new(world, entity, declaration)).collect();

    let Ok(mut target) = world.get_entity_mut(entity) else { return };
    target.insert_if_new(BevueProps::default());
    let mask = world.get::<PropsReadMask>(entity).map(|m| m.0.clone());
    let mut props = take_props(world, entity);
    // Systems running later stamp their changes with a newer tick than this one.
    let this_run = world.increment_change_tick();
    for query in &mut mounted {
        props.update_at(&query.key, |list| {
            list.set(Value::list(Vec::new()));
        });
        query.sync(world, &mut props, mask.as_deref(), this_run);
    }
    restore_props(world, entity, props);
    if let Ok(mut target) = world.get_entity_mut(entity) {
        target.insert(TemplateQueries(mounted));
    }
}

/// Syncs the rows of every mounted query into the props of its template.
pub fn sync_queries(world: &mut World, targets: &mut QueryState<Entity, With<TemplateQueries>>) {
    let entities: Vec<Entity> = targets.iter(world).collect();
    if entities.is_empty() {
        return;
    }
    let this_run = world.increment_change_tick();
    for entity in entities {
        let Some(mut queries) = world
            .get_mut::<TemplateQueries>(entity)
            .map(|mut queries| std::mem::take(&mut queries.bypass_change_detection().0))
        else {
            continue;
        };
        let mask = world.get::<PropsReadMask>(entity).map(|m| m.0.clone());
        let mut props = take_props(world, entity);
        for query in &mut queries {
            query.sync(world, &mut props, mask.as_deref(), this_run);
        }
        let dirty = props.is_dirty();
        restore_props(world, entity, props);
        if let Ok(mut target) = world.get_entity_mut(entity) {
            if dirty && let Some(mut props) = target.get_mut::<BevueProps>() {
                props.set_changed();
            }
            if let Some(mut slot) = target.get_mut::<TemplateQueries>() {
                slot.bypass_change_detection().0 = queries;
            }
        }
    }
}
