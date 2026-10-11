use std::{borrow::Cow, collections::HashSet, path::Path, sync::Arc};

use bevy::{
    asset::{AssetEvent, AssetId, AssetLoadFailedEvent, AssetServer, Assets, Handle},
    ecs::{
        change_detection::{DetectChanges, DetectChangesMut, Ref},
        component::Component,
        entity::Entity,
        error::FallbackErrorHandler,
        event::Event,
        hierarchy::ChildOf,
        message::MessageReader,
        observer::On,
        lifecycle::HookContext,
        query::{Changed, QueryState, With},
        system::{Command, Commands, In, IntoSystem, Query, Res, command::run_system_cached_with},
        world::{DeferredWorld, World},
    },
    log::{debug, error, warn},
    platform::time::Instant,
};
use bevy_flair::prelude::{Styled, StyleSheet};

use crate::asset::BevueAssetRegistry;
use crate::core::compile::CompiledTemplate;
use crate::core::template::{BevueTemplate, TemplateRef};
use crate::core::value::Value;
use crate::runtime::{
    components::{load_components, ComponentDependencies, WorldComponents},
    functions::{
        assert_not_reserved, box_function, BevueCtx, BevueFunctionFactory, BoxedBevueFunction, TemplateFunctions,
    },
    instance::TemplateInstance,
    props::{mount_declared_props, BevueProps},
};

/// Renders a [`BevueTemplate`] asset under this entity.
#[derive(Component, Clone)]
#[component(on_insert = register_template_functions, on_discard = unregister_template_functions)]
pub struct Template {
    pub handle: Handle<BevueTemplate>,
    /// Functions only this template can call, registered while the component is present.
    functions: Vec<(Cow<'static, str>, BevueFunctionFactory)>,
}

impl Template {
    pub fn new(handle: Handle<BevueTemplate>) -> Self {
        Self { handle, functions: Vec::new() }
    }

    pub fn handle(&self) -> &Handle<BevueTemplate> {
        &self.handle
    }

    /// Makes `system` callable as `name(...)` from this template only, taking precedence over a
    /// global function of the same name. The system is registered when the component is inserted
    /// and unregistered when it is removed or replaced. Panics if `name` starts with `$`.
    pub fn with_function<O, M>(
        mut self,
        name: impl Into<Cow<'static, str>>,
        system: impl IntoSystem<In<BevueCtx>, O, M> + Clone + Send + Sync + 'static,
    ) -> Self
    where
        O: Into<Value> + 'static,
    {
        let name = name.into();
        assert_not_reserved(&name);
        let factory: BevueFunctionFactory = Arc::new(move || box_function(system.clone()));
        self.functions.retain(|(existing, _)| *existing != name);
        self.functions.push((name, factory));
        self
    }

    /// The names of the functions registered with [`Template::with_function`].
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.functions.iter().map(|(name, _)| name.as_ref())
    }
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.handle == other.handle && self.function_names().eq(other.function_names())
    }
}

impl Eq for Template {}

impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Template")
            .field("handle", &self.handle)
            .field("functions", &self.function_names().collect::<Vec<_>>())
            .finish()
    }
}

impl From<Handle<BevueTemplate>> for Template {
    fn from(handle: Handle<BevueTemplate>) -> Self {
        Self::new(handle)
    }
}

/// Registers the template's own functions as one-shot systems in a [`TemplateFunctions`].
fn register_template_functions(mut world: DeferredWorld, ctx: HookContext) {
    let Some(template) = world.get::<Template>(ctx.entity) else { return };
    if template.functions.is_empty() {
        return;
    }
    let systems: Vec<(String, BoxedBevueFunction)> =
        template.functions.iter().map(|(name, factory)| (name.to_string(), factory())).collect();
    let entity = ctx.entity;
    world.commands().queue(move |world: &mut World| {
        let mut functions = TemplateFunctions::default();
        for (name, system) in systems {
            functions.0.insert(name, world.register_boxed_system(system));
        }
        match world.get_entity_mut(entity) {
            Ok(mut entity) => {
                entity.insert(functions);
            }
            Err(_) => {
                for id in functions.0.into_values() {
                    let _ = world.unregister_system(id);
                }
            }
        }
    });
}

/// Removes the template's functions; [`TemplateFunctions`] unregisters them when removed.
fn unregister_template_functions(mut world: DeferredWorld, ctx: HookContext) {
    let entity = ctx.entity;
    world.commands().queue(move |world: &mut World| {
        if let Ok(mut entity) = world.get_entity_mut(entity) {
            entity.remove::<TemplateFunctions>();
        }
    });
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
    mut failed_events: MessageReader<AssetLoadFailedEvent<BevueTemplate>>,
    changed_templates: Query<(Entity, &Template), Changed<Template>>,
    waiting_templates: Query<(Entity, &Template), With<WaitingForTemplate>>,
    all_templates: Query<(Entity, &Template, Option<&ComponentDependencies>)>,
) {
    let mut triggered_entities = HashSet::new();

    // 1. Process modified or reloaded asset events (for hot reload / file watching).
    // Assets that became available: templates waiting for a component render again.
    let mut modified_asset_ids = HashSet::new();
    let mut added_asset_ids = HashSet::new();
    for event in asset_events.read() {
        match event {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => {
                modified_asset_ids.insert(id);
            }
            AssetEvent::Added { id } => {
                added_asset_ids.insert(id);
            }
            _ => {}
        }
    }
    // A component that failed to load is no longer waited for.
    let failed: Vec<_> = failed_events.read().map(|event| event.id).collect();
    added_asset_ids.extend(&failed);

    if !modified_asset_ids.is_empty() || !added_asset_ids.is_empty() {
        for (entity, template, dependencies) in &all_templates {
            // A template is rendered again when one of the components it uses changes.
            let modified = modified_asset_ids.contains(&template.handle.id())
                || dependencies.is_some_and(|d| {
                    d.assets.iter().any(|id| modified_asset_ids.contains(id))
                        || d.pending.iter().any(|id| added_asset_ids.contains(id))
                });
            if modified && assets.contains(&template.handle) {
                commands.trigger(RenderTemplate {
                    template: template.handle.clone(),
                    ui_root: Some(entity),
                });
                commands.entity(entity).insert(TemplateRendered(template.handle.id()));
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

        if assets.contains(&template.handle) {
            commands.trigger(RenderTemplate {
                template: template.handle.clone(),
                ui_root: Some(entity),
            });
            commands.entity(entity).insert(TemplateRendered(template.handle.id()));
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

        if assets.contains(&template.handle) {
            commands.trigger(RenderTemplate {
                template: template.handle.clone(),
                ui_root: Some(entity),
            });
            commands.entity(entity).insert(TemplateRendered(template.handle.id()));
            commands.entity(entity).remove::<WaitingForTemplate>();
            triggered_entities.insert(entity);
        }
    }
}

pub fn render_template(input: In<RenderTemplate>, world: &mut World) {
    let start = Instant::now();
    load_components(world);
    let (template, compiled, dependencies, loading) = {
        let Some(templates) = world.get_resource::<Assets<BevueTemplate>>() else {
            error!("Assets<BevueTemplate> resource not found");
            return;
        };
        let Some(template) = templates.get(&input.template) else {
            warn!("BevueTemplate asset not loaded yet");
            return;
        };
        let components = WorldComponents::new(world, templates);
        let compiled = CompiledTemplate::compile_with(template, &components);
        let loading = components.is_loading();
        (Arc::new(template.clone()), compiled, components.into_dependencies(), loading)
    };

    let root_entity = match input.ui_root {
        Some(entity) => entity,
        None => world.spawn_empty().id(),
    };

    let mut root = world.entity_mut(root_entity);
    if !root.contains::<Template>() {
        root.insert(Template::new(input.template.clone()));
    }
    // Wait for the components that are still loading instead of rendering without them. The
    // template is rendered when they arrive or fail to load.
    if loading {
        root.insert(dependencies);
        return;
    }
    // Discard any previous render (e.g. on hot reload).
    root.remove::<TemplateInstance>();
    root.despawn_children();
    if dependencies.assets.is_empty() {
        root.remove::<ComponentDependencies>();
    } else {
        root.insert(dependencies);
    }

    let compiled = match compiled {
        Ok(compiled) => Arc::new(compiled),
        Err(e) => {
            error!("Could not render template {:?}: {e}", input.template.path());
            return;
        }
    };
    mount_declared_props(world, root_entity, &compiled.props);

    let mut props = take_props(world, root_entity);
    let instance = TemplateInstance::render_compiled(world, root_entity, template, compiled, &props);
    let stats = instance.stats();
    world.entity_mut(root_entity).insert(instance);
    debug!(
        "rendered {} on {root_entity}: {} bindings, {} blocks, {} list items, in {:?}",
        template_name(&input.template),
        stats.bindings,
        stats.blocks,
        stats.items,
        start.elapsed()
    );
    // The render reflects the current props, so pending changes are already applied.
    props.clear_changes();
    restore_props(world, root_entity, props);
}

/// A name for a template in logs: its asset path, or its id for templates added in code.
fn template_name(handle: &Handle<BevueTemplate>) -> String {
    match handle.path() {
        Some(path) => path.to_string(),
        None => format!("{:?}", handle.id()),
    }
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
        let start = Instant::now();
        let Some(mut instance) = world
            .get_mut::<TemplateInstance>(entity)
            .map(|mut instance| std::mem::take(instance.bypass_change_detection()))
        else {
            continue;
        };
        let mut props = take_props(world, entity);

        // Props without any change flags were replaced or added wholesale.
        let full = !props.is_dirty();
        let stats = instance.update(world, &props, full);

        props.clear_changes();
        restore_props(world, entity, props);
        if let Some(mut slot) = world.get_mut::<TemplateInstance>(entity) {
            *slot.bypass_change_detection() = instance;
        }
        debug!(
            "updated template on {entity}: {} bindings, {} lists{} in {:?}",
            stats.fired,
            stats.reconciled,
            if stats.full { ", full refresh," } else { "" },
            start.elapsed()
        );
    }
}

/// Takes the props value out of `entity` without triggering change detection.
pub(crate) fn take_props(world: &mut World, entity: Entity) -> Value {
    world
        .get_mut::<BevueProps>(entity)
        .map(|mut props| std::mem::take(&mut props.bypass_change_detection().value))
        .unwrap_or_else(Value::empty_dict)
}

/// Puts back a value taken with [`take_props`].
pub(crate) fn restore_props(world: &mut World, entity: Entity, value: Value) {
    if let Some(mut props) = world.get_mut::<BevueProps>(entity) {
        props.bypass_change_detection().value = value;
    }
}

/// Attaches the template's stylesheet to its root entity: its own styles and those of the
/// components it uses. Classes are applied when nodes are spawned and kept up to date by bindings.
pub fn apply_template_classes_and_styles(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    bevue_assets: Res<BevueAssetRegistry>,
    template_nodes: Query<(Entity, &TemplateRef), Changed<TemplateRef>>,
    root_templates: Query<(&Template, &TemplateInstance)>,
    parents: Query<&ChildOf>,
    styled: Query<&Styled>,
) {
    for (entity, template_ref) in &template_nodes {
        if template_ref.root != entity {
            continue;
        }
        let Ok((root_template, instance)) = root_templates.get(entity) else {
            continue;
        };

        // `bevy_flair` only applies inline styles to entities that have a stylesheet, their own or
        // an inherited one. A template with `style` attributes and neither gets an empty one.
        let needs_stylesheet = || {
            instance.has_inline_styles()
                && std::iter::once(entity)
                    .chain(parents.iter_ancestors(entity))
                    .all(|e| matches!(styled.get(e), Ok(Styled::Inherited) | Err(_)))
        };
        let style_path = match instance.styles() {
            [] if !needs_stylesheet() => None,
            // A single external stylesheet is used as it is.
            [style] if style.inline.trim().is_empty() => style.src.as_ref().map(|s| s.to_string()),
            styles => {
                let html_path_str = if let Some(p) = root_template.handle.path() {
                    p.path().to_string_lossy().to_string()
                } else {
                    format!("inline_{entity}")
                };

                let css_file_name = format!("{html_path_str}.css");
                let css_asset_path = format!("bevue://{css_file_name}");

                // `@import` rules must come before any other rule.
                let mut full_css = String::new();
                for src in styles.iter().filter_map(|style| style.src.as_ref()) {
                    full_css.push_str(&format!("@import \"{src}\";\n"));
                }
                for style in styles {
                    full_css.push_str(&style.inline);
                    full_css.push('\n');
                }

                // Only reload the stylesheet if this render changed it.
                let path = Path::new(&css_file_name);
                let unchanged =
                    bevue_assets.dir.get_asset(path).is_some_and(|asset| asset.value() == full_css.as_bytes());
                if !unchanged {
                    bevue_assets.dir.insert_asset_text(path, &full_css);
                    asset_server.reload(&css_asset_path);
                }

                Some(css_asset_path)
            }
        };

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

        let entity = app.world_mut().spawn(Template::new(handle)).id();

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

        let entity = app.world_mut().spawn(Template::new(handle.clone())).id();

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

        let entity = app.world_mut().spawn(Template::new(handle.clone())).id();
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

        // Update to trigger file watching re-render. The asset event is emitted at the end of a
        // frame and read on the next one.
        app.update();
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

        let entity = app.world_mut().spawn((custom_node, Template::new(handle))).id();
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

        let entity = app.world_mut().spawn(Template::new(handle)).id();
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

        let entity = app.world_mut().spawn(Template::new(handle)).id();
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

        let entity = app.world_mut().spawn(Template::new(handle)).id();
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