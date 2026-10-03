use bevy::{ecs::{entity::Entity, error::FallbackErrorHandler, event::Event, hierarchy::{ChildOf, Children}, observer::On, query::QueryState, system::{Command, Commands, In, Local, Res, command::run_system_cached_with}, world::World}, ui::{Node, widget::Text}};

use crate::core::template::{ElementType, TemplateNode, TemplateText, TextFragment};

#[derive(Event, Clone)]
pub struct RenderTemplate {
    pub template: Entity,
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

pub fn render_template(
    input: In<RenderTemplate>,
    world: &mut World,
    mut templates: Local<QueryState<(&TemplateNode, Option<&TemplateText>, Option<&Children>), ()>>,
    mut stack: Local<Vec<(Entity, Option<Entity>, Option<Entity>)>>,
) {
    println!("Render template");

    stack.clear();
    stack.push((input.template, input.ui_root, None));

    while let Some((template_entity, node, parent_node)) = stack.pop() {
        let node = match node {
            Some(entity) => {
                world.entity_mut(entity).despawn_children();
                entity
            },
            None => {
                world.spawn_empty().id()
            },
        };
        let Ok((template, template_text, children)) = templates.get(world, template_entity)
        else { continue };

        if let Some(children) = children {
            for &child in children.iter().rev() {
                stack.push((child, None, Some(node)));
            }
        }

        let mut text: Option<String> = None;
        if let Some(template_text) = template_text {
            let mut acc = String::new();
            for fragment in &template_text.0 {
                match fragment {
                    TextFragment::Literal(value) => {
                        acc.push_str(value);
                    },
                    TextFragment::Interpolated(_) => {
                    },
                }
            }
            text = Some(acc);
        }

        match &template.element {
            ElementType::Native(name) => {
                println!("element {} {}", name, node);

                if let Some(parent) = parent_node {
                    println!("parent: {parent}");
                    world.entity_mut(node).insert((
                        ChildOf(parent),
                        Node::default(),
                    ));
                }
                else {
                    world.entity_mut(node).insert((
                        Node::default(),
                    ));
                }
            },
            ElementType::Component(_) => panic!("Components not supported yet."),
        }

        if let Some(text) = text {
            println!("text: '{text}'");
            world.entity_mut(node).insert((
                Text::new(text),
            ));
        }
    }
}

#[cfg(test)]
mod test {
    use bevy::{MinimalPlugins, app::App, ecs::children};

    use crate::{core::template::{TemplateNode, TemplateText}, runtime::{BevueRuntimePlugin, render::RenderTemplate}};

    #[test]
    fn render() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, BevueRuntimePlugin));
        app.finish();

        let world = app.world_mut();

        let template = world.spawn((
            TemplateNode::new("div"),
            children! [
                (
                    TemplateNode::new("div"),
                    children! [
                        (
                            TemplateNode::new("span"),
                            TemplateText::new(["You".into()]),
                        ),
                        (
                            TemplateNode::new("span"),
                            TemplateText::new(["!".into()]),
                        )
                    ]
                ),
                (
                    TemplateNode::new("p"),
                    TemplateText::new(["Hello.".into()]),
                ),
            ]
        )).id();

        world.trigger(RenderTemplate { template, ui_root: None });
        app.update();
    }
}