//! End-to-end tests of reactive rendering: props changes applied to spawned UI entities.

use bevy::{
    MinimalPlugins,
    app::App,
    asset::{AssetPlugin, Assets},
    ecs::{change_detection::DetectChanges, entity::Entity, hierarchy::Children, world::World},
    ui::widget::Text,
};
use bevy_flair::prelude::ClassList;

use crate::{
    core::{path::PropPath, template::BevueTemplate, value::Value},
    runtime::{BevueProps, BevueRuntimePlugin, Template, TemplateInstance},
    value,
};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
    app.finish();
    app
}

fn spawn(app: &mut App, source: &str, props: Value) -> Entity {
    let template = crate::parser::parse_template(source).unwrap();
    let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
    let entity = app.world_mut().spawn((Template(handle), BevueProps::new(props))).id();
    app.update();
    entity
}

fn props(app: &mut App, entity: Entity) -> bevy::ecs::change_detection::Mut<'_, BevueProps> {
    app.world_mut().get_mut::<BevueProps>(entity).unwrap()
}

fn path(p: &str) -> PropPath {
    PropPath::parse(p).unwrap()
}

/// Texts of the descendants of `entity`, depth first in child order.
fn texts(world: &World, entity: Entity) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(children) = world.get::<Children>(entity) {
        for &child in children {
            if let Some(text) = world.get::<Text>(child) {
                out.push(text.0.clone());
            }
            out.extend(texts(world, child));
        }
    }
    out
}

fn children(world: &World, entity: Entity) -> Vec<Entity> {
    world.get::<Children>(entity).map(|c| c.to_vec()).unwrap_or_default()
}

fn text_changed_tick(world: &World, entity: Entity) -> bevy::ecs::change_detection::Tick {
    world.entity(entity).get_ref::<Text>().unwrap().last_changed()
}

fn stats(world: &World, entity: Entity) -> crate::runtime::instance::InstanceStats {
    world.get::<TemplateInstance>(entity).unwrap().stats()
}

#[test]
fn interpolation_updates_only_bound_text() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div><p>Hi {{user.name}}!</p><span>{{user.age}}</span></div>"#,
        value!({ "user": { "name": "Ana", "age": 30 } }),
    );
    assert_eq!(texts(app.world(), root), vec!["Hi Ana!", "30"]);

    let [p, span] = children(app.world(), root)[..] else { panic!("expected two children") };
    let p_tick = text_changed_tick(app.world(), p);
    let span_tick = text_changed_tick(app.world(), span);

    props(&mut app, root).at_mut(&path("user.name")).unwrap().set("Bo");
    app.update();

    assert_eq!(texts(app.world(), root), vec!["Hi Bo!", "30"]);
    assert_ne!(text_changed_tick(app.world(), p), p_tick);
    assert_eq!(text_changed_tick(app.world(), span), span_tick);
    assert!(!app.world().get::<BevueProps>(root).unwrap().is_dirty());

    // An update without prop changes touches nothing.
    let p_tick = text_changed_tick(app.world(), p);
    app.update();
    assert_eq!(text_changed_tick(app.world(), p), p_tick);
}

#[test]
fn deep_binding_fires_on_nested_change() {
    let mut app = app();
    let root = spawn(&mut app, r#"<div><p>{{user}}</p></div>"#, value!({ "user": { "tags": ["a"] } }));
    assert_eq!(texts(app.world(), root), vec![r#"{"tags": ["a"]}"#]);

    props(&mut app, root).at_mut(&path("user.tags")).unwrap().push("b");
    app.update();
    assert_eq!(texts(app.world(), root), vec![r#"{"tags": ["a", "b"]}"#]);
}

#[test]
fn replacing_props_refreshes_everything() {
    let mut app = app();
    let root = spawn(&mut app, r#"<div><p>{{a}}</p><p>{{b.c}}</p></div>"#, value!({ "a": 1, "b": { "c": 2 } }));
    assert_eq!(texts(app.world(), root), vec!["1", "2"]);

    props(&mut app, root).value = value!({ "a": 10, "b": { "c": 20 } });
    app.update();
    assert_eq!(texts(app.world(), root), vec!["10", "20"]);
}

#[test]
fn props_added_after_render() {
    let mut app = app();
    let template = crate::parser::parse_template(r#"<div><p>{{name}}</p></div>"#).unwrap();
    let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
    let root = app.world_mut().spawn(Template(handle)).id();
    app.update();
    assert_eq!(texts(app.world(), root), vec![""]);

    app.world_mut().entity_mut(root).insert(BevueProps::new(value!({ "name": "late" })));
    app.update();
    assert_eq!(texts(app.world(), root), vec!["late"]);
}

#[test]
fn class_binding() {
    let mut app = app();
    let root = spawn(&mut app, r#"<div class="base" :class="state"><p>x</p></div>"#, value!({ "state": "on" }));
    let classes = app.world().get::<ClassList>(root).unwrap();
    assert!(classes.contains("base") && classes.contains("on"));

    props(&mut app, root).get_mut("state").unwrap().set("off big");
    app.update();
    let classes = app.world().get::<ClassList>(root).unwrap();
    assert!(classes.contains("base") && classes.contains("off") && classes.contains("big"));
    assert!(!classes.contains("on"));
}

#[test]
fn if_chain_swaps_branches_in_place() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div>
            <p>first</p>
            <p v-if="a">A {{label}}</p>
            <p v-else-if="b">B</p>
            <p v-else>C</p>
            <p>last</p>
        </div>"#,
        value!({ "a": true, "b": false, "label": "x" }),
    );
    assert_eq!(texts(app.world(), root), vec!["first", "A x", "last"]);
    let before = stats(app.world(), root);

    props(&mut app, root).get_mut("a").unwrap().set(false);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["first", "C", "last"]);

    props(&mut app, root).get_mut("b").unwrap().set(true);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["first", "B", "last"]);

    // Bindings of a hidden branch are gone: changing its label does nothing.
    props(&mut app, root).get_mut("label").unwrap().set("y");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["first", "B", "last"]);

    // Toggling back and forth does not leak bindings.
    for i in 0..10 {
        props(&mut app, root).get_mut("a").unwrap().set(i % 2 == 0);
        app.update();
    }
    props(&mut app, root).get_mut("a").unwrap().set(true);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["first", "A y", "last"]);
    assert_eq!(stats(app.world(), root), before);
}

#[test]
fn keyed_list_reuses_entities() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div>
            <p>head</p>
            <span v-for="item in items" :key="item.id">{{item.label}}</span>
            <p>tail</p>
        </div>"#,
        value!({ "items": [
            { "id": 1, "label": "a" },
            { "id": 2, "label": "b" },
            { "id": 3, "label": "c" },
        ] }),
    );
    assert_eq!(texts(app.world(), root), vec!["head", "a", "b", "c", "tail"]);
    let initial = children(app.world(), root);
    let (a, b, c) = (initial[1], initial[2], initial[3]);

    // Editing one element updates only its item.
    let a_tick = text_changed_tick(app.world(), a);
    let c_tick = text_changed_tick(app.world(), c);
    props(&mut app, root).at_mut(&path("items[1].label")).unwrap().set("B");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "a", "B", "c", "tail"]);
    assert_eq!(text_changed_tick(app.world(), a), a_tick);
    assert_eq!(text_changed_tick(app.world(), c), c_tick);

    // Push appends a new entity before the tail.
    props(&mut app, root).get_mut("items").unwrap().push(value!({ "id": 4, "label": "d" }));
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "a", "B", "c", "d", "tail"]);
    let d = children(app.world(), root)[4];

    // Removing the first element keeps the other entities.
    props(&mut app, root).get_mut("items").unwrap().remove_at(0);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "B", "c", "d", "tail"]);
    assert_eq!(children(app.world(), root)[1..4], [b, c, d]);
    assert!(app.world().get_entity(a).is_err());

    // Reordering moves entities with their keys.
    props(&mut app, root).get_mut("items").unwrap().swap(0, 2);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "d", "c", "B", "tail"]);
    assert_eq!(children(app.world(), root)[1..4], [d, c, b]);

    // Edits after a reorder are routed to the item now at that index.
    props(&mut app, root).at_mut(&path("items[0].label")).unwrap().set("D");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "D", "c", "B", "tail"]);
    assert_eq!(app.world().get::<Text>(d).unwrap().0, "D");

    // Clearing the list despawns all items.
    props(&mut app, root).get_mut("items").unwrap().clear();
    app.update();
    assert_eq!(texts(app.world(), root), vec!["head", "tail"]);
    assert_eq!(stats(app.world(), root).items, 0);
}

#[test]
fn key_changed_in_place_moves_identity() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div><span v-for="item in items" :key="item.id">{{item.id}}</span></div>"#,
        value!({ "items": [{ "id": "x" }, { "id": "y" }] }),
    );
    let [x, y] = children(app.world(), root)[..] else { panic!() };

    // Swap the data in place, as a per-index diff of a Rust Vec would.
    let mut p = props(&mut app, root);
    p.at_mut(&path("items[0].id")).unwrap().set("y");
    p.at_mut(&path("items[1].id")).unwrap().set("x");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["y", "x"]);

    // A later removal must despawn the entity showing "x", whichever it is.
    props(&mut app, root).get_mut("items").unwrap().remove_at(1);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["y"]);
    let remaining = children(app.world(), root);
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0] == x || remaining[0] == y);
}

#[test]
fn unkeyed_list_updates_by_position() {
    let mut app = app();
    let root = spawn(&mut app, r#"<div><span v-for="x in xs">{{x}}</span></div>"#, value!({ "xs": ["a", "b", "c"] }));
    let initial = children(app.world(), root);

    props(&mut app, root).get_mut("xs").unwrap().remove_at(0);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["b", "c"]);
    // Items are matched by position, so the first two entities are reused.
    assert_eq!(children(app.world(), root), initial[..2]);

    props(&mut app, root).get_mut("xs").unwrap().set(value!(["z"]));
    app.update();
    assert_eq!(texts(app.world(), root), vec!["z"]);
}

#[test]
fn nested_lists_and_indices() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div>
            <div v-for="(row, i) in rows" :key="row.id">
                <span v-for="(cell, j) in row.cells">{{i}}.{{j}}={{cell}} {{title}}</span>
            </div>
        </div>"#,
        value!({
            "title": "t",
            "rows": [
                { "id": "r0", "cells": ["a", "b"] },
                { "id": "r1", "cells": ["c"] },
            ],
        }),
    );
    assert_eq!(texts(app.world(), root), vec!["0.0=a t", "0.1=b t", "1.0=c t"]);

    props(&mut app, root).at_mut(&path("rows[1].cells")).unwrap().push("d");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["0.0=a t", "0.1=b t", "1.0=c t", "1.1=d t"]);

    props(&mut app, root).at_mut(&path("rows[0].cells[1]")).unwrap().set("B");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["0.0=a t", "0.1=B t", "1.0=c t", "1.1=d t"]);

    // Removing the first row shifts the second row's index without re-rendering its cells.
    let row1 = children(app.world(), root)[1];
    props(&mut app, root).get_mut("rows").unwrap().remove_at(0);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["0.0=c t", "0.1=d t"]);
    assert_eq!(children(app.world(), root), vec![row1]);

    // Cell edits still reach the moved row.
    props(&mut app, root).at_mut(&path("rows[0].cells[0]")).unwrap().set("C");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["0.0=C t", "0.1=d t"]);

    props(&mut app, root).get_mut("title").unwrap().set("T");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["0.0=C T", "0.1=d T"]);
}

#[test]
fn list_inside_if_and_if_inside_list() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div>
            <div v-if="show">
                <span v-for="item in items" :key="item.id">
                    <p v-if="item.done">done {{item.id}}</p>
                    <p v-else>todo {{item.id}}</p>
                </span>
            </div>
        </div>"#,
        value!({ "show": true, "items": [{ "id": 1, "done": false }, { "id": 2, "done": true }] }),
    );
    assert_eq!(texts(app.world(), root), vec!["todo 1", "done 2"]);
    let before = stats(app.world(), root);

    props(&mut app, root).at_mut(&path("items[0].done")).unwrap().set(true);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["done 1", "done 2"]);

    props(&mut app, root).get_mut("show").unwrap().set(false);
    app.update();
    assert_eq!(texts(app.world(), root), Vec::<String>::new());

    props(&mut app, root).at_mut(&path("items[1].done")).unwrap().set(false);
    props(&mut app, root).get_mut("show").unwrap().set(true);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["done 1", "todo 2"]);
    assert_eq!(stats(app.world(), root), before);
}

#[test]
fn dynamic_index() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div><p>{{names[sel]}}</p></div>"#,
        value!({ "names": ["a", "b"], "sel": 0 }),
    );
    assert_eq!(texts(app.world(), root), vec!["a"]);

    props(&mut app, root).get_mut("sel").unwrap().set(1);
    app.update();
    assert_eq!(texts(app.world(), root), vec!["b"]);

    props(&mut app, root).at_mut(&path("names[1]")).unwrap().set("B");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["B"]);
}

mod props_sources {
    use bevy::ecs::{component::Component, resource::Resource};

    use super::*;
    use crate::runtime::{BevuePropsAppExt, PropsResource, PropsSource};

    #[derive(crate::Props, Resource, Default)]
    struct Menu {
        title: String,
        actions: Vec<Action>,
    }

    #[derive(crate::Props, Clone)]
    struct Action {
        id: u32,
        label: String,
    }

    #[derive(crate::Props, Component)]
    struct Player {
        name: String,
        hp: u32,
    }

    const MENU: &str = r#"<div>
        <p>{{menu.title}}</p>
        <span v-for="action in menu.actions" :key="action.id">{{action.label}}</span>
    </div>"#;

    fn menu() -> Menu {
        Menu {
            title: "File".into(),
            actions: vec![Action { id: 1, label: "Open".into() }, Action { id: 2, label: "Save".into() }],
        }
    }

    #[test]
    fn resource_changes_update_bound_nodes() {
        let mut app = app();
        app.register_props_resource::<Menu>();
        app.insert_resource(menu());

        let template = crate::parser::parse_template(MENU).unwrap();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        let root = app.world_mut().spawn((Template(handle), PropsResource::<Menu>::at("menu"))).id();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["File", "Open", "Save"]);

        let [title, open, save] = children(app.world(), root)[..] else { panic!() };
        let title_tick = text_changed_tick(app.world(), title);
        let open_tick = text_changed_tick(app.world(), open);

        app.world_mut().resource_mut::<Menu>().actions[1].label = "Save As".into();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["File", "Open", "Save As"]);
        assert_eq!(text_changed_tick(app.world(), title), title_tick);
        assert_eq!(text_changed_tick(app.world(), open), open_tick);

        // Touching the resource without changing data does not mark the props.
        let props_tick = app.world().entity(root).get_ref::<BevueProps>().unwrap().last_changed();
        app.world_mut().resource_mut::<Menu>().title = "File".into();
        app.update();
        assert_eq!(app.world().entity(root).get_ref::<BevueProps>().unwrap().last_changed(), props_tick);

        // Removing an action from the Rust side despawns its item.
        app.world_mut().resource_mut::<Menu>().actions.remove(0);
        app.update();
        assert_eq!(texts(app.world(), root), vec!["File", "Save As"]);
        assert_eq!(children(app.world(), root)[1], save);
    }

    #[test]
    fn component_source_as_root_and_from_other_entity() {
        let mut app = app();
        app.register_props::<Player>();

        let template = crate::parser::parse_template(r#"<div><p>{{name}}: {{hp}}</p></div>"#).unwrap();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        let local = app
            .world_mut()
            .spawn((Template(handle.clone()), Player { name: "ana".into(), hp: 10 }, PropsSource::<Player>::root()))
            .id();
        let player = app.world_mut().spawn(Player { name: "bo".into(), hp: 3 }).id();
        let remote = app
            .world_mut()
            .spawn((Template(handle), PropsSource::<Player>::root().from_entity(player)))
            .id();
        app.update();
        assert_eq!(texts(app.world(), local), vec!["ana: 10"]);
        assert_eq!(texts(app.world(), remote), vec!["bo: 3"]);

        app.world_mut().get_mut::<Player>(player).unwrap().hp = 2;
        app.update();
        assert_eq!(texts(app.world(), local), vec!["ana: 10"]);
        assert_eq!(texts(app.world(), remote), vec!["bo: 2"]);
    }
}
