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
- [X] Function calls in expressions, backed by Bevy systems, e.g. `{{ format(price) }}`;
- [X] Event handling with `@click`, `@mouseover`, `@mouseleave`, custom events and `$emit`;

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
