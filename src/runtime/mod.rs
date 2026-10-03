use bevy::app::{App, Plugin};

use crate::runtime::render::on_render_template;

pub mod interpolation;
pub mod render;

pub struct BevueRuntimePlugin;

impl Plugin for BevueRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_render_template);
    }
}
