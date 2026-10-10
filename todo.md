# Refactor

- In @props.rs, Deref and DerefMut is implemented manually when it could user Deref and DerefMut derives with #[deref] annotation. Check other places in code that the derive can be used instead (for simple member access without extra logic).

# New Features

## 1. Rework Props registration

Rename `register_props_resource` (for resources) and `register_props` (for components) should be the same method `register_props`. Make so `register_props` receives `T: RegisterableProps` and `RegisterableProps` has a blanket impl for `Resource` and `Component` which implement a method for registering the systems for resource/component props handling. Also, might be better to rename `PropsSource` to `PropsComponent`.

## 2. Add indexmap feature

Add a indexmap feature that implements Props to IndexMap and IndexSet (from indexmap crate).

## 3. Default Props names for derive

Add a #[props(name="counter")] macro attribute on the Props struct. if not set, this should be the struct name but snake case e.g Counter -> counter, PlayerInventory -> player_inventory. PropsResource::<T>::default() should call PropsResource::<T>::at(<T as Props>::name()).

## 4. Support tuple structs for Props

Support tuple structs for Props. If it has a single member, the struct is transparent when converting to Value. If it has two or more, it becomes a List. In template expressions, make sure so value.0 and value[0] are identical semantically.

## 5. Add props definition in component files

In Bevue components, add a top-level props element (along-side template and style) to specify component props. this is specified like so
```
<props>
    <resource name="counter" type="Counter"/>
    <resource name="other" type="path::to::Other"/>
</props>
```
`name` is optional and it uses the default Props::name() if omitted. This will automatically add `ResourceProps::<Counter>::at("counter")` and `ResourceProps::<path::to::Other>::at("other")` to the `Template` entity. It's very likely that `Counter` and `path::to::Other` need to be registered in a type registry (by `register_props_resource`) to map the type path to a type-erased function that inserts the `ResourceProps` component. Make so `Props` derive `type_path` and `short_type_path` methods (that work just like `TypePath` so we don't need to derive it). Also, for ease of use, both the full path `path::to::Other` and the type name `Other` can be used, the latter might cause name collision, but it's okay that the last struct regitered as `Other` should be used.

## 6. Add helper for extracting arguments from BevueCtx

BevueCtx should have a get_args::<T> method to easily unwrap and convert the arg list to T. If T is e.g f32, this is equivalent to `ctx.args.get(0).and_then(|v| v.as_number()).and_then(|v| v as f32)`. T could be a tuple e.g (A, B, C), and it would convert multiple arguments simultaneously and return Result<(A, B, C)>.

## 7. Add component elements

Implement component elements (`ElementType::Component`). They're used in HTML like so
```html
<Foo
    :attr1="expr"
    :attr2="expr"
    @close="handler()"
/>
```
This will instantiate the template element tree of the component named `Foo`. The component names must be registered on the app e.g `app.register_component("Foo", "path/to/foo.html")`. `attr1` and `attr2` are evaluated on the parent scope and injected on `Foo`'s props. Decide whether the root of `Foo` needs the `Template` component, or if it can be handled by the root `Template`. Find a solution that is lightweight to instantiate multiple components.

## 8. Add dynamic styles

Implement a `:style` attribute which can be used to define styles based on props.
```html
<div
    :style="{ backgroundColor: theme.color }"
    :style="{ 'background-color': theme.color }"
></div>

```
Since this is not JavaScript, we can adjust the object syntax to allow for quoted keys to make the fields match exactly the CSS properties. Also, for that to work, Bevue will need to set the `BackgroundColor` component after `bevy-flair` has run so it's guaranteed to overwrite the value defined by CSS.

# Optimization

Measured with `cargo bench --bench reactivity` (`list_insert` case). Inserting one item into a rendered list is linear in the list size when it should be close to constant: at 10 000 items it takes 3.7 ms with a direct `BevueProps` insert and 10.3 ms through a `Props` `Vec` sync.

## 1. Key-aware `Vec` sync

`Vec<T>::sync_value` (`sync_slice` in @src/core/props.rs) diffs by index. Inserting or removing in the middle shifts every later element, so each one is compared against a different element and rewritten field by field. The list itself is not marked `SHAPE` for those positions; instead N/2 elements end up with dirty leaves. The update then walks every one of them, sees their `:key` changed in place and reconciles the list again. This is the 3.7 ms → 10.3 ms difference.

Diff the list as a sequence instead of position by position:
- Trim the common prefix and suffix first. A single insert or remove then becomes one `insert_at` / `remove_at` on the `Value` list, with untouched elements keeping their clean flags. This alone covers push, pop, single insert and single remove.
- For the remaining middle part, match elements by identity instead of position. Options: let `#[derive(Props)]` mark a key field (`#[props(key)]`) and match old and new elements by it, or fall back to the index-by-index diff when no key is declared.
- Moved elements should be moved in the `Value` list (keeping their flags), not rewritten, so the runtime's keyed reconcile sees the same elements in a new order.

Check with `list_insert/sync_*` and `list_insert/update`: after this, `update` should match `update_direct`.

## 2. Cheaper reconcile for small list changes

`TemplateInstance::reconcile` (@src/runtime/instance.rs) always does full work for a `SHAPE` change, even when one element was added:
- It evaluates the `:key` expression for every element and formats each key into a new `String`.
- It builds a `HashMap<String, VecDeque<ItemId>>` of all existing items.
- It rewrites every item's `index`.
- `sync_children` rebuilds the parent's whole child list and calls `replace_children`.

Ideas, roughly in order of payoff:
- Fast path: walk old items and new keys from both ends while they match (common prefix and suffix). Only the unmatched middle needs the map, new spawns and despawns. For a single insert that is one spawned item.
- Insert or remove children in place (`insert_children(index, ..)` / removing one child) when only a contiguous run changed, instead of `replace_children` on the whole list.
- Only update `index` (and fire index bindings) for items after the first changed position.
- Avoid per-element `String` keys: compare key `Value`s directly, or hash them, and reuse the buffers between reconciles.
- Record what changed on the `Value` list itself (e.g. "inserted at i", "removed at i") instead of a single `SHAPE` flag, so the runtime doesn't have to rediscover it by diffing keys.

Check with `list_insert/update_direct`, which should stop growing linearly with the list size.

## 3. Avoid scanning clean elements

A dirty list or dict is scanned child by child to find the dirty ones, both in the change walk and in `Value::clear_changes`. Changing one label among 10 000 costs 23 µs instead of about 1.5 µs. Keeping the indices (or keys) of dirty children on the container would make both proportional to the number of changes.

## 4. Skip the sync when nothing renderable changed

The `Props` sync runs a full diff whenever Bevy reports the resource or component as changed, about 75–90 ns per list item regardless of what changed (0.75 ms at 10 000 items). A system that mutates a `#[props(skip)]` field every frame pays this every frame. Document the pattern (keep per-frame state out of `Props` types), or let users opt into marking changes by hand.
