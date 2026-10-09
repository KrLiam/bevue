# Bevue: call expressions, `$` identifiers, events and Bevy-system functions

## Context

Templates could read props reactively but could not do anything when the user interacted with them. This adds:
- Call expressions `ident(expr, ...)` that work everywhere (decided): text, `:class`, `v-if`, `:key`, `v-for` iterables and handlers.
- A built-in `$emit("name", ...args)`, usable only in event handlers, that triggers a `BevueEvent` on the template root.
- Identifiers that can start with or contain `$` (needed for `$event` / `$args`).
- Vue-style events with `@name` / `v-on:name`, all dispatched through one public `BevueEvent` entity event. Bevue fires it for `click`, `mouseover` and `mouseleave` from `Interaction`; users can trigger any name themselves.
- Functions implemented as Bevy one-shot systems, registered globally with `app.register_bevue_function` or per template with `Template::new(..).with_function`. Component hooks register and unregister them.
- Nesting behaves like the DOM (decided): every listening node under the cursor gets its own `Interaction`. This is already Bevy's default, since `Node` requires `FocusPolicy::Pass` in Bevy 0.19.

## 1. Parser — `src/parser/expr.rs`, `src/core/template.rs`, `src/parser/mod.rs`

- `ident_str`: the first character and the rest may also be `$`, so `[a-zA-Z_$][a-zA-Z0-9_$]*`.
- `Expression::Call { function: Identifier, args: Vec<Expression> }`.
  - In `parse_atom`, an identifier followed by optional whitespace and `(` parses a comma-separated, possibly empty argument list (each argument via `parse_expr_internal`), then `)`. A trailing comma is an error.
  - `true(` and `false(` are errors.
  - Postfix `.field` / `[i]` after a call still go through the existing loop in `parse_expr_internal`.
- `TemplateNode` gets `events: Vec<(Cow<'static, str>, Option<Expression>)>`.
  - In `parse_template`, attributes `@name` and `v-on:name` go into it. A missing value (`<p @click>`) gives `None`.
  - They are handled before the boolean-attribute fallback.
  - Modifiers (`@click.stop`) and empty names are rejected with `ParseError::InvalidDirective` for now.
- **Attributes are read from the raw tag source**, not from `tl`. `tl` 0.7.8 drops the `@` from attribute names, and it cuts the first character off any attribute that follows a valueless one: `<p disabled class="a">` gives `lass="a"`, and the same happened after `v-else`. `parse_attributes` scans the opening tag in source order and supports double-quoted, single-quoted and unquoted values, including quoted values containing `>`.
- Tests:
  - `$event`, `a$b`.
  - `f()`, `f(a.b, 1, "x")`, `f(g(x))[0].name`, `$emit('buy', $event)`, and the errors `f(a,)`, `f(a`, `true(1)`.
  - `@click`, `@click="add()"`, `v-on:custom="h"`, `@click.stop` rejected.
  - Raw attribute scanning, and a regression test for `<p v-else class="x">`.

## 2. Compile — `src/core/compile.rs`

- `CompiledExpr::Call { function: Cow<'static, str>, args: Vec<CompiledExpr>, segments: Vec<CompiledSegment> }`. The segments apply to the result, so `extend()` appends to them.
- `dependencies()` of a call is the union of its arguments' dependencies, plus those of any dynamic segments on its result. Functions are assumed pure: `{{ now() }}` is evaluated once and only re-run on a full refresh.
- `ScopeRef::Event`, valid only in handler expressions:
  - `$event` resolves to `Path { root: Event, segments: [Index(0)] }` (the first event argument).
  - `$args` resolves to `Path { root: Event, segments: [] }` (all arguments as a list).
  - Event arguments are not reactive and add no dependency.
  - Using them outside a handler is a `CompileError`. `Compiler` has an `in_handler` flag, and calls to `$emit` outside a handler are rejected the same way.
- `CompiledNode` gets `listeners: Vec<CompiledListener { name, handler: Option<CompiledHandler> }>`:
  - `CompiledHandler::Call { function, args }`, e.g. `@click="add_user(item.id, $event)"`.
  - `CompiledHandler::Function(name)`: a bare identifier `@click="add_user"`, called with the event's args (Vue method-handler semantics).
  - Any other expression (a literal, a path, or `$event`) is a `CompileError`, "event handler must be a function call or a function name".
- `CompiledNode::interactive()` returns true if any listener is `click`, `mouseover` or `mouseleave` (`INTERACTION_EVENTS`). `CompiledNode::listener(name)` finds a listener.

## 3. Evaluation with calls — `src/runtime/interpolation.rs`

The free-function arguments `(frames, props)` are replaced with a context:
```rust
pub struct EvalCtx<'p, 'f, 'c> {
    pub frames: &'f [Frame],
    pub props: &'p Value,
    pub event_args: Option<&'p [Value]>,
    pub call: &'c mut dyn FnMut(&str, Vec<Value>) -> Value,
}
pub fn no_calls(name: &str, args: Vec<Value>) -> Value; // warns and returns null
```
- `eval<'r, 'p: 'r>(expr: &'r CompiledExpr, ctx: &mut EvalCtx<'p, '_, '_>) -> Cow<'r, Value>`. The separate lifetimes keep the result borrowing only from props and the expression, never from the call closure.
  - A `Call` evaluates its arguments (owned clones), invokes `ctx.call`, then applies its segments to the owned result.
  - `Event` paths read from `event_args`.
- `render_text`, `class_names` and `eval_path` take `&mut EvalCtx`. Dynamic segments may call functions too.
- **Frames for call iterables**: a `v-for` over a call (`v-for="x in evens(nums)"`) has no path in props. A `Frame` is `{ element: FrameElement, index }`, where `FrameElement` is either `Path(PropPath)` (the element's path in props) or `Value(Arc<[Value]>)` (the computed list).
- Tests use a closure that maps names to fixed values.

## 4. Function registry — `src/runtime/functions.rs`

```rust
pub struct BevueCtx { pub entity: Entity, pub root: Entity, pub args: Vec<Value> }
pub type BevueFunctionId = SystemId<In<BevueCtx>, Value>;

#[derive(Resource, Default)] pub struct BevueFunctions(pub HashMap<String, BevueFunctionId>);   // global
#[derive(Component, Default)] pub struct TemplateFunctions(pub HashMap<String, BevueFunctionId>); // per template, registered
```
- `entity` is the node that triggered or is being evaluated, `root` is the template root, and `args` are the evaluated arguments.
- Any system `fn(In<BevueCtx>, ...params) -> O` with `O: Into<Value>` is converted with `IntoSystem::map` (`box_function`). `impl From<()> for Value` gives null.
- `BevueFunctionsAppExt::register_bevue_function(name, system)` registers the boxed system and inserts its id into `BevueFunctions`. Re-registering a name unregisters the old system.
- `pub fn call_function(world, root, entity, name, args) -> Value`:
  - Looks up `TemplateFunctions` on `root` first, then `BevueFunctions`.
  - Runs the system with `world.run_system_with(id, BevueCtx { .. })`.
  - An unknown name, or an error such as recursion while the system is already running, logs `warn!` and returns null.

**Built-in `$emit(name, ...args)`**:
- `BevueRuntimePlugin` registers it in `BevueFunctions` as a system `fn emit(In(ctx): In<BevueCtx>, mut commands: Commands)`.
- `args[0]` must be a string (otherwise it warns and does nothing). It triggers `BevueEvent { entity: ctx.root, name: args[0], args: args[1..] }`, on **the template root**. This matches Vue, where `$emit` raises an event on the component itself; when components arrive, a parent's `@name` listener on the component element will catch it.
- Example: `<button @click="$emit('buy', item.id)">` triggers `buy` with `[item.id]` on the root. User observers (`commands.entity(root).observe(..)` or a global `On<BevueEvent>`) and an `@buy` listener on the root element receive it.
- Handler-only: it is a compile error anywhere outside an `@event` expression (text, `:class`, `v-if`, `v-for`, `:key`), the same as `$event`/`$args`.
- The emitted event is dispatched while the emitting handler is still running, so a root listener that re-emits its own event (`@buy="$emit('buy')"` on the root element) recurses. Nesting is capped by `MAX_DISPATCH_DEPTH` (§5).
- Names starting with `$` are reserved for built-ins: `register_bevue_function` and `with_function` panic on them.

**Per-template functions** (`src/runtime/render.rs` `Template`):
- `Template` is `{ pub handle, functions: Vec<(Cow<'static, str>, BevueFunctionFactory)> }`, where `BevueFunctionFactory = Arc<dyn Fn() -> BoxedSystem<In<BevueCtx>, Value> + Send + Sync>`.
  - `with_function(name, system: impl IntoSystem<In<BevueCtx>, O, M> + Clone + Send + Sync + 'static)` stores a factory, so a cloned `Template` registers its own instance.
  - `Template(handle)` call sites changed to `Template::new(handle)`.
  - `Clone` stays derivable through the `Arc`s. `PartialEq`/`Eq` and `Debug` are written by hand, using the handle and function names.
- Component hooks (Bevy 0.19 calls the replace/remove hook `on_discard`):
  - `Template` has `#[component(on_insert = register_template_functions, on_discard = unregister_template_functions)]`.
  - The insert hook builds the boxed systems from the factories and queues a command. The command calls `world.register_boxed_system` for each and inserts `TemplateFunctions` on the entity (or unregisters them if the entity is gone).
  - The `Template` discard hook queues a command that removes `TemplateFunctions`.
  - `TemplateFunctions` has its own `on_discard` hook that unregisters its systems, so they are freed on removal, replacement and despawn. Keeping this on `TemplateFunctions` means replacing a `Template` before its registration command has run can't leak systems.

## 5. Events — `src/runtime/events.rs`

```rust
#[derive(EntityEvent, Clone, Debug, PartialEq)]
pub struct BevueEvent { pub entity: Entity, pub name: Cow<'static, str>, pub args: Vec<Value> }
```
- Public: users trigger it with `commands.trigger(BevueEvent { .. })` (or `BevueEvent::new(entity, name, args)`) and can observe it with `On<BevueEvent>`, globally or on an entity.
- No propagation. Nested listeners each get their own `Interaction`.

**Runtime bookkeeping** (`src/runtime/instance.rs`):
- `TemplateInstance` has `listeners: HashMap<Entity, (NodeId, Vec<ItemId> /* scope */)>`, filled in `spawn_element` when the compiled node has listeners.
- Teardown removes the entries for `owned.elements`; for the root, the whole map is dropped on re-render.
- `pub fn resolve_event(&self, world, props: &Value, entity, name, args) -> Option<(String, Vec<Value>)>`:
  1. Find the node's listener named `name`. A missing listener or one with no handler means `None`.
  2. Build the frames from the stored scope.
  3. For `Call`, evaluate the arguments with `event_args = Some(args)`, using `call_function` for nested calls; for `Function(name)`, pass the event args through.
  4. Return the function name and arguments; the caller runs the function.
- **Interaction:** in `spawn_element`, if `node.interactive()` and the entity has no `Interaction`, insert `Interaction::default()`. Nodes that don't listen get nothing.
- **Focus policy:** no special handling. In Bevy 0.19 `Node` requires `FocusPolicy`, defaulting to `Pass`, so a plain child `<span>` doesn't block the listening button under it. This also means the template root doesn't block clicks on UI behind it; add `FocusPolicy::Block` to the template entity for that.
- **Calls during render and update:**
  - Every method that evaluates expressions (`apply`, `update_if`, `reconcile`, `frames`, `walk_item`, `resolve_event`) builds an `EvalCtx` whose `call` closure captures `world` and the evaluating entity, through `caller(world, root, entity)`.
  - The closure is dropped (or scoped) before the world is used again, so writes to the entity happen after evaluation.
  - Function systems run while that template's `BevueProps`/`TemplateInstance` are taken out (`take_props` in `render.rs`). Binding functions shouldn't read their own template's props or instance; event handlers can, since they run outside updates.
- **Iterables that aren't static paths** (calls or dynamic paths like `groups[g]`): `reconcile` caches a call's result in `ForBlock.computed` for the items' frames. Such lists get no element routing, so reused items are always re-evaluated when the list updates. This also fixes stale items for dynamic paths.

**Systems and observer** (registered in `BevueRuntimePlugin`, `src/runtime/mod.rs`):
- `emit_interaction_events` runs in `PreUpdate`, `.after(UiSystems::Focus)`, so handlers run before `Update`, and `PostUpdate` renders their prop changes in the same frame.
  - It queries `(Entity, &Interaction, Option<&mut LastInteraction>)` with `(Changed<Interaction>, With<TemplateRef>)`, storing the previous state in a `LastInteraction(Interaction)` component.
  - Transitions:
    - `None` → `Hovered`/`Pressed` triggers `mouseover`.
    - `Pressed` → `Hovered` triggers `click` (release over the node; Bevy goes `Pressed` → `None` → `Hovered` inside one focus run).
    - `Hovered`/`Pressed` → `None` triggers `mouseleave`. `Pressed` → `None` (released outside) gives no click.
  - Uses `commands.trigger(BevueEvent::new(entity, name, vec![]))`.
- A global observer `on_bevue_event(On<BevueEvent>, Commands)` queues a plain command closure calling `dispatch_bevue_event(world, event)`.
  - This is a plain function, not a cached system: a handler can trigger events (as `$emit` does) that are dispatched while the outer handler is still running, and a cached system would hit its own recursion error.
  - It runs in two phases:
    1. **Resolve:** read `TemplateRef` on the target. Take `TemplateInstance` and props out of `root` (`mem::take` / `take_props`). `TemplateInstance::resolve_event` returns the function name and evaluated arguments; nested calls such as `g` in `@click="f(g(x))"` run here. Put both back.
    2. **Call:** `call_function(world, root, entity, name, args)` with the instance and props back in place, so handlers can freely mutate `BevueProps` (the update picks that up the same frame).
  - A `DispatchDepth` resource counts nested handlers. Beyond `MAX_DISPATCH_DEPTH` (32) the event is dropped with a warning, which stops listeners that re-emit their own event.
  - Targets that aren't bevue nodes, nodes without a matching listener, and value-less listeners (`<p @click>`) do nothing here. User observers still see the event.
- Exported from `lib.rs`: `BevueEvent`, `BevueCtx`, `BevueFunctionsAppExt`. `runtime` also exports `BevueFunctions`, `TemplateFunctions` and `LastInteraction`.

## Critical files
- Modified:
  - `src/parser/expr.rs`, `src/parser/mod.rs`, `src/core/template.rs` (calls, events, `$`, raw attribute scanning)
  - `src/core/compile.rs` (calls, Event scope, listeners)
  - `src/runtime/interpolation.rs` (`EvalCtx`, value frames)
  - `src/runtime/instance.rs` (calls, listeners, `Interaction`, computed iterables)
  - `src/runtime/render.rs` (`Template` functions and hooks)
  - `src/runtime/mod.rs` (plugin)
  - `src/core/value.rs` (`From<()>`)
  - `src/lib.rs`, `README.md`, `examples/simple.rs`, `assets/simple.html`
- New: `src/runtime/functions.rs`, `src/runtime/events.rs`.
- Reused:
  - `take_props`/`restore_props` (`render.rs`).
  - `TemplateInstance::frames` and the owned-element teardown (`instance.rs`).
  - The test harness in `src/runtime/tests.rs`.

## Verification
`cargo test` and `cargo clippy --all-targets`, with new tests:
- **Parser and compile:** the cases listed in §1; `$event`, `$args` and `$emit` outside a handler are errors; `@click="1"`, `@click="a.b"` and `@click="$event"` are errors.
- **Calls in bindings:** a global `upper` function renders `{{ upper(name) }}`, re-runs when `name` changes, and doesn't re-run on an unrelated prop change (calls counted in a resource).
- **Call iterables:** `v-for="(x, i) in evens(nums)" :key="x"` renders the computed list, updates when `nums` changes, and reuses keyed items.
- **Events from code:** `BevueEvent::new(node, "custom", vec![1.into()])` with `@custom="record($event, item.id)"` inside a `v-for` calls `record` with `[1, <that item's id>]`. An event without a listener reaches user observers but calls nothing. A bare `@custom="record"` gets the event's arguments.
- **Interaction:**
  - `@click` and `@mouseover` nodes get `Interaction`; others don't.
  - All nodes have `FocusPolicy::Pass` (Bevy's default for `Node`).
  - Simulate by setting `Interaction` directly across frames: `None` → `Hovered` (mouseover), `Pressed` → `Hovered` (click), → `None` (mouseleave), and `Pressed` → `None` (no click).
  - A user `On<BevueEvent>` observer sees them, including for a value-less `@mouseover`.
- **`$emit`:**
  - `@click="$emit('buy', item.id, 2)"` on a `v-for` item, after a click, delivers `BevueEvent { entity: root, name: "buy", args: [id, 2] }` to an observer. An `@buy="record($args)"` on the root element receives it too.
  - `$emit(1)` warns and does nothing.
  - A root `@ping="$emit('ping')"` stops after `MAX_DISPATCH_DEPTH + 1` events.
  - `register_bevue_function("$mine", ..)` panics.
- **Handler mutating props:** `@click="add_user()"` with a system that pushes into the root's `BevueProps` shows the new item after the same `app.update()` that delivered the click.
- **Template-local functions:**
  - `Template::new(h).with_function("f", ..)` is callable from that template only; another template calling `f` warns and gets null, and a global function of the same name is overridden locally.
  - Removing `Template` or despawning the entity unregisters the systems: running a removed `SystemId` errors, and `TemplateFunctions` is gone.
  - A cloned `Template` on two entities registers two independent systems, and replacing a `Template` swaps its functions.
- **Manual:** the example has an "Add item" button `<div class="button" @click="add_item()">`, where `add_item` is a system that pushes into the `Counter` resource. Run `cargo run --example simple` and click.

## Results

Everything in the plan is in: call expressions, `$` identifiers, `@event` / `v-on:` listeners, `BevueEvent`, Bevy systems as functions (global and per-template), and `$emit`. All 80 tests pass (up from 62) and clippy is clean. The example builds but wasn't run, since clicking the button needs a person at the window.

What works now:
- **Calls anywhere:** text, `:class`, `v-if`, `:key`, `v-for` iterables and handlers, with `.field` / `[i]` on the result. A call only re-runs when the props its arguments read change.
- **Functions:** systems `fn(In<BevueCtx>, ...) -> impl Into<Value>`, registered globally or per template; a template's own function wins over a global one with the same name. `BevueCtx` has a `root` field (the template entity), so handlers can find their template's props.
- **Events:** `@name="f(a, $event)"` calls `f` with those arguments, and `@name="f"` calls it with the event's arguments. `<p @click>` with no value only adds `Interaction`. Handlers run before `Update`, so their prop changes render in the same frame.
- **`$emit(name, ...args)`:** handler-only, fires on the template root, and `$`-prefixed names are reserved.
- **Template attributes:** parsed from the raw tag source, which also fixed the existing `tl` bug that cut the first letter off attributes after valueless ones.
- **Example:** an "Add item" button (`@click="add_item()"`, a global function), with each item labeled by a template-only `label(item)` function. The README is updated.
