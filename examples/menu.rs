use bevue::{BevueCtx, BevuePlugin, BevuePropsAppExt, Props, Template};
use bevy::{
    DefaultPlugins,
    app::{App, Startup},
    asset::AssetServer,
    camera::Camera2d,
    ecs::{
        component::Component,
        system::{Commands, In, Query, Res},
    },
};

#[derive(Component, Props)]
pub struct Menu {
    pub user_name: String,
    pub options: Vec<String>,
    pub answered: bool,
}

fn answer(In(ctx): In<BevueCtx>, mut menu: Query<&mut Menu>) {
    let msg: String = ctx.get_args().unwrap();
    println!("You answered: {}", msg);

    let mut menu = menu.get_mut(ctx.root).unwrap();
    menu.answered = true;
}

fn startup(mut cmds: Commands, server: Res<AssetServer>) {
    cmds.spawn(Camera2d);
    cmds.spawn((
        Template::new(server.load("menu.html")).with_function("answer", answer),
        Menu {
            user_name: "John".to_owned(),
            options: vec!["Fine!".to_owned(), "Not fine!".to_owned()],
            answered: false,
        },
    ));
}

fn main() {
    let mut app = App::new();

    app.add_plugins((DefaultPlugins, BevuePlugin))
        .register_props::<Menu>()
        .add_systems(Startup, startup);

    app.run();
}
