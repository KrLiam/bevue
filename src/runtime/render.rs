use std::collections::HashSet;

use bevy::{
    asset::{AssetEvent, AssetId, AssetServer, Assets, Handle},
    ecs::{
        component::Component,
        entity::Entity,
        error::FallbackErrorHandler,
        event::Event,
        hierarchy::ChildOf,
        message::MessageReader,
        observer::On,
        query::{Changed, With},
        system::{Command, Commands, In, Local, Query, Res, command::run_system_cached_with},
        world::World,
    },
    ui::{Node, widget::Text},
};
use bevy_flair::prelude::{ClassList, Styled, StyleSheet};

use crate::core::template::{
    BevueTemplate, ElementType, Expression, NodeId, TemplateRef, TextFragment,
};

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
    println!("On render template");

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

pub type RenderStackItem = (NodeId, Option<Entity>, Option<Entity>);

pub fn render_template(
    input: In<RenderTemplate>,
    world: &mut World,
    mut stack: Local<Vec<RenderStackItem>>,
) {
    println!("Render template");

    let template = {
        let Some(templates) = world.get_resource::<Assets<BevueTemplate>>() else {
            println!("Assets<BevueTemplate> resource not found");
            return;
        };
        let Some(template) = templates.get(&input.template) else {
            println!("BevueTemplate asset not loaded yet");
            return;
        };
        template.clone()
    };

    let Some(root_idx) = template.root_index() else {
        return;
    };

    let root_entity = match input.ui_root {
        Some(entity) => entity,
        None => world.spawn_empty().id(),
    };

    if !world.entity(root_entity).contains::<Template>() {
        world.entity_mut(root_entity).insert(Template(input.template.clone()));
    }

    stack.clear();
    stack.push((root_idx, Some(root_entity), None));

    while let Some((template_node_idx, node, parent_node)) = stack.pop() {
        let Some(template_node) = template.get(template_node_idx) else {
            continue;
        };

        let node = match node {
            Some(entity) => {
                world.entity_mut(entity).despawn_children();
                entity
            }
            None => world.spawn_empty().id(),
        };

        world.entity_mut(node).insert(TemplateRef {
            root: root_entity,
            node: template_node_idx,
        });

        for &child_idx in template.children(template_node_idx).iter().rev() {
            stack.push((child_idx, None, Some(node)));
        }

        let mut text: Option<String> = None;
        if let Some(template_text) = &template_node.text {
            let mut acc = String::new();
            for fragment in &template_text.0 {
                match fragment {
                    TextFragment::Literal(value) => {
                        acc.push_str(value);
                    }
                    TextFragment::Interpolated(_) => {}
                }
            }
            text = Some(acc);
        }

        match &template_node.element {
            ElementType::Native(name) => {
                println!("element {} {}", name, node);

                let mut entity_mut = world.entity_mut(node);
                if let Some(parent) = parent_node {
                    println!("parent: {parent}");
                    entity_mut.insert(ChildOf(parent));
                }
                if !entity_mut.contains::<Node>() {
                    entity_mut.insert(Node::default());
                }
            }
            ElementType::Component(_) => panic!("Components not supported yet."),
        }

        if let Some(text) = text {
            println!("text: '{text}'");
            world.entity_mut(node).insert(Text::new(text));
        } else if parent_node.is_none() {
            world.entity_mut(node).remove::<Text>();
        }
    }
}

pub fn apply_template_classes_and_styles(
    mut commands: Commands,
    asset_server: Option<Res<AssetServer>>,
    template_nodes: Query<(Entity, &TemplateRef), Changed<TemplateRef>>,
    root_templates: Query<&Template>,
    templates: Res<Assets<BevueTemplate>>,
) {
    for (entity, template_ref) in &template_nodes {
        let Ok(root_template) = root_templates.get(template_ref.root) else {
            continue;
        };

        let Some(template) = templates.get(&root_template.0) else {
            continue;
        };

        let Some(template_node) = template.get(template_ref.node) else {
            continue;
        };

        // 1. Copy the "class" attribute string to bevy flair's ClassList
        for (name, expr) in &template_node.attributes {
            if name == "class" && let Expression::String(class_str) = expr {
                commands.entity(entity).insert(ClassList::new(class_str.as_ref()));
            }
        }

        // 2. If the entity is the root, also get the style data and if src is present, add the Styled component
        if template_ref.root == entity
            && let Some(style) = &template_node.style
            && let Some(src) = &style.src
            && let Some(asset_server) = &asset_server
        {
            let style_handle: Handle<StyleSheet> = asset_server.load(src.to_string());
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
                assert!(styled.is_none(), "Non-root node should not have Styled");
            } else if t_ref.node == NodeId(1) {
                // <p>
                found_p = true;
                assert!(class_list.is_none(), "p has no class");
                assert!(styled.is_none());
            } else if t_ref.node == NodeId(3) {
                // <span>
                found_span = true;
                assert!(class_list.is_none(), "span has no class");
                assert!(styled.is_none());
            }
        }

        assert!(found_inner);
        assert!(found_p);
        assert!(found_span);
    }
}