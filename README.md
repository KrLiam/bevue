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

app.register_props_resource::<Menu>();
commands.spawn((Template::new(asset_server.load("menu.html")), PropsResource::<Menu>::at("menu")));
```

Changed Rust data is diffed into the props, so editing `menu.actions[1].label` only updates the
text bound to that label.