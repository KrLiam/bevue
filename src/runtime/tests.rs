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
    let entity = app.world_mut().spawn((Template::new(handle), BevueProps::new(props))).id();
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
    let root = app.world_mut().spawn(Template::new(handle)).id();
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

mod styles {
    use bevy::{color::Color, ecs::change_detection::Tick, ui::BackgroundColor};
    use bevy_flair::prelude::{InlineStyle, Styled};

    use super::*;

    fn style(world: &World, entity: Entity) -> InlineStyle {
        world.get::<InlineStyle>(entity).cloned().expect("expected an inline style")
    }

    fn style_tick(world: &World, entity: Entity) -> Tick {
        world.entity(entity).get_ref::<InlineStyle>().unwrap().last_changed()
    }

    fn background(world: &World, entity: Entity) -> Color {
        world.get::<BackgroundColor>(entity).expect("expected a background color").0
    }

    #[test]
    fn style_values() {
        let mut app = app();
        let root = spawn(
            &mut app,
            r#"
            <div>
                <p :style="{ backgroundColor: theme.color, 'border-radius': '4px', width: 10, '--Gap': theme.gap }">a</p>
                <p :style="theme.panel">b</p>
                <p style="width: 200px; color: red" :style="{ color: theme.color }" :style="theme.list">c</p>
                <p style="left: 1px">d</p>
                <p :style="{ top: '2px', left: false }">e</p>
                <p>f</p>
            </div>
            "#,
            value!({ "theme": { "color": "blue", "gap": "2px", "panel": { "margin_left": "3px", "margin": "1px" }, "list": [{ "margin_left": "3px", "margin": "1px" }, "top: 1px"] } }),
        );
        let [a, b, c, d, e, f] = children(app.world(), root)[..] else { panic!("expected six children") };
        // Declarations keep the order they were written in, which `InlineStyle` compares by.
        let expected = [("background-color", "blue"), ("border-radius", "4px"), ("width", "10"), ("--Gap", "2px")];
        assert_eq!(style(app.world(), a), InlineStyle::from_iter(expected));
        assert_eq!(format!("{:?}", style(app.world(), a)), format!("{:?}", InlineStyle::from_iter(expected)));
        assert_eq!(style(app.world(), b), InlineStyle::new("margin: 1px; margin-left: 3px"));
        assert_eq!(
            style(app.world(), c),
            InlineStyle::new("width: 200px; color: blue; margin: 1px; margin-left: 3px; top: 1px")
        );
        assert_eq!(style(app.world(), d), InlineStyle::new("left: 1px"));
        assert_eq!(style(app.world(), e), InlineStyle::new("top: 2px"));
        assert!(app.world().get::<InlineStyle>(f).is_none());
        // Static styles need no binding.
        assert_eq!(stats(app.world(), root).bindings, 3);
    }

    #[test]
    fn style_updates_only_bound_nodes() {
        let mut app = app();
        let root = spawn(
            &mut app,
            r#"<div><p :style="{ color: a }">x</p><p :style="{ color: b, top: t }">y</p></div>"#,
            value!({ "a": "red", "b": "blue" }),
        );
        let [first, second] = children(app.world(), root)[..] else { panic!("expected two children") };
        assert_eq!(style(app.world(), second), InlineStyle::new("color: blue"));
        let (first_tick, second_tick) = (style_tick(app.world(), first), style_tick(app.world(), second));

        props(&mut app, root).get_mut("a").unwrap().set("green");
        app.update();
        assert_eq!(style(app.world(), first), InlineStyle::new("color: green"));
        assert_ne!(style_tick(app.world(), first), first_tick);
        assert_eq!(style_tick(app.world(), second), second_tick);

        // A value that goes away removes its declaration; an emptied style stays as an empty one.
        props(&mut app, root).insert("t", "1px");
        app.update();
        assert_eq!(style(app.world(), second), InlineStyle::new("color: blue; top: 1px"));
        props(&mut app, root).insert("t", Value::null());
        props(&mut app, root).insert("b", Value::null());
        app.update();
        assert_eq!(style(app.world(), second), InlineStyle::default());
    }

    #[test]
    fn no_style_until_a_value_exists() {
        let mut app = app();
        let root = spawn(&mut app, r#"<div><p :style="{ color: a }">x</p></div>"#, Value::empty_dict());
        let p = children(app.world(), root)[0];
        assert!(app.world().get::<InlineStyle>(p).is_none());

        props(&mut app, root).insert("a", "red");
        app.update();
        assert_eq!(style(app.world(), p), InlineStyle::new("color: red"));
    }

    #[test]
    fn styles_in_lists() {
        let mut app = app();
        let root = spawn(
            &mut app,
            r#"<ul><li v-for="(item, i) in items" :key="item.id" :style="{ color: item.color, order: i }">x</li></ul>"#,
            value!({ "items": [{ "id": 1, "color": "red" }, { "id": 2, "color": "blue" }] }),
        );
        let [one, two] = children(app.world(), root)[..] else { panic!("expected two children") };
        assert_eq!(style(app.world(), two), InlineStyle::new("color: blue; order: 1"));

        props(&mut app, root).get_mut("items").unwrap().swap(0, 1);
        app.update();
        assert_eq!(children(app.world(), root), vec![two, one]);
        assert_eq!(style(app.world(), two), InlineStyle::new("color: blue; order: 0"));
        assert_eq!(style(app.world(), one), InlineStyle::new("color: red; order: 1"));
    }

    #[test]
    fn object_literals_in_class() {
        let mut app = app();
        let root =
            spawn(&mut app, r#"<div class="base" :class="{ active: on, 'is-big': big }"></div>"#, value!({ "on": true }));
        let classes = app.world().get::<ClassList>(root).unwrap();
        assert!(classes.contains("base") && classes.contains("active") && !classes.contains("is-big"));

        props(&mut app, root).insert("on", false);
        props(&mut app, root).insert("big", true);
        app.update();
        let classes = app.world().get::<ClassList>(root).unwrap();
        assert!(!classes.contains("active") && classes.contains("is-big"));
    }

    /// An app whose generated stylesheets can be loaded: the `bevue://` asset source has to be
    /// registered before `AssetPlugin` is added.
    fn styled_app() -> App {
        let mut app = App::new();
        app.add_plugins((crate::asset::BevueAssetPlugin, MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin));
        app.finish();
        app
    }

    /// Updates until `bevy_flair` has loaded the template's stylesheet and styled `entity`.
    fn wait_for_background(app: &mut App, entity: Entity, expected: Color) {
        for _ in 0..500 {
            if app.world().get::<BackgroundColor>(entity).is_some_and(|c| c.0 == expected) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
            app.update();
        }
        panic!("expected {expected:?}, found {:?}", app.world().get::<BackgroundColor>(entity));
    }

    /// The whole way through `bevy_flair`: a style wins over the stylesheet and is applied in the
    /// frame it was set.
    #[test]
    fn applied_by_flair_in_the_same_frame() {
        let mut app = styled_app();
        let root = spawn(
            &mut app,
            r#"
            <template><div><p class="box" :style="{ 'background-color': color }">x</p><p class="box">y</p></div></template>
            <style>.box { background-color: #00ff00; }</style>
            "#,
            value!({ "color": "#ff0000" }),
        );
        let [p, plain] = children(app.world(), root)[..] else { panic!("expected two children") };
        wait_for_background(&mut app, p, Color::srgb(1.0, 0.0, 0.0));
        assert_eq!(background(app.world(), plain), Color::srgb(0.0, 1.0, 0.0));

        props(&mut app, root).get_mut("color").unwrap().set("#0000ff");
        app.update();
        assert_eq!(background(app.world(), p), Color::srgb(0.0, 0.0, 1.0));

        // Without a value the stylesheet rule applies again.
        props(&mut app, root).insert("color", Value::null());
        app.update();
        assert_eq!(background(app.world(), p), Color::srgb(0.0, 1.0, 0.0));
    }

    /// `bevy_flair` ignores inline styles of entities without a stylesheet, so a template that
    /// has none gets an empty one.
    #[test]
    fn applied_without_a_style_block() {
        let mut app = styled_app();
        let root = spawn(&mut app, r#"<div><p :style="{ backgroundColor: color }">x</p></div>"#, value!({ "color": "#ff0000" }));
        let p = children(app.world(), root)[0];
        assert!(matches!(app.world().get::<Styled>(root), Some(Styled::StyleSheet(_))));
        wait_for_background(&mut app, p, Color::srgb(1.0, 0.0, 0.0));

        // A template without style attributes is left without a stylesheet.
        let other = spawn(&mut app, r#"<div><p>x</p></div>"#, Value::empty_dict());
        assert!(!matches!(app.world().get::<Styled>(other), Some(Styled::StyleSheet(_))));
    }
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

#[test]
fn numeric_member_is_an_index() {
    let mut app = app();
    let root = spawn(
        &mut app,
        r#"<div><p>{{pair.0}} {{pair[1]}}</p><span>{{rows.1.0}}</span></div>"#,
        value!({ "pair": ["a", "b"], "rows": [["x"], ["y"]] }),
    );
    assert_eq!(texts(app.world(), root), vec!["a b", "y"]);

    // Both spellings are bound to the same path.
    props(&mut app, root).at_mut(&path("pair[0]")).unwrap().set("A");
    props(&mut app, root).at_mut(&path("rows.1.0")).unwrap().set("Y");
    app.update();
    assert_eq!(texts(app.world(), root), vec!["A b", "Y"]);
}

mod props_sources {
    use bevy::ecs::{component::Component, resource::Resource};

    use super::*;
    use crate::runtime::{BevuePropsAppExt, PropsComponent, PropsResource};

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
        app.register_props::<Menu>();
        app.insert_resource(menu());

        let template = crate::parser::parse_template(MENU).unwrap();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        // Mounted at `Menu::name()`, which is "menu".
        let root = app.world_mut().spawn((Template::new(handle), PropsResource::<Menu>::default())).id();
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
            .spawn((Template::new(handle.clone()), Player { name: "ana".into(), hp: 10 }, PropsComponent::<Player>::root()))
            .id();
        let player = app.world_mut().spawn(Player { name: "bo".into(), hp: 3 }).id();
        let remote = app
            .world_mut()
            .spawn((Template::new(handle), PropsComponent::<Player>::root().from_entity(player)))
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

mod declared_props {
    use bevy::ecs::{component::Component, resource::Resource};

    use super::*;
    use crate::{
        core::props::Props,
        runtime::{BevuePropsAppExt, DeclaredProps, PropsComponent, PropsResource},
    };

    #[derive(crate::Props, Resource)]
    struct Menu {
        title: String,
    }

    #[derive(crate::Props, Component)]
    struct Player {
        name: String,
    }

    const DECLARED: &str = r#"
        <props>
            <resource type="Menu"/>
            <component name="p" type="Player"/>
        </props>
        <template>
            <div><p>{{menu.title}}</p><span>{{p.name}}</span><b>{{m.title}}</b></div>
        </template>
    "#;

    fn declared_app() -> App {
        let mut app = app();
        app.register_props::<Menu>().register_props::<Player>();
        app.insert_resource(Menu { title: "File".into() });
        app
    }

    fn add(app: &mut App, source: &str) -> bevy::asset::Handle<BevueTemplate> {
        let template = crate::parser::parse_template(source).unwrap();
        app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template)
    }

    #[test]
    fn declared_sources_render_on_the_first_frame() {
        let mut app = declared_app();
        let handle = add(&mut app, DECLARED);
        let root = app.world_mut().spawn((Template::new(handle), Player { name: "ana".into() })).id();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["File", "ana", ""]);
        assert!(app.world().get::<PropsResource<Menu>>(root).is_some());
        assert!(app.world().get::<PropsComponent<Player>>(root).is_some());

        app.world_mut().resource_mut::<Menu>().title = "Edit".into();
        app.world_mut().get_mut::<Player>(root).unwrap().name = "bo".into();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["Edit", "bo", ""]);
    }

    #[test]
    fn types_are_found_by_full_path() {
        let mut app = declared_app();
        let source = format!(
            r#"<props><resource name="m" type="{}"/></props><div><p>{{{{m.title}}}}</p></div>"#,
            Menu::type_path()
        );
        let handle = add(&mut app, &source);
        let root = app.world_mut().spawn(Template::new(handle)).id();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["File"]);
    }

    #[test]
    fn manual_sources_are_not_overwritten() {
        let mut app = declared_app();
        let handle = add(&mut app, DECLARED);
        let other = app.world_mut().spawn(Player { name: "bo".into() }).id();
        let root = app
            .world_mut()
            .spawn((
                Template::new(handle),
                Player { name: "ana".into() },
                PropsResource::<Menu>::at("m"),
                PropsComponent::<Player>::at("p").from_entity(other),
            ))
            .id();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["", "bo", "File"]);
        assert_eq!(app.world().get::<PropsResource<Menu>>(root).unwrap().key, Some(path("m")));
        assert_eq!(app.world().get::<PropsComponent<Player>>(root).unwrap().entity, Some(other));
        assert!(app.world().get::<DeclaredProps>(root).is_none());
    }

    #[test]
    fn unknown_types_are_skipped() {
        let mut app = declared_app();
        let handle = add(&mut app, r#"<props><resource type="Nope"/></props><div><p>{{nope.x}}!</p></div>"#);
        let root = app.world_mut().spawn(Template::new(handle)).id();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["!"]);
    }

    #[test]
    fn rerender_replaces_declared_sources_only() {
        let mut app = declared_app();
        let handle = add(&mut app, DECLARED);
        let declared = app.world_mut().spawn((Template::new(handle.clone()), Player { name: "ana".into() })).id();
        let manual = app.world_mut().spawn((Template::new(handle.clone()), PropsResource::<Menu>::at("m"))).id();
        app.update();
        assert!(app.world().get::<PropsResource<Menu>>(declared).is_some());

        // The reloaded template renames one declaration and drops the other.
        let reloaded = crate::parser::parse_template(
            r#"<props><component name="q" type="Player"/></props><div><p>{{q.name}}</p></div>"#,
        )
        .unwrap();
        *app.world_mut().resource_mut::<Assets<BevueTemplate>>().get_mut(&handle).unwrap() = reloaded;
        // Asset events are emitted at the end of a frame and read on the next one.
        app.update();
        app.update();

        assert_eq!(texts(app.world(), declared), vec!["ana"]);
        assert!(app.world().get::<PropsResource<Menu>>(declared).is_none());
        assert_eq!(app.world().get::<PropsComponent<Player>>(declared).unwrap().key, Some(path("q")));
        assert_eq!(app.world().get::<PropsResource<Menu>>(manual).unwrap().key, Some(path("m")));
    }
}

mod events_and_functions {
    use bevy::{
        ecs::{
            observer::On,
            query::With,
            resource::Resource,
            system::{In, Query, ResMut},
        },
        ui::{FocusPolicy, Interaction},
    };

    use super::*;
    use crate::{
        core::template::TemplateRef,
        runtime::{BevueCtx, BevueEvent, BevueFunctionsAppExt, TemplateFunctions},
    };

    #[derive(Resource, Default)]
    struct Calls(u32);

    #[derive(Resource, Default)]
    struct Recorded(Vec<Vec<Value>>);

    #[derive(Resource, Default)]
    struct EventLog(Vec<(Entity, String, Vec<Value>)>);

    fn upper(In(ctx): In<BevueCtx>, mut calls: ResMut<Calls>) -> String {
        calls.0 += 1;
        ctx.args[0].to_text().to_uppercase()
    }

    fn record(In(ctx): In<BevueCtx>, mut recorded: ResMut<Recorded>) {
        recorded.0.push(ctx.args);
    }

    fn evens(In(ctx): In<BevueCtx>) -> Value {
        let list = ctx.args[0].as_list().cloned().unwrap_or_default();
        Value::list(list.into_iter().filter(|v| v.as_number().is_some_and(|n| n % 2.0 == 0.0)).collect())
    }

    fn event_app() -> App {
        let mut app = app();
        app.init_resource::<Calls>()
            .init_resource::<Recorded>()
            .init_resource::<EventLog>()
            .register_bevue_function("upper", upper)
            .register_bevue_function("record", record)
            .register_bevue_function("evens", evens)
            .add_observer(|event: On<BevueEvent>, mut log: ResMut<EventLog>| {
                log.0.push((event.entity, event.name.to_string(), event.args.clone()));
            });
        app
    }

    fn nodes_with<F: bevy::ecs::query::QueryFilter>(app: &mut App) -> Vec<Entity> {
        let mut query = app.world_mut().query_filtered::<Entity, (With<TemplateRef>, F)>();
        query.iter(app.world()).collect()
    }

    fn set_interaction(app: &mut App, entity: Entity, interaction: Interaction) {
        *app.world_mut().get_mut::<Interaction>(entity).unwrap() = interaction;
        app.update();
    }

    fn log_names(app: &mut App) -> Vec<String> {
        std::mem::take(&mut app.world_mut().resource_mut::<EventLog>().0).into_iter().map(|(_, name, _)| name).collect()
    }

    #[test]
    fn calls_in_bindings_rerun_only_when_args_change() {
        let mut app = event_app();
        let root = spawn(&mut app, r#"<div><p>{{upper(name)}}!</p><p>{{other}}</p></div>"#, value!({ "name": "ana", "other": 1 }));
        assert_eq!(texts(app.world(), root), vec!["ANA!", "1"]);
        assert_eq!(app.world().resource::<Calls>().0, 1);

        props(&mut app, root).get_mut("other").unwrap().set(2);
        app.update();
        assert_eq!(texts(app.world(), root), vec!["ANA!", "2"]);
        assert_eq!(app.world().resource::<Calls>().0, 1);

        props(&mut app, root).get_mut("name").unwrap().set("bo");
        app.update();
        assert_eq!(texts(app.world(), root), vec!["BO!", "2"]);
        assert_eq!(app.world().resource::<Calls>().0, 2);
    }

    #[test]
    fn call_iterables() {
        let mut app = event_app();
        let root = spawn(
            &mut app,
            r#"<div><span v-for="(x, i) in evens(nums)" :key="x">{{i}}:{{x}}</span></div>"#,
            value!({ "nums": [1, 2, 3, 4] }),
        );
        assert_eq!(texts(app.world(), root), vec!["0:2", "1:4"]);
        let four = children(app.world(), root)[1];

        props(&mut app, root).get_mut("nums").unwrap().remove_at(1);
        props(&mut app, root).get_mut("nums").unwrap().push(6);
        app.update();
        assert_eq!(texts(app.world(), root), vec!["0:4", "1:6"]);
        assert_eq!(children(app.world(), root)[0], four, "keyed items are reused");
    }

    #[test]
    fn events_from_code_with_loop_scope() {
        let mut app = event_app();
        let root = spawn(
            &mut app,
            r#"<div><span v-for="item in items" :key="item.id" @custom="record($event, item.id)">{{item.id}}</span></div>"#,
            value!({ "items": [{ "id": "a" }, { "id": "b" }] }),
        );
        let b = children(app.world(), root)[1];
        app.world_mut().trigger(BevueEvent::new(b, "custom", vec![Value::from(1)]));
        app.update();
        assert_eq!(app.world().resource::<Recorded>().0, vec![vec![Value::from(1), Value::from("b")]]);

        // Events without a listener reach user observers but call nothing.
        app.world_mut().trigger(BevueEvent::new(b, "other", vec![]));
        app.update();
        assert_eq!(app.world().resource::<Recorded>().0.len(), 1);
        assert_eq!(log_names(&mut app), vec!["custom", "other"]);
    }

    #[test]
    fn bare_function_handler_gets_event_args() {
        let mut app = event_app();
        let root = spawn(&mut app, r#"<div><p @custom="record">x</p></div>"#, Value::empty_dict());
        let p = children(app.world(), root)[0];
        app.world_mut().trigger(BevueEvent::new(p, "custom", vec![Value::from("x"), Value::from(2)]));
        app.update();
        assert_eq!(app.world().resource::<Recorded>().0, vec![vec![Value::from("x"), Value::from(2)]]);
    }

    #[test]
    fn interaction_events() {
        let mut app = event_app();
        let root = spawn(
            &mut app,
            r#"<div><p @click="record('click')">a</p><p @mouseover>b</p><p @custom="record()">c</p><p>d</p></div>"#,
            Value::empty_dict(),
        );
        let [click, hover, custom, plain] = children(app.world(), root)[..] else { panic!() };

        // Only nodes listening to interaction events get `Interaction`.
        let interactive = nodes_with::<With<Interaction>>(&mut app);
        assert!(interactive.contains(&click) && interactive.contains(&hover));
        assert!(!interactive.contains(&custom) && !interactive.contains(&plain) && !interactive.contains(&root));

        // Nodes let interaction pass through to listening ancestors (Bevy's default for `Node`).
        for node in [root, click, hover, custom, plain] {
            assert_eq!(app.world().get::<FocusPolicy>(node), Some(&FocusPolicy::Pass));
        }

        set_interaction(&mut app, click, Interaction::Hovered);
        set_interaction(&mut app, click, Interaction::Pressed);
        set_interaction(&mut app, click, Interaction::Hovered);
        set_interaction(&mut app, click, Interaction::None);
        assert_eq!(log_names(&mut app), vec!["mouseover", "click", "mouseleave"]);
        assert_eq!(app.world().resource::<Recorded>().0, vec![vec![Value::from("click")]]);

        // Releasing outside the node is not a click.
        set_interaction(&mut app, click, Interaction::Pressed);
        set_interaction(&mut app, click, Interaction::None);
        assert_eq!(log_names(&mut app), vec!["mouseover", "mouseleave"]);

        // A listener without a handler still gets events for external observers.
        set_interaction(&mut app, hover, Interaction::Hovered);
        assert_eq!(app.world().resource::<EventLog>().0[0].0, hover);
        assert_eq!(log_names(&mut app), vec!["mouseover"]);
    }

    #[test]
    fn handler_mutates_props_in_the_same_frame() {
        fn add_user(In(ctx): In<BevueCtx>, mut props: Query<&mut BevueProps>) {
            props.get_mut(ctx.root).unwrap().get_mut("users").unwrap().push("new");
        }

        let mut app = event_app();
        app.register_bevue_function("add_user", add_user);
        let root = spawn(
            &mut app,
            r#"<div><p @click="add_user()">add</p><span v-for="u in users">{{u}}</span></div>"#,
            value!({ "users": ["ana"] }),
        );
        let button = children(app.world(), root)[0];
        set_interaction(&mut app, button, Interaction::Pressed);
        set_interaction(&mut app, button, Interaction::Hovered);
        assert_eq!(texts(app.world(), root), vec!["add", "ana", "new"]);
    }

    #[test]
    fn emit_triggers_on_root() {
        let mut app = event_app();
        let root = spawn(
            &mut app,
            r#"<div @buy="record($args)">
                <span v-for="item in items" :key="item.id" @click="$emit('buy', item.id, 2)">{{item.id}}</span>
                <p @click="$emit(1)">bad</p>
            </div>"#,
            value!({ "items": [{ "id": "a" }, { "id": "b" }] }),
        );
        let [_, b, bad] = children(app.world(), root)[..] else { panic!() };

        app.world_mut().trigger(BevueEvent::new(b, "click", vec![]));
        app.update();
        let log = std::mem::take(&mut app.world_mut().resource_mut::<EventLog>().0);
        assert_eq!(
            log,
            vec![
                (b, "click".to_owned(), vec![]),
                (root, "buy".to_owned(), vec![Value::from("b"), Value::from(2)]),
            ]
        );
        assert_eq!(app.world().resource::<Recorded>().0, vec![vec![value!(["b", 2])]]);

        app.world_mut().trigger(BevueEvent::new(bad, "click", vec![]));
        app.update();
        assert_eq!(log_names(&mut app), vec!["click"]);
    }

    #[test]
    fn re_emitting_listener_is_cut_off() {
        let mut app = event_app();
        let root = spawn(&mut app, r#"<div @ping="$emit('ping')"><p>x</p></div>"#, Value::empty_dict());
        app.world_mut().trigger(BevueEvent::new(root, "ping", vec![]));
        app.update();
        let pings = log_names(&mut app).len() as u32;
        assert_eq!(pings, crate::runtime::events::MAX_DISPATCH_DEPTH + 1);
    }

    #[test]
    #[should_panic(expected = "reserved")]
    fn reserved_names_panic() {
        app().register_bevue_function("$mine", record);
    }

    #[test]
    fn template_local_functions() {
        fn local(In(_): In<BevueCtx>) -> &'static str {
            "local"
        }
        fn global(In(_): In<BevueCtx>) -> &'static str {
            "global"
        }

        let mut app = event_app();
        let template = crate::parser::parse_template(r#"<div><p>{{f()}}{{g()}}</p></div>"#).unwrap();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        let with_local = Template::new(handle.clone()).with_function("f", local).with_function("g", local);
        let a = app.world_mut().spawn(with_local.clone()).id();
        let b = app.world_mut().spawn(with_local).id();
        let plain = app.world_mut().spawn(Template::new(handle.clone())).id();
        app.register_bevue_function("g", global);
        app.update();

        assert_eq!(texts(app.world(), a), vec!["locallocal"]);
        assert_eq!(texts(app.world(), b), vec!["locallocal"]);
        assert_eq!(texts(app.world(), plain), vec!["global"], "unknown `f` is null, global `g` is used");

        // Clones register independent systems.
        let id_a = app.world().get::<TemplateFunctions>(a).unwrap().0["f"];
        let id_b = app.world().get::<TemplateFunctions>(b).unwrap().0["f"];
        assert_ne!(id_a, id_b);

        // Removing the template or despawning the entity unregisters its systems.
        app.world_mut().entity_mut(a).remove::<Template>();
        app.world_mut().despawn(b);
        app.update();
        assert!(app.world().get::<TemplateFunctions>(a).is_none());
        let ctx = |entity| BevueCtx { entity, root: entity, args: vec![] };
        assert!(app.world_mut().run_system_with(id_a, ctx(a)).is_err());
        assert!(app.world_mut().run_system_with(id_b, ctx(b)).is_err());

        // Replacing the template swaps its functions.
        app.world_mut().entity_mut(a).insert(Template::new(handle).with_function("f", global));
        app.update();
        let id_new = app.world().get::<TemplateFunctions>(a).unwrap().0["f"];
        assert_eq!(app.world_mut().run_system_with(id_new, ctx(a)).unwrap(), Value::from("global"));
    }
}

mod update_stats {
    use super::*;
    use crate::runtime::UpdateStats;

    /// Runs the instance update by hand, as `update_templates` does, to read its stats.
    fn update(app: &mut App, root: Entity) -> UpdateStats {
        let world = app.world_mut();
        let mut instance = std::mem::take(&mut *world.get_mut::<TemplateInstance>(root).unwrap());
        let mut value = std::mem::take(&mut world.get_mut::<BevueProps>(root).unwrap().value);
        let stats = instance.update(world, &value, !value.is_dirty());
        value.clear_changes();
        world.get_mut::<BevueProps>(root).unwrap().value = value;
        *world.get_mut::<TemplateInstance>(root).unwrap() = instance;
        stats
    }

    #[test]
    fn reports_what_an_update_did() {
        let mut app = app();
        let root = spawn(
            &mut app,
            r#"<div><p>{{title}}</p><span v-for="item in items" :key="item.id">{{item.label}}</span></div>"#,
            value!({ "title": "t", "items": [{ "id": 1, "label": "a" }, { "id": 2, "label": "b" }] }),
        );

        props(&mut app, root).at_mut(&path("items[1].label")).unwrap().set("B");
        assert_eq!(update(&mut app, root), UpdateStats { fired: 1, reconciled: 0, full: false });
        assert_eq!(texts(app.world(), root), vec!["t", "a", "B"]);

        props(&mut app, root).get_mut("items").unwrap().push(value!({ "id": 3, "label": "c" }));
        assert_eq!(update(&mut app, root), UpdateStats { fired: 0, reconciled: 1, full: false });
        assert_eq!(texts(app.world(), root), vec!["t", "a", "B", "c"]);

        // Nothing dirty means the props were replaced: everything is re-evaluated.
        let stats = update(&mut app, root);
        assert_eq!(stats, UpdateStats { fired: 4, reconciled: 1, full: true });
    }
}

mod components {
    use bevy::{
        asset::Handle,
        ecs::{
            observer::On,
            resource::Resource,
            system::{In, ResMut},
        },
    };
    use bevy_flair::prelude::Styled;

    use super::*;
    use crate::{
        asset::BevueAssetRegistry,
        core::template::TemplateRef,
        runtime::{
            BevueComponents, BevueCtx, BevueEvent, BevueFunctionsAppExt, BevuePropsAppExt, ComponentDependencies,
            PropsResource, UpdateStats,
        },
    };

    #[derive(Resource, Default)]
    struct Recorded(Vec<Vec<Value>>);

    #[derive(Resource, Default)]
    struct EventLog(Vec<(Entity, String)>);

    #[derive(crate::Props, Resource)]
    struct Menu {
        title: String,
    }

    fn record(In(ctx): In<BevueCtx>, mut recorded: ResMut<Recorded>) {
        recorded.0.push(ctx.args);
    }

    fn component_app() -> App {
        let mut app = app();
        app.init_resource::<Recorded>()
            .init_resource::<EventLog>()
            .register_bevue_function("record", record)
            .add_observer(|event: On<BevueEvent>, mut log: ResMut<EventLog>| {
                log.0.push((event.entity, event.name.to_string()));
            });
        app
    }

    /// Registers `source` as the component `name`.
    fn component(app: &mut App, name: &str, source: &str) -> Handle<BevueTemplate> {
        let template = crate::parser::parse_template(source).unwrap();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        app.world_mut().get_resource_or_init::<BevueComponents>().insert(name, handle.clone());
        handle
    }

    fn node_count(app: &mut App) -> usize {
        app.world_mut().query::<&TemplateRef>().iter(app.world()).count()
    }

    fn update(app: &mut App, root: Entity) -> UpdateStats {
        let world = app.world_mut();
        let mut instance = std::mem::take(&mut *world.get_mut::<TemplateInstance>(root).unwrap());
        let mut value = std::mem::take(&mut world.get_mut::<BevueProps>(root).unwrap().value);
        let stats = instance.update(world, &value, !value.is_dirty());
        value.clear_changes();
        world.get_mut::<BevueProps>(root).unwrap().value = value;
        *world.get_mut::<TemplateInstance>(root).unwrap() = instance;
        stats
    }

    const ROW: &str = r#"
        <li class="row" @click="record('own', item.id)">
            <p>{{ item.label }}</p>
            <span>{{ n }}</span>
            <b v-if="item.done">done</b>
        </li>
    "#;

    fn items() -> Value {
        value!({ "items": [
            { "id": 1, "label": "a", "done": false },
            { "id": 2, "label": "b", "done": true },
            { "id": 3, "label": "c", "done": false },
        ] })
    }

    #[test]
    fn costs_the_same_as_inlined_markup() {
        // The same template written with a component and by hand.
        let mut with_component = component_app();
        component(&mut with_component, "Row", ROW);
        let a = spawn(
            &mut with_component,
            r#"<div><Row v-for="(it, i) in items" :key="it.id" class="x" :item="it" :n="i" @custom="record(i)"/></div>"#,
            items(),
        );
        let mut inlined = component_app();
        let b = spawn(
            &mut inlined,
            r#"<div>
                <li v-for="(it, i) in items" :key="it.id" class="row" class="x" @click="record('own', it.id)" @custom="record(i)">
                    <p>{{ it.label }}</p>
                    <span>{{ i }}</span>
                    <b v-if="it.done">done</b>
                </li>
            </div>"#,
            items(),
        );

        let compare = |with_component: &mut App, inlined: &mut App| {
            assert_eq!(texts(with_component.world(), a), texts(inlined.world(), b));
            assert_eq!(stats(with_component.world(), a), stats(inlined.world(), b));
            assert_eq!(node_count(with_component), node_count(inlined));
            // Every element carries the same components: a component adds none.
            let (rows_a, rows_b) = (children(with_component.world(), a), children(inlined.world(), b));
            assert_eq!(rows_a.len(), rows_b.len());
            for (row_a, row_b) in rows_a.into_iter().zip(rows_b) {
                let count = |world: &World, entity: Entity| world.entity(entity).archetype().components().len();
                assert_eq!(count(with_component.world(), row_a), count(inlined.world(), row_b));
                assert_eq!(
                    with_component.world().get::<ClassList>(row_a),
                    inlined.world().get::<ClassList>(row_b)
                );
            }
        };
        compare(&mut with_component, &mut inlined);
        assert_eq!(texts(with_component.world(), a), vec!["a", "0", "b", "1", "done", "c", "2"]);
        assert!(with_component.world().get::<ClassList>(children(with_component.world(), a)[0]).unwrap().contains("x"));

        // The same prop changes do the same work.
        let changes: [fn(&mut Value); 4] = [
            |props| {
                props.at_mut(&path("items[1].label")).unwrap().set("B");
            },
            |props| {
                props.at_mut(&path("items[0].done")).unwrap().set(true);
            },
            |props| props.get_mut("items").unwrap().push(value!({ "id": 4, "label": "d", "done": true })),
            |props| {
                props.get_mut("items").unwrap().remove_at(0);
            },
        ];
        for change in changes {
            change(&mut props(&mut with_component, a).value);
            change(&mut props(&mut inlined, b).value);
            assert_eq!(update(&mut with_component, a), update(&mut inlined, b));
            compare(&mut with_component, &mut inlined);
        }
        assert_eq!(texts(with_component.world(), a), vec!["B", "0", "done", "c", "1", "d", "2", "done"]);
    }

    #[test]
    fn attributes() {
        let mut app = component_app();
        component(&mut app, "Card", r#"<div><p>{{ title }}</p><p v-if="open">open</p><p>{{ item_count }}</p></div>"#);
        let root = spawn(
            &mut app,
            r#"<div><Card title="Hi" open :item-count="n"/><Card :title="name" :item-count="2"/></div>"#,
            value!({ "n": 1, "name": "ana" }),
        );
        assert_eq!(texts(app.world(), root), vec!["Hi", "open", "1", "ana", "2"]);

        let first = children(app.world(), root)[0];
        let [title, _, count] = children(app.world(), first)[..] else { panic!("expected three children") };
        let title_tick = text_changed_tick(app.world(), title);
        let count_tick = text_changed_tick(app.world(), count);

        props(&mut app, root).get_mut("n").unwrap().set(5);
        app.update();
        assert_eq!(texts(app.world(), root), vec!["Hi", "open", "5", "ana", "2"]);
        assert_eq!(text_changed_tick(app.world(), title), title_tick);
        assert_ne!(text_changed_tick(app.world(), count), count_tick);
    }

    #[test]
    fn keyed_components_and_conditions() {
        let mut app = component_app();
        component(&mut app, "Row", ROW);
        let root = spawn(
            &mut app,
            r#"<div>
                <Row v-for="(it, i) in items" :key="it.id" :item="it" :n="i"/>
                <Row v-if="extra" :item="extra" n="x"/>
                <p v-else>none</p>
            </div>"#,
            items(),
        );
        assert_eq!(texts(app.world(), root), vec!["a", "0", "b", "1", "done", "c", "2", "none"]);
        let [a, b, c, _] = children(app.world(), root)[..] else { panic!("expected four children") };

        // Reordering moves the entities of the components.
        props(&mut app, root).get_mut("items").unwrap().remove_at(0);
        props(&mut app, root).get_mut("items").unwrap().push(value!({ "id": 1, "label": "a", "done": false }));
        props(&mut app, root).insert("extra", value!({ "id": 9, "label": "z", "done": true }));
        app.update();
        assert_eq!(texts(app.world(), root), vec!["b", "0", "done", "c", "1", "a", "2", "z", "x", "done"]);
        assert_eq!(children(app.world(), root)[..3], [b, c, a], "keyed components are reused");
    }

    #[test]
    fn nested_components() {
        let mut app = component_app();
        component(&mut app, "Tag", r#"<span>{{ label }}:{{ owner }}</span>"#);
        component(
            &mut app,
            "Row",
            r#"<li><p>{{ item.name }}</p><Tag v-for="tag in item.tags" :label="tag" :owner="item.name"/></li>"#,
        );
        let root = spawn(
            &mut app,
            r#"<div><Row v-for="item in items" :item="item"/></div>"#,
            value!({ "items": [{ "name": "a", "tags": ["x", "y"] }, { "name": "b", "tags": [] }] }),
        );
        assert_eq!(texts(app.world(), root), vec!["a", "x:a", "y:a", "b"]);

        props(&mut app, root).at_mut(&path("items[1].tags")).unwrap().push("z");
        props(&mut app, root).at_mut(&path("items[0].name")).unwrap().set("A");
        app.update();
        assert_eq!(texts(app.world(), root), vec!["A", "x:A", "y:A", "b", "z:b"]);
    }

    #[test]
    fn emit_reaches_the_component_element() {
        let mut app = component_app();
        component(
            &mut app,
            "Row",
            r#"<li @click="record('own')"><button @custom="$emit('remove', item.label, $event)">x</button></li>"#,
        );
        let root = spawn(
            &mut app,
            r#"<div><Row v-for="(it, i) in items" :key="it.id" :item="it" @remove="record(i, it.id, $args)" @click="record('host', i)"/></div>"#,
            items(),
        );
        let row = children(app.world(), root)[1];
        let button = children(app.world(), row)[0];

        app.world_mut().trigger(BevueEvent::new(button, "custom", vec![Value::from("!")]));
        app.update();
        assert_eq!(
            std::mem::take(&mut app.world_mut().resource_mut::<Recorded>().0),
            vec![vec![Value::from(1), Value::from(2), value!(["b", "!"])]]
        );
        let log = std::mem::take(&mut app.world_mut().resource_mut::<EventLog>().0);
        assert_eq!(log, vec![(button, "custom".to_owned()), (row, "remove".to_owned())]);

        // The root of the component and the element both listen to `click`.
        app.world_mut().trigger(BevueEvent::new(row, "click", vec![]));
        app.update();
        assert_eq!(
            app.world().resource::<Recorded>().0,
            vec![vec![Value::from("own")], vec![Value::from("host"), Value::from(1)]]
        );
    }

    #[test]
    fn nested_emit() {
        let mut app = component_app();
        component(&mut app, "Inner", r#"<b @custom="$emit('ping', 1)">x</b>"#);
        component(&mut app, "Outer", r#"<section><Inner @ping="$emit('pong', $event, 2)"/></section>"#);
        let root = spawn(&mut app, r#"<div><Outer @pong="$emit('done', $args)"/></div>"#, Value::empty_dict());
        let outer = children(app.world(), root)[0];
        let inner = children(app.world(), outer)[0];

        app.world_mut().trigger(BevueEvent::new(inner, "custom", vec![]));
        app.update();
        let log = std::mem::take(&mut app.world_mut().resource_mut::<EventLog>().0);
        let expected = [(inner, "custom"), (inner, "ping"), (outer, "pong"), (root, "done")];
        assert_eq!(log, expected.map(|(entity, name)| (entity, name.to_owned())));
    }

    #[test]
    fn props_and_styles_of_components_are_the_hosts() {
        let mut app = component_app();
        app.register_props::<Menu>();
        app.insert_resource(Menu { title: "File".into() });
        component(
            &mut app,
            "Title",
            r#"<props><resource type="Menu"/></props><template><p>{{ menu.title }}</p></template><style>.title { color: red; }</style>"#,
        );
        let root = spawn(
            &mut app,
            r#"<template><div><Title/><Title/><span>{{ menu.title }}</span></div></template><style>.host { color: blue; }</style>"#,
            Value::empty_dict(),
        );
        assert_eq!(texts(app.world(), root), vec!["File", "File", "File"]);
        assert!(app.world().get::<PropsResource<Menu>>(root).is_some());

        app.world_mut().resource_mut::<Menu>().title = "Edit".into();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["Edit", "Edit", "Edit"]);

        let Some(Styled::StyleSheet(sheet)) = app.world().get::<Styled>(root) else { panic!("expected a stylesheet") };
        let registry = app.world().resource::<BevueAssetRegistry>();
        let css = registry.get_asset(sheet.path().unwrap().path()).unwrap();
        let css = std::str::from_utf8(css.value()).unwrap();
        let (title, host) = (css.find(".title").unwrap(), css.find(".host").unwrap());
        assert!(title < host, "the host's rules come last: {css}");
        assert_eq!(css.matches(".title").count(), 1);
    }

    #[test]
    fn late_and_modified_component_assets() {
        let mut app = component_app();
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().reserve_handle();
        app.world_mut().get_resource_or_init::<BevueComponents>().insert("Row", handle.clone());

        let root = spawn(&mut app, r#"<div><Row :label="name"/><p>after</p></div>"#, value!({ "name": "ana" }));
        assert_eq!(texts(app.world(), root), vec!["after"]);
        assert_eq!(children(app.world(), root).len(), 2, "an empty node takes the component's place");
        assert_eq!(app.world().get::<ComponentDependencies>(root).unwrap().assets, vec![handle.id()]);

        // The component arrives. Asset events are emitted at the end of a frame and read on the
        // next one.
        let parse = |source: &str| crate::parser::parse_template(source).unwrap();
        let _ = app.world_mut().resource_mut::<Assets<BevueTemplate>>().insert(&handle, parse(r#"<b>{{ label }}</b>"#));
        app.update();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["ana", "after"]);

        // Props set in the meantime survive a reload of the component.
        props(&mut app, root).get_mut("name").unwrap().set("bo");
        *app.world_mut().resource_mut::<Assets<BevueTemplate>>().get_mut(&handle).unwrap() = parse(r#"<i>{{ label }}!</i>"#);
        app.update();
        app.update();
        assert_eq!(texts(app.world(), root), vec!["bo!", "after"]);

        props(&mut app, root).get_mut("name").unwrap().set("cy");
        app.update();
        assert_eq!(texts(app.world(), root), vec!["cy!", "after"]);
    }

    #[test]
    fn invalid_components_do_not_render() {
        let mut app = component_app();
        component(&mut app, "Loop", r#"<div><Loop/></div>"#);
        for source in [r#"<div><Nope/></div>"#, r#"<div><Loop/></div>"#] {
            let root = spawn(&mut app, source, Value::empty_dict());
            assert!(app.world().get::<TemplateInstance>(root).is_none(), "{source}");
            assert!(children(app.world(), root).is_empty(), "{source}");
        }
    }

    #[test]
    #[should_panic(expected = "uppercase")]
    fn component_names_are_capitalised() {
        BevueComponents::default().register("row", "row.html");
    }
}
