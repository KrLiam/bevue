//! Proxied property values for the API.
//! 

use std::fmt;

use bevy::{platform::collections::HashMap, reflect::PartialReflect};

/// An attribute value.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// The type variant of this value.
    t: ValueType,
}
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.t.fmt(f)
    }
}
impl Value {
    /// Creates a bool value.
    pub fn boolean(value: bool) -> Self {
        Self {
            t: ValueType::Boolean(value),
        }
    }
    /// Creates a number value.
    pub fn number(value: f64) -> Self {
        Self {
            t: ValueType::Number(value),
        }
    }
    /// Creates a string value.
    pub fn string(value: String) -> Self {
        Self {
            t: ValueType::String(value),
        }
    }
    /// Creates a list value.
    pub fn list(value: Vec<Value>) -> Self {
        Self {
            t: ValueType::List(value),
        }
    }
    /// Creates an dict value.
    pub fn dict(value: HashMap<String, Value>) -> Self {
        Self {
            t: ValueType::Dict(value),
        }
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
}
/// An enum containing all the type variants for attribute values.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueType {
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
            ValueType::Boolean(b) => { b.debug(f)?; },
            ValueType::Number(n) => { n.debug(f)?; },
            ValueType::String(s) => { s.debug(f)?; },
            ValueType::List(values) => {
                f.write_str("[")?;
                for (i, el) in values.iter().enumerate() {
                    el.fmt(f)?;
                    if i + 1 < values.len() {
                        f.write_str(", ")?;
                    }
                }
                f.write_str("]")?;
            }
            ValueType::Dict(map) => {
                f.write_str("{")?;
                for (i, (key, value)) in map.iter().enumerate() {
                    key.debug(f)?;
                    f.write_str(": ")?;
                    value.fmt(f)?;
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
impl From<i8> for Value {
    fn from(value: i8) -> Self {
        Self::number(value as f64)
    }
}
impl From<i16> for Value {
    fn from(value: i16) -> Self {
        Self::number(value as f64)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::number(value as f64)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::number(value as f64)
    }
}
impl From<i128> for Value {
    fn from(value: i128) -> Self {
        Self::number(value as f64)
    }
}
impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::number(value as f64)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::number(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::string(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::string(value)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(value: Vec<T>) -> Self {
        let mut converted: Vec<Value> = vec![];
        for v in value {
            converted.push(v.into())
        }
        Value::list(converted)
    }
}
impl<T: Into<Value>> From<HashMap<String, T>> for Value {
    fn from(value: HashMap<String, T>) -> Self {
        let mut converted: HashMap<String, Value> = HashMap::new();
        for (key, value) in value {
            converted.insert(key, value.into());
        }
        Value::dict(converted)
    }
}
impl<T: Into<Value>> From<std::collections::HashMap<String, T>> for Value {
    fn from(value: std::collections::HashMap<String, T>) -> Self {
        let mut converted: HashMap<String, Value> = HashMap::new();
        for (key, value) in value {
            converted.insert(key, value.into());
        }
        Value::dict(converted)
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
            hash_map!(@insert map, $key, $value);
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
        let mut elements = Vec::<Value>::new();
        $(
            value!(@push elements, value!($tt));
        )*
        Value::list(elements)
    }};

    (@push $elements:ident, $value:expr) => {
        $elements.push($value);
    };

    // dict

    ( { $( $key:tt : $value:tt ),* $(,)? } ) => {{
        #[allow(unused_mut)]
        let mut map = bevy::platform::collections::HashMap::<String, Value>::new();
        $(
            value!(@insert map, $key, value!($value));
        )*
        Value::dict(map)
    }};

    (@insert $map:ident, $key:literal, $value:expr) => {
        $map.insert($key.to_owned(), $value);
    };

    (@insert $map:ident, $key:expr, $value:expr) => {
        $map.insert($key.to_owned(), $value);
    };

    // others

    ($expr:expr) => {
        Value::from($expr)
    };
}



#[cfg(test)]
mod tests {
    use bevy::platform::collections::HashMap;

    use super::Value;
    
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
                "count": (value.count as f64)
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
}
