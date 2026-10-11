//! Proxied property values for the API.
//!
//! A [`Value`] is a dynamically typed tree (booleans, numbers, strings, lists and dicts) that
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

use std::{fmt, ops::DerefMut};

use bevy::platform::collections::HashMap;

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
}

impl Clone for Value {
    /// Clones the data. The clone starts with no change flags.
    fn clone(&self) -> Self {
        Self {
            t: self.t.clone(),
            flags: ChangeFlags::NONE,
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
    pub const NULL: Value = Value { t: ValueType::Null, flags: ChangeFlags::NONE };

    fn new(t: ValueType) -> Self {
        Self {
            t,
            flags: ChangeFlags::NONE,
        }
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
    /// Creates a list value.
    pub fn list(value: Vec<Value>) -> Self {
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

    /// Unwraps the list value if value is `List`.
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match &self.t { ValueType::List(v) => Some(v), _ => None }
    }

    /// Unwraps the dict value if value is `Dict`.
    pub fn as_dict(&self) -> Option<&HashMap<String, Value>> {
        match &self.t { ValueType::Dict(v) => Some(v), _ => None }
    }

    /// Truthiness used by `v-if`: `false`, `0`, `NaN`, `""`, `null` and empty lists and dicts are
    /// falsy, everything else is truthy.
    pub fn is_truthy(&self) -> bool {
        match &self.t {
            ValueType::Null => false,
            ValueType::Boolean(b) => *b,
            ValueType::Number(n) => *n != 0.0 && !n.is_nan(),
            ValueType::String(s) => !s.is_empty(),
            ValueType::List(list) => !list.is_empty(),
            ValueType::Dict(map) => !map.is_empty(),
        }
    }

    /// Renders this value as interpolated text: strings are written raw and `null` is empty.
    pub fn to_text(&self) -> String {
        match &self.t {
            ValueType::Null => String::new(),
            ValueType::Boolean(b) => b.to_string(),
            ValueType::Number(n) => n.to_string(),
            ValueType::String(s) => s.clone(),
            ValueType::List(_) | ValueType::Dict(_) => self.to_string(),
        }
    }

    // Navigation

    /// Gets the dict entry at `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict()?.get(key)
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

    /// Gets the dict entry at `key` for mutation, marking this node as [`ChangeFlags::SUBTREE`].
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        let ValueType::Dict(map) = &mut self.t else { return None };
        let child = map.get_mut(key)?;
        self.flags.insert(ChangeFlags::SUBTREE);
        Some(child)
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
    /// A `null` value is turned into an empty dict. Panics if this value is any other non-dict.
    pub fn entry(&mut self, key: &str) -> &mut Value {
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
        if let PathSegment::Key(key) = segment {
            let map = self.dict_mut_for_insert("update_at");
            if !map.contains_key(key.as_ref()) {
                map.insert(key.to_string(), Value::changed(ValueType::Null));
                self.flags.insert(ChangeFlags::SHAPE | ChangeFlags::SUBTREE);
            }
        }
        let Value { t, flags } = self;
        let child = match (segment, t) {
            (PathSegment::Key(key), ValueType::Dict(map)) => map.get_mut(key.as_ref()).unwrap(),
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
            return false;
        }
        self.t = value.t;
        self.flags.insert(ChangeFlags::SELF);
        true
    }

    /// Sets the dict entry at `key`. An existing entry is updated with [`Value::set`], a new entry
    /// changes the dict's shape. Returns whether anything changed.
    ///
    /// A `null` value is turned into an empty dict. Panics if this value is any other non-dict.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> bool {
        let key = key.into();
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

    /// Removes the dict entry at `key`, changing the dict's shape if it existed.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
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

    /// Removes all elements of this list or dict.
    pub fn clear(&mut self) {
        let was_empty = match &mut self.t {
            ValueType::List(list) => std::mem::take(list).is_empty(),
            ValueType::Dict(map) => std::mem::take(map).is_empty(),
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
            _ => {}
        }
    }

    /// Creates a value already marked as [`ChangeFlags::SELF`], used for new children.
    fn changed(t: ValueType) -> Self {
        Self {
            t,
            flags: ChangeFlags::SELF,
        }
    }

    fn dict_mut_for_insert(&mut self, op: &str) -> &mut HashMap<String, Value> {
        if self.is_null() {
            self.t = ValueType::Dict(HashMap::new());
            self.flags.insert(ChangeFlags::SELF);
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
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// A dynamically sized list.
    List(Vec<Value>),
    /// A key-value map.
    Dict(HashMap<String, Value>),
}
impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueType::Null => { f.write_str("null")?; },
            ValueType::Boolean(b) => { fmt::Debug::fmt(b, f)?; },
            ValueType::Number(n) => { fmt::Debug::fmt(n, f)?; },
            ValueType::String(s) => { fmt::Debug::fmt(s, f)?; },
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
            ValueType::Dict(map) => {
                f.write_str("{")?;
                for (i, (key, value)) in map.iter().enumerate() {
                    fmt::Debug::fmt(key, f)?;
                    f.write_str(": ")?;
                    fmt::Display::fmt(value, f)?;
                    if i + 1 < map.len() {
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

    impl Value {
        fn deref_mut_for_test(&mut self) -> &mut ValueType {
            std::ops::DerefMut::deref_mut(self)
        }
    }
}
