# About

Bevue brings [Vue](https://vuejs.org/guide/introduction.html)-like templating to Bevy UI, enabling fast prototyping of user interfaces while leveraging the power of Bevy's ECS. For that, it provides:
- An abstract UI node hierarchy to represent concepts such as interpolation, condition rendering and list rendering.
- A parser and asset loader to let Bevue templates be specified with Bevy assets.
- Lightweight (and very limited) scripting in template expressions, including the use of Rust functions in templates.
- A runtime engine that turn templates into concrete UI nodes with change-detection, immediate mode by default.

Bevue is only inspired by Vue, it won't:
- Support *all* syntax of Vue;
- Embed the Vue framework or a JavaScript engine. Everything still happens in Rust.

# Features

- [X] Vue-like components in HTML files;
- [X] Loading components from assets with hot reloading;
- [X] Easy styling with CSS through `bevy-flair` crate;
- [X] Rendering static components to Bevy UI;
- [X] Text and class interpolation e.g `Hello {{ user.name }}!`, `:class="state"`;
- [X] Conditional rendering using the `v-if`, `v-else-if` and `v-else` directives;
- [X] Keyed list rendering using the `v-for` directive, e.g. `v-for="(item, i) in items" :key="item.id"`;
- [X] Fine-grained reactivity: only UI nodes bound to the props that changed are updated;
- [X] `#[derive(Props)]` to render Bevy components and resources, with path access like `a.b[0].c`;
- [X] Props declared in the component file with `<props>`;
- [X] Function calls in expressions, backed by Bevy systems, e.g. `{{ format(price) }}`;
- [X] Event handling with `@click`, `@mouseover`, `@mouseleave`, custom events and `$emit`;
- [X] Component elements such as `<ItemRow :item="item" @remove="remove_item(i)"/>`, at the cost of inlined markup;

# Reactivity

A template renders the `BevueProps` component on its entity. Mutate it through the `Value`
mutators (`get_mut`, `at_mut`, `set`, `push`, `insert`, ...), which record exactly what changed:

```rust
fn rename(mut props: Query<&mut BevueProps>) {
    for mut props in &mut props {
        props.at_mut(&"user.name".parse().unwrap()).unwrap().set("Ana");
    }
}
```

Or derive `Props` on a component or resource and let Bevy change detection sync it:

```rust
#[derive(Resource, Props)]
struct Menu { title: String, actions: Vec<Action> }

app.register_props::<Menu>();
commands.spawn((Template::new(asset_server.load("menu.html")), PropsResource::<Menu>::at("menu")));
```

`PropsResource::<Menu>::default()` mounts the resource at its default name: the type name in snake
case (`menu`, or `player_inventory` for `PlayerInventory`), or the one set with
`#[props(name = "...")]` on the type.

Changed Rust data is diffed into the props, so editing `menu.actions[1].label` only updates the
text bound to that label.

A template can also declare the registered types it renders, in a `<props>` element next to
`<template>` and `<style>`. Then spawning `Template` alone is enough:

```html
<props>
    <resource name="menu" type="Menu"/>
    <resource type="my_game::ui::Settings"/>
    <component type="Player"/>
</props>
```

- `<resource>` mounts a resource, like `PropsResource`. `<component>` mounts a component of the
  template entity, like `PropsComponent`.
- `type` is the full path of the type or just its name. If two registered types share a name, the
  name refers to the one registered last.
- `name` is the key in the props. Without it, the default name of the type is used.
- A `PropsResource` or `PropsComponent` you inserted yourself for the same type is kept as it is.

# Functions and events

Functions are Bevy systems taking `In<BevueCtx>`. They can be called anywhere in a template; calls
in bindings re-run only when the props their arguments read change.

```rust
fn add_user(In(ctx): In<BevueCtx>, mut props: Query<&mut BevueProps>) {
    props.get_mut(ctx.root).unwrap().get_mut("users").unwrap().push("new");
}

app.register_bevue_function("add_user", add_user); // global
Template::new(handle).with_function("add_user", add_user); // only for this template
```

`ctx.args` holds the call arguments as `Value`s. `ctx.get_args()` converts them to Rust types: a
single type reads the first argument and a tuple reads one argument per element.

```rust
fn rename_user(In(ctx): In<BevueCtx>, mut props: Query<&mut BevueProps>) {
    let Ok((index, name)) = ctx.get_args::<(usize, String)>() else { return };
    // ...
}
```

```html
<button @click="add_user()">Add</button>
<li v-for="user in users" @click="$emit('select', user.id)">{{ user.name }}</li>
<p @click>Observed from Bevy</p>
```

Bevue adds `Interaction` to nodes that listen to `click`, `mouseover` or `mouseleave`, and triggers
a `BevueEvent { entity, name, args }` for them. Trigger your own with
`commands.trigger(BevueEvent { .. })`, and observe any of them with `On<BevueEvent>`. Handlers can
read `$event` (the first event argument) and `$args` (all of them). `$emit(name, ...args)` triggers
an event on the template root.

# Components

Register a template file under a capitalised name and use it as an element in any template:

```rust
app.register_component("ItemRow", "item_row.html");
```

```html
<!-- inventory.html -->
<ItemRow v-for="(item, i) in inventory.items" :item="item" @remove="remove_item(i)"/>

<!-- item_row.html -->
<li class="item">
    <p>{{ item.id }}</p>
    <button @click="$emit('remove')">X</button>
</li>
```

- Attributes are the component's props: `:item="expr"` is evaluated in the template using the
  component, `title="text"` is a string and a bare `open` is `true`. A `-` in a name becomes `_`
  (`:item-count` is read as `item_count`).
- `class`, `:class`, `v-if`, `v-else-if`, `v-else`, `v-for` and `:key` apply to the root element of
  the component, which takes the place of the component element.
- `$emit(name, ...args)` inside a component triggers the event on the component's root element,
  where the `@name` listener of the component element handles it.

Components are expanded when the template using them is compiled: `{{ item.id }}` above becomes the
same binding as if the `<li>` was written inside `inventory.html`. A component has no entity,
component or props of its own, so splitting a template into components costs nothing at runtime.
This also means that:

- An identifier that is not an attribute reads the props of the template the component is used in
  (but not its `v-for` aliases). The `<props>` declared by a component file are mounted on that
  template's entity.
- The `<style>` of a component is added to the stylesheet of the template, it is not scoped.
- An attribute expression is evaluated wherever the component uses it.
- A component cannot contain itself, and cannot have children (there are no slots yet).
- A template or component cannot have a component as its root element.

A template is rendered again when the file of a component it uses is loaded or changed.

# Debugging performance

Bevue logs how long its work took at the `debug` level. Enable it with Bevy's log filter:

```rust
DefaultPlugins.set(LogPlugin { filter: "wgpu=error,naga=warn,bevue=debug".into(), ..Default::default() })
```

```text
DEBUG bevue::runtime::props: synced simple::Counter into 1 template(s), 1 changed, in 4.1µs
DEBUG bevue::runtime::render: rendered simple.html on 12v0: 4 bindings, 2 blocks, 0 list items, in 310µs
DEBUG bevue::runtime::render: updated template on 12v0: 2 bindings, 1 lists in 95µs
```

- `synced` is the cost of diffing a `Props` component or resource into `BevueProps`. It is logged
  when the diff found a change; syncs that found nothing are logged at the `trace` level.
- `rendered` is a full render: the first one, or a re-render after the template asset changed.
- `updated` is an incremental update: the bindings re-evaluated and the lists reconciled.

`cargo bench --bench reactivity` measures the same phases for lists of 100 to 10 000 items.
