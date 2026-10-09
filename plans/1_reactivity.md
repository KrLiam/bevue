# Bevue reactivity: Value change tracking, interpolation, v-if / v-for, `Props` derive

## Context

Bevue rendered templates statically. `render_template` (`src/runtime/render.rs`) dropped every `TextFragment::Interpolated`, and the code ignored `ListDirective` and `ConditionDirective`. On hot reload it despawned and rebuilt the whole tree. `Value` (`src/core/value.rs`) was a plain enum wrapper with no change tracking.

Goal: data-driven templates that patch only the UI entities whose inputs changed:
- A `BevueProps { pub value: Value }` component sits on the same entity as `Template`.
- `Value` records **where** it was mutated (fine-grained dirty flags reached through `Deref`/`DerefMut` and typed mutators).
- A per-template **dependency trie** maps prop paths (`menu.actions[3].label`) to the bindings that read them. A change walk visits only the parts that are both dirty and subscribed.
- `#[derive(Props)]` on user Components and Resources does three things: produces a `Value` (struct → `Dict`), gives Reflect-like path access (`a.b[0].c`), and lets Bevy's `Changed<T>` feed a diffing sync into `BevueProps`.

## 1. `Value` with fine-grained change flags — `src/core/value.rs`

```rust
pub struct Value { t: ValueType, flags: ChangeFlags }   // flags: u8 bitset
// SELF = 1 (replaced / scalar changed), SHAPE = 2 (list/dict keys or length changed),
// SUBTREE = 4 (some descendant may be dirty)
```
- `ValueType::Null` is used for `Option::None` and for missing paths (renders as `""`). `Value::NULL` is a constant for it, and `Value::empty_dict()` builds an empty dict.
- `Deref<Target = ValueType>` is read-only and free. `DerefMut` sets `SELF`, because raw access could do anything, so it is the conservative path.
- Precise mutators (preferred):
  - `set(v) -> bool`: compares with `!=` and sets `SELF` only if the value differs.
  - `get_mut(key)` / `index_mut(i)` / `child_mut(segment)` / `at_mut(path)`: set `SUBTREE` on `self` (and on every ancestor walked) and return the child `&mut Value`. They use the inner `t` directly so `self` is not marked `SELF`.
  - `insert`, `remove`, `push`, `pop`, `insert_at`, `remove_at`, `truncate`, `swap`, `clear`: set `SHAPE` on the container. New children start with `SELF`. `insert` on an existing key is a child change (`set`), not a shape change.
  - `entry(key)`: gets a dict entry, inserting `null` if missing. `insert`, `entry` and `push` turn a `null` value into an empty dict or list, and panic on any other type mismatch.
  - `update_at(path, f)`: runs `f` on a descendant and marks ancestors `SUBTREE` only if the descendant ends up dirty. Missing dict keys are inserted as `null`. Used for diffing writes that often change nothing.
  - `sync_from(&other)`: updates the value to equal `other`, diffing dicts and lists so only the parts that differ are marked.
- `PartialEq` is written by hand so it ignores `flags`. `Clone` copies `t` and resets `flags`.
- Readers: `get(key)`, `index(i)`, `child(segment)`, `at(&[PathSegment])`, `flags()`, `is_dirty()`, `is_truthy()` and `to_text()`. `to_text()` prints strings raw, without the quotes `Display` adds, and is used for interpolation.
- `fn clear_changes(&mut self)` descends only into branches with flags set. It still scans every child of a flagged list or dict to find the dirty ones.
- The existing `value!`/`hash_map!` macros and `From` impls are kept; the macros now use `$crate` paths. Added `From<Option<T>>`, `From<&String>`, `From<ValueType>` and `From` for every integer type.

Marking ancestors `SUBTREE` eagerly when navigating with `get_mut` can over-approximate: the caller may only read. That over-approximation affects traversal only and never causes re-rendering, because the change walk (§4) confirms a real `SELF`/`SHAPE` below before firing deep bindings.

### Paths — `src/core/path.rs`
- `enum PathSegment { Key(Cow<'static, str>), Index(usize) }`
- `struct PropPath(pub Vec<PathSegment>)`, with `Deref<Target = [PathSegment]>` so a path can be passed wherever a slice is expected.
- `PropPath::parse("a.b[0].c")`, plus `FromStr`, `Display`, `join` and `push`. `AsPropPath` lets APIs take either a string or a path. It is shared by `Value::at`, the `Props` trait and the compiler.

## 2. Expressions and compilation

**Parser** (`src/parser/expr.rs`, `src/core/template.rs`, `src/parser/mod.rs`):
- `Expression::Index { target, index: Box<Expression> }`, parsed as postfix `[expr]` in the same loop as `.field` in `parse_expr_internal`.
- `ConditionDirective` is `{ kind: If | ElseIf | Else, expr: Option<Expression> }`. Previously `v-else-if` was parsed as `v-if` and `v-else` was dropped.
- `ListDirective` has an `index: Option<Identifier>` (`(item, i) in list`, `in` or `of`) and a `key: Option<Expression>`. `:key` is moved out of the attributes into this field.

**Compile step** (`src/core/compile.rs`):
- `CompiledTemplate::compile` runs at every render; the result is not stored on the asset. The asset loader also compiles each template, so directive and scoping errors are reported at load time.
- Walk the node tree with a scope stack in which each `v-for` pushes `alias → (scope level, iterable)`. Every expression becomes a `CompiledExpr`: a literal, or a path rooted at a `ScopeRef`:
  - `ScopeRef::Props` is the root of `BevueProps`. `ScopeRef::Item(level)` is the element of the enclosing `v-for` at that nesting level. `ScopeRef::Index(level)` is the loop index.
  - A static index (`a[0]`, `a["k"]`) becomes a path segment. A dynamic index (`a[i]`) depends on all of `a` plus the dependencies of `i`.
- `DepMode::Deep` applies to text, classes and `v-if`: fire on any change at or below the path. `DepMode::Shallow` applies to a static-path `v-for` iterable: fire only on `SELF`/`SHAPE`, because element-level changes are routed into the items (§4).
- Sibling `v-if` / `v-else-if` / `v-else` nodes are grouped into one `IfChain` slot.
- Compile errors:
  - `v-else` or `v-else-if` with no preceding `v-if` sibling.
  - `v-if` and `v-for` on the same element (the workaround is to wrap the element), and `v-for` on a `v-else` / `v-else-if` element.
  - `v-if` or `v-for` on the root element.
  - Field access on a literal or a loop index.
- Each element node's child list becomes `Vec<Slot>`, where `Slot = Static(NodeId) | If(IfChain) | For(NodeId)`. `CompiledNode` also holds the compiled text and all `class` / `:class` attributes, merged at render time.

**Evaluation** (`src/runtime/interpolation.rs`):
- `eval` evaluates a `CompiledExpr` against the resolved loop frames and props, returning a borrowed or owned `Value`. A missing path evaluates to `Null`.
- `render_text` renders compiled text; `class_names` evaluates classes like Vue, from a string, a list, or a `{name: bool}` dict.
- `Value::is_truthy` follows JS-like rules: false, 0, NaN, "", and Null are falsy. An empty list or dict is truthy, as in JS.

## 3. Runtime state per rendered template — `src/runtime/instance.rs`

Component `TemplateInstance` on the template root entity, built by `render_template`:
```rust
struct TemplateInstance {
    root: Entity,
    template: Arc<BevueTemplate>,
    compiled: Arc<CompiledTemplate>,
    root_trie: DepTrie,                       // paths from ScopeRef::Props
    root_owned: Owned,
    bindings: Arena<Binding>,                 // generational ids
    blocks: Arena<Block>,                     // IfBlock / ForBlock
    items: Arena<Item>,                       // v-for items
    elements: HashMap<Entity, Vec<SlotState>>, // per parent element: entities per slot, to rebuild child order
}
enum BindingKind { Text, Class, If(BlockId), For(BlockId) }
struct DepTrie { bindings: Vec<(BindingId, DepMode)>, children: HashMap<PathSegment, DepTrie>,
                 for_blocks: Vec<BlockId> /* element-level routing into list items */ }
struct ForBlock { parent, node, scope: Vec<ItemId>, items: Vec<ItemId> /* in element order */, keyed, binding, route }
struct Item { block, index, key: String, entity, trie: DepTrie, index_bindings: Vec<BindingId>, owned: Owned }
struct Owned { bindings, blocks, elements }  // what a v-if branch or list item must tear down
```
- **Initial render**: `render_template` takes `BevueProps` from the root (an empty dict if absent), compiles the template and calls `TemplateInstance::render`. Rendering evaluates text and classes, spawns the active `v-if` branch and one subtree per `v-for` element, and registers every binding.
  - A binding in an `Item(level)` scope is registered in that item's own trie, relative to the element.
  - `spawn_element` is shared by the initial render and by updates that spawn new branches or items.
  - Static and dynamic classes are applied to `ClassList` when nodes are spawned.
- **Child ordering**: a parent's Bevy `Children` is the concatenation of its slots' entity lists. After an `If` or `For` block changes, only that parent's order is rebuilt, with `replace_children`.
- **Teardown**: when a branch or item is destroyed, its top entity is despawned recursively. Its bindings and child blocks are freed, and their trie registrations are removed eagerly, so toggling a `v-if` never leaks bindings.
- **Hot reload** keeps its previous behavior: drop the `TemplateInstance` and do a full re-render.

## 4. Change propagation — `update_templates` system

Exclusive system in `PostUpdate`. The `BevueSystems` sets run in this order: `SyncProps`, `Render`, `Update`, `Style`.
1. Query `(Entity, Ref<TemplateInstance>)` with `Changed<BevueProps>`. Bevy's change detection is the gate: untouched templates cost nothing. Instances added since the last run are skipped, because they were rendered from the current props.
2. Take the `TemplateInstance` and the props value out of the entity with `mem::take`. If the props have no change flags at all (replaced wholesale, or `BevueProps` added after the render), every binding is re-evaluated. Otherwise walk the `Value` and the `DepTrie` together, following only branches whose value is dirty. The walk returns `real_change: bool`:
   - `SELF` at a node fires every binding in that trie subtree and marks its lists for a full refresh.
   - `SHAPE` fires the bindings at the node. A missing child that still has a trie entry fires that subtree, and so do index children of a list, since their elements may have shifted.
   - `SUBTREE` recurses into dirty children. For a list node with `for_blocks`, each dirty element `i` is queued to be walked against `block.items[i].trie`. Deep bindings at this node fire only if some descendant had a real change.
3. Structural work runs first, in a loop:
   - Fired `For` bindings **reconcile** their list: a keyed diff (index as key if there is no `:key`). It reuses items by key, spawns new ones fully evaluated and marked fresh, despawns removed ones, reorders, and fires the index bindings of items whose index changed.
   - Queued element walks then run with the mapping after reconcile, skipping fresh items. If a keyed element's key changed in place, the list is reconciled again so entity identity keeps following keys.
   - Reused items are fully re-evaluated when the list was replaced wholesale, or when it has no `:key`.
4. Then fired `If` bindings swap branches, and fired text and class bindings are re-evaluated:
   - Text: written only if it differs.
   - `:class`: `ClassList` replaced only if it differs.
5. Clear the props' change flags and put the props and the `TemplateInstance` back without triggering change detection.

Cost is O(dirty ∩ subscribed paths) plus the work of the fired bindings. Unrelated prop changes are only visited in the flag-clearing pass.

`apply_template_classes_and_styles` now only attaches the root's stylesheet (`Styled`), since classes are applied at spawn and kept up to date by bindings.

## 5. `Props` trait and `#[derive(Props)]`

**Trait** in `src/core/props.rs`, object-safe so deep paths can be walked as `dyn`:
```rust
pub trait Props: Send + Sync + 'static {
    fn to_value(&self) -> Value;
    fn sync_value(&self, target: &mut Value) { target.set(self.to_value()); } // diffing write
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError>;
    fn from_value(value: &Value) -> Result<Self, PropsError> where Self: Sized;
    fn kind(&self) -> PropsKind { PropsKind::Leaf }                // for error messages
    fn field(&self, name: &str) -> Option<&dyn Props> { None }
    fn field_mut(&mut self, name: &str) -> Option<&mut dyn Props> { None }
    fn element(&self, index: usize) -> Option<&dyn Props> { None }
    fn element_mut(&mut self, index: usize) -> Option<&mut dyn Props> { None }
    fn element_count(&self) -> usize { 0 }                          // for error messages
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
pub trait PropsExt: Props {   // blanket impl, delegating to inherent methods on `dyn Props`
    fn path(&self, path: impl AsPropPath) -> Result<&dyn Props, PathError>;
    fn path_mut(&mut self, path: impl AsPropPath) -> Result<&mut dyn Props, PathError>;
    fn get_path<T: Props>(&self, path: impl AsPropPath) -> Result<&T, PathError>;          // downcast via as_any
    fn get_path_mut<T: Props>(&mut self, path: impl AsPropPath) -> Result<&mut T, PathError>;
    fn set_path(&mut self, path: impl AsPropPath, value: impl Into<Value>) -> Result<(), PathError>; // apply_value at leaf
}
```
- Path errors name where they failed. Examples: "'owner' is a Leaf and cannot be accessed with 'x'", "index 5 is out of bounds for 'slots' of length 2", "cannot set 'slots[0][0]': expected number, found \"x\"". For `HashMap<String, T>`, `field()` does a key lookup, so `a.map.key` works.
- Built-in impls: `bool`, all ints and floats, `String`, `&'static str` (read-only), `Option<T>`, `Vec<T>`, `[T; N]`, `HashMap<String, T>` (bevy and std) and `Value` itself.
  - `Option<T>` forwards member access to its inner value.
  - There is no impl for `Entity`, because its 64-bit id doesn't fit exactly in a `Number` (f64).
- `Vec<T>::sync_value`: sync the common prefix per index, then push or truncate the rest (`SHAPE`). A scalar uses `target.set(...)`, which is compare-then-mark. Helpers `sync_field` / `sync_element` use `Value::update_at`, so unchanged fields mark nothing.

**Derive**: proc-macro crate `crates/bevue_macros` (syn, quote, proc-macro2). The root is a workspace and re-exports it as `bevue::Props`.
- Named struct: dict keyed by field name. `field`/`field_mut` match on names, `sync_value` recurses per field, `apply_value` applies the fields present in the dict, and `from_value` requires every non-skipped field.
- Tuple struct: list. Unit struct: empty dict. Unit-only enum: the variant name as a string. Other enums: compile error for now.
- Attributes `#[props(skip)]` (the field must implement `Default`) and `#[props(rename = "x")]`, also on enum variants.
- Generic type parameters get a `Props` bound. The macro refers to `::bevue::...` paths; `lib.rs` has `extern crate self as bevue;` so the derive also works inside the crate.

**Bevy wiring** (`src/runtime/props.rs`, `BevueSystems::SyncProps`):
- `BevueProps { pub value: Value }`, a `Component` with `Deref`/`DerefMut` to `Value`. It defaults to an empty dict.
- Component source: `app.register_props::<T: Component + Props>()` adds `sync_component_props::<T>`.
  - It iterates `(Entity, Ref<PropsSource<T>>, &mut BevueProps)` and syncs when `T` or the `PropsSource` changed.
  - `PropsSource::<T>::root()` makes `T` the props root; `PropsSource::<T>::at("inventory")` mounts it at that key.
  - `T` is read from the same entity by default, or from another one with `.from_entity(entity)`.
  - `PropsSource` requires `BevueProps`, so the props component is added automatically.
- Resource source: `app.register_props_resource::<R: Resource + Props>()` adds a system. When `R` or a `PropsResource<R>` changed, it syncs `R` into every `(PropsResource<R>, BevueProps)`. This matches `<param name="menu" source="resource">` in `assets/context_menu.html`; the `type=` string there stays documentation only, and binding is set up in ECS.
- `sync_props` diffs with change detection bypassed and marks `BevueProps` changed only if something differs.
- Flow: user mutates `ResMut<Menu>` → Bevy `Changed` → `sync_value` diffs and only touches changed leaves → `BevueProps` changes → the §4 walk updates only the entities bound to those leaves.
- The reverse direction, `set_path` (`v-model` and event handlers writing back), is available on the trait for a later step.

## Implementation order (each step lands with tests)
1. `Value` flags, mutators, `clear_changes`, `PropPath`, plus tests in `value.rs`/`path.rs` that check flags after each mutator and that `PartialEq` ignores flags.
2. `Expression::Index`, new directive shapes, `:key` and index alias, parser tests.
3. `compile.rs` (scopes, deps, slots, if-chains) and `interpolation.rs` eval.
4. `BevueProps`, `TemplateInstance`, slot-based `render_template` with interpolation, then `update_templates` for Text and `:class`.
5. `v-if` chains with branch swap and child reordering.
6. Keyed `v-for` reconcile, item tries, nested loops.
7. `Props` trait, built-in impls, `PropsExt` path API.
8. `bevue_macros` derive, the `register_props` / `register_props_resource` sync systems, and a README update.

## Critical files
- Modified: `src/core/value.rs`, `src/core/template.rs`, `src/parser/expr.rs`, `src/parser/mod.rs`, `src/asset/mod.rs` (compile at load), `src/runtime/render.rs`, `src/runtime/interpolation.rs`, `src/runtime/mod.rs` (plugin, system sets), `src/lib.rs`, `Cargo.toml` (workspace).
- New: `src/core/path.rs`, `src/core/compile.rs`, `src/core/props.rs`, `src/runtime/instance.rs`, `src/runtime/props.rs`, `src/runtime/tests.rs`, `crates/bevue_macros/`.
- Reused: the `TemplateRef` component, `Template`/`WaitingForTemplate` asset watching (`watch_template_components`), `run_system_cached_with` dispatch, the parser's `try_parse_*` functions, and the existing test harness pattern (`MinimalPlugins + AssetPlugin + BevueRuntimePlugin`).

## Verification
- `cargo test` with unit tests per step. The runtime tests in `src/runtime/tests.rs` use the App harness:
  - Interpolation: render `{{user.name}}` → `props.at_mut("user.name").set("Bo")` → `app.update()` → the Text changed. A sibling bound to `user.age` is **not** changed, checked through `Text` change ticks.
  - `v-if`/`v-else`: toggling a bool swaps the branch entity and preserves sibling order; toggling repeatedly does not leak bindings.
  - `v-for`: push, remove-middle, swap and clear with `:key`. Reused entities keep their `Entity` ids, and editing `items[1].label` changes only that item's Text. Also covered: nested loops with indices, unkeyed lists, keys changed in place, lists inside `v-if` and `v-if` inside lists.
  - Clearing: after an update, `props.value.is_dirty() == false`, and an update without prop changes touches nothing.
  - Derive: `get_path::<u32>("slots[0][1].count")`, `set_path`, error cases. `sync_value` on a struct whose single nested field changed marks only that leaf; syncing equal data marks nothing.
  - Sources: a resource change updates only the bound text; touching a resource without changing its data doesn't mark the props; component sources work as the root and from another entity.
- Manual: `examples/simple.rs` has a `#[derive(Resource, Props)]` counter mounted via `PropsResource`, interpolated in `assets/simple.html` and incremented by a timer system. Run `cargo run --example simple` and confirm the text updates and hot reload still works.

## Results

Reactivity, interpolation, `v-if` / `v-for` and `#[derive(Props)]` all work. All 62 tests pass (up from 23) and clippy is clean. The example builds and ran for about 20 seconds with no errors in the log; the rendered UI wasn't checked visually.

What's in place:
- Change tracking (`src/core/value.rs`, `src/core/path.rs`): `Value` records whether a node itself changed, whether a list or dict changed length or order, or whether something below it changed. Mutators mark only what they touch, `set` with an equal value marks nothing, and equality ignores the flags.
- Parser: `a[0]` and `rows[i]` indexing, real `v-else-if` / `v-else`, `(item, i) in list`, and `:key` moved onto the list directive.
- Compile step (`src/core/compile.rs`): resolves names inside loops, groups `v-if` chains, and catches mistakes such as a stray `v-else`. Broken templates fail at load time with an error.
- Runtime (`src/runtime/instance.rs`, `update_templates` in `render.rs`): updates only run for templates whose `BevueProps` changed this frame, and only visit paths that are both changed and bound. Keyed lists keep each item's entity across push, remove and reorder, and removing the first row doesn't redo the rows after it.
- `Props` (`src/core/props.rs`, `crates/bevue_macros`): the derive, deep `get_path` / `set_path` with clear errors, and diffing syncs that only mark the leaves that differ.
- Bevy wiring: `PropsSource<T>` / `PropsResource<R>` with `register_props` / `register_props_resource`. Changing `menu.actions[1].label` updates only that one text entity.
- The example and `assets/simple.html` show a ticking counter, an even/odd `v-if` and a growing list. The README is updated.

Things to know:
- Only `class` is bound dynamically. Other `:attr` bindings are parsed but not applied, because attributes don't map to any Bevy component yet.
- Assigning `props.value = ...` directly carries no change flags. It still works, but every binding in the template is re-evaluated.
- An update scans all children of a list or dict that has changed, so one edit in a 10k-element list costs a quick pass over 10k elements, but only that one binding re-runs.
- Lists without `:key` re-evaluate every item when their length or order changes, as in Vue.
