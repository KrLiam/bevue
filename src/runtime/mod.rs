use bevy::{
    app::{App, Plugin, PostUpdate, PreUpdate}, asset::AssetEventSystems, ecs::{message::Messages, schedule::{IntoScheduleConfigs, SystemSet}}, ui::UiSystems, window::{RequestRedraw, WindowEvent},
};
use bevy_flair::{FlairPlugin, parser::FlairCssParserPlugin};

use crate::asset::BevueAssetPlugin;

pub mod components;
pub mod events;
pub mod functions;
pub mod instance;
pub mod interpolation;
pub mod props;
pub mod render;

pub use crate::core::template::TemplateRef;
pub use components::{BevueComponents, BevueComponentsAppExt, ComponentDependencies};
pub use events::{BevueEvent, LastInteraction};
pub use functions::{BevueCtx, BevueFunctions, BevueFunctionsAppExt, FromArgs, TemplateFunctions};
pub use instance::{TemplateInstance, UpdateStats};
pub use props::{
    BevueProps, BevuePropsAppExt, DeclaredProps, PropsComponent, PropsResource, PropsTypeRegistry, RegisterableProps,
};
pub use render::{
    apply_template_classes_and_styles, on_render_template, update_templates, watch_template_components, Template,
    TemplateRendered, WaitingForTemplate,
};

/// The systems of the Bevue runtime, run in this order in `PostUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum BevueSystems {
    /// Copies changed `Props` components and resources into `BevueProps`.
    SyncProps,
    /// Renders templates whose asset was added, loaded or modified.
    Render,
    /// Applies `BevueProps` changes to rendered templates.
    Update,
    /// Attaches stylesheets.
    Style,
}

pub struct BevueRuntimePlugin;

impl Plugin for BevueRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<BevueAssetPlugin>() {
            app.add_plugins(BevueAssetPlugin);
        }
        if !app.is_plugin_added::<FlairCssParserPlugin>() {
            app.add_plugins(FlairPlugin);
        }
        app.init_resource::<Messages<WindowEvent>>();
        app.init_resource::<Messages<RequestRedraw>>();
        app.add_observer(on_render_template);
        app.add_observer(events::on_bevue_event);
        functions::insert_global_function(app.world_mut(), "$emit".to_owned(), functions::box_function(functions::emit));
        // Runs after `Interaction` is updated, so handlers run before `Update` and their prop
        // changes are rendered in the same frame.
        app.add_systems(PreUpdate, events::emit_interaction_events.after(UiSystems::Focus));
        app.configure_sets(
            PostUpdate,
            (BevueSystems::SyncProps, BevueSystems::Render, BevueSystems::Update, BevueSystems::Style)
                .chain()
                .after(AssetEventSystems),
        );
        app.add_systems(
            PostUpdate,
            (
                (components::load_components, watch_template_components)
                    .chain()
                    .in_set(BevueSystems::Render),
                update_templates.in_set(BevueSystems::Update),
                apply_template_classes_and_styles.in_set(BevueSystems::Style),
            ),
        );
    }
}

#[cfg(test)]
mod tests;
