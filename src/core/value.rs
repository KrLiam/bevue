//! Proxied property values for the API.
//!
//! A [`Value`] is a dynamically typed tree (booleans, numbers, strings, entities, lists, dicts and
//! structs) that
//! records *where* it was mutated, so the runtime can update only the UI nodes bound to the parts
//! that changed.
//!
//! Every node carries [`ChangeFlags`]:
//! - [`ChangeFlags::SELF`]: the node was replaced or its scalar value changed.
//! - [`ChangeFlags::SHAPE`]: a list or dict had elements added, removed or reordered.
//! - [`ChangeFlags::SUBTREE`]: some descendant may have changed.
//!
//! Reading through [`Deref`](std::ops::Deref) is free. Mutating through [`DerefMut`] conservatively marks the node
//! as [`ChangeFlags::SELF`]; prefer the typed mutators ([`Value::set`], [`Value::get_mut`],
//! [`Value::push`], [`Value::insert`], ...) which mark only what actually changed.

use std::{
    fmt,
    ops::DerefMut,
    sync::atomic::{AtomicU32, Ordering},
};

use bevy::{ecs::entity::Entity, platform::collections::HashMap};

use crate::core::path::PathSegment;

/// Bitset describing how a [`Value`] node changed since the last [`Value::clear_changes`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ChangeFlags(u8);

impl ChangeFlags {
    pub const NONE: Self = Self(0);
    /// The node was replaced or its scalar value changed.
    pub const SELF: Self = Self(1);
    /// Elements were added, removed or reordered in this list or dict.
    pub const SHAPE: Self = Self(2);
    /// Some descendant may have changed.
    pub const SUBTREE: Self = Self(4);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for ChangeFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Debug for ChangeFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = Vec::new();
        if self.contains(Self::SELF) {
            names.push("SELF");
        }
        if self.contains(Self::SHAPE) {
            names.push("SHAPE");
        }
        if self.contains(Self::SUBTREE) {
            names.push("SUBTREE");
        }
        write!(f, "ChangeFlags({})", names.join(" | "))
    }
}

/// An attribute value.
#[derive(Default, bevy::prelude::Deref)]
pub struct Value {
    /// The type variant of this value.
    #[deref]
    t: ValueType,
    /// How this node changed since the last [`Value::clear_changes`].
    flags: ChangeFlags,
    /// Identifies a list element across moves, so lists can match elements to what rendered them
    /// without re-reading them. `0` for values that were never put in a list. Stored in what would
    /// otherwise be padding.
    id: u32,
}

/// The next element id. Ids wrap after 2³² elements were created, which could only confuse two
/// elements of one list if both lived through all of those insertions.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Reserves `count` consecutive element ids, returning the first. Callers skip `0`.
fn reserve_ids(count: u32) -> u32 {
    NEXT_ID.fetch_add(count, Ordering::Relaxed).max(1)
}

impl Clone for Value {
    /// Clones the data, keeping element ids. The clone starts with no change flags.
    fn clone(&self) -> Self {
        Self {
            t: self.t.clone(),
            flags: ChangeFlags::NONE,
            id: self.id,
        }
    }
}

impl PartialEq for Value {
    /// Compares the data only, ignoring change flags.
    fn eq(&self, other: &Self) -> bool {
        self.t == other.t
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.t, f)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.t.fmt(f)
    }
}

impl DerefMut for Value {
    /// Gives raw mutable access to the data, marking this node as [`ChangeFlags::SELF`].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.flags.insert(ChangeFlags::SELF);
        &mut self.t
    }
}

impl Value {
    /// The null value.
    pub const NULL: Value = Value { t: ValueType::Null, flags: ChangeFlags::NONE, id: 0 };

    fn new(t: ValueType) -> Self {
        Self {
            t,
            flags: ChangeFlags::NONE,
            id: 0,
        }
    }

    /// The id of this list element: it follows the element when the list is reordered, and a new
    /// element gets a new one. `0` if the value was never put in a list.
    pub fn element_id(&self) -> u32 {
        self.id
    }

    /// Creates a null value.
    pub fn null() -> Self {
        Self::new(ValueType::Null)
    }
    /// Creates a bool value.
    pub fn boolean(value: bool) -> Self {
        Self::new(ValueType::Boolean(value))
    }
    /// Creates a number value.
    pub fn number(value: f64) -> Self {
        Self::new(ValueType::Number(value))
    }
    /// Creates a string value.
    pub fn string(value: String) -> Self {
        Self::new(ValueType::String(value))
    }
    /// Creates an entity value.
    pub fn entity(value: Entity) -> Self {
        Self::new(ValueType::Entity(value))
    }
    /// Creates a list value. Elements without an [element id](Value::element_id) get one.
    pub fn list(mut value: Vec<Value>) -> Self {
        let missing = value.iter().filter(|v| v.id == 0).count() as u32;
        if missing > 0 {
            let mut next = reserve_ids(missing);
            for element in value.iter_mut().filter(|v| v.id == 0) {
                element.id = next;
                next = next.wrapping_add(1).max(1);
            }
        }
        Self::new(ValueType::List(value))
    }
    /// Creates an dict value.
    pub fn dict(value: HashMap<String, Value>) -> Self {
        Self::new(ValueType::Dict(value))
    }
    /// Creates an empty dict value.
    pub fn empty_dict() -> Self {
        Self::dict(HashMap::new())
    }
    /// Creates a struct value: a dict with a fixed set of keys, `names`, shared by every value of
    /// the same Rust type. `fields` holds the value of each name, in order.
    ///
    /// Panics if `names` and `fields` have different lengths.
    pub fn structure(names: &'static [&'static str], fields: Vec<Value>) -> Self {
        Self::new(ValueType::Struct(StructValue::new(names, fields)))
    }

    /// Returns the type variant of this value.
    pub fn value_type(&self) -> &ValueType {
        &self.t
    }

    /// Returns true if this value is `Null`.
    pub fn is_null(&self) -> bool {
        matches!(self.t, ValueType::Null)
    }

    /// Unwraps the bool value if value is `Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match &self.t { ValueType::Boolean(v) => Some(*v), _ => None }
    }

    /// Unwraps the number value if value is `Number`.
    pub fn as_number(&self) -> Option<f64> {
        match &self.t { ValueType::Number(v) => Some(*v), _ => None }
    }

    /// Unwraps the string value if value is `String`.
    pub fn as_string(&self) -> Option<&String> {
        match &self.t { ValueType::String(v) => Some(v), _ => None }
    }

    /// Unwraps the entity value if value is `Entity`.
    pub fn as_entity(&self) -> Option<Entity> {
        match &self.t { ValueType::Entity(v) => Some(*v), _ => None }
    }

    /// Unwraps the list value if value is `List`.
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match &self.t { ValueType::List(v) => Some(v), _ => None }
    }

    /// Unwraps the dict value if value is `Dict`. Structs are not dicts, see [`Value::is_map`].
    pub fn as_dict(&self) -> Option<&HashMap<String, Value>> {
        match &self.t { ValueType::Dict(v) => Some(v), _ => None }
    }

    /// Unwraps the struct value if value is `Struct`.
    pub fn as_struct(&self) -> Option<&StructValue> {
        match &self.t { ValueType::Struct(v) => Some(v), _ => None }
    }

    /// Returns true if this value has named entries: a dict or a struct.
    pub fn is_map(&self) -> bool {
        matches!(self.t, ValueType::Dict(_) | ValueType::Struct(_))
    }

    /// Iterates the entries of a dict or struct. Dict entries come in no particular order, struct
    /// entries in field order.
    pub fn entries(&self) -> Option<Entries<'_>> {
        match &self.t {
            ValueType::Dict(map) => Some(Entries::Dict(map.iter())),
            ValueType::Struct(s) => Some(Entries::Struct(s.names.iter().zip(s.fields.iter()))),
            _ => None,
        }
    }

    /// Truthiness used by `v-if`: `false`, `0`, `NaN`, `""`, `null` and empty lists and dicts are
    /// falsy, everything else (including entities) is truthy.
    pub fn is_truthy(&self) -> bool {
        match &self.t {
            ValueType::Null => false,
            ValueType::Boolean(b) => *b,
            ValueType::Number(n) => *n != 0.0 && !n.is_nan(),
            ValueType::String(s) => !s.is_empty(),
            ValueType::Entity(_) => true,
            ValueType::List(list) => !list.is_empty(),
            ValueType::Dict(map) => !map.is_empty(),
            ValueType::Struct(s) => !s.is_empty(),
        }
    }

    /// Renders this value as interpolated text: strings are written raw and `null` is empty.
    pub fn to_text(&self) -> String {
        match &self.t {
            ValueType::String(s) => s.clone(),
            _ => {
                let mut out = String::new();
                self.write_text(&mut out);
                out
            }
        }
    }

    /// Appends this value as interpolated text to `out`, like [`Value::to_text`].
    pub fn write_text(&self, out: &mut String) {
        use std::fmt::Write;
        match &self.t {
            ValueType::Null => {}
            ValueType::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
            ValueType::Number(n) => {
                let _ = write!(out, "{n}");
            }
            ValueType::String(s) => out.push_str(s),
            ValueType::Entity(e) => {
                let _ = write!(out, "{e}");
            }
            ValueType::List(_) | ValueType::Dict(_) | ValueType::Struct(_) => {
                let _ = write!(out, "{self}");
            }
        }
    }

    /// Unwraps the list if value is `List`, without copying it.
    pub fn into_list(self) -> Option<Vec<Value>> {
        match self.t { ValueType::List(v) => Some(v), _ => None }
    }

    // Navigation

    /// Gets the dict or struct entry at `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match &self.t {
            ValueType::Dict(map) => map.get(key),
            ValueType::Struct(s) => s.get(key),
            _ => None,
        }
    }

    /// Gets the list element at `index`.
    pub fn index(&self, index: usize) -> Option<&Value> {
        self.as_list()?.get(index)
    }

    /// Gets the child at `segment`.
    pub fn child(&self, segment: &PathSegment) -> Option<&Value> {
        match segment {
            PathSegment::Key(key) => self.get(key),
            PathSegment::Index(i) => self.index(*i),
        }
    }

    /// Gets the descendant at `path`. The empty path returns `self`.
    pub fn at(&self, path: &[PathSegment]) -> Option<&Value> {
        path.iter().try_fold(self, |value, segment| value.child(segment))
    }

    /// Gets the dict or struct entry at `key` for mutation, marking this node as
    /// [`ChangeFlags::SUBTREE`].
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        let child = match &mut self.t {
            ValueType::Dict(map) => map.get_mut(key)?,
            ValueType::Struct(s) => s.get_mut(key)?,
            _ => return None,
        };
        self.flags.insert(ChangeFlags::SUBTREE);
        Some(child)
    }

    /// Runs `f` on field `index` of this struct, marking this node as [`ChangeFlags::SUBTREE`]
    /// only if the field ends up dirty. Does nothing if this value is not a struct with that
    /// field. Used by `#[derive(Props)]`.
    pub fn update_field(&mut self, index: usize, f: impl FnOnce(&mut Value)) {
        let ValueType::Struct(s) = &mut self.t else { return };
        let Some(field) = s.fields.get_mut(index) else { return };
        f(field);
        if field.is_dirty() {
            self.flags.insert(ChangeFlags::SUBTREE);
        }
    }

    /// Gets the list element at `index` for mutation, marking this node as
    /// [`ChangeFlags::SUBTREE`].
    pub fn index_mut(&mut self, index: usize) -> Option<&mut Value> {
        let ValueType::List(list) = &mut self.t else { return None };
        let child = list.get_mut(index)?;
        self.flags.insert(ChangeFlags::SUBTREE);
        Some(child)
    }

    /// Gets the child at `segment` for mutation, marking this node as [`ChangeFlags::SUBTREE`].
    pub fn child_mut(&mut self, segment: &PathSegment) -> Option<&mut Value> {
        match segment {
            PathSegment::Key(key) => self.get_mut(key),
            PathSegment::Index(i) => self.index_mut(*i),
        }
    }

    /// Gets the descendant at `path` for mutation, marking every node walked through as
    /// [`ChangeFlags::SUBTREE`]. The empty path returns `self`.
    pub fn at_mut(&mut self, path: &[PathSegment]) -> Option<&mut Value> {
        match path.split_first() {
            None => Some(self),
            Some((segment, rest)) => self.child_mut(segment)?.at_mut(rest),
        }
    }

    /// Gets the dict entry at `key` for mutation, inserting `null` first if it is missing.
    ///
    /// A `null` value is turned into an empty dict, and a struct missing `key` into a dict. Panics
    /// if this value is any other non-dict.
    pub fn entry(&mut self, key: &str) -> &mut Value {
        if self.as_struct().is_some_and(|s| s.position(key).is_some()) {
            return self.get_mut(key).unwrap();
        }
        let map = self.dict_mut_for_insert("entry");
        if !map.contains_key(key) {
            map.insert(key.to_owned(), Value::changed(ValueType::Null));
            self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
        } else {
            self.flags.insert(ChangeFlags::SUBTREE);
        }
        let ValueType::Dict(map) = &mut self.t else { unreachable!() };
        map.get_mut(key).unwrap()
    }

    /// Runs `f` on the descendant at `path`, marking ancestors as [`ChangeFlags::SUBTREE`] only if
    /// the descendant ends up dirty. Missing dict keys are inserted as `null` (a `null` parent
    /// becomes a dict); a missing list index skips `f`. Returns whether `f` ran.
    ///
    /// Use this instead of [`Value::at_mut`] for diffing writes that often change nothing.
    pub fn update_at(&mut self, path: &[PathSegment], f: impl FnOnce(&mut Value)) -> bool {
        let mut f = Some(f);
        self.update_at_inner(path, &mut |value| (f.take().unwrap())(value))
    }

    fn update_at_inner(&mut self, path: &[PathSegment], f: &mut dyn FnMut(&mut Value)) -> bool {
        let Some((segment, rest)) = path.split_first() else {
            f(self);
            return true;
        };
        if let PathSegment::Key(key) = segment
            && !self.as_struct().is_some_and(|s| s.position(key).is_some())
        {
            let map = self.dict_mut_for_insert("update_at");
            if !map.contains_key(key.as_ref()) {
                map.insert(key.to_string(), Value::changed(ValueType::Null));
                self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
            }
        }
        let Value { t, flags, .. } = self;
        let child = match (segment, t) {
            (PathSegment::Key(key), ValueType::Dict(map)) => map.get_mut(key.as_ref()).unwrap(),
            (PathSegment::Key(key), ValueType::Struct(s)) => s.get_mut(key).unwrap(),
            (PathSegment::Index(i), ValueType::List(list)) => match list.get_mut(*i) {
                Some(child) => child,
                None => return false,
            },
            _ => return false,
        };
        let ran = child.update_at_inner(rest, f);
        if child.is_dirty() {
            flags.insert(ChangeFlags::SUBTREE);
        }
        ran
    }

    /// Updates this value to equal `other`, diffing dicts and lists so that only the parts that
    /// differ are marked as changed.
    pub fn sync_from(&mut self, other: &Value) {
        match (&self.t, &other.t) {
            (ValueType::Struct(old), ValueType::Struct(new)) if old.same_schema(new) => {
                for (index, value) in new.fields.iter().enumerate() {
                    self.update_field(index, |child| child.sync_from(value));
                }
            }
            (ValueType::Dict(_), ValueType::Dict(new)) => {
                let removed: Vec<String> = self
                    .as_dict()
                    .unwrap()
                    .keys()
                    .filter(|k| !new.contains_key(*k))
                    .cloned()
                    .collect();
                for key in removed {
                    self.remove(&key);
                }
                for (key, value) in new {
                    let segment = PathSegment::Key(key.clone().into());
                    self.update_at(std::slice::from_ref(&segment), |child| child.sync_from(value));
                }
            }
            (ValueType::List(old), ValueType::List(new)) => {
                let common = old.len().min(new.len());
                for (i, value) in new.iter().enumerate().take(common) {
                    self.update_at(&[PathSegment::Index(i)], |child| child.sync_from(value));
                }
                self.truncate(new.len());
                for value in &new[common..] {
                    self.push(value.clone());
                }
            }
            _ => {
                self.set(other.clone());
            }
        }
    }

    // Mutation

    /// Replaces this value if it differs from `value`, marking it as [`ChangeFlags::SELF`].
    /// Returns whether the value changed.
    pub fn set(&mut self, value: impl Into<Value>) -> bool {
        let value = value.into();
        if self.t == value.t {
            // An equal dict takes the struct form, so later syncs of the struct can diff by field.
            if matches!((&self.t, &value.t), (ValueType::Dict(_), ValueType::Struct(_))) && !self.is_dirty() {
                self.t = value.t;
            }
            return false;
        }
        self.t = value.t;
        self.flags.insert(ChangeFlags::SELF);
        true
    }

    /// Sets the dict entry at `key`. An existing entry is updated with [`Value::set`], a new entry
    /// changes the dict's shape. Returns whether anything changed.
    ///
    /// A `null` value is turned into an empty dict, and a struct missing `key` into a dict.
    /// Panics if this value is any other non-dict.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> bool {
        let key = key.into();
        if let ValueType::Struct(s) = &mut self.t
            && let Some(existing) = s.get_mut(&key)
        {
            let changed = existing.set(value);
            if changed {
                self.flags.insert(ChangeFlags::SUBTREE);
            }
            return changed;
        }
        let map = self.dict_mut_for_insert("insert");
        if let Some(existing) = map.get_mut(&key) {
            let changed = existing.set(value);
            if changed {
                self.flags.insert(ChangeFlags::SUBTREE);
            }
            return changed;
        }
        map.insert(key, Value::changed(value.into().t));
        self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
        true
    }

    /// Removes the dict entry at `key`, changing the dict's shape if it existed. A struct with
    /// `key` is turned into a dict first.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        if self.as_struct().is_some_and(|s| s.position(key).is_some()) {
            self.dict_mut_for_insert("remove");
        }
        let ValueType::Dict(map) = &mut self.t else { return None };
        let removed = map.remove(key)?;
        self.flags.insert(ChangeFlags::SHAPE);
        Some(removed)
    }

    /// Appends an element to this list.
    ///
    /// A `null` value is turned into an empty list. Panics if this value is any other non-list.
    pub fn push(&mut self, value: impl Into<Value>) {
        let value = Value::changed(value.into().t);
        self.list_mut_for_insert("push").push(value);
        self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
    }

    /// Inserts an element into this list at `index`.
    ///
    /// A `null` value is turned into an empty list. Panics if this value is any other non-list, or
    /// if `index > len`.
    pub fn insert_at(&mut self, index: usize, value: impl Into<Value>) {
        let value = Value::changed(value.into().t);
        self.list_mut_for_insert("insert_at").insert(index, value);
        self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
    }

    /// Removes and returns the last element of this list.
    pub fn pop(&mut self) -> Option<Value> {
        let ValueType::List(list) = &mut self.t else { return None };
        let popped = list.pop()?;
        self.flags.insert(ChangeFlags::SHAPE);
        Some(popped)
    }

    /// Removes and returns the element at `index` of this list.
    pub fn remove_at(&mut self, index: usize) -> Option<Value> {
        let ValueType::List(list) = &mut self.t else { return None };
        if index >= list.len() {
            return None;
        }
        let removed = list.remove(index);
        self.flags.insert(ChangeFlags::SHAPE);
        Some(removed)
    }

    /// Shortens this list to `len` elements.
    pub fn truncate(&mut self, len: usize) {
        let ValueType::List(list) = &mut self.t else { return };
        if len < list.len() {
            list.truncate(len);
            self.flags.insert(ChangeFlags::SHAPE);
        }
    }

    /// Swaps two elements of this list. Panics if either index is out of bounds.
    pub fn swap(&mut self, a: usize, b: usize) {
        let ValueType::List(list) = &mut self.t else { return };
        if a != b {
            list.swap(a, b);
            self.flags.insert(ChangeFlags::SHAPE);
        }
    }

    /// Moves the list element at `from` to index `to`, shifting the elements in between, and
    /// changing the list's shape if they differ. Panics if either index is out of bounds.
    pub fn move_element(&mut self, from: usize, to: usize) {
        let ValueType::List(list) = &mut self.t else { return };
        if from == to {
            return;
        }
        if from < to {
            list[from..=to].rotate_left(1);
        } else {
            list[to..=from].rotate_right(1);
        }
        self.flags.insert(ChangeFlags::SHAPE);
    }

    /// Keeps the list elements for which `keep(index, element)` is true, changing the list's shape
    /// if any is removed.
    pub fn retain(&mut self, mut keep: impl FnMut(usize, &Value) -> bool) {
        let ValueType::List(list) = &mut self.t else { return };
        let len = list.len();
        let mut index = 0;
        list.retain(|element| {
            let kept = keep(index, element);
            index += 1;
            kept
        });
        if list.len() != len {
            self.flags.insert(ChangeFlags::SHAPE);
        }
    }

    /// Reorders this list so that element `i` is the element previously at `order[i]`, changing
    /// its shape if the order differs. `order` must be a permutation of the list's indices.
    pub fn permute(&mut self, order: &[usize]) {
        let ValueType::List(list) = &mut self.t else { return };
        assert_eq!(order.len(), list.len(), "Value::permute needs one index per element");
        if order.iter().enumerate().all(|(i, o)| i == *o) {
            return;
        }
        let mut old: Vec<Option<Value>> = std::mem::take(list).into_iter().map(Some).collect();
        *list = order.iter().map(|&i| old[i].take().expect("Value::permute needs a permutation")).collect();
        self.flags.insert(ChangeFlags::SHAPE);
    }

    /// Removes all elements of this list or dict. A struct becomes an empty dict.
    pub fn clear(&mut self) {
        let was_empty = match &mut self.t {
            ValueType::List(list) => std::mem::take(list).is_empty(),
            ValueType::Dict(map) => std::mem::take(map).is_empty(),
            ValueType::Struct(s) => {
                let was_empty = s.is_empty();
                self.t = ValueType::Dict(HashMap::new());
                was_empty
            }
            _ => return,
        };
        if !was_empty {
            self.flags.insert(ChangeFlags::SHAPE);
        }
    }

    // Change tracking

    /// Returns how this node changed since the last [`Value::clear_changes`].
    pub fn flags(&self) -> ChangeFlags {
        self.flags
    }

    /// Returns true if this node or any descendant may have changed.
    pub fn is_dirty(&self) -> bool {
        !self.flags.is_empty()
    }

    /// Clears the change flags of this node and of every dirty descendant. Only branches with
    /// flags set are descended into.
    pub fn clear_changes(&mut self) {
        if self.flags.is_empty() {
            return;
        }
        self.flags = ChangeFlags::NONE;
        match &mut self.t {
            ValueType::List(list) => list.iter_mut().for_each(Value::clear_changes),
            ValueType::Dict(map) => map.values_mut().for_each(Value::clear_changes),
            ValueType::Struct(s) => s.fields.iter_mut().for_each(Value::clear_changes),
            _ => {}
        }
    }

    /// Creates a value already marked as [`ChangeFlags::SELF`], used for new children. It gets a
    /// new element id.
    fn changed(t: ValueType) -> Self {
        Self {
            t,
            flags: ChangeFlags::SELF,
            id: reserve_ids(1),
        }
    }

    fn dict_mut_for_insert(&mut self, op: &str) -> &mut HashMap<String, Value> {
        if self.is_null() {
            self.t = ValueType::Dict(HashMap::new());
            self.flags.insert(ChangeFlags::SELF);
        }
        // Turning a struct into a dict keeps its entries, so it is not a change by itself.
        if let ValueType::Struct(s) = &mut self.t {
            let names = s.names;
            let fields = std::mem::take(&mut s.fields);
            self.t = ValueType::Dict(names.iter().map(|n| n.to_string()).zip(fields).collect());
        }
        match &mut self.t {
            ValueType::Dict(map) => map,
            other => panic!("Value::{op} called on a non-dict value: {other}"),
        }
    }

    fn list_mut_for_insert(&mut self, op: &str) -> &mut Vec<Value> {
        if self.is_null() {
            self.t = ValueType::List(Vec::new());
            self.flags.insert(ChangeFlags::SELF);
        }
        match &mut self.t {
            ValueType::List(list) => list,
            other => panic!("Value::{op} called on a non-list value: {other}"),
        }
    }
}

/// An enum containing all the type variants for attribute values.
#[derive(Clone, Debug, Default)]
pub enum ValueType {
    /// The absence of a value.
    #[default]
    Null,
    /// A boolean value.
    Boolean(bool),
    /// A 64 bits floating-point number.
    Number(f64),
    /// An owned string.
    String(String),
    /// A Bevy entity.
    Entity(Entity),
    /// A dynamically sized list.
    List(Vec<Value>),
    /// A key-value map.
    Dict(HashMap<String, Value>),
    /// A key-value map with a fixed set of keys, used for Rust structs. Equal to a dict with the
    /// same entries.
    Struct(StructValue),
}

impl PartialEq for ValueType {
    fn eq(&self, other: &Self) -> bool {
        use ValueType::*;
        match (self, other) {
            (Null, Null) => true,
            (Boolean(a), Boolean(b)) => a == b,
            (Number(a), Number(b)) => a == b,
            (String(a), String(b)) => a == b,
            (Entity(a), Entity(b)) => a == b,
            (List(a), List(b)) => a == b,
            (Dict(a), Dict(b)) => a == b,
            (Struct(a), Struct(b)) => a == b,
            (Struct(s), Dict(map)) | (Dict(map), Struct(s)) => {
                s.len() == map.len() && s.iter().all(|(k, v)| map.get(k) == Some(v))
            }
            _ => false,
        }
    }
}

/// The data of a [`ValueType::Struct`]: field names shared by every value of a Rust type, and the
/// value of each field.
#[derive(Clone)]
pub struct StructValue {
    names: &'static [&'static str],
    fields: Vec<Value>,
}

impl StructValue {
    /// Panics if `names` and `fields` have different lengths.
    pub fn new(names: &'static [&'static str], fields: Vec<Value>) -> Self {
        assert_eq!(names.len(), fields.len(), "a struct value needs one field per name");
        Self { names, fields }
    }

    pub fn names(&self) -> &'static [&'static str] {
        self.names
    }

    pub fn fields(&self) -> &[Value] {
        &self.fields
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The index of field `name`.
    pub fn position(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| *n == name)
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.position(name).map(|i| &self.fields[i])
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.position(name).map(|i| &mut self.fields[i])
    }

    /// Iterates the fields with their names, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &Value)> {
        self.names.iter().copied().zip(self.fields.iter())
    }

    /// Returns true if `other` has the same field names, in the same order.
    pub fn same_schema(&self, other: &StructValue) -> bool {
        std::ptr::eq(self.names, other.names) || self.names == other.names
    }
}

impl PartialEq for StructValue {
    fn eq(&self, other: &Self) -> bool {
        if self.same_schema(other) {
            self.fields == other.fields
        } else {
            self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
        }
    }
}

impl fmt::Debug for StructValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// An iterator over the entries of a dict or struct, see [`Value::entries`].
pub enum Entries<'a> {
    Dict(bevy::platform::collections::hash_map::Iter<'a, String, Value>),
    Struct(std::iter::Zip<std::slice::Iter<'a, &'static str>, std::slice::Iter<'a, Value>>),
}

impl<'a> Iterator for Entries<'a> {
    type Item = (&'a str, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Entries::Dict(iter) => iter.next().map(|(k, v)| (k.as_str(), v)),
            Entries::Struct(iter) => iter.next().map(|(k, v)| (*k, v)),
        }
    }
}
impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueType::Null => { f.write_str("null")?; },
            ValueType::Boolean(b) => { fmt::Debug::fmt(b, f)?; },
            ValueType::Number(n) => { fmt::Debug::fmt(n, f)?; },
            ValueType::String(s) => { fmt::Debug::fmt(s, f)?; },
            ValueType::Entity(e) => { fmt::Display::fmt(e, f)?; },
            ValueType::List(values) => {
                f.write_str("[")?;
                for (i, el) in values.iter().enumerate() {
                    fmt::Display::fmt(el, f)?;
                    if i + 1 < values.len() {
                        f.write_str(", ")?;
                    }
                }
                f.write_str("]")?;
            }
            ValueType::Dict(_) | ValueType::Struct(_) => {
                let (len, entries) = match self {
                    ValueType::Dict(map) => (map.len(), Entries::Dict(map.iter())),
                    ValueType::Struct(s) => (s.len(), Entries::Struct(s.names.iter().zip(s.fields.iter()))),
                    _ => unreachable!(),
                };
                f.write_str("{")?;
                for (i, (key, value)) in entries.enumerate() {
                    fmt::Debug::fmt(key, f)?;
                    f.write_str(": ")?;
                    fmt::Display::fmt(value, f)?;
                    if i + 1 < len {
                        f.write_str(", ")?;
                    }
                }
                f.write_str("}")?;
            },
        }

        Ok(())
    }
}


impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::boolean(value)
    }
}

macro_rules! impl_from_number {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for Value {
                fn from(value: $ty) -> Self {
                    Self::number(value as f64)
                }
            }
        )*
    };
}
impl_from_number!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Self::null()
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::string(value.to_owned())
    }
}
impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::string(value.clone())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::string(value)
    }
}
impl From<Entity> for Value {
    fn from(value: Entity) -> Self {
        Self::entity(value)
    }
}
impl From<ValueType> for Value {
    fn from(value: ValueType) -> Self {
        Self::new(value)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or_default()
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(value: Vec<T>) -> Self {
        Value::list(value.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Value>> From<HashMap<String, T>> for Value {
    fn from(value: HashMap<String, T>) -> Self {
        Value::dict(value.into_iter().map(|(k, v)| (k, v.into())).collect())
    }
}
impl<T: Into<Value>> From<std::collections::HashMap<String, T>> for Value {
    fn from(value: std::collections::HashMap<String, T>) -> Self {
        Value::dict(value.into_iter().map(|(k, v)| (k, v.into())).collect())
    }
}

// Macros

/// Hash map macro for bevy HashMap.
#[macro_export]
macro_rules! hash_map {
    { $( $key:expr => $value:expr ),* $(,)? } => {{
        #[allow(unused_mut)]
        let mut map = bevy::platform::collections::HashMap::new();
        $(
            $crate::hash_map!(@insert map, $key, $value);
        )*
        map
    }};

    (@insert $map:ident, $key:expr, $value:expr) => {
        $map.insert($key, $value);
    };
}


/// A macro for creating a Value.
#[macro_export]
macro_rules! value {
    // list

    ( [ $($tt:tt),* $(,)? ] ) => {{
        #[allow(unused_mut)]
        let mut elements = Vec::<$crate::core::value::Value>::new();
        $(
            $crate::value!(@push elements, $crate::value!($tt));
        )*
        $crate::core::value::Value::list(elements)
    }};

    (@push $elements:ident, $value:expr) => {
        $elements.push($value);
    };

    // dict

    ( { $( $key:tt : $value:tt ),* $(,)? } ) => {{
        #[allow(unused_mut)]
        let mut map = bevy::platform::collections::HashMap::<String, $crate::core::value::Value>::new();
        $(
            $crate::value!(@insert map, $key, $crate::value!($value));
        )*
        $crate::core::value::Value::dict(map)
    }};

    (@insert $map:ident, $key:literal, $value:expr) => {
        $map.insert($key.to_owned(), $value);
    };

    (@insert $map:ident, $key:expr, $value:expr) => {
        $map.insert($key.to_owned(), $value);
    };

    // others

    ($expr:expr) => {
        $crate::core::value::Value::from($expr)
    };
}



#[cfg(test)]
mod tests {
    use bevy::platform::collections::HashMap;

    use super::{ChangeFlags, Value, ValueType};
    use crate::core::path::PropPath;

    fn four() -> i64 {
        4
    }

    #[test]
    fn literal() {
        let a = 40;

        let values = value!([
            true,
            false,
            "hello",
            {
                "a": [1, 2, 3, a, (four())],
            },
            45.0,
        ]);

        let base = Value::list(vec![
            Value::boolean(true),
            Value::boolean(false),
            Value::string("hello".to_owned()),
            Value::dict(hash_map! {
                "a".to_owned() => Value::list(vec![
                    Value::number(1.0),
                    Value::number(2.0),
                    Value::number(3.0),
                    Value::number(a as f64),
                    Value::number(four() as f64),
                ])
            }),
            Value::number(45.0)
        ]);
        assert_eq!(values, base);
    }

    struct Stack {
        id: String,
        count: u32,
    }
    impl From<Stack> for Value {
        fn from(value: Stack) -> Self {
            value!({
                "id": (value.id),
                "count": (value.count)
            })
        }
    }

    #[test]
    fn custom() {
        let values = value!([
            (Stack { id: "iron_pickaxe".to_owned(), count: 1 }),
            (Stack { id: "coal".to_owned(), count: 10 }),
            (Stack { id: "planks".to_owned(), count: 32 }),
        ]);
        println!("{values}");

        let base = Value::list(vec![
            Value::dict(hash_map! {
                "id".to_owned() => Value::string("iron_pickaxe".to_owned()),
                "count".to_owned() => Value::number(1.0),
            }),
            Value::dict(hash_map! {
                "id".to_owned() => Value::string("coal".to_owned()),
                "count".to_owned() => Value::number(10.0),
            }),
            Value::dict(hash_map! {
                "id".to_owned() => Value::string("planks".to_owned()),
                "count".to_owned() => Value::number(32.0),
            }),
        ]);

        assert_eq!(values, base);
    }

    #[test]
    fn vector() {
        let vector = vec![1,2,3,4,5];
        let values = value!(vector);
        println!("{values}");

        let base = Value::list(vec![
            Value::number(1.0),
            Value::number(2.0),
            Value::number(3.0),
            Value::number(4.0),
            Value::number(5.0),
        ]);
        assert_eq!(values, base);
    }

    #[test]
    fn hash_map() {
        let map: HashMap<String, Vec<i32>> = hash_map! {
            "a".to_owned() => vec![1,2,3],
            "b".to_owned() => vec![4,5,6],
            "c".to_owned() => vec![7,8,9],
        };
        let values = value!(map);
        println!("{values}");

        let base = Value::dict(hash_map! {
            "a".to_string() => Value::list(vec![Value::number(1.0),Value::number(2.0),Value::number(3.0)]),
            "b".to_string() => Value::list(vec![Value::number(4.0),Value::number(5.0),Value::number(6.0)]),
            "c".to_string() => Value::list(vec![Value::number(7.0),Value::number(8.0),Value::number(9.0)]),
        });
        assert_eq!(values, base);
    }

    fn sample() -> Value {
        value!({
            "user": { "name": "A", "age": 30 },
            "items": [{ "label": "x" }, { "label": "y" }],
        })
    }

    #[test]
    fn fresh_values_are_clean() {
        let value = sample();
        assert!(!value.is_dirty());
        assert!(!value.at(&PropPath::parse("items[1].label").unwrap()).unwrap().is_dirty());
    }

    #[test]
    fn set_marks_only_the_changed_leaf() {
        let mut value = sample();

        // Setting an equal value is not a change.
        assert!(!value.get_mut("user").unwrap().get_mut("name").unwrap().set("A"));
        value.clear_changes();
        assert!(!value.is_dirty());

        assert!(value.get_mut("user").unwrap().get_mut("name").unwrap().set("B"));
        assert_eq!(value.flags(), ChangeFlags::SUBTREE);
        let user = value.get("user").unwrap();
        assert_eq!(user.flags(), ChangeFlags::SUBTREE);
        assert_eq!(user.get("name").unwrap().flags(), ChangeFlags::SELF);
        assert!(!user.get("age").unwrap().is_dirty());
        assert!(!value.get("items").unwrap().is_dirty());
    }

    #[test]
    fn at_mut_walks_deep_paths() {
        let mut value = sample();
        let path = PropPath::parse("items[1].label").unwrap();
        value.at_mut(&path).unwrap().set("z");

        assert_eq!(value.at(&path).unwrap().as_string().unwrap(), "z");
        let items = value.get("items").unwrap();
        assert_eq!(items.flags(), ChangeFlags::SUBTREE);
        assert!(!items.index(0).unwrap().is_dirty());
        assert_eq!(items.index(1).unwrap().flags(), ChangeFlags::SUBTREE);
        assert!(value.at_mut(&PropPath::parse("items[5].label").unwrap()).is_none());
    }

    #[test]
    fn deref_mut_marks_self() {
        let mut value = sample();
        let name = value.get_mut("user").unwrap().get_mut("name").unwrap();
        *name.deref_mut_for_test() = ValueType::String("C".into());
        assert_eq!(
            value.get("user").unwrap().get("name").unwrap().flags(),
            ChangeFlags::SELF
        );
    }

    #[test]
    fn shape_mutators() {
        let mut value = sample();
        let items = value.get_mut("items").unwrap();
        items.push(value!({ "label": "w" }));
        assert!(items.flags().contains(ChangeFlags::SHAPE));
        assert_eq!(items.index(2).unwrap().flags(), ChangeFlags::SELF);
        assert!(!items.index(0).unwrap().is_dirty());
        value.clear_changes();

        let items = value.get_mut("items").unwrap();
        items.remove_at(0);
        assert_eq!(items.flags(), ChangeFlags::SHAPE);
        assert_eq!(items.as_list().unwrap().len(), 2);
        value.clear_changes();

        // Updating an existing key is a child change, adding a key is a shape change.
        let user = value.get_mut("user").unwrap();
        assert!(user.insert("age", 31));
        assert_eq!(user.flags(), ChangeFlags::SUBTREE);
        assert!(!user.insert("age", 31));
        assert!(user.insert("email", "a@b.c"));
        assert!(user.flags().contains(ChangeFlags::SHAPE));
        assert!(user.remove("email").is_some());
        value.clear_changes();

        let mut null = Value::null();
        null.entry("a").push(1);
        assert_eq!(null, value!({ "a": [1] }));
    }

    #[test]
    fn clear_changes_resets_all_flags() {
        let mut value = sample();
        value.at_mut(&PropPath::parse("items[0].label").unwrap()).unwrap().set("q");
        value.get_mut("user").unwrap().insert("x", 1);
        value.clear_changes();

        assert!(!value.is_dirty());
        assert!(!value.get("items").unwrap().is_dirty());
        assert!(!value.at(&PropPath::parse("items[0].label").unwrap()).unwrap().is_dirty());
        assert!(!value.at(&PropPath::parse("user.x").unwrap()).unwrap().is_dirty());
    }

    #[test]
    fn equality_ignores_flags_and_clone_is_clean() {
        let mut a = sample();
        a.get_mut("user").unwrap().get_mut("name").unwrap().set("B");
        let mut b = sample();
        b.get_mut("user").unwrap().insert("name", "B");
        assert_eq!(a, b);
        assert!(!a.clone().is_dirty());
    }

    #[test]
    fn text_and_truthiness() {
        assert_eq!(Value::from("hi").to_text(), "hi");
        assert_eq!(Value::from(3.0).to_text(), "3");
        assert_eq!(Value::from(2.5).to_text(), "2.5");
        assert_eq!(Value::null().to_text(), "");
        assert!(!Value::from(0).is_truthy());
        assert!(!Value::from("").is_truthy());
        assert!(!Value::null().is_truthy());
        assert!(!Value::list(vec![]).is_truthy());
        assert!(Value::list(vec![Value::from(0)]).is_truthy());
        assert!(!Value::dict(HashMap::new()).is_truthy());
        assert!(Value::dict(HashMap::from([("a".to_owned(), Value::null())])).is_truthy());
        assert!(Value::from(Some(1)).is_truthy());
        assert!(!Value::from(None::<i32>).is_truthy());
    }

    const POINT: &[&str] = &["x", "y"];

    fn point(x: i32, y: i32) -> Value {
        Value::structure(POINT, vec![Value::from(x), Value::from(y)])
    }

    #[test]
    fn struct_reads_like_a_dict() {
        let p = point(1, 2);
        assert_eq!(p.get("y"), Some(&Value::from(2)));
        assert_eq!(p.get("z"), None);
        assert!(p.is_map());
        assert!(p.as_dict().is_none());
        assert_eq!(p.entries().unwrap().map(|(k, _)| k).collect::<Vec<_>>(), vec!["x", "y"]);
        assert_eq!(p.to_string(), r#"{"x": 1.0, "y": 2.0}"#);
        assert_eq!(p, value!({ "x": 1, "y": 2 }));
        assert_eq!(value!({ "y": 2, "x": 1 }), p);
        assert_ne!(p, value!({ "x": 1 }));
        assert_ne!(p, point(1, 3));
        assert!(p.is_truthy());
        assert!(!Value::structure(&[], vec![]).is_truthy());
    }

    #[test]
    fn struct_mutation() {
        let mut p = point(1, 2);
        assert!(!p.insert("x", 1));
        assert!(p.insert("x", 5));
        assert_eq!(p.flags(), ChangeFlags::SUBTREE);
        assert!(p.as_struct().is_some());
        p.clear_changes();

        p.get_mut("y").unwrap().set(7);
        assert_eq!(p.get("y").unwrap().flags(), ChangeFlags::SELF);
        p.clear_changes();

        // Unknown keys turn the struct into a dict with the same entries.
        assert!(p.insert("z", 0));
        assert!(p.as_dict().is_some());
        assert!(p.flags().contains(ChangeFlags::SHAPE));
        assert_eq!(p, value!({ "x": 5, "y": 7, "z": 0 }));

        let mut p = point(1, 2);
        assert_eq!(p.remove("x"), Some(Value::from(1)));
        assert_eq!(p, value!({ "y": 2 }));
    }

    #[test]
    fn struct_sync_and_update_field() {
        let mut p = point(1, 2);
        p.sync_from(&point(1, 3));
        assert_eq!(p.flags(), ChangeFlags::SUBTREE);
        assert!(!p.get("x").unwrap().is_dirty());
        assert_eq!(p.get("y").unwrap().flags(), ChangeFlags::SELF);
        p.clear_changes();

        p.update_field(0, |x| {
            x.set(1);
        });
        assert!(!p.is_dirty());
        p.update_field(0, |x| {
            x.set(9);
        });
        assert_eq!(p.flags(), ChangeFlags::SUBTREE);

        // An equal dict takes the struct form without a change.
        let mut d = value!({ "x": 1, "y": 2 });
        assert!(!d.set(point(1, 2)));
        assert!(d.as_struct().is_some());
        assert!(!d.is_dirty());
    }

    #[test]
    fn element_ids() {
        let mut list = value!([1, 2, 3]);
        let ids: Vec<u32> = list.as_list().unwrap().iter().map(Value::element_id).collect();
        assert!(ids.iter().all(|id| *id != 0));
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 3);

        // Ids follow elements when the list is reordered, and are kept by clones.
        list.permute(&[2, 0, 1]);
        let moved: Vec<u32> = list.as_list().unwrap().iter().map(Value::element_id).collect();
        assert_eq!(moved, vec![ids[2], ids[0], ids[1]]);
        assert_eq!(list.clone().as_list().unwrap()[0].element_id(), ids[2]);

        // New elements get new ids, also when re-inserting a removed element.
        let removed = list.remove_at(0).unwrap();
        list.insert_at(1, removed);
        list.push(4);
        let after: Vec<u32> = list.as_list().unwrap().iter().map(Value::element_id).collect();
        assert_eq!(after[0], ids[0]);
        assert!(!ids.contains(&after[1]) && !ids.contains(&after[3]));

        // Ids are not data.
        assert_eq!(list, value!([1, 3, 2, 4]));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn element_ids_take_no_space() {
        assert_eq!(std::mem::size_of::<Value>(), std::mem::size_of::<(ValueType, u64)>());
    }

    #[test]
    fn entity() {
        let entity = bevy::ecs::entity::Entity::from_raw_u32(12).unwrap();
        let value = Value::from(entity);
        assert_eq!(value.as_entity(), Some(entity));
        assert_eq!(Value::from(12).as_entity(), None);
        assert!(value.is_truthy());
        assert_eq!(value.to_text(), entity.to_string());
        assert_eq!(value.to_string(), entity.to_string());
        assert_eq!(value, Value::entity(entity));
        assert_ne!(value, Value::entity(bevy::ecs::entity::Entity::from_raw_u32(13).unwrap()));
    }

    impl Value {
        fn deref_mut_for_test(&mut self) -> &mut ValueType {
            std::ops::DerefMut::deref_mut(self)
        }
    }
}
