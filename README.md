# About

Bevue brings [Vue](https://vuejs.org/guide/introduction.html)-like templating to Bevy UI, enabling fast prototyping of user interfaces while leveraging the power of Bevy's ECS.

## Features

- [X] Declare Bevy UI with Vue-like HTML templates;
- [X] Loading templates from assets with hot reloading;
- [X] Easy styling with CSS through [bevy-flair](https://github.com/eckz/bevy_flair) crate;
- [X] Using Bevy components/resources (`Props`) as template inputs;
- [X] Fine-grained reactivity: only UI nodes bound to the props that changed are updated;
- [X] Text interpolation e.g `Hello {{ user.name }}!`;
- [X] Conditional rendering using the `v-if`, `v-else-if` and `v-else` directives;
- [X] List rendering using the `v-for` directive, e.g. `v-for="(item, i) in items" :key="item.id"`;
- [X] Dynamic styles with `:style`, e.g. `:style="{ 'background-color': theme.color }"`;
- [X] Operators in expressions, e.g. `{{ item.count * item.price }}`, `v-if="!done && count > 0"`;
- [X] Template strings, e.g. `` :style="{ width: `${progress * 100}%` }" ``;
- [X] Function calls in expressions, backed by Bevy one-shot systems, e.g. `{{ pressed_button(button.id) }}`;
- [X] Event handling with `@click`, `@mouseover`, `@mouseleave`, custom events and `$emit`;
- [X] Importing components from within HTML, e.g. `<import src="ui/item.html"/>`;
- [X] Component elements such as `<ItemSlot :item="item" @pickup="pickup_item(i)"/>`;

Bevue is only inspired by Vue, it won't:
- Support *all* of Vue's features;
- Embed the Vue framework or a JavaScript engine. Everything still happens in Rust.

## Getting started

```sh
cargo add bevue
```

Add `BevuePlugin` after `DefaultPlugins`, then spawn a `Template` with the data it renders:

```rust
use bevue::{BevuePlugin, BevuePropsAppExt, Props, Template};
use bevy::prelude::*;

#[derive(Component, Props)]
struct Menu {
    user_name: String,
}

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, BevuePlugin))
        .register_props::<Menu>()
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    commands.spawn((
        Template::new(assets.load("menu.html")),
        Menu { user_name: "John".into() },
    ));
}
```

```html
<!-- assets/menu.html -->
<props>
    <component type="Menu"/>
</props>

<template>
    <div class="menu">
        <p>Hello, {{ menu.user_name }}! How are you?</p>
    </div>
</template>

<style>
.menu { flex-direction: column; row-gap: 20px; }
</style>
```

A template file has a `<template>` with one root element, and optionally `<props>`, `<style>` and
`<import>` next to it. Every element becomes a Bevy UI `Node` on its own entity and its text a
`Text`; tag names are only labels, so style elements by class. Saving the file updates the running
app.

Check out [`examples/`](examples) for more complete showcases.

## Props

Props are the data a template reads. Derive `Props` on a component or a resource, register the
type, and name it in the template's `<props>`:

```rust
#[derive(Resource, Props, Default)]
struct Inventory {
    items: Vec<Item>,
    #[props(skip)]          // not visible to templates
    last_saved: f32,
}

#[derive(Props, Clone)]
struct Item { id: u32, count: u32 }

app.register_props::<Inventory>().init_resource::<Inventory>();
```

```html
<props>
    <resource type="Inventory"/>                 <!-- read as `inventory` -->
    <resource name="settings" type="my_game::ui::Settings"/>
    <component type="Menu"/>                     <!-- a component of the template entity -->
</props>
```

A type is read under its name in snake case (`PlayerInventory` is `player_inventory`) unless
`name` says otherwise.

There is nothing else to wire up: change the component or resource as usual and the UI follows.
Bevue compares the new data with what it rendered and updates only the nodes bound to what
changed, so editing `inventory.items[1].count` touches a single text.

## Text and expressions

```html
<p>Hello, {{ menu.user_name }}!</p>
<p>{{ inventory.items[0].count }} of {{ format_name(inventory.items[0].id) }}</p>
```

An expression is a path into the props (`a.b[0].c`, `rows[i].name`), a string, number or boolean,
a function call, or an object `{ key: value }`, combined with operators:

```html
<p v-if="!menu.answered && inventory.items[0].count > 0">...</p>
<p>{{ item.count * item.price }} gold, {{ item.name || 'unnamed' }}</p>
<li v-for="(item, i) in items" :class="{ last: i == total - 1 }">{{ i + 1 }}</li>
<p :class="item.count > 0 ? 'in-stock' : 'sold-out'">{{ 'Item #' + item.id }}</p>
```

| Operators, loosest first | |
|---|---|
| `a ? b : c` | evaluates only the branch it picks |
| `\|\|` `&&` | return one of their operands, so `name \|\| 'unnamed'` is a default |
| `==` `!=` | deep comparison, without type conversion (`1 == '1'` is false) |
| `<` `<=` `>` `>=` | two numbers, or two strings alphabetically |
| `+` `-` `*` `/` `%` | numbers; `+` joins as text when either side is a string |
| `!` `-` `+` | prefix |

Arithmetic that makes no sense for its operands (`'a' * 2`, or a prop that is not there yet) gives
`null`, which renders as empty text, and such a comparison is false. Neither is an error.

A template string builds text from expressions:

```html
<p>{{ `Hello, ${user.name || 'stranger'}!` }}</p>
<div :class="`slot slot-${item.rarity}`" :style="{ width: `${progress * 100}%` }"></div>
```

## Conditions and lists

```html
<p v-if="menu.answered">Thank you for your answer!</p>
<p v-else-if="menu.skipped">Maybe later.</p>
<div v-else>
    <button v-for="option in menu.options">{{ option }}</button>
</div>

<li v-for="(item, i) in inventory.items" :key="item.id">
    {{ i }}: {{ item.count }}
</li>
```

With `:key`, the entities of a list follow their items when the list is reordered, instead of
being rewritten in place. `v-if` and `v-for` cannot be on the same element; wrap one in the other.

## Functions

A function is a Bevy system that takes `In<BevueCtx>`. It can have any other system parameter and
can return a value:

```rust
fn format_name(In(ctx): In<BevueCtx>, names: Res<ItemNames>) -> String {
    let id: u32 = ctx.get_args().unwrap();
    names.get(id).to_owned()
}

fn remove_item(In(ctx): In<BevueCtx>, mut inventory: ResMut<Inventory>) {
    let Ok((index, _reason)) = ctx.get_args::<(usize, Option<String>)>() else { return };
    inventory.items.remove(index);
}

app.register_bevue_function("format_name", format_name);                  // every template
Template::new(handle).with_function("remove_item", remove_item);          // this template only
```

```html
<p>{{ format_name(item.id) }}</p>
<p v-if="is_empty(inventory.items)">Nothing here.</p>
<li v-for="item in sorted(inventory.items)">{{ item.id }}</li>
```

`ctx.get_args()` converts the arguments to Rust types: one type reads the first argument and a
tuple reads one per element. `ctx.root` is the template entity and `ctx.entity` the node that made
the call. A call in text, a condition or a list runs again only when the props passed to it
change.

## Events

```html
<button @click="remove_item(i, 'clicked')">Remove</button>
<div @mouseover="highlight(item.id)" @mouseleave="highlight()">...</div>
```

```rust
fn answer(In(ctx): In<BevueCtx>, mut menus: Query<&mut Menu>) {
    let option: String = ctx.get_args().unwrap();
    info!("answered {option}");
    menus.get_mut(ctx.root).unwrap().answered = true;
}
```

`click`, `mouseover` and `mouseleave` are triggered by Bevue for the nodes that listen to them. A
handler is a function call, and its changes to the props are on screen in the same frame.

Every event is also a Bevy `BevueEvent`, so it can be observed from Rust. `$emit` triggers one of
your own:

```html
<button @click="$emit('close', menu.user_name)">Close</button>
```

```rust
app.add_observer(|event: On<BevueEvent>| {
    if event.name == "close" {
        info!("closed by {:?}", event.args);
    }
});
```

## Styles

Styling is CSS, applied by [bevy-flair](https://github.com/eckz/bevy_flair). Write it in the
template, load it from a file in the assets folder, or both:

```html
<style>
.menu { flex-direction: column; row-gap: 20px; }
.option:hover { background-color: #444; }
</style>
```

```html
<style src="ui/menu.css"></style>
```

```html
<style src="ui/menu.css">
.option:hover { background-color: #444; }
</style>
```

A template has a single `<style>` element.

Classes can come from props. A string is a list of class names and an object turns each class on
or off:

```html
<div class="option" :class="item.rarity"></div>
<div class="option" :class="{ selected: item.selected, 'is-empty': item.empty }"></div>
```

`:style` sets individual properties from props, over whatever the stylesheet says:

```html
<p :style="{ color: item.color, 'font-size': '18px' }">{{ item.id }}</p>
<div style="width: 200px" :style="theme.panel"></div>
```

A key can be written `backgroundColor`, `background_color` or `'background-color'`. Values are
used as they are, so a length needs its unit (`'10px'`), and a `null` or `false` value leaves the
property out.

## Components

A template file can be used as an element of another one. Import it, and pass it data through
attributes:

```html
<!-- assets/inventory.html -->
<import src="ui/item_slot.html"/>

<template>
    <div class="inventory">
        <ItemSlot v-for="(item, i) in inventory.items" :item="item" @pickup="pickup_item(i)"/>
    </div>
</template>
```

```html
<!-- assets/ui/item_slot.html -->
<template>
    <div class="slot">
        <p>{{ item.count }}</p>
        <button @click="$emit('pickup')">Take</button>
    </div>
</template>

<style>
.slot { padding: 5px; background-color: #333; }
</style>
```

- The element is named after the file in PascalCase (`item_slot.html` is `ItemSlot`), or by
  `<import src="..." as="Slot"/>`. A `src` starting with `./` or `../` is relative to the importing
  file.
- `:item="expr"` passes a value, `title="text"` a string. Inside the component they are read by
  name, with `-` as `_`.
- `$emit('pickup')` in the component reaches `@pickup` on its element.
- `class`, `style`, `v-if`, `v-for` and `:key` on the element apply to the component's root.

Components are inlined into the template that uses them, so splitting a template into components
costs nothing at runtime. In exchange, a component cannot contain itself or have children (there
are no slots yet), its `<style>` is not scoped to it, and it can read the props of the template it
is used in.

## Bevy compatibility

| bevy | bevue         |
|------|---------------|
| 0.19 | 0.1           |
