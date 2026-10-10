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
    },
    log::{debug, trace},
    platform::time::Instant,
    prelude::{Deref, DerefMut},
};

use crate::{
    core::{path::PropPath, props::Props, value::Value},
    runtime::BevueSystems,
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

/// A [`Props`] type that can be registered as a template props source with
/// [`BevuePropsAppExt::register_props`].
///
/// It is implemented for every [`Props`] component. Resources are components too, so the same
/// impl covers them: a registered type can be mounted with a [`PropsComponent`] and, if it is a
/// resource, with a [`PropsResource`].
pub trait RegisterableProps: Props + Sized {
    /// Adds what is needed to sync `Self` into template props.
    fn register(app: &mut App);
}

impl<T: Component + Props> RegisterableProps for T {
    fn register(app: &mut App) {
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
