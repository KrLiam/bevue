//! The `BevueProps` component holding the data a template renders.

use std::marker::PhantomData;

use bevy::{
    app::{App, PostUpdate},
    ecs::{
        change_detection::{DetectChanges, DetectChangesMut, Mut, Ref},
        component::Component,
        entity::Entity,
        query::With,
        resource::{IsResource, Resource},
        schedule::IntoScheduleConfigs,
        system::Query,
        world::World,
    },
    log::{debug, trace, warn},
    platform::{collections::HashMap, time::Instant},
    prelude::{Deref, DerefMut},
};

use crate::{
    core::{
        path::PropPath,
        props::Props,
        template::{PropsDeclaration, PropsSourceKind},
        value::Value,
    },
    runtime::{
        render::{restore_props, take_props},
        BevueSystems,
    },
};

/// The props of a template, attached to the same entity as its [`Template`](crate::Template).
///
/// Mutate the value through the precise [`Value`] mutators (`get_mut`, `set`, `push`, ...) so the
/// runtime only updates the UI nodes bound to what changed. Replacing `value` wholesale works too,
/// but re-evaluates every binding of the template.
#[derive(Component, Debug, Clone, PartialEq, Deref, DerefMut)]
pub struct BevueProps {
    pub value: Value,
}

impl BevueProps {
    pub fn new(value: impl Into<Value>) -> Self {
        Self { value: value.into() }
    }
}

impl Default for BevueProps {
    fn default() -> Self {
        Self { value: Value::empty_dict() }
    }
}

impl From<Value> for BevueProps {
    fn from(value: Value) -> Self {
        Self { value }
    }
}

// Syncing `Props` components and resources

/// Renders component `T` into the [`BevueProps`] of this template.
///
/// `T` is read from this entity, or from [`PropsComponent::from_entity`]. With a key, it is mounted
/// at that path of the props (`{{inventory.slots}}`); without one it becomes the props root
/// (`{{slots}}`). The default mounts it at its [`Props::name`]. Register `T` with [`BevuePropsAppExt::register_props`].
#[derive(Component)]
#[require(BevueProps)]
pub struct PropsComponent<T: Send + Sync + 'static> {
    pub key: Option<PropPath>,
    pub entity: Option<Entity>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> PropsComponent<T> {
    /// Uses `T` as the props root.
    pub fn root() -> Self {
        Self { key: None, entity: None, _marker: PhantomData }
    }

    /// Mounts `T` at `key`, e.g. `"inventory"`. Panics if `key` is not a valid path.
    pub fn at(key: &str) -> Self {
        let key = PropPath::parse(key).unwrap_or_else(|e| panic!("invalid props key: {e}"));
        Self { key: Some(key), entity: None, _marker: PhantomData }
    }

    /// Reads `T` from another entity.
    pub fn from_entity(mut self, entity: Entity) -> Self {
        self.entity = Some(entity);
        self
    }
}

impl<T: Props> Default for PropsComponent<T> {
    /// Mounts `T` at its [`Props::name`].
    fn default() -> Self {
        Self::at(T::name())
    }
}

/// Renders resource `R` into the [`BevueProps`] of this template, at `key` or as the props root.
/// The default mounts it at its [`Props::name`].
/// Register `R` with [`BevuePropsAppExt::register_props`].
#[derive(Component)]
#[require(BevueProps)]
pub struct PropsResource<R: Send + Sync + 'static> {
    pub key: Option<PropPath>,
    _marker: PhantomData<fn() -> R>,
}

impl<R: Resource> PropsResource<R> {
    /// Uses `R` as the props root.
    pub fn root() -> Self {
        Self { key: None, _marker: PhantomData }
    }

    /// Mounts `R` at `key`, e.g. `"menu"`. Panics if `key` is not a valid path.
    pub fn at(key: &str) -> Self {
        let key = PropPath::parse(key).unwrap_or_else(|e| panic!("invalid props key: {e}"));
        Self { key: Some(key), _marker: PhantomData }
    }
}

impl<R: Resource + Props> Default for PropsResource<R> {
    /// Mounts `R` at its [`Props::name`].
    fn default() -> Self {
        Self::at(R::name())
    }
}

/// Diffs `data` into `props` at `key`, marking `props` as changed only if something differs.
/// Returns whether the props have changes to render.
pub fn sync_props(data: &dyn Props, props: &mut Mut<BevueProps>, key: Option<&PropPath>) -> bool {
    let value = &mut props.bypass_change_detection().value;
    let path = key.map(|k| k.segments()).unwrap_or(&[]);
    value.update_at(path, |target| data.sync_value(target));
    let dirty = value.is_dirty();
    if dirty {
        props.set_changed();
    }
    dirty
}

/// Times a sync system's work, logging once per run that synced anything.
#[derive(Default)]
struct SyncTimer {
    start: Option<Instant>,
    synced: usize,
    changed: usize,
}

impl SyncTimer {
    fn record(&mut self, sync: impl FnOnce() -> bool) {
        self.start.get_or_insert_with(Instant::now);
        self.synced += 1;
        self.changed += sync() as usize;
    }

    /// Logs at `debug` when a sync found changes, and at `trace` when the data was touched but
    /// nothing differed (which can happen every frame).
    fn log<T>(self) {
        let Some(start) = self.start else { return };
        let elapsed = start.elapsed();
        let name = std::any::type_name::<T>();
        if self.changed > 0 {
            debug!("synced {name} into {} template(s), {} changed, in {elapsed:?}", self.synced, self.changed);
        } else {
            trace!("synced {name} into {} template(s), nothing changed, in {elapsed:?}", self.synced);
        }
    }
}

/// Copies changed `T` components into the props of templates with a [`PropsComponent<T>`].
pub fn sync_component_props<T: Component + Props>(
    mut targets: Query<(Entity, Ref<PropsComponent<T>>, &mut BevueProps)>,
    sources: Query<Ref<T>>,
) {
    let mut timer = SyncTimer::default();
    for (entity, source, mut props) in &mut targets {
        let Ok(data) = sources.get(source.entity.unwrap_or(entity)) else { continue };
        if data.is_changed() || source.is_changed() {
            timer.record(|| sync_props(&*data, &mut props, source.key.as_ref()));
        }
    }
    timer.log::<T>();
}

/// Copies a changed `R` resource into the props of templates with a [`PropsResource<R>`].
///
/// A resource is a component of its resource entity, so `R` is read from there. This lets the
/// system be registered for any component, see [`RegisterableProps`].
pub fn sync_resource_props<R: Component + Props>(
    resource: Query<Ref<R>, With<IsResource>>,
    mut targets: Query<(Ref<PropsResource<R>>, &mut BevueProps)>,
) {
    let Some(resource) = resource.iter().next() else { return };
    let mut timer = SyncTimer::default();
    for (source, mut props) in &mut targets {
        if resource.is_changed() || source.is_changed() {
            timer.record(|| sync_props(&*resource, &mut props, source.key.as_ref()));
        }
    }
    timer.log::<R>();
}

// Props declared by templates

/// How a registered [`Props`] type is mounted on a template that declares it in `<props>`.
#[derive(Debug, Clone, Copy)]
pub struct PropsRegistration {
    /// The type's [`Props::type_path`].
    pub type_path: &'static str,
    /// The type's [`Props::name`], the key it is mounted at when the declaration has no name.
    pub name: &'static str,
    mount: fn(&mut World, Entity, PropsSourceKind, PropPath) -> bool,
    unmount: fn(&mut World, Entity, PropsSourceKind),
}

/// The [`Props`] types registered with [`BevuePropsAppExt::register_props`], by the names a
/// template can use for them in `<props>`: their [`Props::type_path`] and their
/// [`Props::short_type_path`]. If two types share a short name, it refers to the one registered
/// last.
#[derive(Resource, Default, Debug)]
pub struct PropsTypeRegistry {
    types: HashMap<&'static str, PropsRegistration>,
}

impl PropsTypeRegistry {
    pub fn register<T: Component + Props>(&mut self) {
        let registration = PropsRegistration {
            type_path: T::type_path(),
            name: T::name(),
            mount: mount_props::<T>,
            unmount: unmount_props::<T>,
        };
        self.types.insert(T::type_path(), registration);
        self.types.insert(T::short_type_path(), registration);
    }

    /// Gets a type by its full or short type path.
    pub fn get(&self, type_path: &str) -> Option<&PropsRegistration> {
        self.types.get(type_path)
    }
}

/// The props sources a template inserted on its entity for its `<props>` declarations. They are
/// removed when the template is rendered again, so sources inserted by hand are never touched.
#[derive(Component, Default, Debug)]
pub struct DeclaredProps(Vec<(PropsRegistration, PropsSourceKind)>);

/// Inserts the props source of `T` on `entity` unless it already has one, and syncs it right away
/// so the render that follows shows its data. Returns whether the source was inserted.
fn mount_props<T: Component + Props>(world: &mut World, entity: Entity, kind: PropsSourceKind, key: PropPath) -> bool {
    let Ok(mut target) = world.get_entity_mut(entity) else { return false };
    let source = match kind {
        PropsSourceKind::Resource => {
            if target.contains::<PropsResource<T>>() {
                return false;
            }
            target.insert_if_new(PropsResource::<T> { key: Some(key.clone()), _marker: PhantomData });
            world.component_id::<T>().and_then(|id| world.resource_entities().get(id))
        }
        PropsSourceKind::Component => {
            if target.contains::<PropsComponent<T>>() {
                return false;
            }
            target.insert_if_new(PropsComponent::<T> { key: Some(key.clone()), entity: None, _marker: PhantomData });
            Some(entity)
        }
    };

    let mut value = take_props(world, entity);
    if let Some(data) = source.and_then(|source| world.get::<T>(source)) {
        value.update_at(key.segments(), |target| data.sync_value(target));
    }
    restore_props(world, entity, value);
    true
}

fn unmount_props<T: Component + Props>(world: &mut World, entity: Entity, kind: PropsSourceKind) {
    let Ok(mut target) = world.get_entity_mut(entity) else { return };
    match kind {
        PropsSourceKind::Resource => target.remove::<PropsResource<T>>(),
        PropsSourceKind::Component => target.remove::<PropsComponent<T>>(),
    };
}

/// Mounts the props a template declares in `<props>` on its entity, replacing the ones mounted
/// for its previous render. A source the entity already has is left as it is.
pub(crate) fn mount_declared_props(world: &mut World, entity: Entity, declarations: &[PropsDeclaration]) {
    let previous = world.get_mut::<DeclaredProps>(entity).map(|mut declared| std::mem::take(&mut declared.0));
    if previous.is_none() && declarations.is_empty() {
        return;
    }
    for (registration, kind) in previous.unwrap_or_default() {
        (registration.unmount)(world, entity, kind);
    }

    let mut mounted = Vec::new();
    for declaration in declarations {
        let registration = world.get_resource::<PropsTypeRegistry>().and_then(|r| r.get(&declaration.type_path));
        let Some(registration) = registration.copied() else {
            warn!(
                "template on {entity} declares props of unknown type '{}'; register it with `app.register_props`",
                declaration.type_path
            );
            continue;
        };
        let name = declaration.name.as_deref().unwrap_or(registration.name);
        let key = match PropPath::parse(name) {
            Ok(key) => key,
            Err(error) => {
                warn!("cannot mount props '{}' on {entity}: {error}", registration.type_path);
                continue;
            }
        };
        if (registration.mount)(world, entity, declaration.source, key) {
            mounted.push((registration, declaration.source));
        }
    }

    if let Ok(mut target) = world.get_entity_mut(entity) {
        if mounted.is_empty() {
            target.remove::<DeclaredProps>();
        } else {
            target.insert(DeclaredProps(mounted));
        }
    }
}

/// A [`Props`] type that can be registered as a template props source with
/// [`BevuePropsAppExt::register_props`].
///
/// It is implemented for every [`Props`] component. Resources are components too, so the same
/// impl covers them: a registered type can be mounted with a [`PropsComponent`] and, if it is a
/// resource, with a [`PropsResource`]. A registered type can also be declared in the `<props>`
/// of a template, see [`PropsTypeRegistry`].
pub trait RegisterableProps: Props + Sized {
    /// Adds what is needed to sync `Self` into template props.
    fn register(app: &mut App);
}

impl<T: Component + Props> RegisterableProps for T {
    fn register(app: &mut App) {
        app.world_mut().get_resource_or_init::<PropsTypeRegistry>().register::<T>();
        app.add_systems(
            PostUpdate,
            (sync_component_props::<T>, sync_resource_props::<T>).in_set(BevueSystems::SyncProps),
        );
    }
}

/// Registers [`Props`] components and resources as template props sources.
pub trait BevuePropsAppExt {
    /// Syncs `T` into templates with a [`PropsComponent<T>`] or a [`PropsResource<T>`] whenever it
    /// changes.
    fn register_props<T: RegisterableProps>(&mut self) -> &mut Self;
}

impl BevuePropsAppExt for App {
    fn register_props<T: RegisterableProps>(&mut self) -> &mut Self {
        T::register(self);
        self
    }
}
