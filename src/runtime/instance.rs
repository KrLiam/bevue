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

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use bevy::{
    ecs::{component::Component, entity::Entity, hierarchy::ChildOf, world::World},
    ui::{Interaction, Node, widget::Text},
};
use bevy_flair::prelude::{ClassList, InlineStyle};

use crate::{
    core::{
        compile::{
            CompileError, CompiledExpr, CompiledHandler, CompiledSegment, CompiledTemplate, IfChain, ScopeRef, Slot,
        },
        path::PathSegment,
        template::{BevueTemplate, NodeId, TemplateRef, TemplateStyle},
        value::{ChangeFlags, Value},
    },
    runtime::{
        functions::call_function,
        interpolation::{class_names, eval, eval_path, inline_style, no_calls, render_text, EvalCtx, Frame, FrameElement},
    },
};

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
    children: HashMap<PathSegment, DepTrie>,
    /// `v-for` blocks iterating the list at this path; element changes are routed to their items.
    for_blocks: Vec<BlockId>,
}

impl DepTrie {
    fn node_mut(&mut self, path: &[PathSegment]) -> &mut DepTrie {
        path.iter().fold(self, |node, segment| node.children.entry(segment.clone()).or_default())
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
                let Some(child) = self.children.get_mut(segment) else { return };
                child.remove(rest, binding, block);
                if child.is_empty() {
                    self.children.remove(segment);
                }
            }
        }
    }

    /// Returns the number of bindings registered in this trie.
    pub fn binding_count(&self) -> usize {
        self.bindings.len() + self.children.values().map(DepTrie::binding_count).sum::<usize>()
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
    /// The `v-for` items enclosing the binding, outermost first.
    scope: Vec<ItemId>,
    registrations: Vec<(TrieOwner, Vec<PathSegment>)>,
}

/// Bookkeeping owned by a subtree that can be torn down: a `v-if` branch or a list item.
#[derive(Default)]
struct Owned {
    bindings: Vec<BindingId>,
    blocks: Vec<BlockId>,
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
    chain: IfChain,
    scope: Vec<ItemId>,
    active: Option<usize>,
    entity: Option<Entity>,
    owned: Owned,
    binding: Option<BindingId>,
}

struct ForBlock {
    parent: Entity,
    node: NodeId,
    scope: Vec<ItemId>,
    items: Vec<ItemId>,
    keyed: bool,
    binding: Option<BindingId>,
    /// Where element changes are routed from, if the iterable is a static path.
    route: Option<(TrieOwner, Vec<PathSegment>)>,
    /// The elements of an iterable that is not a prop path (e.g. `sorted(items)`), as computed
    /// by the last reconcile.
    computed: Option<Arc<[Value]>>,
}

enum Block {
    If(IfBlock),
    For(ForBlock),
}

struct Item {
    block: BlockId,
    index: usize,
    key: String,
    entity: Entity,
    trie: DepTrie,
    index_bindings: Vec<BindingId>,
    owned: Owned,
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
    /// Items spawned during this update, already up to date.
    fresh: HashSet<ItemId>,
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
    template: Arc<BevueTemplate>,
    compiled: Arc<CompiledTemplate>,
    root_trie: DepTrie,
    root_owned: Owned,
    bindings: Arena<Binding>,
    blocks: Arena<Block>,
    items: Arena<Item>,
    elements: HashMap<Entity, Vec<SlotState>>,
    /// Nodes with event listeners: their template node and `v-for` scope.
    listeners: HashMap<Entity, (NodeId, Vec<ItemId>)>,
}

impl Default for TemplateInstance {
    fn default() -> Self {
        Self {
            root: Entity::PLACEHOLDER,
            template: Arc::default(),
            compiled: Arc::default(),
            root_trie: DepTrie::default(),
            root_owned: Owned::default(),
            bindings: Arena::default(),
            blocks: Arena::default(),
            items: Arena::default(),
            elements: HashMap::new(),
            listeners: HashMap::new(),
        }
    }
}

impl TemplateInstance {
    /// Spawns the UI tree of `template` under `root`, evaluated against `props`.
    pub fn render(
        world: &mut World,
        root: Entity,
        template: Arc<BevueTemplate>,
        props: &Value,
    ) -> Result<Self, CompileError> {
        let compiled = Arc::new(CompiledTemplate::compile(&template)?);
        Ok(Self::render_compiled(world, root, template, compiled, props))
    }

    /// Spawns the UI tree of an already compiled `template` under `root`, evaluated against
    /// `props`.
    pub fn render_compiled(
        world: &mut World,
        root: Entity,
        template: Arc<BevueTemplate>,
        compiled: Arc<CompiledTemplate>,
        props: &Value,
    ) -> Self {
        let mut instance = Self { root, template, compiled, ..Default::default() };
        if let Some(root_id) = instance.compiled.root {
            let mut pending = Pending::default();
            instance.spawn_element(world, props, &mut pending, root_id, Some(root), None, &[], OwnerRef::Root);
        }
        instance
    }

    /// Returns true if any node of the template has a `style` or `:style` attribute.
    pub fn has_inline_styles(&self) -> bool {
        self.compiled.nodes.iter().any(|node| !node.styles.is_empty())
    }

    /// The styles of the template and of the components it uses.
    pub fn styles(&self) -> &[TemplateStyle] {
        &self.compiled.styles
    }

    pub fn template(&self) -> &Arc<BevueTemplate> {
        &self.template
    }

    pub fn stats(&self) -> InstanceStats {
        InstanceStats { bindings: self.bindings.len(), blocks: self.blocks.len(), items: self.items.len() }
    }

    /// Applies the changes recorded in `props` to the UI. With `full`, every binding is
    /// re-evaluated, which is used when props were replaced without change flags.
    pub fn update(&mut self, world: &mut World, props: &Value, full: bool) -> UpdateStats {
        let mut stats = UpdateStats { full, ..Default::default() };
        let mut pending = Pending::default();
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
                self.reconcile(world, props, &mut pending, block, force);
            }
            for (block_id, index) in routed {
                self.walk_item(world, props, &mut pending, block_id, index);
            }
        }

        let mut seen = HashSet::new();
        let fired: Vec<_> = pending.fired.drain(..).filter(|id| seen.insert(*id)).collect();
        for &id in &fired {
            if let Some(BindingKind::If(block)) = self.bindings.get(id).map(|b| b.kind) {
                self.update_if(world, props, &mut pending, block);
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
        let frames = self.frames(world, &block.scope, props);
        let mut call = caller(world, self.root, parent);
        let Some(path) = eval_path(&list.iterable, &mut EvalCtx::new(&frames, props, &mut call)) else { return };
        let Some(element) = props.at(&path.join(index)) else { return };
        let keyed = block.keyed;
        let block_binding = block.binding;

        let item = self.items.get(item_id).unwrap();
        walk(&item.trie, element, false, pending);

        // If the element's key changed in place, the list must be reconciled again so entity
        // identity keeps following keys.
        if keyed && let Some(key_expr) = &list.key {
            let mut key_frames = frames;
            key_frames.push(Frame::path(path.join(index), index));
            let key = eval(key_expr, &mut EvalCtx::new(&key_frames, props, &mut call)).to_string();
            if key != item.key
                && let Some(binding) = block_binding
            {
                pending.fired.push(binding);
            }
        }
    }

    // Spawning

    /// Spawns the element for `node_id` (or reuses `entity`) with its subtree.
    #[allow(clippy::too_many_arguments)]
    fn spawn_element(
        &mut self,
        world: &mut World,
        props: &Value,
        pending: &mut Pending,
        node_id: NodeId,
        entity: Option<Entity>,
        parent: Option<Entity>,
        scope: &[ItemId],
        owner: OwnerRef,
    ) -> Entity {
        let compiled = self.compiled.clone();
        let node = compiled.get(node_id);
        let entity = entity.unwrap_or_else(|| world.spawn_empty().id());

        let mut entity_mut = world.entity_mut(entity);
        entity_mut.insert(TemplateRef::new(self.root, node_id));
        if !entity_mut.contains::<Node>() {
            entity_mut.insert(Node::default());
        }
        if let Some(parent) = parent {
            entity_mut.insert(ChildOf(parent));
            self.owned_mut(owner).elements.push(entity);
        }
        // `Node` requires `FocusPolicy::Pass`, so every listening node under the cursor gets its own
        // `Interaction`, like DOM events reaching all ancestors.
        if node.interactive() && !entity_mut.contains::<Interaction>() {
            entity_mut.insert(Interaction::default());
        }
        if !node.listeners.is_empty() {
            self.listeners.insert(entity, (node_id, scope.to_vec()));
        }

        match &node.text {
            Some(text) if text.is_static() => {
                let text = render_text(text, &mut EvalCtx::new(&[], props, &mut no_calls));
                world.entity_mut(entity).insert(Text::new(text));
            }
            Some(text) => {
                let exprs = text.0.iter().filter_map(|f| match f {
                    crate::core::compile::CompiledFragment::Expr(e) => Some(e),
                    _ => None,
                });
                let id = self.add_binding(entity, node_id, BindingKind::Text, scope, owner, exprs, DepMode::Deep);
                self.apply(world, props, id);
            }
            None if parent.is_none() => {
                world.entity_mut(entity).remove::<Text>();
            }
            None => {}
        }

        if node.has_dynamic_classes() {
            let id = self.add_binding(entity, node_id, BindingKind::Class, scope, owner, node.classes.iter(), DepMode::Deep);
            self.apply(world, props, id);
        } else if !node.classes.is_empty() {
            let names = class_names(&node.classes, &mut EvalCtx::new(&[], props, &mut no_calls));
            world.entity_mut(entity).insert(ClassList::new_with_classes(names));
        }

        if node.has_dynamic_styles() {
            let id = self.add_binding(entity, node_id, BindingKind::Style, scope, owner, node.styles.iter(), DepMode::Deep);
            self.apply(world, props, id);
        } else if !node.styles.is_empty() {
            let style = inline_style(&node.styles, &mut EvalCtx::new(&[], props, &mut no_calls));
            world.entity_mut(entity).insert(style);
        }

        let mut slots = Vec::with_capacity(node.slots.len());
        for slot in &node.slots {
            match slot {
                Slot::Static(child) => {
                    let child = self.spawn_element(world, props, pending, *child, None, Some(entity), scope, owner);
                    slots.push(SlotState::Static(child));
                }
                Slot::If(chain) => {
                    let block = self.blocks.insert(Block::If(IfBlock {
                        parent: entity,
                        chain: chain.clone(),
                        scope: scope.to_vec(),
                        active: None,
                        entity: None,
                        owned: Owned::default(),
                        binding: None,
                    }));
                    self.owned_mut(owner).blocks.push(block);
                    let conditions = chain.0.iter().filter_map(|(cond, _)| cond.as_ref());
                    let binding =
                        self.add_binding(entity, node_id, BindingKind::If(block), scope, owner, conditions, DepMode::Deep);
                    if let Some(Block::If(b)) = self.blocks.get_mut(block) {
                        b.binding = Some(binding);
                    }
                    slots.push(SlotState::Block(block));
                    self.update_if(world, props, pending, block);
                }
                Slot::For(child) => {
                    let list = compiled.get(*child).list.as_ref().unwrap();
                    let keyed = list.key.is_some();
                    let block = self.blocks.insert(Block::For(ForBlock {
                        parent: entity,
                        node: *child,
                        scope: scope.to_vec(),
                        items: Vec::new(),
                        keyed,
                        binding: None,
                        route: None,
                        computed: None,
                    }));
                    self.owned_mut(owner).blocks.push(block);

                    // A static iterable path routes element changes into items; a dynamic one
                    // (e.g. `groups[g]`) re-reconciles on any change below its static prefix.
                    let is_static = matches!(&list.iterable, CompiledExpr::Path { root, segments }
                        if !matches!(root, ScopeRef::Index(_)) && segments.iter().all(|s| matches!(s, CompiledSegment::Static(_))));
                    let mode = if is_static { DepMode::Shallow } else { DepMode::Deep };
                    let binding = self.add_binding(
                        entity,
                        *child,
                        BindingKind::For(block),
                        scope,
                        owner,
                        std::iter::once(&list.iterable),
                        mode,
                    );
                    let route = if is_static {
                        let mut deps = Vec::new();
                        list.iterable.dependencies(&mut deps);
                        deps.pop().and_then(|(root, path)| {
                            let owner = self.trie_owner(root, scope)?;
                            self.trie_mut(owner)?.node_mut(&path).for_blocks.push(block);
                            Some((owner, path))
                        })
                    } else {
                        None
                    };
                    if let Some(Block::For(b)) = self.blocks.get_mut(block) {
                        b.binding = Some(binding);
                        b.route = route;
                    }
                    slots.push(SlotState::Block(block));
                    self.reconcile(world, props, pending, block, false);
                }
            }
        }

        self.elements.insert(entity, slots);
        self.sync_children(world, entity);
        entity
    }

    /// Re-evaluates an if-chain and swaps the rendered branch if it changed.
    fn update_if(&mut self, world: &mut World, props: &Value, pending: &mut Pending, block_id: BlockId) {
        let Some(Block::If(block)) = self.blocks.get(block_id) else { return };
        let frames = self.frames(world, &block.scope, props);
        let active = {
            let mut call = caller(world, self.root, block.parent);
            let mut ctx = EvalCtx::new(&frames, props, &mut call);
            block.chain.0.iter().position(|(cond, _)| cond.as_ref().is_none_or(|c| eval(c, &mut ctx).is_truthy()))
        };
        if active == block.active {
            return;
        }

        let parent = block.parent;
        let scope = block.scope.clone();
        let node = active.map(|i| block.chain.0[i].1);

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
                self.spawn_element(world, props, pending, node, None, Some(parent), &scope, OwnerRef::Branch(block_id));
            if let Some(Block::If(block)) = self.blocks.get_mut(block_id) {
                block.entity = Some(entity);
            }
        }
        self.sync_children(world, parent);
    }

    /// Matches the items of a list block against the current elements by key, reusing, spawning
    /// and despawning items as needed.
    fn reconcile(&mut self, world: &mut World, props: &Value, pending: &mut Pending, block_id: BlockId, force: bool) {
        let compiled = self.compiled.clone();
        let Some(Block::For(block)) = self.blocks.get(block_id) else { return };
        let list = compiled.get(block.node).list.as_ref().unwrap();
        let frames = self.frames(world, &block.scope, props);
        let mut call = caller(world, self.root, block.parent);

        // An iterable that is a prop path is read in place; anything else (a call) is computed
        // and cached for the items' frames.
        let mut ctx = EvalCtx::new(&frames, props, &mut call);
        let base = eval_path(&list.iterable, &mut ctx);
        let iterable = eval(&list.iterable, &mut ctx);
        let elements: &[Value] = iterable.as_list().map(Vec::as_slice).unwrap_or(&[]);
        let computed: Option<Arc<[Value]>> = match base {
            Some(_) => None,
            None => Some(Arc::from(elements.to_vec())),
        };
        let element_frame = |index: usize| match (&base, &computed) {
            (Some(base), _) => Frame::path(base.join(index), index),
            (None, Some(list)) => Frame { element: FrameElement::Value(list.clone()), index },
            (None, None) => unreachable!(),
        };

        let keys: Vec<String> = (0..elements.len())
            .map(|i| match &list.key {
                Some(key) => {
                    let mut key_frames = frames.clone();
                    key_frames.push(element_frame(i));
                    eval(key, &mut EvalCtx::new(&key_frames, props, &mut call)).to_string()
                }
                None => format!("#{i}"),
            })
            .collect();
        drop(call);
        // Items of an iterable that is not a static path get no element routing, so reused
        // items must always be re-evaluated.
        let force = force || block.route.is_none();

        let mut by_key: HashMap<String, VecDeque<ItemId>> = HashMap::new();
        for &item_id in &block.items {
            let key = self.items.get(item_id).unwrap().key.clone();
            by_key.entry(key).or_default().push_back(item_id);
        }
        let mut reused: Vec<Option<ItemId>> =
            keys.iter().map(|k| by_key.get_mut(k.as_str()).and_then(VecDeque::pop_front)).collect();
        let removed: Vec<ItemId> = by_key.into_values().flatten().collect();

        let keyed = block.keyed;
        let parent = block.parent;
        let node = block.node;
        let scope = block.scope.clone();

        if let Some(Block::For(block)) = self.blocks.get_mut(block_id) {
            block.computed = computed;
        }
        for item_id in removed {
            self.remove_item(world, item_id);
        }

        let mut new_items = Vec::with_capacity(keys.len());
        for (index, key) in keys.into_iter().enumerate() {
            match reused[index].take() {
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
                    });
                    pending.fresh.insert(item_id);
                    let mut item_scope = scope.clone();
                    item_scope.push(item_id);
                    let entity =
                        self.spawn_element(world, props, pending, node, None, Some(parent), &item_scope, OwnerRef::Item(item_id));
                    self.items.get_mut(item_id).unwrap().entity = entity;
                    new_items.push(item_id);
                }
            }
        }

        if let Some(Block::For(block)) = self.blocks.get_mut(block_id) {
            block.items = new_items;
        }
        self.sync_children(world, parent);
    }

    // Bindings

    #[allow(clippy::too_many_arguments)]
    fn add_binding<'e>(
        &mut self,
        entity: Entity,
        node: NodeId,
        kind: BindingKind,
        scope: &[ItemId],
        owner: OwnerRef,
        exprs: impl Iterator<Item = &'e CompiledExpr>,
        mode: DepMode,
    ) -> BindingId {
        let mut deps = Vec::new();
        for expr in exprs {
            expr.dependencies(&mut deps);
        }
        let id = self.bindings.insert(Binding { entity, node, kind, scope: scope.to_vec(), registrations: Vec::new() });

        let mut registrations = Vec::with_capacity(deps.len());
        for (root, path) in deps {
            let Some(trie_owner) = self.trie_owner(root, scope) else { continue };
            match trie_owner {
                TrieOwner::ItemIndex(item) => {
                    if let Some(item) = self.items.get_mut(item) {
                        item.index_bindings.push(id);
                    }
                }
                _ => {
                    if let Some(trie) = self.trie_mut(trie_owner) {
                        trie.node_mut(&path).bindings.push((id, mode));
                    }
                }
            }
            registrations.push((trie_owner, path));
        }
        self.bindings.get_mut(id).unwrap().registrations = registrations;
        self.owned_mut(owner).bindings.push(id);
        id
    }

    /// Re-evaluates a text, class or style binding and writes it to its entity if it changed.
    fn apply(&mut self, world: &mut World, props: &Value, id: BindingId) {
        let Some(binding) = self.bindings.get(id) else { return };
        let node = self.compiled.get(binding.node);
        match binding.kind {
            BindingKind::Text => {
                let frames = self.frames(world, &binding.scope, props);
                let mut call = caller(world, self.root, binding.entity);
                let text = render_text(node.text.as_ref().unwrap(), &mut EvalCtx::new(&frames, props, &mut call));
                drop(call);
                let Ok(mut entity) = world.get_entity_mut(binding.entity) else { return };
                match entity.get_mut::<Text>() {
                    Some(mut current) => {
                        if current.0 != text {
                            current.0 = text;
                        }
                    }
                    None => {
                        entity.insert(Text::new(text));
                    }
                }
            }
            BindingKind::Class => {
                let frames = self.frames(world, &binding.scope, props);
                let mut call = caller(world, self.root, binding.entity);
                let names = class_names(&node.classes, &mut EvalCtx::new(&frames, props, &mut call));
                drop(call);
                let classes = ClassList::new_with_classes(names);
                let Ok(mut entity) = world.get_entity_mut(binding.entity) else { return };
                if entity.get::<ClassList>() != Some(&classes) {
                    entity.insert(classes);
                }
            }
            BindingKind::Style => {
                let frames = self.frames(world, &binding.scope, props);
                let mut call = caller(world, self.root, binding.entity);
                let style = inline_style(&node.styles, &mut EvalCtx::new(&frames, props, &mut call));
                drop(call);
                let Ok(mut entity) = world.get_entity_mut(binding.entity) else { return };
                // An emptied style still replaces the previous one, or flair keeps applying it.
                let current = entity.get::<InlineStyle>();
                if current != Some(&style) && (current.is_some() || style != InlineStyle::default()) {
                    entity.insert(style);
                }
            }
            BindingKind::If(_) | BindingKind::For(_) => {}
        }
    }

    /// Resolves the frames of the `v-for` items in `scope` for evaluation.
    fn frames(&self, world: &mut World, scope: &[ItemId], props: &Value) -> Vec<Frame> {
        let mut frames: Vec<Frame> = Vec::with_capacity(scope.len());
        for &item_id in scope {
            let Some(item) = self.items.get(item_id) else { break };
            let Some(Block::For(block)) = self.blocks.get(item.block) else { break };
            let element = match &block.computed {
                Some(list) => FrameElement::Value(list.clone()),
                None => {
                    let list = self.compiled.get(block.node).list.as_ref().unwrap();
                    let mut call = caller(world, self.root, block.parent);
                    let base = eval_path(&list.iterable, &mut EvalCtx::new(&frames, props, &mut call));
                    FrameElement::Path(base.unwrap_or_default().join(item.index))
                }
            };
            frames.push(Frame { element, index: item.index });
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
        let Some((node_id, scope)) = self.listeners.get(&entity) else { return Vec::new() };
        let compiled = self.compiled.clone();
        let mut resolved = Vec::new();
        for listener in compiled.get(*node_id).listeners_for(name) {
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

    fn trie_owner(&self, root: ScopeRef, scope: &[ItemId]) -> Option<TrieOwner> {
        Some(match root {
            ScopeRef::Props => TrieOwner::Root,
            ScopeRef::Item(level) => TrieOwner::Item(*scope.get(level)?),
            ScopeRef::Index(level) => TrieOwner::ItemIndex(*scope.get(level)?),
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

    /// Orders `parent`'s children as its slots dictate.
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
        if let Ok(mut parent) = world.get_entity_mut(parent) {
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
    for child in trie.children.values() {
        fire_all(child, pending);
    }
}
