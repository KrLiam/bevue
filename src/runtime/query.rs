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
        component::{Component, ComponentId},
        entity::{Entity, EntityHashMap},
        event::EntityEvent,
        lifecycle::{Add, Despawn, Remove},
        observer::{Observer, On},
        query::{QueryBuilder, QueryState, With},
        resource::Resource,
        system::ResMut,
        world::{FilteredEntityRef, World},
    },
    log::warn,
    platform::collections::HashSet,
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
        props::{BevueProps, ChangedEntities, PropsReadMask, PropsRegistration, PropsTypeRegistry},
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

/// Counts the times a component used by a query was added to or removed from an entity, which is
/// the only way an entity can start or stop matching a query (spawning and despawning included).
/// Queries only look for entities that started or stopped matching when it changed.
#[derive(Resource, Default)]
pub struct QueryMembership {
    generation: u64,
    /// The components observed.
    watched: HashSet<ComponentId>,
}

fn bump_membership<E: EntityEvent>(_: On<E>, mut membership: ResMut<QueryMembership>) {
    membership.generation += 1;
}

/// Observes additions and removals of `components`, if not already.
fn watch_components(world: &mut World, components: impl IntoIterator<Item = ComponentId>) {
    let new: Vec<ComponentId> = {
        let mut membership = world.get_resource_or_init::<QueryMembership>();
        components.into_iter().filter(|id| membership.watched.insert(*id)).collect()
    };
    if new.is_empty() {
        return;
    }
    world.spawn(Observer::new(bump_membership::<Add>).with_components(new.iter().copied()));
    world.spawn(Observer::new(bump_membership::<Remove>).with_components(new.iter().copied()));
    world.spawn(Observer::new(bump_membership::<Despawn>).with_components(new.iter().copied()));
}

/// What one sync of a query did, for logs.
#[derive(Debug, Default, Clone, Copy)]
#[cfg_attr(not(feature = "debug"), allow(dead_code))]
struct QuerySyncStats {
    /// Whether every matching entity was visited, to find the ones that started or stopped
    /// matching, instead of only those with changed components.
    full: bool,
    rows: usize,
    changed: usize,
    added: usize,
    removed: usize,
}

/// A `<query>` mounted on a template entity.
pub struct MountedQuery {
    key: PropPath,
    /// The field names of a row.
    names: &'static [&'static str],
    /// The components of a row, and whether each is optional.
    data: Vec<(PropsRegistration, bool)>,
    /// For each component in `data`, finds the entities where it changed.
    changed_queries: Vec<Box<dyn ChangedEntities>>,
    /// Reused buffer of entities whose components changed.
    candidates: Vec<Entity>,
    state: QueryState<FilteredEntityRef<'static, 'static>>,
    order_by: Vec<SortKey>,
    /// The entity of each row, in list order.
    rows: Vec<Entity>,
    /// The row of each entity.
    index: EntityHashMap<usize>,
    /// For each row, the last sync that saw its entity match, to find the ones that stopped.
    stamps: Vec<u32>,
    stamp: u32,
    last_run: Tick,
    /// The [`QueryMembership`] generation of the last sync that looked for entities starting or
    /// stopping to match. `None` to look on the next sync.
    membership: Option<u64>,
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
        let components = data.iter().map(|(r, _)| r).chain(&with).chain(&without).map(|r| r.component_id);
        watch_components(world, components.collect::<Vec<_>>());

        let names = row_schema(std::iter::once("entity").chain(data.iter().map(|(r, _)| r.name)).collect());
        let changed_queries = data.iter().map(|(r, _)| (r.changed_query)(world)).collect();
        Some(Self {
            key,
            names,
            data,
            changed_queries,
            candidates: Vec::new(),
            state,
            order_by: declaration.order_by.clone(),
            rows: Vec::new(),
            index: EntityHashMap::default(),
            stamps: Vec::new(),
            stamp: 0,
            last_run: Tick::default(),
            membership: None,
        })
    }

    /// Updates the rows in `props` from the components changed since the last sync. `mask` is
    /// what the template reads of the whole props; without it, everything is synced.
    ///
    /// Unless the [`QueryMembership`] generation changed since the last full pass, no entity can
    /// have started or stopped matching: only entities with changed components are looked at.
    /// Returns what it did, or `None` if the template does not read the query.
    fn sync(&mut self, world: &World, props: &mut Value, mask: Option<&ReadMask>, this_run: Tick) -> Option<QuerySyncStats> {
        let last_run = std::mem::replace(&mut self.last_run, this_run);
        let generation = world.get_resource::<QueryMembership>().map_or(0, |m| m.generation);
        let full = self.membership != Some(generation);
        self.membership = Some(generation);
        // What the template reads of a row.
        let row_mask = match mask {
            None => Some(ReadMask::all_ref()),
            // `None` if the template does not read this query.
            Some(mask) => mask.at(&self.key)?.elements(),
        };
        let MountedQuery {
            key,
            names,
            data,
            changed_queries,
            candidates,
            state,
            order_by,
            rows,
            index,
            stamps,
            stamp,
            membership,
            ..
        } = self;
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

        let mut stats = QuerySyncStats { full, ..Default::default() };
        props.update_at(key, |list| {
            if list.as_list().is_none() {
                list.set(Value::list(Vec::new()));
                rows.clear();
                stamps.clear();
                index.clear();
            }
            *stamp = stamp.wrapping_add(1).max(1);
            let stamp = *stamp;
            let mut seen = 0;
            let mut added = Vec::new();
            let mut changed_rows = Vec::new();

            // Diffs the changed components of `entity_ref` into `row`. Returns whether it changed.
            let sync_row = |list: &mut Value, row: usize, entity_ref: &FilteredEntityRef| {
                let mut row_changed = false;
                for (field, ((registration, _), mask)) in data.iter().zip(&field_masks).enumerate() {
                    let Some(mask) = mask else { continue };
                    let ticks = entity_ref.get_change_ticks_by_id(registration.component_id);
                    if ticks.is_some_and(|t| !t.is_changed(last_run, this_run)) {
                        continue;
                    }
                    // A missing optional component was removed, which changes the membership
                    // generation: the full pass sets it to null.
                    if ticks.is_none() && !full {
                        continue;
                    }
                    // Changed, inserted, or a missing optional component.
                    let component = (registration.read_filtered)(entity_ref);
                    list.update_at(&[PathSegment::Index(row)], |row| {
                        row.update_field(field + 1, |value| match component {
                            Some(component) => component.sync_value_masked(value, mask),
                            None => {
                                value.set(Value::null());
                            }
                        });
                        row_changed |= row.is_dirty();
                    });
                }
                row_changed
            };

            if full {
                for entity_ref in state.iter(world) {
                    let entity = entity_ref.id();
                    let Some(&row) = index.get(&entity) else {
                        added.push((entity, row_value(&entity_ref)));
                        continue;
                    };
                    stamps[row] = stamp;
                    seen += 1;
                    if sync_row(list, row, &entity_ref) {
                        changed_rows.push(row);
                    }
                }
            } else {
                // No entity started or stopped matching: only visit the entities whose read
                // components changed, found by scanning change-tick columns.
                candidates.clear();
                for (changed, mask) in changed_queries.iter_mut().zip(&field_masks) {
                    if mask.is_some() {
                        changed.changed(world, candidates);
                    }
                }
                candidates.sort_unstable();
                candidates.dedup();
                for &entity in candidates.iter() {
                    // Entities with the component that are not in this query have no row.
                    let Some(&row) = index.get(&entity) else { continue };
                    let Ok(entity_ref) = state.get(world, entity) else {
                        // Cannot happen without a membership change; look on the next sync.
                        *membership = None;
                        continue;
                    };
                    if sync_row(list, row, &entity_ref) {
                        changed_rows.push(row);
                    }
                }
            }

            stats.changed = changed_rows.len();
            stats.added = added.len();

            // Rows whose sort keys changed move to their new position, before rows are removed or
            // added: new rows are inserted by binary search, which needs a sorted list.
            if !order_by.is_empty() && !changed_rows.is_empty() {
                let mut rows = Rows { list, entities: rows, stamps, index };
                rows.restore_order(&mut changed_rows, order_by, stamp);
            }

            // Entities that stopped matching.
            if full && seen < rows.len() {
                stats.removed = rows.len() - seen;
                let first = stamps.iter().position(|s| *s != stamp).unwrap_or(rows.len());
                for (entity, row_stamp) in rows.iter().zip(stamps.iter()) {
                    if *row_stamp != stamp {
                        index.remove(entity);
                    }
                }
                list.retain(|i, _| stamps[i] == stamp);
                let mut i = 0;
                rows.retain(|_| {
                    i += 1;
                    stamps[i - 1] == stamp
                });
                stamps.retain(|s| *s == stamp);
                reindex(rows, index, first..rows.len());
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
                stamps.insert(position, stamp);
                reindex(rows, index, position..rows.len());
            }
            stats.rows = rows.len();
        });
        Some(stats)
    }
}

/// The rows of a query being synced: the list value and, for each row, its entity and stamp,
/// with the index from entities to rows.
struct Rows<'a> {
    list: &'a mut Value,
    entities: &'a mut Vec<Entity>,
    stamps: &'a mut Vec<u32>,
    index: &'a mut EntityHashMap<usize>,
}

impl Rows<'_> {
    /// Moves the rows at `changed`, whose data changed, back to their sorted position. The other
    /// rows are still sorted, so only the pairs around changed rows can be out of order.
    fn restore_order(&mut self, changed: &mut Vec<usize>, keys: &[SortKey], stamp: u32) {
        let elements = self.list.as_list().unwrap();
        let len = elements.len();
        let ordered = |a: usize, b: usize| compare_rows(&elements[a], &elements[b], keys) != Ordering::Greater;
        if changed.iter().all(|&i| (i == 0 || ordered(i - 1, i)) && (i + 1 >= len || ordered(i, i + 1))) {
            return;
        }
        changed.sort_unstable();
        changed.dedup();

        if let [from] = changed[..] {
            // Without the row the list is sorted: find its place on the side it belongs to.
            let row = &elements[from];
            let before = |r: &Value| compare_rows(r, row, keys) != Ordering::Greater;
            let to = if from > 0 && !ordered(from - 1, from) {
                elements[..from].partition_point(before)
            } else {
                from + elements[from + 1..].partition_point(before)
            };
            self.list.move_element(from, to);
            let range = from.min(to)..=from.max(to);
            if from < to {
                self.entities[range.clone()].rotate_left(1);
                self.stamps[range.clone()].rotate_left(1);
            } else {
                self.entities[range.clone()].rotate_right(1);
                self.stamps[range.clone()].rotate_right(1);
            }
            reindex(self.entities, self.index, range);
            return;
        }

        if changed.len() > (len / 16).max(8) {
            // Many rows changed: sort them all, keeping the current order between equal rows.
            let mut order: Vec<usize> = (0..len).collect();
            order.sort_by(|a, b| compare_rows(&elements[*a], &elements[*b], keys));
            self.list.permute(&order);
            let (entities, stamps) = (std::mem::take(self.entities), std::mem::take(self.stamps));
            *self.entities = order.iter().map(|i| entities[*i]).collect();
            *self.stamps = order.iter().map(|i| stamps[*i]).collect();
            reindex(self.entities, self.index, 0..len);
            return;
        }

        // A few rows changed: take them out, then insert each at its place.
        let mut moved = Vec::with_capacity(changed.len());
        for &i in changed.iter().rev() {
            moved.push((self.list.remove_at(i).unwrap(), self.entities.remove(i)));
            self.stamps.remove(i);
        }
        let (mut low, mut high) = (changed[0], changed[changed.len() - 1]);
        for (value, entity) in moved {
            let elements = self.list.as_list().unwrap();
            let position = elements.partition_point(|r| compare_rows(r, &value, keys) != Ordering::Greater);
            self.list.insert_at(position, value);
            self.entities.insert(position, entity);
            self.stamps.insert(position, stamp);
            low = low.min(position);
            high = high.max(position);
        }
        // Rows past `high + moved rows` were shifted down and back up by the same amount.
        let end = (high + changed.len() + 1).min(self.entities.len());
        reindex(self.entities, self.index, low..end);
    }
}

/// Updates the index of the rows in `range`.
fn reindex(rows: &[Entity], index: &mut EntityHashMap<usize>, range: impl IntoIterator<Item = usize>) {
    for i in range {
        index.insert(rows[i], i);
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

/// Logs a query sync at `debug` when rows changed, were added or removed, and at `trace` otherwise
/// (queries are synced every frame), like the sync of `Props` components and resources. Only with
/// the `debug` feature.
#[cfg(feature = "debug")]
fn log_sync(key: &PropPath, entity: Entity, stats: QuerySyncStats, start: bevy::platform::time::Instant) {
    use bevy::log::{debug, trace};
    let elapsed = start.elapsed();
    let QuerySyncStats { full, rows, changed, added, removed } = stats;
    let pass = if full { "full" } else { "changed rows only" };
    if changed + added + removed > 0 {
        debug!(
            "synced query '{key}' into the template on {entity}: {rows} rows, {changed} changed, {added} added, \
             {removed} removed ({pass}), in {elapsed:?}"
        );
    } else {
        trace!("synced query '{key}' into the template on {entity}: {rows} rows, nothing changed ({pass}), in {elapsed:?}");
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
            #[cfg(feature = "debug")]
            let start = bevy::platform::time::Instant::now();
            let _stats = query.sync(world, &mut props, mask.as_deref(), this_run);
            #[cfg(feature = "debug")]
            if let Some(stats) = _stats {
                log_sync(&query.key, entity, stats, start);
            }
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
