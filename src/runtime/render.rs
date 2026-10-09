use std::{collections::HashSet, path::Path, sync::Arc};

use bevy::{
    asset::{AssetEvent, AssetId, AssetServer, Assets, Handle},
    ecs::{
        change_detection::{DetectChanges, DetectChangesMut, Ref},
        component::Component,
        entity::Entity,
        error::FallbackErrorHandler,
        event::Event,
        message::MessageReader,
        observer::On,
        query::{Changed, QueryState, With},
        system::{Command, Commands, In, Query, Res, command::run_system_cached_with},
        world::World,
    },
    log::{error, warn},
};
use bevy_flair::prelude::{Styled, StyleSheet};

use crate::asset::BevueAssetRegistry;
use crate::core::template::{BevueTemplate, TemplateRef};
use crate::core::value::Value;
use crate::runtime::{instance::TemplateInstance, props::BevueProps};

#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Template(pub Handle<BevueTemplate>);

impl Template {
    pub fn new(handle: Handle<BevueTemplate>) -> Self {
        Self(handle)
    }

    pub fn handle(&self) -> &Handle<BevueTemplate> {
        &self.0
    }
}

impl From<Handle<BevueTemplate>> for Template {
    fn from(handle: Handle<BevueTemplate>) -> Self {
        Self(handle)
    }
}


#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateRendered(pub AssetId<BevueTemplate>);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitingForTemplate;

#[derive(Event, Clone)]
pub struct RenderTemplate {
    pub template: Handle<BevueTemplate>,
    pub ui_root: Option<Entity>,
}

pub fn on_render_template(
    event: On<RenderTemplate>,
    mut commands: Commands,
    error_handler: Option<Res<FallbackErrorHandler>>,
) {
    let error_handler = error_handler.map(|h| *h).unwrap_or_default();
    commands.queue(
        run_system_cached_with(render_template, event.clone())
            .handle_error_with(error_handler.0),
    );
}

pub fn watch_template_components(
    mut commands: Commands,
    assets: Res<Assets<BevueTemplate>>,
    mut asset_events: MessageReader<AssetEvent<BevueTemplate>>,
    changed_templates: Query<(Entity, &Template), Changed<Template>>,
    waiting_templates: Query<(Entity, &Template), With<WaitingForTemplate>>,
    all_templates: Query<(Entity, &Template)>,
) {
    let mut triggered_entities = HashSet::new();

    // 1. Process modified or reloaded asset events (for hot reload / file watching).
    let mut modified_asset_ids = HashSet::new();
    for event in asset_events.read() {
        match event {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => {
                modified_asset_ids.insert(id);
            }
            _ => {}
        }
    }

    if !modified_asset_ids.is_empty() {
        for (entity, template) in &all_templates {
            if modified_asset_ids.contains(&template.0.id()) && assets.contains(&template.0) {
                commands.trigger(RenderTemplate {
                    template: template.0.clone(),
                    ui_root: Some(entity),
                });
                commands.entity(entity).insert(TemplateRendered(template.0.id()));
                commands.entity(entity).remove::<WaitingForTemplate>();
                triggered_entities.insert(entity);
            }
        }
    }

    // 2. Process newly added or changed Template components.
    for (entity, template) in &changed_templates {
        if triggered_entities.contains(&entity) {
            continue;
        }

        if assets.contains(&template.0) {
            commands.trigger(RenderTemplate {
                template: template.0.clone(),
                ui_root: Some(entity),
            });
            commands.entity(entity).insert(TemplateRendered(template.0.id()));
            commands.entity(entity).remove::<WaitingForTemplate>();
            triggered_entities.insert(entity);
        } else {
            commands.entity(entity).insert(WaitingForTemplate);
            commands.entity(entity).remove::<TemplateRendered>();
        }
    }

    // 3. Process entities that are waiting for their asset to load.
    for (entity, template) in &waiting_templates {
        if triggered_entities.contains(&entity) {
            continue;
        }

        if assets.contains(&template.0) {
            commands.trigger(RenderTemplate {
                template: template.0.clone(),
                ui_root: Some(entity),
            });
            commands.entity(entity).insert(TemplateRendered(template.0.id()));
            commands.entity(entity).remove::<WaitingForTemplate>();
            triggered_entities.insert(entity);
        }
    }
}

pub fn render_template(input: In<RenderTemplate>, world: &mut World) {
    let template = {
        let Some(templates) = world.get_resource::<Assets<BevueTemplate>>() else {
            error!("Assets<BevueTemplate> resource not found");
            return;
        };
        let Some(template) = templates.get(&input.template) else {
            warn!("BevueTemplate asset not loaded yet");
            return;
        };
        Arc::new(template.clone())
    };

    let root_entity = match input.ui_root {
        Some(entity) => entity,
        None => world.spawn_empty().id(),
    };

    let mut root = world.entity_mut(root_entity);
    if !root.contains::<Template>() {
        root.insert(Template(input.template.clone()));
    }
    // Discard any previous render (e.g. on hot reload).
    root.remove::<TemplateInstance>();
    root.despawn_children();

    let mut props = take_props(world, root_entity);
    match TemplateInstance::render(world, root_entity, template, &props) {
        Ok(instance) => {
            world.entity_mut(root_entity).insert(instance);
        }
        Err(e) => error!("Could not render template {:?}: {e}", input.template.path()),
    }
    // The render reflects the current props, so pending changes are already applied.
    props.clear_changes();
    restore_props(world, root_entity, props);
}

/// Applies prop changes to the UI of every rendered template whose [`BevueProps`] changed.
pub fn update_templates(
    world: &mut World,
    changed: &mut QueryState<(Entity, Ref<TemplateInstance>), Changed<BevueProps>>,
) {
    // Templates rendered since the last run already reflect the current props.
    let entities: Vec<Entity> =
        changed.iter(world).filter(|(_, instance)| !instance.is_added()).map(|(entity, _)| entity).collect();
    for entity in entities {
        let Some(mut instance) = world
            .get_mut::<TemplateInstance>(entity)
            .map(|mut instance| std::mem::take(instance.bypass_change_detection()))
        else {
            continue;
        };
        let mut props = take_props(world, entity);

        // Props without any change flags were replaced or added wholesale.
        let full = !props.is_dirty();
        instance.update(world, &props, full);

        props.clear_changes();
        restore_props(world, entity, props);
        if let Some(mut slot) = world.get_mut::<TemplateInstance>(entity) {
            *slot.bypass_change_detection() = instance;
        }
    }
}

/// Takes the props value out of `entity` without triggering change detection.
fn take_props(world: &mut World, entity: Entity) -> Value {
    world
        .get_mut::<BevueProps>(entity)
        .map(|mut props| std::mem::take(&mut props.bypass_change_detection().value))
        .unwrap_or_else(Value::empty_dict)
}

/// Puts back a value taken with [`take_props`].
fn restore_props(world: &mut World, entity: Entity, value: Value) {
    if let Some(mut props) = world.get_mut::<BevueProps>(entity) {
        props.bypass_change_detection().value = value;
    }
}

/// Attaches the template's stylesheet to its root entity. Classes are applied when nodes are
/// spawned and kept up to date by bindings.
pub fn apply_template_classes_and_styles(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    bevue_assets: Res<BevueAssetRegistry>,
    template_nodes: Query<(Entity, &TemplateRef), Changed<TemplateRef>>,
    root_templates: Query<&Template>,
    templates: Res<Assets<BevueTemplate>>,
) {
    for (entity, template_ref) in &template_nodes {
        if template_ref.root != entity {
            continue;
        }
        let Ok(root_template) = root_templates.get(entity) else {
            continue;
        };
        let Some(template) = templates.get(&root_template.0) else {
            continue;
        };
        let Some(style) = template.get(template_ref.node).and_then(|node| node.style.as_ref()) else {
            continue;
        };

        let mut style_path = style.src.as_ref().map(|s| s.to_string());

        let inline = style.inline.trim();
        if !inline.is_empty() {
            let html_path_str = if let Some(p) = root_template.0.path() {
                p.path().to_string_lossy().to_string()
            } else {
                format!("inline_{entity}")
            };

            let css_file_name = format!("{html_path_str}.css");
            let css_asset_path = format!("bevue://{css_file_name}");

            let mut full_css = String::new();
            if let Some(src) = &style.src {
                let import_stmt = format!("@import \"{src}\";\n");
                full_css.push_str(&import_stmt);
            }
            full_css.push_str(&style.inline);

            let path = Path::new(&css_file_name);
            bevue_assets.dir.insert_asset_text(path, &full_css);
            asset_server.reload(&css_asset_path);

            style_path = Some(css_asset_path);
        }

        if let Some(path) = style_path {
            let style_handle: Handle<StyleSheet> = asset_server.load(path);
            commands.entity(entity).insert(Styled::new(style_handle));
        }
    }
}


#[cfg(test)]
mod test {
    use bevy::{
        app::App,
        asset::{AssetPlugin, Assets},
        ecs::hierarchy::ChildOf,
        ui::{Node, Val, widget::Text},
        MinimalPlugins,
    };

    use crate::{
        core::template::{BevueTemplate, TemplateNode, TemplateText},
        runtime::{
            render::{RenderTemplate, Template, WaitingForTemplate},
            BevueRuntimePlugin,
        },
    };

    #[test]
    fn render() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1).with_child(4),
            TemplateNode::new("div").with_child(2).with_child(3),
            TemplateNode::new("span").with_text(TemplateText::new(["You".into()])),
            TemplateNode::new("span").with_text(TemplateText::new(["!".into()])),
            TemplateNode::new("p").with_text(TemplateText::new(["Hello.".into()])),
        ]);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        app.world_mut()
            .trigger(RenderTemplate { template: handle, ui_root: None });
        app.update();
    }

    #[test]
    fn render_parsed() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let source = r#"
            <div>
                <p>Hello.</p>
                <span>World!</span>
            </div>
        "#;
        let template = crate::parser::parse_template(source).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        app.world_mut()
            .trigger(RenderTemplate { template: handle, ui_root: None });
        app.update();
    }

    #[test]
    fn test_template_component_renders_when_asset_loaded() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1).with_child(2),
            TemplateNode::new("p").with_text(TemplateText::new(["Title".into()])),
            TemplateNode::new("span").with_text(TemplateText::new(["Subtitle".into()])),
        ]);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        let entity = app.world_mut().spawn(Template(handle)).id();

        app.update();

        // Check that children were spawned under `entity`
        let mut children_texts = Vec::new();
        let mut query = app.world_mut().query::<(Option<&Text>, &ChildOf)>();
        for (text, child_of) in query.iter(app.world()) {
            if child_of.0 == entity && let Some(t) = text {
                children_texts.push(t.0.clone());
            }
        }

        assert_eq!(children_texts.len(), 2);
        assert!(children_texts.contains(&"Title".to_string()));
        assert!(children_texts.contains(&"Subtitle".to_string()));
        assert!(app.world().get::<WaitingForTemplate>(entity).is_none());
    }

    #[test]
    fn test_template_component_waits_for_deferred_load() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .reserve_handle();

        let entity = app.world_mut().spawn(Template(handle.clone())).id();

        // First update: asset is not loaded yet
        app.update();

        assert!(app.world().get::<WaitingForTemplate>(entity).is_some());

        let mut child_count = 0;
        let mut query = app.world_mut().query::<&ChildOf>();
        for child_of in query.iter(app.world()) {
            if child_of.0 == entity {
                child_count += 1;
            }
        }
        assert_eq!(child_count, 0);

        // Now load the asset into Assets<BevueTemplate>
        let template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1),
            TemplateNode::new("h1").with_text(TemplateText::new(["Deferred".into()])),
        ]);
        let _ = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .insert(&handle, template);

        // Second update: should detect asset is loaded and render
        app.update();

        assert!(app.world().get::<WaitingForTemplate>(entity).is_none());

        let mut texts = Vec::new();
        let mut query = app.world_mut().query::<(Option<&Text>, &ChildOf)>();
        for (text, child_of) in query.iter(app.world()) {
            if child_of.0 == entity && let Some(t) = text {
                texts.push(t.0.clone());
            }
        }
        assert_eq!(texts, vec!["Deferred".to_string()]);
    }

    #[test]
    fn test_template_component_watches_for_modifications() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let initial_template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1),
            TemplateNode::new("p").with_text(TemplateText::new(["Initial".into()])),
        ]);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(initial_template);

        let entity = app.world_mut().spawn(Template(handle.clone())).id();
        app.update();

        let count_children = |app: &mut App| -> (usize, Vec<String>) {
            let mut texts = Vec::new();
            let mut count = 0;
            let mut query = app.world_mut().query::<(Option<&Text>, &ChildOf)>();
            for (text, child_of) in query.iter(app.world()) {
                if child_of.0 == entity {
                    count += 1;
                    if let Some(t) = text {
                        texts.push(t.0.clone());
                    }
                }
            }
            (count, texts)
        };

        let (count, texts) = count_children(&mut app);
        assert_eq!(count, 1);
        assert_eq!(texts, vec!["Initial".to_string()]);

        // Modify the asset (simulating file watcher reload)
        let updated_template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1).with_child(2),
            TemplateNode::new("p").with_text(TemplateText::new(["Updated 1".into()])),
            TemplateNode::new("p").with_text(TemplateText::new(["Updated 2".into()])),
        ]);
        {
            let mut assets = app.world_mut().resource_mut::<Assets<BevueTemplate>>();
            let mut asset = assets.get_mut(&handle).unwrap();
            *asset = updated_template;
        }

        // Update to trigger file watching re-render
        app.update();

        let (new_count, new_texts) = count_children(&mut app);
        assert_eq!(new_count, 2);
        assert_eq!(new_texts, vec!["Updated 1".to_string(), "Updated 2".to_string()]);
    }

    #[test]
    fn test_template_preserves_custom_node() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let template = BevueTemplate::new(vec![
            TemplateNode::new("div").with_child(1),
            TemplateNode::new("span").with_text(TemplateText::new(["Content".into()])),
        ]);

        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        let custom_node = Node {
            left: Val::Px(42.0),
            ..Default::default()
        };

        let entity = app.world_mut().spawn((custom_node, Template(handle))).id();
        app.update();

        let node = app.world().get::<Node>(entity).unwrap();
        assert_eq!(node.left, Val::Px(42.0));
    }

    #[test]
    fn test_template_ref_class_list_and_styled() {
        use bevy::ecs::entity::Entity;
        use bevy_flair::prelude::{ClassList, Styled};
        use crate::core::template::{NodeId, TemplateRef};

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let source = r#"
            <template>
                <div class="root">
                    <p>Hello!</p>
                    <div class="inner">
                        <span>You</span>
                    </div>
                </div>
            </template>

            <style src="css/simple.css"></style>
        "#;
        let template = crate::parser::parse_template(source).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        let entity = app.world_mut().spawn(Template(handle)).id();
        app.update();

        // 1. Verify root entity
        let root_ref = app.world().get::<TemplateRef>(entity).expect("Root must have TemplateRef");
        assert_eq!(root_ref.root, entity);
        assert_eq!(root_ref.node, NodeId(0));

        let root_classes = app.world().get::<ClassList>(entity).expect("Root must have ClassList");
        assert!(root_classes.contains("root"));

        let root_styled = app.world().get::<Styled>(entity).expect("Root must have Styled component");
        match root_styled {
            Styled::StyleSheet(handle) => {
                assert_eq!(handle.path().map(|p| p.path()), Some(std::path::Path::new("css/simple.css")));
            }
            _ => panic!("Expected Styled::StyleSheet"),
        }

        // 2. Verify children entities
        let mut query = app.world_mut().query::<(Entity, &TemplateRef, Option<&ClassList>, Option<&Styled>)>();
        let mut found_inner = false;
        let mut found_p = false;
        let mut found_span = false;

        for (_e, t_ref, class_list, styled) in query.iter(app.world()) {
            assert_eq!(t_ref.root, entity);

            if t_ref.node == NodeId(2) {
                // <div class="inner">
                found_inner = true;
                let classes = class_list.expect("Inner div must have ClassList");
                assert!(classes.contains("inner"));
                assert!(!matches!(styled, Some(Styled::StyleSheet(_))), "Non-root node should not have Styled::StyleSheet");
            } else if t_ref.node == NodeId(1) {
                // <p>
                found_p = true;
                assert!(class_list.is_none(), "p has no class");
                assert!(!matches!(styled, Some(Styled::StyleSheet(_))));
            } else if t_ref.node == NodeId(3) {
                // <span>
                found_span = true;
                assert!(class_list.is_none(), "span has no class");
                assert!(!matches!(styled, Some(Styled::StyleSheet(_))));
            }
        }

        assert!(found_inner);
        assert!(found_p);
        assert!(found_span);
    }

    #[test]
    fn test_template_ref_inline_style() {
        use bevy_flair::prelude::{ClassList, Styled};
        use crate::core::template::{NodeId, TemplateRef};
        use crate::asset::{BevueAssetPlugin, BevueAssetRegistry};

        let mut app = App::new();
        app.add_plugins((BevueAssetPlugin, MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let source = r#"
            <template>
                <div class="root">
                    <p>Hello!</p>
                </div>
            </template>

            <style>
                .root {
                    background-color: red;
                }
            </style>
        "#;
        let template = crate::parser::parse_template(source).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        let entity = app.world_mut().spawn(Template(handle)).id();
        app.update();

        // 1. Verify root entity
        let root_ref = app.world().get::<TemplateRef>(entity).expect("Root must have TemplateRef");
        assert_eq!(root_ref.root, entity);
        assert_eq!(root_ref.node, NodeId(0));

        let root_classes = app.world().get::<ClassList>(entity).expect("Root must have ClassList");
        assert!(root_classes.contains("root"));

        // 2. Verify root entity has Styled pointing to fabricated asset in bevue://
        let root_styled = app.world().get::<Styled>(entity).expect("Root must have Styled component");
        match root_styled {
            Styled::StyleSheet(handle) => {
                let path = handle.path().expect("Fabricated stylesheet must have an AssetPath");
                assert_eq!(path.source().as_str(), Some("bevue"));
                assert!(path.path().to_string_lossy().ends_with(".css"));

                // Verify file content in BevueAssetRegistry
                let registry = app.world().resource::<BevueAssetRegistry>();
                let asset_data = registry.get_asset(path.path()).expect("Asset must exist in BevueAssetRegistry");
                let css_text = std::str::from_utf8(asset_data.value()).unwrap();
                assert!(css_text.contains(".root"));
                assert!(css_text.contains("background-color: red;"));
            }
            _ => panic!("Expected Styled::StyleSheet"),
        }
    }

    #[test]
    fn test_template_ref_inline_and_src_style() {
        use bevy_flair::prelude::Styled;
        use crate::asset::{BevueAssetPlugin, BevueAssetRegistry};

        let mut app = App::new();
        app.add_plugins((BevueAssetPlugin, MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();

        let source = r#"
            <template>
                <div class="root">
                    <p>Hello!</p>
                </div>
            </template>

            <style src="shared.css">
                .root {
                    color: blue;
                }
            </style>
        "#;
        let template = crate::parser::parse_template(source).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<BevueTemplate>>()
            .add(template);

        let entity = app.world_mut().spawn(Template(handle)).id();
        app.update();

        let root_styled = app.world().get::<Styled>(entity).expect("Root must have Styled component");
        match root_styled {
            Styled::StyleSheet(handle) => {
                let path = handle.path().expect("Fabricated stylesheet must have an AssetPath");
                assert_eq!(path.source().as_str(), Some("bevue"));

                let registry = app.world().resource::<BevueAssetRegistry>();
                let asset_data = registry.get_asset(path.path()).expect("Asset must exist in BevueAssetRegistry");
                let css_text = std::str::from_utf8(asset_data.value()).unwrap();
                assert!(css_text.contains("@import \"shared.css\";"));
                assert!(css_text.contains("color: blue;"));
            }
            _ => panic!("Expected Styled::StyleSheet"),
        }
    }
}