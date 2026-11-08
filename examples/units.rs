//! Queries and entities as props: a list of units read straight from their components, sorted by
//! health, and a panel showing the selected unit through an `Entity` prop.
//!
//! Nothing here tells Bevue what changed: units take damage in an ordinary system and the rows
//! (and their order) follow.

use bevue::{BevueCtx, BevuePlugin, BevuePropsAppExt, Props, Template};
use bevy::{
    DefaultPlugins,
    app::{App, Startup, Update},
    asset::AssetServer,
    camera::Camera2d,
    ecs::{
        component::Component,
        entity::Entity,
        name::Name,
        query::With,
        resource::Resource,
        system::{Commands, In, Query, Res, ResMut},
    },
    time::Time,
};

#[derive(Component, Props)]
struct Health {
    current: u32,
    max: u32,
}

#[derive(Component, Props)]
struct Unit;

#[derive(Component, Props)]
struct Target(Entity);

#[derive(Resource, Props, Default)]
struct Selection {
    entity: Option<Entity>,
}

fn select(In(ctx): In<BevueCtx>, mut selection: ResMut<Selection>) {
    // `select()` without an argument clears the selection.
    selection.entity = ctx.get_args().unwrap();
}

/// Every second, a random-ish unit loses some health.
fn take_damage(time: Res<Time>, mut units: Query<&mut Health, With<Unit>>) {
    let second = time.elapsed_secs() as u32;
    if time.elapsed_secs() - (second as f32) > time.delta_secs() {
        return;
    }
    let count = units.iter().count().max(1);
    if let Some(mut health) = units.iter_mut().nth(second as usize * 7 % count) {
        health.current = health.current.saturating_sub(3);
    }
}

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, BevuePlugin))
        .register_props::<Name>()
        .register_props::<Health>()
        .register_props::<Unit>()
        .register_props::<Target>()
        .register_props::<Selection>()
        .init_resource::<Selection>()
        .add_systems(Startup, startup)
        .add_systems(Update, take_damage)
        .run();
}

fn startup(mut cmds: Commands, server: Res<AssetServer>) {
    cmds.spawn(Camera2d);

    let boss = cmds.spawn((Name::new("Dragon"), Health { current: 50, max: 50 })).id();
    for (name, hp) in [("Knight", 30), ("Archer", 20), ("Mage", 15), ("Rogue", 25)] {
        cmds.spawn((Name::new(name), Health { current: hp, max: hp }, Unit, Target(boss)));
    }

    // The query and the selection are declared in the template's `<props>`.
    cmds.spawn(Template::new(server.load("units.html")).with_function("select", select));
}
