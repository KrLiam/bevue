use bevue::{Template, asset::BevueAssetPlugin, runtime::BevueRuntimePlugin};
use bevy::{DefaultPlugins, app::{App, Startup}, asset::AssetServer, camera::Camera2d, ecs::system::{Commands, Res}};


fn main() {
    let mut app = App::new();

    app.add_plugins((BevueAssetPlugin, DefaultPlugins, BevueRuntimePlugin))
        .add_systems(Startup, startup);

    app.run();
}

fn startup(asset_server: Res<AssetServer>, mut cmds: Commands) {
    cmds.spawn(Camera2d);
    
    cmds.spawn((
        Template::new(asset_server.load("simple.html")),
    ));
}
