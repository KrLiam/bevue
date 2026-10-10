//! Component elements: `<Foo :item="item" @close="handler()"/>` renders the template registered
//! as `Foo`.
//!
//! Components are expanded when the template using them is compiled (see
//! [`compile`](crate::core::compile)), so a rendered component has no entity, component or state
//! of its own: it costs the same as writing its elements in place.

use std::cell::RefCell;

use bevy::{
    app::App,
    asset::{AssetId, AssetServer, Assets, Handle},
    ecs::{change_detection::DetectChangesMut, component::Component, resource::Resource, world::World},
    platform::collections::HashMap,
};

use crate::core::{
    compile::{ComponentResolver, ResolvedComponent},
    template::BevueTemplate,
};

#[derive(Debug, Default)]
struct RegisteredComponent {
    path: Option<String>,
    handle: Option<Handle<BevueTemplate>>,
}

/// The templates that component elements refer to, by name.
#[derive(Resource, Debug, Default)]
pub struct BevueComponents {
    components: HashMap<String, RegisteredComponent>,
}

/// Panics if `name` cannot be used as a component element.
fn assert_component_name(name: &str) {
    assert!(
        name.chars().next().is_some_and(char::is_uppercase),
        "Bevue component names must start with an uppercase letter: '{name}'"
    );
}

impl BevueComponents {
    /// Makes `<name/>` render the template asset at `path`, loaded when first needed. Panics if
    /// `name` does not start with an uppercase letter.
    pub fn register(&mut self, name: impl Into<String>, path: impl Into<String>) {
        let name = name.into();
        assert_component_name(&name);
        self.components.insert(name, RegisteredComponent { path: Some(path.into()), handle: None });
    }

    /// Makes `<name/>` render the template `handle`. Panics if `name` does not start with an
    /// uppercase letter.
    pub fn insert(&mut self, name: impl Into<String>, handle: Handle<BevueTemplate>) {
        let name = name.into();
        assert_component_name(&name);
        self.components.insert(name, RegisteredComponent { path: None, handle: Some(handle) });
    }

    pub fn contains(&self, name: &str) -> bool {
        self.components.contains_key(name)
    }

    /// The template of component `name`, if it is registered and its asset was requested.
    pub fn handle(&self, name: &str) -> Option<&Handle<BevueTemplate>> {
        self.components.get(name)?.handle.as_ref()
    }
}

/// Registers the templates of component elements.
pub trait BevueComponentsAppExt {
    /// Makes `<name/>` render the template asset at `path` in every template. Panics if `name`
    /// does not start with an uppercase letter.
    fn register_component(&mut self, name: impl Into<String>, path: impl Into<String>) -> &mut Self;
}

impl BevueComponentsAppExt for App {
    fn register_component(&mut self, name: impl Into<String>, path: impl Into<String>) -> &mut Self {
        self.world_mut().get_resource_or_init::<BevueComponents>().register(name, path);
        self
    }
}

/// Starts loading the components registered by path that are not loading yet, ahead of the render
/// that needs them. Does nothing until the `AssetServer` exists.
pub fn load_components(world: &mut World) {
    let Some(asset_server) = world.get_resource::<AssetServer>().cloned() else { return };
    let Some(mut components) = world.get_resource_mut::<BevueComponents>() else { return };
    for component in components.bypass_change_detection().components.values_mut() {
        if component.handle.is_none()
            && let Some(path) = &component.path
        {
            component.handle = Some(asset_server.load(path.clone()));
        }
    }
}

/// The component templates the last render of this template asked for, loaded or not. The
/// template is rendered again when one of them is loaded or modified.
#[derive(Component, Debug, Default, Clone, PartialEq, Eq)]
pub struct ComponentDependencies {
    pub assets: Vec<AssetId<BevueTemplate>>,
    /// Those of `assets` that were not available, for which an empty node was rendered. The
    /// template is rendered again when one of them is added.
    pub pending: Vec<AssetId<BevueTemplate>>,
}

/// Resolves components from the registry and the loaded template assets, recording the ones
/// asked for.
pub(crate) struct WorldComponents<'w> {
    components: Option<&'w BevueComponents>,
    templates: &'w Assets<BevueTemplate>,
    used: RefCell<ComponentDependencies>,
}

impl<'w> WorldComponents<'w> {
    pub(crate) fn new(world: &'w World, templates: &'w Assets<BevueTemplate>) -> Self {
        Self { components: world.get_resource::<BevueComponents>(), templates, used: RefCell::default() }
    }

    pub(crate) fn into_dependencies(self) -> ComponentDependencies {
        self.used.into_inner()
    }
}

impl ComponentResolver for WorldComponents<'_> {
    fn resolve(&self, name: &str) -> ResolvedComponent<'_> {
        let Some(component) = self.components.and_then(|components| components.components.get(name)) else {
            return ResolvedComponent::Unknown;
        };
        let Some(handle) = &component.handle else { return ResolvedComponent::Pending };
        let mut used = self.used.borrow_mut();
        if !used.assets.contains(&handle.id()) {
            used.assets.push(handle.id());
        }
        match self.templates.get(handle) {
            Some(template) => ResolvedComponent::Ready(template),
            None => {
                if !used.pending.contains(&handle.id()) {
                    used.pending.push(handle.id());
                }
                ResolvedComponent::Pending
            }
        }
    }
}
