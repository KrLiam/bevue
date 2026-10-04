use bevy::{
    app::{App, Plugin, PostUpdate},
    asset::AssetEventSystems,
    ecs::schedule::IntoScheduleConfigs,
};

use crate::{
    asset::BevueAssetPlugin,
    runtime::render::{on_render_template, watch_template_components},
};

pub mod interpolation;
pub mod render;

pub use render::{Template, TemplateRendered, WaitingForTemplate};

pub struct BevueRuntimePlugin;

impl Plugin for BevueRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<BevueAssetPlugin>() {
            app.add_plugins(BevueAssetPlugin);
        }
        app.add_observer(on_render_template);
        app.add_systems(
            PostUpdate,
            watch_template_components.after(AssetEventSystems),
        );
    }
}


