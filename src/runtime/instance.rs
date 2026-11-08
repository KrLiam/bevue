//! Runtime state of a rendered template: bindings between prop paths and UI entities, and the
//! `v-if`/`v-for` blocks that own dynamically spawned subtrees.
//!
//! Every binding (an interpolated text, a class list, an if-chain or a list) registers the prop
//! paths it reads in a [`DepTrie`]. When props change, [`TemplateInstance::update`] walks the
//! dirty parts of the [`Value`] together with the trie, so only bindings whose paths changed are
//! re-evaluated.
//!
//! Bindings inside a `v-for` that read the loop element are registered in that item's own trie,
//! relative to the element. Element changes are routed to the item at that index, so reordering a
//! list never re-registers the bindings of the items that moved.
//!
//! Everything about a node that does not depend on props (static texts, classes and styles, the
//! paths each binding reads) is computed once when the template is compiled.

use std::sync::Arc;

use bevy::{
    ecs::{
        component::Component,
        entity::{Entity, EntityHashMap},
        hierarchy::{ChildOf, Children},
        world::{EntityWorldMut, World},
    },
    platform::collections::{HashMap, HashSet},
    ui::{Interaction, Node, widget::Text},
};
use bevy_flair::prelude::{ClassList, InlineStyle};

use crate::{
    core::{
        compile::{CompileError, CompiledHandler, CompiledTemplate, Dep, ScopeRef, Slot},
        path::PathSegment,
        template::{BevueTemplate, NodeId, TemplateRef, TemplateStyle},
        value::{ChangeFlags, Value, ValueType},
    },
    runtime::{
        functions::call_function,
        interpolation::{class_names, eval, inline_style, resolve, write_text, EvalCtx, Frame},
    },
};

static NULL: Value = Value::NULL;

/// A function caller for expressions evaluated on `entity` of the template rooted at `root`.
fn caller(world: &mut World, root: Entity, entity: Entity) -> impl FnMut(&str, Vec<Value>) -> Value + '_ {
    move |name: &str, args: Vec<Value>| call_function(world, root, entity, name, args)
}

// Arena

/// A generational index into an [`Arena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArenaId {
    index: u32,
    generation: u32,
}

struct Arena<T> {
    entries: Vec<(u32, Option<T>)>,
    free: Vec<u32>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { entries: Vec::new(), free: Vec::new() }
    }
}

impl<T> Arena<T> {
    fn insert(&mut self, value: T) -> ArenaId {
        if let Some(index) = self.free.pop() {
            let entry = &mut self.entries[index as usize];
            entry.0 += 1;
            entry.1 = Some(value);
            return ArenaId { index, generation: entry.0 };
        }
        self.entries.push((0, Some(value)));
        ArenaId { index: self.entries.len() as u32 - 1, generation: 0 }
    }

    fn get(&self, id: ArenaId) -> Option<&T> {
        match self.entries.get(id.index as usize) {
            Some((generation, value)) if *generation == id.generation => value.as_ref(),
            _ => None,
        }
    }

    fn get_mut(&mut self, id: ArenaId) -> Option<&mut T> {
        match self.entries.get_mut(id.index as usize) {
            Some((generation, value)) if *generation == id.generation => value.as_mut(),
            _ => None,
        }
    }

    fn remove(&mut self, id: ArenaId) -> Option<T> {
        let (generation, value) = self.entries.get_mut(id.index as usize)?;
        if *generation != id.generation {
            return None;
        }
        let removed = value.take()?;
        self.free.push(id.index);
        Some(removed)
    }

    fn len(&self) -> usize {
        self.entries.len() - self.free.len()
    }
}

pub type BindingId = ArenaId;
pub type BlockId = ArenaId;
pub type ItemId = ArenaId;

// Dependency trie

/// When a binding registered at a path fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepMode {
    /// On any change at or below the path.
    Deep,
    /// Only when the value at the path is replaced or its elements are added, removed or
    /// reordered.
    Shallow,
}

/// Bindings registered by prop path.
#[derive(Debug, Default)]
pub struct DepTrie {
    bindings: Vec<(BindingId, DepMode)>,
    /// Children by path segment. Tries are small, so a list is faster than a map and saves an
    /// allocation per list item.
    children: Vec<(PathSegment, DepTrie)>,
    /// `v-for` blocks iterating the list at this path; element changes are routed to their items.
    for_blocks: Vec<BlockId>,
}

impl DepTrie {
    fn node_mut(&mut self, path: &[PathSegment]) -> &mut DepTrie {
        let mut node = self;
        for segment in path {
            let index = match node.children.iter().position(|(s, _)| s == segment) {
                Some(index) => index,
                None => {
                    node.children.push((segment.clone(), DepTrie::default()));
                    node.children.len() - 1
                }
            };
            node = &mut node.children[index].1;
        }
        node
    }

    fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.children.is_empty() && self.for_blocks.is_empty()
    }

    /// Removes `binding` (or `block`) registered at `path`, pruning nodes left empty.
    fn remove(&mut self, path: &[PathSegment], binding: Option<BindingId>, block: Option<BlockId>) {
        match path.split_first() {
            None => {
                if let Some(binding) = binding {
                    self.bindings.retain(|(id, _)| *id != binding);
                }
                if let Some(block) = block {
                    self.for_blocks.retain(|id| *id != block);
                }
            }
            Some((segment, rest)) => {
                let Some(index) = self.children.iter().position(|(s, _)| s == segment) else { return };
                let child = &mut self.children[index].1;
                child.remove(rest, binding, block);
                if child.is_empty() {
                    self.children.swap_remove(index);
                }
            }
        }
    }

    /// Returns the number of bindings registered in this trie.
    pub fn binding_count(&self) -> usize {
        self.bindings.len() + self.children.iter().map(|(_, child)| child.binding_count()).sum::<usize>()
    }
}

/// Where a binding's dependency is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrieOwner {
    /// The template's root trie, for paths into props.
    Root,
    /// A list item's trie, for paths relative to the item's element.
    Item(ItemId),
    /// A list item's index bindings.
    ItemIndex(ItemId),
}

// Bindings and blocks

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Text,
    Class,
    Style,
    If(BlockId),
    For(BlockId),
}

struct Binding {
    entity: Entity,
    node: NodeId,
    kind: BindingKind,
    /// The innermost `v-for` item enclosing the binding.
    scope: Option<ItemId>,
    registrations: Vec<(TrieOwner, Arc<[PathSegment]>)>,
}

/// Bookkeeping owned by a subtree that can be torn down: a `v-if` branch or a list item.
#[derive(Default)]
struct Owned {
    bindings: Vec<BindingId>,
    blocks: Vec<BlockId>,
    /// Elements with an entry in `elements` or `listeners`.
    elements: Vec<Entity>,
}

#[derive(Clone, Copy)]
enum OwnerRef {
    Root,
    Branch(BlockId),
    Item(ItemId),
}

struct IfBlock {
    parent: Entity,
    /// The node whose slot `slot` is the chain.
    node: NodeId,
    slot: usize,
    scope: Option<ItemId>,
    active: Option<usize>,
    entity: Option<Entity>,
    owned: Owned,
    binding: Option<BindingId>,
}

struct ForBlock {
    parent: Entity,
    node: NodeId,
    scope: Option<ItemId>,
    items: Vec<ItemId>,
    binding: Option<BindingId>,
    /// Where element changes are routed from, if the iterable is a static path.
    route: Option<(TrieOwner, Arc<[PathSegment]>)>,
    /// The elements of an iterable that is not a prop path (e.g. `sorted(items)`), as computed
    /// by the last reconcile.
    computed: Option<Arc<[Value]>>,
}

enum Block {
    If(IfBlock),
    For(ForBlock),
}

/// What identifies a list item across updates: its `:key`, or its position without one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ItemKey {
    Position(usize),
    Null,
    Bool(bool),
    Number(u64),
    String(Box<str>),
    Entity(Entity),
    Other(String),
}

/// The bits of a number key, with `-0` and `0` the same key.
fn number_key(n: f64) -> u64 {
    if n == 0.0 { 0 } else { n.to_bits() }
}

impl ItemKey {
    fn of(value: &Value) -> Self {
        match value.value_type() {
            ValueType::Null => ItemKey::Null,
            ValueType::Boolean(b) => ItemKey::Bool(*b),
            ValueType::Number(n) => ItemKey::Number(number_key(*n)),
            ValueType::String(s) => ItemKey::String(s.as_str().into()),
            ValueType::Entity(e) => ItemKey::Entity(*e),
            ValueType::List(_) | ValueType::Dict(_) => ItemKey::Other(value.to_string()),
        }
    }

    /// Returns true if `value` is this key, without allocating for scalars.
    fn matches(&self, value: &Value) -> bool {
        match (self, value.value_type()) {
            (ItemKey::Null, ValueType::Null) => true,
            (ItemKey::Bool(a), ValueType::Boolean(b)) => a == b,
            (ItemKey::Number(a), ValueType::Number(b)) => *a == number_key(*b),
            (ItemKey::String(a), ValueType::String(b)) => **a == **b,
            (ItemKey::Entity(a), ValueType::Entity(b)) => a == b,
            (ItemKey::Other(a), ValueType::List(_) | ValueType::Dict(_)) => *a == value.to_string(),
            _ => false,
        }
    }
}

struct Item {
    block: BlockId,
    index: usize,
    key: ItemKey,
    entity: Entity,
    trie: DepTrie,
    index_bindings: Vec<BindingId>,
    owned: Owned,
    /// The item enclosing this item's list.
    parent: Option<ItemId>,
    /// The nesting level of the item's list.
    level: usize,
}

#[derive(Clone, Copy)]
enum SlotState {
    Static(Entity),
    Block(BlockId),
}

/// Work collected by a change walk.
#[derive(Default)]
struct Pending {
    fired: Vec<BindingId>,
    /// Dirty list elements to walk with the item at that index: `(block, element index)`.
    routed: Vec<(BlockId, usize)>,
    /// Lists replaced wholesale: all of their items must be re-evaluated.
    force_full: HashSet<BlockId>,
    /// Items spawned during this update, already up to date. Only tracked during updates.
    fresh: HashSet<ItemId>,
    track_fresh: bool,
}

/// An event handler to run: the function to call with its evaluated arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedHandler {
    pub function: String,
    pub args: Vec<Value>,
    /// Where `$emit` in this handler triggers its event: the root element of the component the
    /// handler was written in, or the template root.
    pub emit_target: Entity,
}

/// Counters describing a template instance, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceStats {
    pub bindings: usize,
    pub blocks: usize,
    pub items: usize,
}

/// What a [`TemplateInstance::update`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateStats {
    /// Text, class, style and `v-if` bindings that were re-evaluated.
    pub fired: usize,
    /// Lists that were reconciled against their elements.
    pub reconciled: usize,
    /// Whether every binding was re-evaluated because props carried no change flags.
    pub full: bool,
}

/// The runtime state of a rendered template, stored on its root entity.
#[derive(Component)]
pub struct TemplateInstance {
    root: Entity,
    compiled: Arc<CompiledTemplate>,
    root_trie: DepTrie,
    root_owned: Owned,
    bindings: Arena<Binding>,
    blocks: Arena<Block>,
    items: Arena<Item>,
    /// The slots of elements whose children can be reordered: those with an if-chain or a list.
    elements: EntityHashMap<Vec<SlotState>>,
    /// Nodes with event listeners: their template node and innermost `v-for` item.
    listeners: EntityHashMap<(NodeId, Option<ItemId>)>,
    /// Reused to render texts during updates.
    text_buffer: String,
}

impl Default for TemplateInstance {
    fn default() -> Self {
        Self {
            root: Entity::PLACEHOLDER,
            compiled: Arc::default(),
            root_trie: DepTrie::default(),
            root_owned: Owned::default(),
            bindings: Arena::default(),
            blocks: Arena::default(),
            items: Arena::default(),
            elements: EntityHashMap::default(),
            listeners: EntityHashMap::default(),
            text_buffer: String::new(),
        }
    }
}

/// Sets `style` on `entity` if it differs from its current one. An emptied style still replaces
/// the previous one, or flair keeps applying it.
fn write_style(entity: &mut EntityWorldMut, style: InlineStyle) {
    let current = entity.get::<InlineStyle>();
    if current != Some(&style) && (current.is_some() || style != InlineStyle::default()) {
        entity.insert(style);
    }
}

impl TemplateInstance {
    /// Compiles `template` and spawns its UI tree under `root`, evaluated against `props`.
    pub fn render(world: &mut World, root: Entity, template: &BevueTemplate, props: &Value) -> Result<Self, CompileError> {
        let compiled = Arc::new(CompiledTemplate::compile(template)?);
        Ok(Self::render_compiled(world, root, compiled, props))
    }

    /// Spawns the UI tree of an already compiled template under `root`, evaluated against
    /// `props`.
    pub fn render_compiled(world: &mut World, root: Entity, compiled: Arc<CompiledTemplate>, props: &Value) -> Self {
        let mut instance = Self { root, compiled, ..Default::default() };
        if let Some(root_id) = instance.compiled.root {
            let mut pending = Pending::default();
            instance.spawn_element(world, props, &mut pending, root_id, Some(root), None, &[], None, OwnerRef::Root);
        }
        instance
    }

    /// Returns true if any node of the template has a `style` or `:style` attribute.
    pub fn has_inline_styles(&self) -> bool {
        self.compiled.has_inline_styles
    }

    /// The styles of the template and of the components it uses.
    pub fn styles(&self) -> &[TemplateStyle] {
        &self.compiled.styles
    }

    /// The compiled template this instance renders.
    pub fn compiled(&self) -> &Arc<CompiledTemplate> {
        &self.compiled
    }

    pub fn stats(&self) -> InstanceStats {
        InstanceStats { bindings: self.bindings.len(), blocks: self.blocks.len(), items: self.items.len() }
    }

    /// Applies the changes recorded in `props` to the UI. With `full`, every binding is
    /// re-evaluated, which is used when props were replaced without change flags.
    pub fn update(&mut self, world: &mut World, props: &Value, full: bool) -> UpdateStats {
        let mut stats = UpdateStats { full, ..Default::default() };
        let mut pending = Pending { track_fresh: true, ..Default::default() };
        if full {
            fire_all(&self.root_trie, &mut pending);
        } else {
            walk(&self.root_trie, props, false, &mut pending);
        }

        // Structural work first: reconciling lists changes which item each element index maps
        // to, so routed element walks must run afterwards. Walks can fire more lists.
        loop {
            let mut fors = Vec::new();
            pending.fired.retain(|id| match self.bindings.get(*id).map(|b| b.kind) {
                Some(BindingKind::For(block)) => {
                    if !fors.contains(&block) {
                        fors.push(block);
                    }
                    false
                }
                _ => true,
            });
            let routed = std::mem::take(&mut pending.routed);
            if fors.is_empty() && routed.is_empty() {
                break;
            }

            stats.reconciled += fors.len();
            for block in fors {
                let force = pending.force_full.contains(&block);
                self.reconcile(world, props, &mut pending, block, force, None);
            }
            for (block_id, index) in routed {
                self.walk_item(world, props, &mut pending, block_id, index);
            }
        }

        let mut seen = HashSet::new();
        let fired: Vec<_> = pending.fired.drain(..).filter(|id| seen.insert(*id)).collect();
        for &id in &fired {
            if let Some(BindingKind::If(block)) = self.bindings.get(id).map(|b| b.kind) {
                self.update_if(world, props, &mut pending, block, None);
            }
        }
        for &id in &fired {
            self.apply(world, props, id);
        }
        stats.fired = fired.len();
        stats
    }

    /// Walks the element at `index` of a list against the item rendering it.
    fn walk_item(&mut self, world: &mut World, props: &Value, pending: &mut Pending, block_id: BlockId, index: usize) {
        if pending.force_full.contains(&block_id) {
            return;
        }
        let Some(Block::For(block)) = self.blocks.get(block_id) else { return };
        let Some(&item_id) = block.items.get(index) else { return };
        if pending.fresh.contains(&item_id) {
            return;
        }
        let compiled = self.compiled.clone();
        let list = compiled.get(block.node).list.as_ref().unwrap();
        let parent = block.parent;
        let block_binding = block.binding;
        let mut frames = self.frames(world, block.scope, props);
        let mut call = caller(world, self.root, parent);
        let Some(list_value) = resolve(&list.iterable, &mut EvalCtx::new(&frames, props, &mut call)) else { return };
        let Some(element) = list_value.index(index) else { return };

        let item = self.items.get(item_id).unwrap();
        walk(&item.trie, element, false, pending);

        // If the element's key changed in place, the list must be reconciled again so entity
        // identity keeps following keys.
        if let Some(key_expr) = &list.key {
            frames.push(Frame::new(element, index));
            let key = eval(key_expr, &mut EvalCtx::new(&frames, props, &mut call));
            if !item.key.matches(&key)
                && let Some(binding) = block_binding
            {
                pending.fired.push(binding);
            }
        }
    }

    // Spawning

    /// Spawns the element for `node_id` (or reuses `entity`) with its subtree. `frames` are the
    /// frames of the `v-for` items in `scope`.
    #[allow(clippy::too_many_arguments)]
    fn spawn_element<'p>(
        &mut self,
        world: &mut World,
        props: &'p Value,
        pending: &mut Pending,
        node_id: NodeId,
        entity: Option<Entity>,
        parent: Option<Entity>,
        frames: &[Frame<'p>],
        scope: Option<ItemId>,
        owner: OwnerRef,
    ) -> Entity {
        let compiled = self.compiled.clone();
        let node = compiled.get(node_id);
        let template_ref = TemplateRef::new(self.root, node_id);

        // A new child whose expressions call no function is evaluated before it exists, and spawned
        // with all of its components at once. Functions are given the entity, so otherwise it is
        // spawned first.
        let deferred = entity.is_none() && parent.is_some() && !node.calls_functions;
        let existing = (!deferred).then(|| {
            let entity = entity.unwrap_or_else(|| world.spawn_empty().id());
            let mut entity_mut = world.entity_mut(entity);
            entity_mut.insert(template_ref);
            if !entity_mut.contains::<Node>() {
                entity_mut.insert(Node::default());
            }
            if let Some(parent) = parent {
                entity_mut.insert(ChildOf(parent));
            }
            entity
        });
        let call_entity = existing.unwrap_or(Entity::PLACEHOLDER);

        let text = if node.dynamic_text {
            let mut text = String::new();
            let mut call = caller(world, self.root, call_entity);
            write_text(node.text.as_ref().unwrap(), &mut EvalCtx::new(frames, props, &mut call), &mut text);
            Some(text)
        } else {
            node.static_text.clone()
        };
        let classes = if node.dynamic_classes {
            let mut call = caller(world, self.root, call_entity);
            Some(ClassList::new_with_classes(class_names(&node.classes, &mut EvalCtx::new(frames, props, &mut call))))
        } else {
            node.static_classes.clone()
        };
        let style = if node.dynamic_styles {
            let mut call = caller(world, self.root, call_entity);
            Some(inline_style(&node.styles, &mut EvalCtx::new(frames, props, &mut call)))
        } else {
            node.static_style.clone()
        };

        let mut entity_mut = match existing {
            Some(entity) => {
                let mut entity_mut = world.entity_mut(entity);
                match (text, classes) {
                    (Some(text), Some(classes)) => {
                        entity_mut.insert((Text::new(text), classes));
                    }
                    (Some(text), None) => {
                        entity_mut.insert(Text::new(text));
                    }
                    (None, classes) => {
                        if parent.is_none() {
                            entity_mut.remove::<Text>();
                        }
                        if let Some(classes) = classes {
                            entity_mut.insert(classes);
                        }
                    }
                }
                entity_mut
            }
            None => {
                let base = (template_ref, Node::default(), ChildOf(parent.unwrap()));
                match (text, classes) {
                    (Some(text), Some(classes)) => world.spawn((base, Text::new(text), classes)),
                    (Some(text), None) => world.spawn((base, Text::new(text))),
                    (None, Some(classes)) => world.spawn((base, classes)),
                    (None, None) => world.spawn(base),
                }
            }
        };
        if let Some(style) = style {
            write_style(&mut entity_mut, style);
        }
        // `Node` requires `FocusPolicy::Pass`, so every listening node under the cursor gets its own
        // `Interaction`, like DOM events reaching all ancestors.
        if node.interactive && !entity_mut.contains::<Interaction>() {
            entity_mut.insert(Interaction::default());
        }
        let entity = entity_mut.id();

        if node.dynamic_text {
            self.add_binding(entity, node_id, BindingKind::Text, scope, owner, &node.text_deps, DepMode::Deep);
        }
        if node.dynamic_classes {
            self.add_binding(entity, node_id, BindingKind::Class, scope, owner, &node.class_deps, DepMode::Deep);
        }
        if node.dynamic_styles {
            self.add_binding(entity, node_id, BindingKind::Style, scope, owner, &node.style_deps, DepMode::Deep);
        }
        if !node.listeners.is_empty() {
            self.listeners.insert(entity, (node_id, scope));
        }
        if parent.is_some() && (node.has_blocks || !node.listeners.is_empty()) {
            self.owned_mut(owner).elements.push(entity);
        }

        let mut slots = Vec::new();
        if node.has_blocks {
            slots.reserve_exact(node.slots.len());
        }
        for (slot_index, slot) in node.slots.iter().enumerate() {
            match slot {
                Slot::Static(child) => {
                    let child = self.spawn_element(world, props, pending, *child, None, Some(entity), frames, scope, owner);
                    if node.has_blocks {
                        slots.push(SlotState::Static(child));
                    }
                }
                Slot::If(chain) => {
                    let block = self.blocks.insert(Block::If(IfBlock {
                        parent: entity,
                        node: node_id,
                        slot: slot_index,
                        scope,
                        active: None,
                        entity: None,
                        owned: Owned::default(),
                        binding: None,
                    }));
                    self.owned_mut(owner).blocks.push(block);
                    let binding = self.add_binding(entity, node_id, BindingKind::If(block), scope, owner, &chain.1, DepMode::Deep);
                    if let Some(Block::If(b)) = self.blocks.get_mut(block) {
                        b.binding = Some(binding);
                    }
                    slots.push(SlotState::Block(block));
                    self.update_if(world, props, pending, block, Some(frames));
                }
                Slot::For(child) => {
                    let list = compiled.get(*child).list.as_ref().unwrap();
                    let block = self.blocks.insert(Block::For(ForBlock {
                        parent: entity,
                        node: *child,
                        scope,
                        items: Vec::new(),
                        binding: None,
                        route: None,
                        computed: None,
                    }));
                    self.owned_mut(owner).blocks.push(block);

                    // A static iterable path routes element changes into items; a dynamic one
                    // (e.g. `groups[g]`) re-reconciles on any change below its static prefix.
                    let mode = if list.route.is_some() { DepMode::Shallow } else { DepMode::Deep };
                    let binding = self.add_binding(entity, *child, BindingKind::For(block), scope, owner, &list.deps, mode);
                    let route = list.route.as_ref().and_then(|dep| {
                        let owner = self.trie_owner(dep.root, scope)?;
                        self.trie_mut(owner)?.node_mut(&dep.path).for_blocks.push(block);
                        Some((owner, dep.path.clone()))
                    });
                    if let Some(Block::For(b)) = self.blocks.get_mut(block) {
                        b.binding = Some(binding);
                        b.route = route;
                    }
                    slots.push(SlotState::Block(block));
                    self.reconcile(world, props, pending, block, false, Some(frames));
                }
            }
        }

        // Children are spawned in slot order, so they start in the right order.
        if node.has_blocks {
            self.elements.insert(entity, slots);
        }
        entity
    }

    /// Re-evaluates an if-chain and swaps the rendered branch if it changed. `frames` are given
    /// while the chain's parent is being spawned, which also places the branch in order.
    fn update_if<'p>(
        &mut self,
        world: &mut World,
        props: &'p Value,
        pending: &mut Pending,
        block_id: BlockId,
        frames: Option<&[Frame<'p>]>,
    ) {
        let Some(Block::If(block)) = self.blocks.get(block_id) else { return };
        let (parent, scope, current) = (block.parent, block.scope, block.active);
        let compiled = self.compiled.clone();
        let Slot::If(chain) = &compiled.get(block.node).slots[block.slot] else { return };
        let initial = frames.is_some();
        let owned_frames;
        let frames = match frames {
            Some(frames) => frames,
            None => {
                owned_frames = self.frames(world, scope, props);
                &owned_frames
            }
        };
        let active = {
            let mut call = caller(world, self.root, parent);
            let mut ctx = EvalCtx::new(frames, props, &mut call);
            chain.0.iter().position(|(cond, _)| cond.as_ref().is_none_or(|c| eval(c, &mut ctx).is_truthy()))
        };
        if active == current {
            return;
        }
        let node = active.map(|i| chain.0[i].1);

        let Some(Block::If(block)) = self.blocks.get_mut(block_id) else { return };
        let old_entity = block.entity.take();
        let owned = std::mem::take(&mut block.owned);
        block.active = active;
        self.teardown(owned);
        if let Some(old) = old_entity
            && let Ok(old) = world.get_entity_mut(old)
        {
            old.despawn();
        }

        if let Some(node) = node {
            let entity =
                self.spawn_element(world, props, pending, node, None, Some(parent), frames, scope, OwnerRef::Branch(block_id));
            if let Some(Block::If(block)) = self.blocks.get_mut(block_id) {
                block.entity = Some(entity);
            }
        }
        if !initial {
            self.sync_children(world, parent);
        }
    }

    /// Matches the items of a list block against the current elements by key, reusing, spawning
    /// and despawning items as needed. `frames` are given while the list's parent is being
    /// spawned, which also places the items in order.
    fn reconcile<'p>(
        &mut self,
        world: &mut World,
        props: &'p Value,
        pending: &mut Pending,
        block_id: BlockId,
        force: bool,
        frames: Option<&[Frame<'p>]>,
    ) {
        let compiled = self.compiled.clone();
        let Some(Block::For(block)) = self.blocks.get(block_id) else { return };
        let (parent, node, scope) = (block.parent, block.node, block.scope);
        // Items of an iterable that is not a static path get no element routing, so reused
        // items must always be re-evaluated.
        let force = force || block.route.is_none();
        let list = compiled.get(node).list.as_ref().unwrap();
        let initial = frames.is_some();
        let mut item_frames = match frames {
            Some(frames) => frames.to_vec(),
            None => self.frames(world, scope, props),
        };

        // An iterable that is a prop path is read in place; anything else (a call) is computed
        // and kept for the items' frames.
        let mut call = caller(world, self.root, parent);
        let base = resolve(&list.iterable, &mut EvalCtx::new(&item_frames, props, &mut call));
        let computed: Option<Arc<[Value]>> = match base {
            Some(_) => None,
            None => {
                let value = eval(&list.iterable, &mut EvalCtx::new(&item_frames, props, &mut call)).into_owned();
                Some(value.into_list().unwrap_or_default().into())
            }
        };
        let elements: &[Value] = match (base, &computed) {
            (Some(base), _) => base.as_list().map(Vec::as_slice).unwrap_or(&[]),
            (None, Some(list)) => list,
            (None, None) => &[],
        };
        let element_frame = |index: usize| match &computed {
            Some(list) => Frame::computed(list.clone(), index),
            None => Frame::new(&elements[index], index),
        };

        let keys: Vec<ItemKey> = match &list.key {
            Some(key) => (0..elements.len())
                .map(|index| {
                    item_frames.push(element_frame(index));
                    let key = ItemKey::of(&eval(key, &mut EvalCtx::new(&item_frames, props, &mut call)));
                    item_frames.pop();
                    key
                })
                .collect(),
            None => Vec::new(),
        };
        drop(call);
        let keyed = list.key.is_some();

        let Some(Block::For(block)) = self.blocks.get(block_id) else { return };
        let (reused, removed): (Vec<Option<ItemId>>, Vec<ItemId>) = if keyed {
            // Items are matched by key, in order among items with the same key.
            let mut by_key: HashMap<&ItemKey, ItemId> = HashMap::with_capacity(block.items.len());
            let mut duplicates: Vec<(&ItemKey, ItemId)> = Vec::new();
            for &item_id in &block.items {
                let key = &self.items.get(item_id).unwrap().key;
                if by_key.contains_key(key) {
                    duplicates.push((key, item_id));
                } else {
                    by_key.insert(key, item_id);
                }
            }
            let reused = keys
                .iter()
                .map(|key| {
                    by_key.remove(key).or_else(|| {
                        let index = duplicates.iter().position(|(k, _)| *k == key)?;
                        Some(duplicates.remove(index).1)
                    })
                })
                .collect();
            let removed = by_key.into_values().chain(duplicates.into_iter().map(|(_, id)| id)).collect();
            (reused, removed)
        } else {
            // Without keys, items are matched by position.
            let reused = (0..elements.len()).map(|index| block.items.get(index).copied()).collect();
            let removed = block.items.get(elements.len()..).unwrap_or_default().to_vec();
            (reused, removed)
        };

        if let Some(Block::For(block)) = self.blocks.get_mut(block_id) {
            block.computed = computed.clone();
        }
        for item_id in removed {
            self.remove_item(world, item_id);
        }

        let mut keys = keys.into_iter();
        let mut new_items = Vec::with_capacity(reused.len());
        for (index, reused) in reused.into_iter().enumerate() {
            let key = if keyed { keys.next().unwrap() } else { ItemKey::Position(index) };
            match reused {
                Some(item_id) => {
                    let item = self.items.get_mut(item_id).unwrap();
                    if item.index != index {
                        item.index = index;
                        pending.fired.extend(item.index_bindings.iter().copied());
                    }
                    // Without keys, items are matched by position, so a reused item may now
                    // show a different element.
                    if force || !keyed {
                        fire_all(&item.trie, pending);
                        pending.fired.extend(item.index_bindings.iter().copied());
                    }
                    new_items.push(item_id);
                }
                None => {
                    let item_id = self.items.insert(Item {
                        block: block_id,
                        index,
                        key,
                        entity: Entity::PLACEHOLDER,
                        trie: DepTrie::default(),
                        index_bindings: Vec::new(),
                        owned: Owned::default(),
                        parent: scope,
                        level: list.level,
                    });
                    if pending.track_fresh {
                        pending.fresh.insert(item_id);
                    }
                    item_frames.push(element_frame(index));
                    let entity = self.spawn_element(
                        world,
                        props,
                        pending,
                        node,
                        None,
                        Some(parent),
                        &item_frames,
                        Some(item_id),
                        OwnerRef::Item(item_id),
                    );
                    item_frames.pop();
                    self.items.get_mut(item_id).unwrap().entity = entity;
                    new_items.push(item_id);
                }
            }
        }

        if let Some(Block::For(block)) = self.blocks.get_mut(block_id) {
            block.items = new_items;
        }
        if !initial {
            self.sync_children(world, parent);
        }
    }

    // Bindings

    #[allow(clippy::too_many_arguments)]
    fn add_binding(
        &mut self,
        entity: Entity,
        node: NodeId,
        kind: BindingKind,
        scope: Option<ItemId>,
        owner: OwnerRef,
        deps: &[Dep],
        mode: DepMode,
    ) -> BindingId {
        let id = self.bindings.insert(Binding { entity, node, kind, scope, registrations: Vec::new() });

        let mut registrations = Vec::with_capacity(deps.len());
        for dep in deps {
            let Some(trie_owner) = self.trie_owner(dep.root, scope) else { continue };
            match trie_owner {
                TrieOwner::ItemIndex(item) => {
                    if let Some(item) = self.items.get_mut(item) {
                        item.index_bindings.push(id);
                    }
                }
                _ => {
                    if let Some(trie) = self.trie_mut(trie_owner) {
                        trie.node_mut(&dep.path).bindings.push((id, mode));
                    }
                }
            }
            registrations.push((trie_owner, dep.path.clone()));
        }
        self.bindings.get_mut(id).unwrap().registrations = registrations;
        self.owned_mut(owner).bindings.push(id);
        id
    }

    /// Re-evaluates a text, class or style binding and writes it to its entity if it changed.
    fn apply(&mut self, world: &mut World, props: &Value, id: BindingId) {
        let Some(binding) = self.bindings.get(id) else { return };
        let (entity, scope) = (binding.entity, binding.scope);
        let node = self.compiled.get(binding.node);
        match binding.kind {
            BindingKind::Text => {
                let frames = self.frames(world, scope, props);
                let mut text = std::mem::take(&mut self.text_buffer);
                text.clear();
                let mut call = caller(world, self.root, entity);
                write_text(node.text.as_ref().unwrap(), &mut EvalCtx::new(&frames, props, &mut call), &mut text);
                drop(call);
                if let Ok(mut entity) = world.get_entity_mut(entity) {
                    match entity.get_mut::<Text>() {
                        Some(mut current) => {
                            if current.0 != text {
                                current.0.clone_from(&text);
                            }
                        }
                        None => {
                            entity.insert(Text::new(text.clone()));
                        }
                    }
                }
                self.text_buffer = text;
            }
            BindingKind::Class => {
                let frames = self.frames(world, scope, props);
                let mut call = caller(world, self.root, entity);
                let names = class_names(&node.classes, &mut EvalCtx::new(&frames, props, &mut call));
                drop(call);
                let classes = ClassList::new_with_classes(names);
                let Ok(mut entity) = world.get_entity_mut(entity) else { return };
                if entity.get::<ClassList>() != Some(&classes) {
                    entity.insert(classes);
                }
            }
            BindingKind::Style => {
                let frames = self.frames(world, scope, props);
                let mut call = caller(world, self.root, entity);
                let style = inline_style(&node.styles, &mut EvalCtx::new(&frames, props, &mut call));
                drop(call);
                let Ok(mut entity) = world.get_entity_mut(entity) else { return };
                write_style(&mut entity, style);
            }
            BindingKind::If(_) | BindingKind::For(_) => {}
        }
    }

    /// Resolves the frames of the `v-for` items enclosing `scope`, outermost first.
    fn frames<'p>(&self, world: &mut World, scope: Option<ItemId>, props: &'p Value) -> Vec<Frame<'p>> {
        let mut chain = Vec::new();
        let mut current = scope;
        while let Some(item_id) = current {
            let Some(item) = self.items.get(item_id) else { break };
            chain.push(item);
            current = item.parent;
        }

        let mut frames: Vec<Frame<'p>> = Vec::with_capacity(chain.len());
        for item in chain.into_iter().rev() {
            let Some(Block::For(block)) = self.blocks.get(item.block) else { break };
            let frame = match &block.computed {
                Some(list) => Frame::computed(list.clone(), item.index),
                None => {
                    let list = self.compiled.get(block.node).list.as_ref().unwrap();
                    let mut call = caller(world, self.root, block.parent);
                    let element = resolve(&list.iterable, &mut EvalCtx::new(&frames, props, &mut call))
                        .and_then(|list| list.index(item.index));
                    Frame::new(element.unwrap_or(&NULL), item.index)
                }
            };
            frames.push(frame);
        }
        frames
    }

    /// Resolves the handlers of `entity`'s listeners for the event `name`. The root element of a
    /// component can have two: its own and the one of the component element.
    pub fn resolve_event(
        &self,
        world: &mut World,
        props: &Value,
        entity: Entity,
        name: &str,
        args: &[Value],
    ) -> Vec<ResolvedHandler> {
        let Some(&(node_id, scope)) = self.listeners.get(&entity) else { return Vec::new() };
        let compiled = self.compiled.clone();
        let mut resolved = Vec::new();
        for listener in compiled.get(node_id).listeners_for(name) {
            let Some(handler) = &listener.handler else { continue };
            let (function, args) = match handler {
                CompiledHandler::Function(function) => (function.to_string(), args.to_vec()),
                CompiledHandler::Call { function, args: exprs } => {
                    let frames = self.frames(world, scope, props);
                    let mut call = caller(world, self.root, entity);
                    let mut ctx = EvalCtx::new(&frames, props, &mut call);
                    ctx.event_args = Some(args);
                    let values = exprs.iter().map(|expr| eval(expr, &mut ctx).into_owned()).collect();
                    (function.to_string(), values)
                }
            };
            let emit_target = self.emit_target(world, entity, listener.emit_root);
            resolved.push(ResolvedHandler { function, args, emit_target });
        }
        resolved
    }

    /// Finds the element rendered for node `emit_root` that contains `entity`: the root element of
    /// the component a listener was written in. Without one it is the template root.
    fn emit_target(&self, world: &World, entity: Entity, emit_root: Option<NodeId>) -> Entity {
        let Some(node) = emit_root else { return self.root };
        let mut current = entity;
        while current != self.root {
            if world.get::<TemplateRef>(current).is_some_and(|r| r.root == self.root && r.node == node) {
                return current;
            }
            let Some(parent) = world.get::<ChildOf>(current) else { break };
            current = parent.0;
        }
        self.root
    }

    /// The trie a dependency on `root` is registered in, for a binding inside `scope`.
    fn trie_owner(&self, root: ScopeRef, scope: Option<ItemId>) -> Option<TrieOwner> {
        let item_at = |level: usize| {
            let mut current = scope;
            while let Some(item_id) = current {
                let item = self.items.get(item_id)?;
                if item.level == level {
                    return Some(item_id);
                }
                current = item.parent;
            }
            None
        };
        Some(match root {
            ScopeRef::Props => TrieOwner::Root,
            ScopeRef::Item(level) => TrieOwner::Item(item_at(level)?),
            ScopeRef::Index(level) => TrieOwner::ItemIndex(item_at(level)?),
            ScopeRef::Event => return None,
        })
    }

    fn trie_mut(&mut self, owner: TrieOwner) -> Option<&mut DepTrie> {
        match owner {
            TrieOwner::Root => Some(&mut self.root_trie),
            TrieOwner::Item(item) => self.items.get_mut(item).map(|item| &mut item.trie),
            TrieOwner::ItemIndex(_) => None,
        }
    }

    fn owned_mut(&mut self, owner: OwnerRef) -> &mut Owned {
        match owner {
            OwnerRef::Root => &mut self.root_owned,
            OwnerRef::Branch(block) => match self.blocks.get_mut(block) {
                Some(Block::If(block)) => &mut block.owned,
                _ => panic!("v-if branch owner is not a live if block"),
            },
            OwnerRef::Item(item) => &mut self.items.get_mut(item).expect("list item owner is not alive").owned,
        }
    }

    // Teardown

    /// Despawns a list item's entity and frees its bookkeeping.
    fn remove_item(&mut self, world: &mut World, item_id: ItemId) {
        let Some(item) = self.items.remove(item_id) else { return };
        self.teardown(item.owned);
        if let Ok(entity) = world.get_entity_mut(item.entity) {
            entity.despawn();
        }
    }

    /// Frees the bindings, blocks and element state owned by a subtree. Entities are not
    /// despawned: they are all descendants of the subtree's top entity, which the caller despawns.
    fn teardown(&mut self, owned: Owned) {
        for id in owned.bindings {
            self.remove_binding(id);
        }
        for block_id in owned.blocks {
            match self.blocks.remove(block_id) {
                Some(Block::If(block)) => self.teardown(block.owned),
                Some(Block::For(block)) => {
                    if let Some((owner, path)) = block.route
                        && let Some(trie) = self.trie_mut(owner)
                    {
                        trie.remove(&path, None, Some(block_id));
                    }
                    for item_id in block.items {
                        if let Some(item) = self.items.remove(item_id) {
                            self.teardown(item.owned);
                        }
                    }
                }
                None => {}
            }
        }
        for entity in owned.elements {
            self.elements.remove(&entity);
            self.listeners.remove(&entity);
        }
    }

    fn remove_binding(&mut self, id: BindingId) {
        let Some(binding) = self.bindings.remove(id) else { return };
        for (owner, path) in binding.registrations {
            match owner {
                TrieOwner::ItemIndex(item) => {
                    if let Some(item) = self.items.get_mut(item) {
                        item.index_bindings.retain(|b| *b != id);
                    }
                }
                _ => {
                    if let Some(trie) = self.trie_mut(owner) {
                        trie.remove(&path, Some(id), None);
                    }
                }
            }
        }
    }

    /// Orders `parent`'s children as its slots dictate, after a block changed.
    fn sync_children(&self, world: &mut World, parent: Entity) {
        let Some(slots) = self.elements.get(&parent) else { return };
        let mut children = Vec::new();
        for slot in slots {
            match slot {
                SlotState::Static(entity) => children.push(*entity),
                SlotState::Block(block) => match self.blocks.get(*block) {
                    Some(Block::If(block)) => children.extend(block.entity),
                    Some(Block::For(block)) => {
                        children.extend(block.items.iter().filter_map(|id| self.items.get(*id)).map(|i| i.entity))
                    }
                    None => {}
                },
            }
        }
        children.retain(|e| *e != Entity::PLACEHOLDER);
        let Ok(mut parent) = world.get_entity_mut(parent) else { return };
        let current: &[Entity] = parent.get::<Children>().map_or(&[], |c| &**c);
        if current == children.as_slice() {
            return;
        }
        if current.len() == children.len() {
            // The same children in another order: every rendered child is already a child of
            // `parent`, as spawned items and branches are, so sorting is enough.
            let positions: EntityHashMap<usize> = children.iter().enumerate().map(|(i, e)| (*e, i)).collect();
            if let Some(mut current) = parent.get_mut::<Children>() {
                current.sort_by_key(|e| positions.get(e).copied().unwrap_or(usize::MAX));
            }
        } else {
            parent.replace_children(&children);
        }
    }
}

// Change walk

/// Walks the dirty parts of `value` together with `trie`, collecting the bindings to update.
///
/// Returns whether a real change (a replaced value or a changed list/dict shape) was found below,
/// which decides whether deep bindings fire. The result is only exact when `need_real` is set or
/// the node has deep bindings itself.
fn walk(trie: &DepTrie, value: &Value, need_real: bool, pending: &mut Pending) -> bool {
    let flags = value.flags();
    if flags.is_empty() {
        return false;
    }
    if flags.contains(ChangeFlags::SELF) {
        fire_all(trie, pending);
        return true;
    }

    let has_deep = trie.bindings.iter().any(|(_, mode)| *mode == DepMode::Deep);
    let need_real = need_real || has_deep;
    let shape = flags.contains(ChangeFlags::SHAPE);
    let mut real = shape;
    if shape {
        pending.fired.extend(trie.bindings.iter().map(|(id, _)| *id));
    }

    for (segment, child) in &trie.children {
        // After a shape change, indices may point at different elements, and keys may be gone.
        let shifted = shape && matches!(segment, PathSegment::Index(_));
        match value.child(segment) {
            Some(child_value) if !shifted => real |= walk(child, child_value, need_real, pending),
            _ if shape => fire_all(child, pending),
            _ => {}
        }
    }

    if !trie.for_blocks.is_empty()
        && let Some(list) = value.as_list()
    {
        for (index, element) in list.iter().enumerate() {
            if element.is_dirty() {
                pending.routed.extend(trie.for_blocks.iter().map(|block| (*block, index)));
            }
        }
    }

    if need_real && !real {
        real = match value.value_type() {
            crate::core::value::ValueType::List(list) => list.iter().any(has_real_change),
            crate::core::value::ValueType::Dict(map) => map.values().any(has_real_change),
            _ => false,
        };
    }
    if real && has_deep && !shape {
        pending
            .fired
            .extend(trie.bindings.iter().filter(|(_, mode)| *mode == DepMode::Deep).map(|(id, _)| *id));
    }
    real
}

/// Returns true if a value was replaced or a shape changed at or below `value`.
fn has_real_change(value: &Value) -> bool {
    let flags = value.flags();
    if flags.is_empty() {
        return false;
    }
    if flags.intersects(ChangeFlags::SELF | ChangeFlags::SHAPE) {
        return true;
    }
    match value.value_type() {
        crate::core::value::ValueType::List(list) => list.iter().any(has_real_change),
        crate::core::value::ValueType::Dict(map) => map.values().any(has_real_change),
        _ => false,
    }
}

/// Fires every binding in `trie`, marking its lists for a full re-evaluation.
fn fire_all(trie: &DepTrie, pending: &mut Pending) {
    pending.fired.extend(trie.bindings.iter().map(|(id, _)| *id));
    pending.force_full.extend(trie.for_blocks.iter().copied());
    for (_, child) in &trie.children {
        fire_all(child, pending);
    }
}
