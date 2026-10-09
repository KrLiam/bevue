//! Template events: `@name="handler"` listeners and the [`BevueEvent`] that triggers them.

use std::borrow::Cow;

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        event::EntityEvent,
        observer::On,
        query::{Changed, With},
        resource::Resource,
        system::{Commands, Query},
        world::World,
    },
    log::warn,
    ui::Interaction,
};

use crate::{
    core::{template::TemplateRef, value::Value},
    runtime::{
        functions::call_function,
        instance::TemplateInstance,
        render::{restore_props, take_props},
    },
};

/// An event on a rendered UI node, handled by the node's `@name` listener.
///
/// Bevue triggers `click`, `mouseover` and `mouseleave` for nodes with [`Interaction`], and
/// `$emit` triggers events on the template root. Trigger it yourself with
/// `commands.trigger(BevueEvent { .. })`, and observe it with `On<BevueEvent>`.
#[derive(EntityEvent, Clone, Debug, PartialEq)]
pub struct BevueEvent {
    /// The UI node the event happened on.
    pub entity: Entity,
    pub name: Cow<'static, str>,
    pub args: Vec<Value>,
}

impl BevueEvent {
    pub fn new(entity: Entity, name: impl Into<Cow<'static, str>>, args: Vec<Value>) -> Self {
        Self { entity, name: name.into(), args }
    }
}

/// The previous [`Interaction`] of a node, to detect transitions.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastInteraction(pub Interaction);

type InteractionChanges<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Interaction, Option<&'static mut LastInteraction>),
    (Changed<Interaction>, With<TemplateRef>),
>;

/// Triggers `mouseover`, `click` and `mouseleave` from [`Interaction`] changes on template nodes.
///
/// A click is a press released while the cursor is still over the node.
pub fn emit_interaction_events(
    mut commands: Commands,
    mut nodes: InteractionChanges,
) {
    for (entity, &interaction, last) in &mut nodes {
        let previous = last.as_ref().map_or(Interaction::None, |last| last.0);
        match last {
            Some(mut last) => last.0 = interaction,
            None => {
                commands.entity(entity).insert(LastInteraction(interaction));
            }
        }

        let mut trigger = |name: &'static str| commands.trigger(BevueEvent::new(entity, name, Vec::new()));
        match (previous, interaction) {
            (Interaction::None, Interaction::Hovered | Interaction::Pressed) => trigger("mouseover"),
            (Interaction::Pressed, Interaction::Hovered) => trigger("click"),
            (Interaction::Hovered | Interaction::Pressed, Interaction::None) => trigger("mouseleave"),
            _ => {}
        }
    }
}

/// Dispatches every [`BevueEvent`] to the listener of its target node, if any.
pub fn on_bevue_event(event: On<BevueEvent>, mut commands: Commands) {
    let event = event.event().clone();
    commands.queue(move |world: &mut World| dispatch_bevue_event(world, event));
}

/// How many event handlers may run nested inside each other, e.g. a handler calling `$emit`
/// whose event is handled by another handler. Deeper dispatches are dropped with a warning,
/// which stops listeners that re-emit their own event.
pub const MAX_DISPATCH_DEPTH: u32 = 32;

#[derive(Resource, Default)]
struct DispatchDepth(u32);

/// Resolves the handler of the event's target node and runs its function.
///
/// This is a plain function rather than a registered system: handlers may trigger events (as
/// `$emit` does) that are dispatched while the outer handler is still running.
pub fn dispatch_bevue_event(world: &mut World, event: BevueEvent) {
    let Some(&TemplateRef { root, .. }) = world.get::<TemplateRef>(event.entity) else { return };

    // Resolve the handler with the instance and props taken out, as nested calls may run here.
    let Some(instance) = world
        .get_mut::<TemplateInstance>(root)
        .map(|mut instance| std::mem::take(&mut *instance))
    else {
        return;
    };
    let props = take_props(world, root);
    let resolved = instance.resolve_event(world, &props, event.entity, &event.name, &event.args);
    restore_props(world, root, props);
    if let Some(mut slot) = world.get_mut::<TemplateInstance>(root) {
        *slot = instance;
    }

    // Run the handler with everything back in place, so it can freely mutate the template.
    let Some((function, args)) = resolved else { return };
    let depth = world.get_resource_or_init::<DispatchDepth>().0;
    if depth >= MAX_DISPATCH_DEPTH {
        warn!("Dropped event '{}': more than {MAX_DISPATCH_DEPTH} nested event handlers", event.name);
        return;
    }
    world.resource_mut::<DispatchDepth>().0 += 1;
    call_function(world, root, event.entity, &function, args);
    world.resource_mut::<DispatchDepth>().0 -= 1;
}
