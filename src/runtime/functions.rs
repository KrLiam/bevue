//! Functions callable from template expressions, implemented as Bevy one-shot systems.
//!
//! A function is any system taking `In<BevueCtx>` and returning something convertible to a
//! [`Value`] (`()` becomes `null`). Register it globally with
//! [`BevueFunctionsAppExt::register_bevue_function`], or for a single template with
//! [`Template::with_function`](crate::Template::with_function).
//!
//! Calls in bindings (`{{ format(price) }}`) are treated as pure: they re-run only when the props
//! their arguments read change. While a binding is evaluated, the template's own `BevueProps` and
//! `TemplateInstance` are taken out of the world, so binding functions should not query them.
//! Event handlers run outside updates and may mutate anything.

use std::{borrow::Cow, sync::Arc};

use bevy::{
    app::App,
    ecs::{
        component::Component,
        entity::Entity,
        lifecycle::HookContext,
        resource::Resource,
        system::{BoxedSystem, Commands, In, IntoSystem, SystemId},
        world::{DeferredWorld, World},
    },
    log::warn,
    platform::collections::HashMap,
};

use crate::{core::value::Value, runtime::events::BevueEvent};

/// The input of a Bevue function.
#[derive(Debug, Clone)]
pub struct BevueCtx {
    /// The UI node whose expression called the function.
    pub entity: Entity,
    /// The root entity of the template, which holds its `BevueProps`.
    pub root: Entity,
    /// The evaluated call arguments.
    pub args: Vec<Value>,
}

pub type BevueFunctionId = SystemId<In<BevueCtx>, Value>;
pub type BoxedBevueFunction = BoxedSystem<In<BevueCtx>, Value>;

/// Creates a fresh instance of a template-local function system.
pub type BevueFunctionFactory = Arc<dyn Fn() -> BoxedBevueFunction + Send + Sync>;

/// Functions available to every template.
#[derive(Resource, Default, Debug)]
pub struct BevueFunctions(pub HashMap<String, BevueFunctionId>);

/// Functions registered for a single template from [`Template::with_function`](crate::Template::with_function).
/// Managed by `Template`'s component hooks; its systems are unregistered when it is removed,
/// replaced or despawned.
#[derive(Component, Default, Debug)]
#[component(on_discard = unregister_functions)]
pub struct TemplateFunctions(pub HashMap<String, BevueFunctionId>);

fn unregister_functions(mut world: DeferredWorld, ctx: HookContext) {
    let Some(functions) = world.get::<TemplateFunctions>(ctx.entity) else { return };
    let ids: Vec<BevueFunctionId> = functions.0.values().copied().collect();
    world.commands().queue(move |world: &mut World| {
        for id in ids {
            let _ = world.unregister_system(id);
        }
    });
}

fn into_value<O: Into<Value>>(output: O) -> Value {
    output.into()
}

/// Boxes a function system, converting its output to a [`Value`].
pub fn box_function<O, M>(system: impl IntoSystem<In<BevueCtx>, O, M> + 'static) -> BoxedBevueFunction
where
    O: Into<Value> + 'static,
{
    Box::new(IntoSystem::into_system(system.map(into_value::<O>)))
}

/// Panics if `name` is reserved for built-in functions.
pub(crate) fn assert_not_reserved(name: &str) {
    assert!(!name.starts_with('$'), "Bevue function names starting with '$' are reserved: '{name}'");
}

/// Registers functions available to every template.
pub trait BevueFunctionsAppExt {
    /// Makes `system` callable as `name(...)` from any template. Panics if `name` starts with `$`.
    fn register_bevue_function<O, M>(
        &mut self,
        name: impl Into<String>,
        system: impl IntoSystem<In<BevueCtx>, O, M> + 'static,
    ) -> &mut Self
    where
        O: Into<Value> + 'static;
}

impl BevueFunctionsAppExt for App {
    fn register_bevue_function<O, M>(
        &mut self,
        name: impl Into<String>,
        system: impl IntoSystem<In<BevueCtx>, O, M> + 'static,
    ) -> &mut Self
    where
        O: Into<Value> + 'static,
    {
        let name = name.into();
        assert_not_reserved(&name);
        insert_global_function(self.world_mut(), name, box_function(system));
        self
    }
}

/// Registers a global function without checking for reserved names.
pub(crate) fn insert_global_function(world: &mut World, name: String, system: BoxedBevueFunction) {
    let id = world.register_boxed_system(system);
    if let Some(old) = world.get_resource_or_init::<BevueFunctions>().0.insert(name, id) {
        let _ = world.unregister_system(old);
    }
}

/// Calls the function `name` for a node of the template rooted at `root`: the template's own
/// functions take precedence over global ones. Unknown functions and failures warn and return
/// `null`.
pub fn call_function(world: &mut World, root: Entity, entity: Entity, name: &str, args: Vec<Value>) -> Value {
    let local = world.get::<TemplateFunctions>(root).and_then(|f| f.0.get(name).copied());
    let id = local.or_else(|| world.get_resource::<BevueFunctions>().and_then(|f| f.0.get(name).copied()));
    let Some(id) = id else {
        warn!("Unknown Bevue function '{name}'");
        return Value::null();
    };
    match world.run_system_with(id, BevueCtx { entity, root, args }) {
        Ok(value) => value,
        Err(error) => {
            warn!("Bevue function '{name}' failed: {error}");
            Value::null()
        }
    }
}

/// The built-in `$emit(name, ...args)`: triggers a [`BevueEvent`] named `name` with the remaining
/// arguments on the template root. Only usable in event handlers.
pub fn emit(In(ctx): In<BevueCtx>, mut commands: Commands) {
    let mut args = ctx.args.into_iter();
    let Some(name) = args.next().and_then(|name| name.as_string().cloned()) else {
        warn!("$emit expects an event name string as its first argument");
        return;
    };
    commands.trigger(BevueEvent { entity: ctx.root, name: Cow::Owned(name), args: args.collect() });
}
