use bevue::{
    BevueCtx, BevueFunctionsAppExt, BevuePropsAppExt, Props, Template, asset::BevueAssetPlugin,
    runtime::BevueRuntimePlugin,
};
use bevy::{
    DefaultPlugins,
    app::{App, Startup, Update},
    asset::AssetServer,
    camera::Camera2d,
    ecs::{
        resource::Resource,
        system::{Commands, In, Res, ResMut},
    },
    log::LogPlugin,
    prelude::PluginGroup,
    time::Time,
};

/// Data rendered by `assets/simple.html` as `counter`: the template declares it in its `<props>`,
/// and it is mounted at the name of the type in snake case.
#[derive(Resource, Props, Default)]
struct Counter {
    seconds: u32,
    even: bool,
    items: Vec<u32>,
    #[props(skip)]
    elapsed: f32,
}

fn main() {
    let mut app = App::new();

    // `bevue=debug` logs how long each props sync, render and update took.
    let log = LogPlugin { filter: "wgpu=error,naga=warn,bevue=debug".into(), ..Default::default() };

    app.add_plugins((BevueAssetPlugin, DefaultPlugins.set(log), BevueRuntimePlugin))
        .register_props::<Counter>()
        .init_resource::<Counter>()
        .register_bevue_function("add_item", add_item)
        .add_systems(Startup, startup)
        .add_systems(Update, tick);

    app.run();
}

fn startup(asset_server: Res<AssetServer>, mut cmds: Commands) {
    cmds.spawn(Camera2d);

    cmds.spawn(Template::new(asset_server.load("simple.html")).with_function("label", label));
}

/// Called by `@click="add_item()"`: adds an item right away.
fn add_item(In(_ctx): In<BevueCtx>, mut counter: ResMut<Counter>) {
    let next = counter.items.last().map_or(1, |last| last + 1);
    counter.items.push(next);
}

/// Called by `{{ label(item) }}`, only available to this template.
fn label(In(ctx): In<BevueCtx>) -> String {
    let item: u32 = ctx.get_args().unwrap();
    format!("Item #{item}")
}

/// Updates the counter once per second. Only the template nodes bound to the fields that changed
/// are updated.
fn tick(time: Res<Time>, mut counter: ResMut<Counter>) {
    counter.elapsed += time.delta_secs();
    if counter.elapsed < 1.0 {
        return;
    }
    counter.elapsed -= 1.0;
    counter.seconds += 1;
    counter.even = counter.seconds.is_multiple_of(2);
    if counter.items.len() >= 5 {
        counter.items.clear();
    } else {
        let next = counter.seconds;
        counter.items.push(next);
    }
}
