use bevue::{
    BevueComponentsAppExt, BevueCtx, BevuePropsAppExt, Props, Template, asset::BevueAssetPlugin, runtime::BevueRuntimePlugin,
};
use bevy::{
    DefaultPlugins, app::{App, Startup, Update}, asset::AssetServer, camera::Camera2d, ecs::{
        resource::Resource, schedule::{IntoScheduleConfigs, common_conditions::resource_changed}, system::{Commands, In, Res, ResMut},
    }, log::info,
};

#[derive(Resource, Props)]
struct PlayerName {
    pub value: String,
}

#[derive(Resource, Props, Default)]
struct Inventory {
    items: Vec<Item>,
}

#[derive(Props)]
struct Item {
    id: String,
    count: u32,
}


fn add_item(_: In<BevueCtx>, mut inv: ResMut<Inventory>) {
    let count = inv.items.len() + 1;
    inv.items.push(Item { id: "apple".to_owned(), count: count as u32 });
}

fn remove_item(In(ctx): In<BevueCtx>, mut inv: ResMut<Inventory>) {
    let index: usize = ctx.get_args().unwrap();
    inv.items.remove(index);
}

// a normal bevy system. this will be called whenever inventory changes.
fn inventory_changed(inv: Res<Inventory>) {
    info!("inventory changed!");
    for (i, item) in inv.items.iter().enumerate() {
        println!("{}. id={}, count={}", i, item.id, item.count);
    }
}

fn main() {
    let mut app = App::new();

    app.add_plugins((BevueAssetPlugin, DefaultPlugins, BevueRuntimePlugin))
        .register_props::<Inventory>()
        .init_resource::<Inventory>()
        .register_props::<PlayerName>()
        .insert_resource(PlayerName { value: "John".to_owned() })
        // `<ItemRow/>` in a template renders this file.
        .register_component("Item", "item_row.html")
        .add_systems(Startup, startup)
        .add_systems(Update, inventory_changed.run_if(resource_changed::<Inventory>));

    app.run();
}

fn startup(asset_server: Res<AssetServer>, mut cmds: Commands) {
    cmds.spawn(Camera2d);

    // `Inventory` and `PlayerName` are declared in the `<props>` of the template.
    cmds.spawn(
        Template::new(asset_server.load("inventory.html"))
            .with_function("add_item", add_item)
            .with_function("remove_item", remove_item),
    );
}

