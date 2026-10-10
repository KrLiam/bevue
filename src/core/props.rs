//! The [`Props`] trait: Rust data that can be rendered by a template.
//!
//! A `Props` type converts to a [`Value`] (structs become dicts), can sync itself into an existing
//! value by diffing so that only changed leaves are marked, and supports Reflect-like access to
//! nested members by path, e.g. `inventory.slots[0].count`.
//!
//! Derive it with `#[derive(Props)]`, which supports structs with named fields (dicts), tuple
//! structs (transparent with one field, lists with more) and enums with only unit variants
//! (strings). Fields can be renamed with
//! `#[props(rename = "name")]` or left out with `#[props(skip)]`.
//!
//! A `Props` type also has a [`Props::name`], the key it is mounted at in template props by
//! default: the type name in snake case, or `#[props(name = "name")]` on the type.

use std::{any::Any, borrow::Cow, fmt};

use bevy::{platform::collections::HashMap, reflect::utility::GenericTypePathCell};

use crate::core::{
    path::{AsPropPath, PathParseError, PathSegment, PropPath},
    value::{Value, ValueType},
};

/// The shape of a [`Props`] value, used in error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropsKind {
    Leaf,
    Struct,
    List,
    Map,
}

/// Rust data that can be used as template props.
pub trait Props: Send + Sync + 'static {
    /// Converts this data into a [`Value`].
    fn to_value(&self) -> Value;

    /// Updates `target` to match this data, marking only the parts that changed. The default
    /// replaces `target` if it differs.
    fn sync_value(&self, target: &mut Value) {
        target.set(self.to_value());
    }

    /// Updates this data from `value`.
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError>;

    /// Creates this data from `value`.
    fn from_value(value: &Value) -> Result<Self, PropsError>
    where
        Self: Sized;

    fn kind(&self) -> PropsKind {
        PropsKind::Leaf
    }

    /// Gets a named member: a struct field or a map entry.
    fn field(&self, _name: &str) -> Option<&dyn Props> {
        None
    }

    fn field_mut(&mut self, _name: &str) -> Option<&mut dyn Props> {
        None
    }

    /// Gets an indexed member: a list element or a tuple struct field.
    fn element(&self, _index: usize) -> Option<&dyn Props> {
        None
    }

    fn element_mut(&mut self, _index: usize) -> Option<&mut dyn Props> {
        None
    }

    /// The number of indexed members, used in error messages.
    fn element_count(&self) -> usize {
        0
    }

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// The key this type is mounted at in template props by default, e.g. `player_inventory` for
    /// `PlayerInventory`. `#[derive(Props)]` sets it from the type name or from
    /// `#[props(name = "...")]`.
    fn name() -> &'static str
    where
        Self: Sized,
    {
        static CELL: GenericTypePathCell = GenericTypePathCell::new();
        CELL.get_or_insert::<Self, _>(|| {
            let short = short_type_name(std::any::type_name::<Self>());
            let ident = short.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or_default();
            to_snake_case(ident)
        })
    }
}

/// Removes the module paths from a type name: `a::B<c::D>` becomes `B<D>`.
pub fn short_type_name(full: &str) -> String {
    let mut out = String::new();
    // Where the path segment being read starts in `out`.
    let mut segment = 0;
    let mut chars = full.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            out.truncate(segment);
        } else {
            out.push(c);
            if !c.is_alphanumeric() && c != '_' {
                segment = out.len();
            }
        }
    }
    out
}

/// Converts a type name to snake case: `PlayerInventory` becomes `player_inventory`.
pub fn to_snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let previous = i.checked_sub(1).map(|p| chars[p]);
        let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let boundary = previous.is_some_and(|p| p.is_lowercase() || p.is_numeric() || (p.is_uppercase() && next_is_lower));
        if c.is_uppercase() && boundary {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

impl dyn Props {
    /// Gets the member at `path`, e.g. `"a.b[0].c"`.
    pub fn path(&self, path: impl AsPropPath) -> Result<&dyn Props, PathError> {
        let path = path.as_prop_path()?;
        let mut current: &dyn Props = self;
        for (i, segment) in path.iter().enumerate() {
            current = step(current, segment).ok_or_else(|| PathError::missing(current, &path, i))?;
        }
        Ok(current)
    }

    /// Gets the member at `path` for mutation.
    pub fn path_mut(&mut self, path: impl AsPropPath) -> Result<&mut dyn Props, PathError> {
        let path = path.as_prop_path()?;
        let mut current: &mut dyn Props = self;
        for (i, segment) in path.iter().enumerate() {
            if step(current, segment).is_none() {
                return Err(PathError::missing(current, &path, i));
            }
            current = step_mut(current, segment).unwrap();
        }
        Ok(current)
    }

    /// Gets the member at `path` as a `T`.
    pub fn get_path<T: Props>(&self, path: impl AsPropPath) -> Result<&T, PathError> {
        let path = path.as_prop_path()?;
        self.path(&*path)?
            .as_any()
            .downcast_ref::<T>()
            .ok_or_else(|| PathError::TypeMismatch { at: path.to_string(), expected: std::any::type_name::<T>() })
    }

    /// Gets the member at `path` as a `T` for mutation.
    pub fn get_path_mut<T: Props>(&mut self, path: impl AsPropPath) -> Result<&mut T, PathError> {
        let path = path.as_prop_path()?;
        self.path_mut(&*path)?
            .as_any_mut()
            .downcast_mut::<T>()
            .ok_or_else(|| PathError::TypeMismatch { at: path.to_string(), expected: std::any::type_name::<T>() })
    }

    /// Sets the member at `path` from a value.
    pub fn set_path(&mut self, path: impl AsPropPath, value: impl Into<Value>) -> Result<(), PathError> {
        let path = path.as_prop_path()?;
        self.path_mut(&*path)?
            .apply_value(&value.into())
            .map_err(|error| PathError::Apply { at: path.to_string(), error })
    }
}

fn step<'a>(props: &'a dyn Props, segment: &PathSegment) -> Option<&'a dyn Props> {
    match segment {
        PathSegment::Key(key) => props.field(key),
        PathSegment::Index(i) => props.element(*i),
    }
}

fn step_mut<'a>(props: &'a mut dyn Props, segment: &PathSegment) -> Option<&'a mut dyn Props> {
    match segment {
        PathSegment::Key(key) => props.field_mut(key),
        PathSegment::Index(i) => props.element_mut(*i),
    }
}

/// Path access for any [`Props`] type, e.g. `inventory.get_path::<u32>("slots[0].count")`.
pub trait PropsExt: Props {
    fn path(&self, path: impl AsPropPath) -> Result<&dyn Props, PathError>;
    fn path_mut(&mut self, path: impl AsPropPath) -> Result<&mut dyn Props, PathError>;
    fn get_path<T: Props>(&self, path: impl AsPropPath) -> Result<&T, PathError>;
    fn get_path_mut<T: Props>(&mut self, path: impl AsPropPath) -> Result<&mut T, PathError>;
    fn set_path(&mut self, path: impl AsPropPath, value: impl Into<Value>) -> Result<(), PathError>;
}

impl<P: Props> PropsExt for P {
    fn path(&self, path: impl AsPropPath) -> Result<&dyn Props, PathError> {
        (self as &dyn Props).path(path)
    }
    fn path_mut(&mut self, path: impl AsPropPath) -> Result<&mut dyn Props, PathError> {
        (self as &mut dyn Props).path_mut(path)
    }
    fn get_path<T: Props>(&self, path: impl AsPropPath) -> Result<&T, PathError> {
        (self as &dyn Props).get_path(path)
    }
    fn get_path_mut<T: Props>(&mut self, path: impl AsPropPath) -> Result<&mut T, PathError> {
        (self as &mut dyn Props).get_path_mut(path)
    }
    fn set_path(&mut self, path: impl AsPropPath, value: impl Into<Value>) -> Result<(), PathError> {
        (self as &mut dyn Props).set_path(path, value)
    }
}

/// Syncs `field` into the dict entry `name` of `target`, marking `target` only if it changed.
/// Used by `#[derive(Props)]`.
pub fn sync_field(target: &mut Value, name: &'static str, field: &dyn Props) {
    target.update_at(&[PathSegment::Key(Cow::Borrowed(name))], |child| field.sync_value(child));
}

/// Syncs `element` into index `index` of `target`, marking `target` only if it changed.
pub fn sync_element(target: &mut Value, index: usize, element: &dyn Props) {
    target.update_at(&[PathSegment::Index(index)], |child| element.sync_value(child));
}

/// Gets the dict entry `name` of `value`, for `#[derive(Props)]`.
pub fn require_field<'a>(value: &'a Value, name: &'static str) -> Result<&'a Value, PropsError> {
    let dict = value.as_dict().ok_or_else(|| PropsError::expected("dict", value))?;
    dict.get(name).ok_or(PropsError::MissingField(name))
}

// Errors

/// An error converting a [`Value`] into Rust data.
#[derive(Debug, Clone, PartialEq)]
pub enum PropsError {
    TypeMismatch { expected: &'static str, found: String },
    MissingField(&'static str),
    UnknownVariant(String),
    ReadOnly,
    Field { name: String, error: Box<PropsError> },
}

impl PropsError {
    pub fn expected(expected: &'static str, found: &Value) -> Self {
        PropsError::TypeMismatch { expected, found: found.to_string() }
    }

    /// Wraps an error with the field it happened in.
    pub fn in_field(self, name: impl Into<String>) -> Self {
        PropsError::Field { name: name.into(), error: Box::new(self) }
    }
}

impl fmt::Display for PropsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropsError::TypeMismatch { expected, found } => write!(f, "expected {expected}, found {found}"),
            PropsError::MissingField(name) => write!(f, "missing field '{name}'"),
            PropsError::UnknownVariant(name) => write!(f, "unknown variant '{name}'"),
            PropsError::ReadOnly => f.write_str("value is read-only"),
            PropsError::Field { name, error } => write!(f, "{name}: {error}"),
        }
    }
}

impl std::error::Error for PropsError {}

/// An error accessing a member by path.
#[derive(Debug, Clone, PartialEq)]
pub enum PathError {
    Parse(PathParseError),
    /// The struct or map at `at` has no member `name`.
    NoField { at: String, name: String },
    /// The list at `at` has no element `index`.
    IndexOutOfBounds { at: String, index: usize, len: usize },
    /// The value at `at` cannot be indexed by the segment that followed.
    WrongKind { at: String, kind: PropsKind, segment: String },
    TypeMismatch { at: String, expected: &'static str },
    Apply { at: String, error: PropsError },
}

impl PathError {
    fn missing(props: &dyn Props, path: &PropPath, i: usize) -> Self {
        let at = PropPath::new(path[..i].iter().cloned()).to_string();
        match (&path[i], props.kind()) {
            (PathSegment::Key(name), PropsKind::Struct | PropsKind::Map) => {
                PathError::NoField { at, name: name.to_string() }
            }
            (PathSegment::Index(index), PropsKind::List) => {
                PathError::IndexOutOfBounds { at, index: *index, len: props.element_count() }
            }
            (segment, kind) => PathError::WrongKind { at, kind, segment: segment.to_string() },
        }
    }
}

impl From<PathParseError> for PathError {
    fn from(value: PathParseError) -> Self {
        PathError::Parse(value)
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = |at: &str| if at.is_empty() { "<root>".to_owned() } else { format!("'{at}'") };
        match self {
            PathError::Parse(e) => e.fmt(f),
            PathError::NoField { at: a, name } => write!(f, "{} has no field '{name}'", at(a)),
            PathError::IndexOutOfBounds { at: a, index, len } => {
                write!(f, "index {index} is out of bounds for {} of length {len}", at(a))
            }
            PathError::WrongKind { at: a, kind, segment } => {
                write!(f, "{} is a {kind:?} and cannot be accessed with '{segment}'", at(a))
            }
            PathError::TypeMismatch { at: a, expected } => write!(f, "{} is not a {expected}", at(a)),
            PathError::Apply { at: a, error } => write!(f, "cannot set {}: {error}", at(a)),
        }
    }
}

impl std::error::Error for PathError {}

// Built-in impls

macro_rules! leaf_any {
    () => {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    };
}

impl Props for bool {
    fn to_value(&self) -> Value {
        Value::boolean(*self)
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        *self = Self::from_value(value)?;
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        value.as_bool().ok_or_else(|| PropsError::expected("bool", value))
    }
    leaf_any!();
}

macro_rules! impl_number {
    ($($ty:ty),*) => {
        $(
            impl Props for $ty {
                fn to_value(&self) -> Value {
                    Value::number(*self as f64)
                }
                fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
                    *self = Self::from_value(value)?;
                    Ok(())
                }
                fn from_value(value: &Value) -> Result<Self, PropsError> {
                    value.as_number().map(|n| n as $ty).ok_or_else(|| PropsError::expected("number", value))
                }
                leaf_any!();
            }
        )*
    };
}
impl_number!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

impl Props for String {
    fn to_value(&self) -> Value {
        Value::string(self.clone())
    }
    fn sync_value(&self, target: &mut Value) {
        if target.as_string() != Some(self) {
            target.set(self.as_str());
        }
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        *self = Self::from_value(value)?;
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        value.as_string().cloned().ok_or_else(|| PropsError::expected("string", value))
    }
    leaf_any!();
}

impl Props for &'static str {
    fn to_value(&self) -> Value {
        Value::from(*self)
    }
    fn sync_value(&self, target: &mut Value) {
        if target.as_string().map(String::as_str) != Some(*self) {
            target.set(*self);
        }
    }
    fn apply_value(&mut self, _value: &Value) -> Result<(), PropsError> {
        Err(PropsError::ReadOnly)
    }
    fn from_value(_value: &Value) -> Result<Self, PropsError> {
        Err(PropsError::ReadOnly)
    }
    leaf_any!();
}

impl<T: Props> Props for Option<T> {
    fn to_value(&self) -> Value {
        self.as_ref().map(Props::to_value).unwrap_or_default()
    }
    fn sync_value(&self, target: &mut Value) {
        match self {
            None => {
                target.set(Value::null());
            }
            Some(value) if target.is_null() => {
                target.set(value.to_value());
            }
            Some(value) => value.sync_value(target),
        }
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        match (self.as_mut(), value.is_null()) {
            (_, true) => *self = None,
            (Some(inner), false) => inner.apply_value(value)?,
            (None, false) => *self = Some(T::from_value(value)?),
        }
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        if value.is_null() { Ok(None) } else { T::from_value(value).map(Some) }
    }
    fn kind(&self) -> PropsKind {
        self.as_ref().map_or(PropsKind::Leaf, Props::kind)
    }
    // Members of the inner value are reachable through the option, like JS optional chaining.
    fn field(&self, name: &str) -> Option<&dyn Props> {
        self.as_ref()?.field(name)
    }
    fn field_mut(&mut self, name: &str) -> Option<&mut dyn Props> {
        self.as_mut()?.field_mut(name)
    }
    fn element(&self, index: usize) -> Option<&dyn Props> {
        self.as_ref()?.element(index)
    }
    fn element_mut(&mut self, index: usize) -> Option<&mut dyn Props> {
        self.as_mut()?.element_mut(index)
    }
    fn element_count(&self) -> usize {
        self.as_ref().map_or(0, Props::element_count)
    }
    leaf_any!();
}

/// Syncs a slice into a list value: elements are diffed by index, then the list is grown or
/// truncated.
fn sync_slice<T: Props>(slice: &[T], target: &mut Value) {
    let Some(len) = target.as_list().map(Vec::len) else {
        target.set(Value::list(slice.iter().map(Props::to_value).collect()));
        return;
    };
    for (i, element) in slice.iter().enumerate().take(len) {
        sync_element(target, i, element);
    }
    target.truncate(slice.len());
    for element in slice.iter().skip(len) {
        target.push(element.to_value());
    }
}

impl<T: Props> Props for Vec<T> {
    fn to_value(&self) -> Value {
        Value::list(self.iter().map(Props::to_value).collect())
    }
    fn sync_value(&self, target: &mut Value) {
        sync_slice(self, target);
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        let list = value.as_list().ok_or_else(|| PropsError::expected("list", value))?;
        self.truncate(list.len());
        for (i, element) in list.iter().enumerate() {
            match self.get_mut(i) {
                Some(existing) => existing.apply_value(element),
                None => T::from_value(element).map(|new| self.push(new)),
            }
            .map_err(|e| e.in_field(format!("[{i}]")))?;
        }
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        let list = value.as_list().ok_or_else(|| PropsError::expected("list", value))?;
        list.iter().enumerate().map(|(i, v)| T::from_value(v).map_err(|e| e.in_field(format!("[{i}]")))).collect()
    }
    fn kind(&self) -> PropsKind {
        PropsKind::List
    }
    fn element(&self, index: usize) -> Option<&dyn Props> {
        self.get(index).map(|v| v as &dyn Props)
    }
    fn element_mut(&mut self, index: usize) -> Option<&mut dyn Props> {
        self.get_mut(index).map(|v| v as &mut dyn Props)
    }
    fn element_count(&self) -> usize {
        Vec::len(self)
    }
    leaf_any!();
}

impl<T: Props, const N: usize> Props for [T; N] {
    fn to_value(&self) -> Value {
        Value::list(self.iter().map(Props::to_value).collect())
    }
    fn sync_value(&self, target: &mut Value) {
        sync_slice(self, target);
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        let list = value.as_list().ok_or_else(|| PropsError::expected("list", value))?;
        if list.len() != N {
            return Err(PropsError::TypeMismatch { expected: "list of matching length", found: value.to_string() });
        }
        for (i, (existing, element)) in self.iter_mut().zip(list).enumerate() {
            existing.apply_value(element).map_err(|e| e.in_field(format!("[{i}]")))?;
        }
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        let elements = Vec::<T>::from_value(value)?;
        elements
            .try_into()
            .map_err(|_| PropsError::TypeMismatch { expected: "list of matching length", found: value.to_string() })
    }
    fn kind(&self) -> PropsKind {
        PropsKind::List
    }
    fn element(&self, index: usize) -> Option<&dyn Props> {
        self.get(index).map(|v| v as &dyn Props)
    }
    fn element_mut(&mut self, index: usize) -> Option<&mut dyn Props> {
        self.get_mut(index).map(|v| v as &mut dyn Props)
    }
    fn element_count(&self) -> usize {
        N
    }
    leaf_any!();
}

macro_rules! impl_map {
    ($map:ident) => {
        impl<T: Props> Props for $map<String, T> {
            fn to_value(&self) -> Value {
                Value::dict(self.iter().map(|(k, v)| (k.clone(), v.to_value())).collect())
            }
            fn sync_value(&self, target: &mut Value) {
                let Some(dict) = target.as_dict() else {
                    target.set(self.to_value());
                    return;
                };
                let removed: Vec<String> = dict.keys().filter(|k| !self.contains_key(*k)).cloned().collect();
                for key in removed {
                    target.remove(&key);
                }
                for (key, value) in self {
                    let segment = PathSegment::Key(key.clone().into());
                    target.update_at(std::slice::from_ref(&segment), |child| value.sync_value(child));
                }
            }
            fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
                let dict = value.as_dict().ok_or_else(|| PropsError::expected("dict", value))?;
                self.retain(|k, _| dict.contains_key(k));
                for (key, element) in dict {
                    match self.get_mut(key) {
                        Some(existing) => existing.apply_value(element),
                        None => T::from_value(element).map(|new| {
                            self.insert(key.clone(), new);
                        }),
                    }
                    .map_err(|e| e.in_field(key.clone()))?;
                }
                Ok(())
            }
            fn from_value(value: &Value) -> Result<Self, PropsError> {
                let dict = value.as_dict().ok_or_else(|| PropsError::expected("dict", value))?;
                dict.iter()
                    .map(|(k, v)| T::from_value(v).map(|v| (k.clone(), v)).map_err(|e| e.in_field(k.clone())))
                    .collect()
            }
            fn kind(&self) -> PropsKind {
                PropsKind::Map
            }
            fn field(&self, name: &str) -> Option<&dyn Props> {
                self.get(name).map(|v| v as &dyn Props)
            }
            fn field_mut(&mut self, name: &str) -> Option<&mut dyn Props> {
                self.get_mut(name).map(|v| v as &mut dyn Props)
            }
            leaf_any!();
        }
    };
}
impl_map!(HashMap);
use std::collections::HashMap as StdHashMap;
impl_map!(StdHashMap);

impl Props for Value {
    fn to_value(&self) -> Value {
        self.clone()
    }
    fn sync_value(&self, target: &mut Value) {
        target.sync_from(self);
    }
    fn apply_value(&mut self, value: &Value) -> Result<(), PropsError> {
        self.sync_from(value);
        Ok(())
    }
    fn from_value(value: &Value) -> Result<Self, PropsError> {
        Ok(value.clone())
    }
    fn kind(&self) -> PropsKind {
        match self.value_type() {
            ValueType::List(_) => PropsKind::List,
            ValueType::Dict(_) => PropsKind::Map,
            _ => PropsKind::Leaf,
        }
    }
    fn field(&self, name: &str) -> Option<&dyn Props> {
        self.get(name).map(|v| v as &dyn Props)
    }
    fn field_mut(&mut self, name: &str) -> Option<&mut dyn Props> {
        // Mutation through this path is tracked by the value's own change flags.
        self.get_mut(name).map(|v| v as &mut dyn Props)
    }
    fn element(&self, index: usize) -> Option<&dyn Props> {
        self.index(index).map(|v| v as &dyn Props)
    }
    fn element_mut(&mut self, index: usize) -> Option<&mut dyn Props> {
        self.index_mut(index).map(|v| v as &mut dyn Props)
    }
    fn element_count(&self) -> usize {
        self.as_list().map_or(0, Vec::len)
    }
    leaf_any!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::value::ChangeFlags, value};

    #[derive(crate::Props, Debug, Clone, PartialEq, Default)]
    struct Stack {
        id: String,
        count: u32,
    }

    #[derive(crate::Props, Debug, Clone, PartialEq, Default)]
    enum Rarity {
        #[default]
        Common,
        #[props(rename = "epic")]
        Epic,
    }

    #[derive(crate::Props, Debug, Clone, PartialEq, Default)]
    struct Slot(u8, Option<Stack>);

    #[derive(crate::Props, Debug, Clone, PartialEq, Default)]
    struct Inventory {
        owner: String,
        slots: Vec<Slot>,
        #[props(rename = "best")]
        best_rarity: Rarity,
        tags: HashMap<String, bool>,
        #[props(skip)]
        _cache: Vec<u64>,
    }

    #[derive(crate::Props, Debug, Clone, PartialEq)]
    struct Wrapper<T> {
        inner: T,
    }

    #[derive(crate::Props)]
    struct PlayerInventory;

    #[derive(crate::Props, Debug, Clone, PartialEq)]
    struct Health(u32);

    #[derive(crate::Props, Debug, Clone, PartialEq)]
    struct Tags(Vec<String>);

    #[derive(crate::Props, Debug, Clone, PartialEq)]
    struct Player {
        health: Health,
        tags: Tags,
    }

    #[test]
    fn single_field_tuple_structs_are_transparent() {
        let mut player = Player { health: Health(3), tags: Tags(vec!["a".into(), "b".into()]) };
        let mut value = player.to_value();
        assert_eq!(value, value!({ "health": 3, "tags": ["a", "b"] }));
        assert_eq!(Player::from_value(&value).unwrap(), player);
        assert_eq!(Health::from_value(&value!(7)).unwrap(), Health(7));

        // Members of the field are reached through the struct, with `.0` or `[0]`.
        assert_eq!(player.get_path::<String>("tags[1]").unwrap(), "b");
        assert_eq!(player.get_path::<String>("tags.1").unwrap(), "b");
        assert_eq!(player.get_path::<Health>("health").unwrap(), &Health(3));
        player.set_path("health", 5).unwrap();
        player.set_path("tags.0", "c").unwrap();
        assert_eq!(player, Player { health: Health(5), tags: Tags(vec!["c".into(), "b".into()]) });

        player.sync_value(&mut value);
        assert_eq!(value, player.to_value());
        assert_eq!(value.get("health").unwrap().flags(), ChangeFlags::SELF);
        assert!(!value.at(&PropPath::parse("tags[1]").unwrap()).unwrap().is_dirty());
    }

    #[derive(crate::Props)]
    #[props(name = "hud")]
    struct HeadsUpDisplay;

    #[test]
    fn names() {
        assert_eq!(Stack::name(), "stack");
        assert_eq!(Rarity::name(), "rarity");
        assert_eq!(PlayerInventory::name(), "player_inventory");
        assert_eq!(HeadsUpDisplay::name(), "hud");
        assert_eq!(Wrapper::<u8>::name(), "wrapper");

        // Types without a derive get a name from their type name.
        assert_eq!(u32::name(), "u32");
        assert_eq!(Vec::<String>::name(), "vec");
        assert_eq!(short_type_name("a::B<c::d::E, [f::G; 2]>"), "B<E, [G; 2]>");
        assert_eq!(to_snake_case("HTTPServer2D"), "http_server2_d");
    }

    fn inventory() -> Inventory {
        Inventory {
            owner: "ana".into(),
            slots: vec![
                Slot(0, Some(Stack { id: "coal".into(), count: 10 })),
                Slot(1, None),
            ],
            best_rarity: Rarity::Epic,
            tags: [("full".to_owned(), false)].into_iter().collect(),
            _cache: vec![1, 2, 3],
        }
    }

    #[test]
    fn to_value() {
        assert_eq!(
            inventory().to_value(),
            value!({
                "owner": "ana",
                "slots": [[0, { "id": "coal", "count": 10 }], [1, (Value::null())]],
                "best": "epic",
                "tags": { "full": false },
            })
        );
        assert_eq!(Wrapper { inner: 3u8 }.to_value(), value!({ "inner": 3 }));
    }

    #[test]
    fn from_value_round_trip() {
        let original = inventory();
        let mut back = Inventory::from_value(&original.to_value()).unwrap();
        assert!(back._cache.is_empty());
        back._cache = original._cache.clone();
        assert_eq!(back, original);

        let err = Inventory::from_value(&value!({ "owner": "x" })).unwrap_err();
        assert_eq!(err.to_string(), "missing field 'slots'");
        assert!(Rarity::from_value(&value!("rare")).is_err());
    }

    #[test]
    fn deep_paths() {
        let mut inv = inventory();
        assert_eq!(inv.get_path::<u32>("slots[0][1].count").unwrap(), &10);
        assert_eq!(inv.get_path::<String>("owner").unwrap(), "ana");
        assert_eq!(inv.get_path::<bool>("tags.full").unwrap(), &false);
        assert_eq!(inv.get_path::<Rarity>("best").unwrap(), &Rarity::Epic);

        *inv.get_path_mut::<u32>("slots[0][1].count").unwrap() += 1;
        inv.set_path("slots[0][1].id", "iron").unwrap();
        inv.set_path("best", "Common").unwrap();
        assert_eq!(inv.slots[0].1, Some(Stack { id: "iron".into(), count: 11 }));
        assert_eq!(inv.best_rarity, Rarity::Common);

        // Setting a whole option creates its value.
        inv.set_path("slots[1][1]", value!({ "id": "gem", "count": 1 })).unwrap();
        assert_eq!(inv.slots[1].1, Some(Stack { id: "gem".into(), count: 1 }));
    }

    #[test]
    fn path_errors() {
        let mut inv = inventory();
        let err = |e: PathError| e.to_string();
        assert_eq!(err(inv.path("nope").err().unwrap()), "<root> has no field 'nope'");
        assert_eq!(err(inv.path("slots[5]").err().unwrap()), "index 5 is out of bounds for 'slots' of length 2");
        assert_eq!(err(inv.path("owner.x").err().unwrap()), "'owner' is a Leaf and cannot be accessed with 'x'");
        assert_eq!(err(inv.path("slots.x").err().unwrap()), "'slots' is a List and cannot be accessed with 'x'");
        assert!(matches!(inv.get_path::<String>("slots[0][0]"), Err(PathError::TypeMismatch { .. })));
        assert!(matches!(inv.path("a..b"), Err(PathError::Parse(_))));
        assert_eq!(
            err(inv.set_path("slots[0][0]", "x").unwrap_err()),
            "cannot set 'slots[0][0]': expected number, found \"x\""
        );
    }

    #[test]
    fn sync_marks_only_changed_leaves() {
        let mut inv = inventory();
        let mut value = inv.to_value();

        inv.sync_value(&mut value);
        assert!(!value.is_dirty(), "syncing equal data must not mark anything");

        inv.slots[0].1.as_mut().unwrap().count = 11;
        inv.sync_value(&mut value);
        assert_eq!(value, inv.to_value());
        assert_eq!(value.flags(), ChangeFlags::SUBTREE);
        let count = value.at(&PropPath::parse("slots[0][1].count").unwrap()).unwrap();
        assert_eq!(count.flags(), ChangeFlags::SELF);
        assert!(!value.get("owner").unwrap().is_dirty());
        assert!(!value.at(&PropPath::parse("slots[1]").unwrap()).unwrap().is_dirty());
        assert!(!value.at(&PropPath::parse("slots[0][1].id").unwrap()).unwrap().is_dirty());
        value.clear_changes();

        inv.slots.push(Slot(2, None));
        inv.tags.insert("new".into(), true);
        inv.sync_value(&mut value);
        assert_eq!(value, inv.to_value());
        assert!(value.get("slots").unwrap().flags().contains(ChangeFlags::SHAPE));
        assert!(value.get("tags").unwrap().flags().contains(ChangeFlags::SHAPE));
        assert!(!value.get("owner").unwrap().is_dirty());
        value.clear_changes();

        inv.slots.truncate(1);
        inv.tags.clear();
        inv.slots[0].1 = None;
        inv.sync_value(&mut value);
        assert_eq!(value, inv.to_value());
    }
}
