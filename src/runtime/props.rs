//! The `BevueProps` component holding the data a template renders.

use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use bevy::{
    app::{App, PostUpdate},
    ecs::{
        change_detection::{DetectChanges, DetectChangesMut, Mut, Ref},
        component::Component,
        entity::Entity,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Query, Res},
    },
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
#[derive(Component, Debug, Clone, PartialEq)]
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

impl Deref for BevueProps {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl DerefMut for BevueProps {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
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
/// `T` is read from this entity, or from [`PropsSource::from_entity`]. With a key, it is mounted
/// at that path of the props (`{{inventory.slots}}`); without one it becomes the props root
/// (`{{slots}}`). Register `T` with [`BevuePropsAppExt::register_props`].
#[derive(Component)]
#[require(BevueProps)]
pub struct PropsSource<T: Send + Sync + 'static> {
    pub key: Option<PropPath>,
    pub entity: Option<Entity>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> PropsSource<T> {
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

/// Renders resource `R` into the [`BevueProps`] of this template, at `key` or as the props root.
/// Register `R` with [`BevuePropsAppExt::register_props_resource`].
#[derive(Component)]
#[require(BevueProps)]
pub struct PropsResource<R: Send + Sync + 'static> {
    pub key: Option<PropPath>,
    _marker: PhantomData<fn() -> R>,
}

impl<R: Send + Sync + 'static> PropsResource<R> {
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

/// Diffs `data` into `props` at `key`, marking `props` as changed only if something differs.
pub fn sync_props(data: &dyn Props, props: &mut Mut<BevueProps>, key: Option<&PropPath>) {
    let value = &mut props.bypass_change_detection().value;
    let path = key.map(|k| k.segments()).unwrap_or(&[]);
    value.update_at(path, |target| data.sync_value(target));
    if value.is_dirty() {
        props.set_changed();
    }
}

/// Copies changed `T` components into the props of templates with a [`PropsSource<T>`].
pub fn sync_component_props<T: Component + Props>(
    mut targets: Query<(Entity, Ref<PropsSource<T>>, &mut BevueProps)>,
    sources: Query<Ref<T>>,
) {
    for (entity, source, mut props) in &mut targets {
        let Ok(data) = sources.get(source.entity.unwrap_or(entity)) else { continue };
        if data.is_changed() || source.is_changed() {
            sync_props(&*data, &mut props, source.key.as_ref());
        }
    }
}

/// Copies a changed `R` resource into the props of templates with a [`PropsResource<R>`].
pub fn sync_resource_props<R: Resource + Props>(
    resource: Option<Res<R>>,
    mut targets: Query<(Ref<PropsResource<R>>, &mut BevueProps)>,
) {
    let Some(resource) = resource else { return };
    for (source, mut props) in &mut targets {
        if resource.is_changed() || source.is_changed() {
            sync_props(&*resource, &mut props, source.key.as_ref());
        }
    }
}

/// Registers [`Props`] components and resources as template props sources.
pub trait BevuePropsAppExt {
    /// Syncs component `T` into templates with a [`PropsSource<T>`] whenever it changes.
    fn register_props<T: Component + Props>(&mut self) -> &mut Self;

    /// Syncs resource `R` into templates with a [`PropsResource<R>`] whenever it changes.
    fn register_props_resource<R: Resource + Props>(&mut self) -> &mut Self;
}

impl BevuePropsAppExt for App {
    fn register_props<T: Component + Props>(&mut self) -> &mut Self {
        self.add_systems(PostUpdate, sync_component_props::<T>.in_set(BevueSystems::SyncProps))
    }

    fn register_props_resource<R: Resource + Props>(&mut self) -> &mut Self {
        self.add_systems(PostUpdate, sync_resource_props::<R>.in_set(BevueSystems::SyncProps))
    }
}
