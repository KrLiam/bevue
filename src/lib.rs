// Lets `#[derive(Props)]` refer to `::bevue` from inside this crate.
extern crate self as bevue;

pub mod asset;
pub mod core;
pub mod parser;
pub mod runtime;

pub use asset::{BevueAssetPlugin, BevueAssetRegistry, BevueAssets};
pub use core::template::{TemplateRef, TemplateStyle};
pub use runtime::{
    BevueComponents, BevueComponentsAppExt, BevueCtx, BevueEvent, BevueFunctionsAppExt, BevueProps, BevuePropsAppExt, BevueSystems, FromArgs, PropsComponent,
    PropsResource, RegisterableProps, Template,
};
pub use core::value::Value;

pub use core::props::{Props, PropsExt};
pub use bevue_macros::Props;

#[doc(hidden)]
pub mod __private {
    pub use bevy::platform::collections::HashMap;
}
