// Lets `#[derive(Props)]` refer to `::bevue` from inside this crate.
extern crate self as bevue;

pub mod asset;
pub mod core;
pub mod parser;
pub mod runtime;

pub use core::template::{TemplateRef, TemplateStyle};
use bevy::app::Plugin;
pub use runtime::{
    BevueComponents, BevueComponentsAppExt, BevueCtx, BevueEvent, BevueFunctionsAppExt, BevueProps, BevuePropsAppExt, BevueSystems, FromArgs, PropsComponent,
    PropsResource, RegisterableProps, Template,
};
pub use core::value::Value;

pub use core::props::{Props, PropsExt};
pub use bevue_macros::Props;

use crate::{
    asset::BevueAssetPlugin,
    runtime::BevueRuntimePlugin,
};

/// Adds everything Bevue needs: [`BevueAssetPlugin`] and [`runtime::BevueRuntimePlugin`].
///
/// Add it after Bevy's `AssetPlugin`, which `DefaultPlugins` includes.
pub struct BevuePlugin;

impl Plugin for BevuePlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_plugins((BevueAssetPlugin, BevueRuntimePlugin));
    }
}

#[cfg(test)]
mod tests {
    use bevy::{MinimalPlugins, app::App, asset::{AssetPlugin, Assets}};

    use super::*;

    #[test]
    fn plugin() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevuePlugin));
        app.finish();
        app.update();
        assert!(app.is_plugin_added::<BevueAssetPlugin>());
        assert!(app.is_plugin_added::<runtime::BevueRuntimePlugin>());
        assert!(app.world().contains_resource::<Assets<core::template::BevueTemplate>>());
    }
}
