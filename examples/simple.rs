use bevue::{BevuePropsAppExt, Props, PropsResource, Template, asset::BevueAssetPlugin, runtime::BevueRuntimePlugin};
use bevy::{
    DefaultPlugins,
    app::{App, Startup, Update},
    asset::AssetServer,
    camera::Camera2d,
    ecs::{
        resource::Resource,
        system::{Commands, Res, ResMut},
    },
    time::Time,
};

/// Data rendered by `assets/simple.html` as `counter`.
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

    app.add_plugins((BevueAssetPlugin, DefaultPlugins, BevueRuntimePlugin))
        .register_props_resource::<Counter>()
        .init_resource::<Counter>()
        .add_systems(Startup, startup)
        .add_systems(Update, tick);

    app.run();
}

fn startup(asset_server: Res<AssetServer>, mut cmds: Commands) {
    cmds.spawn(Camera2d);

    cmds.spawn((
        Template::new(asset_server.load("simple.html")),
        PropsResource::<Counter>::at("counter"),
    ));
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
