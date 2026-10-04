use bevy::{
    app::{App, Plugin, PostUpdate},
    asset::AssetEventSystems,
    ecs::schedule::IntoScheduleConfigs,
};
use bevy_flair::{FlairPlugin, parser::FlairCssParserPlugin};

use crate::asset::BevueAssetPlugin;

pub mod interpolation;
pub mod render;

pub use crate::core::template::TemplateRef;
pub use render::{
    apply_template_classes_and_styles, on_render_template, watch_template_components, Template,
    TemplateRendered, WaitingForTemplate,
};

pub struct BevueRuntimePlugin;

impl Plugin for BevueRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<BevueAssetPlugin>() {
            app.add_plugins(BevueAssetPlugin);
        }
        if !app.is_plugin_added::<FlairCssParserPlugin>() {
            app.add_plugins(FlairPlugin);
        }
        app.add_observer(on_render_template);
        app.add_systems(
            PostUpdate,
            (
                watch_template_components.after(AssetEventSystems),
                apply_template_classes_and_styles.after(watch_template_components),
            ),
        );
    }
}


