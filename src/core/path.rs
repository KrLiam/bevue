//! Paths into nested prop values, e.g. `a.b[0].c`.

use std::{borrow::Cow, fmt, str::FromStr};

/// A single step of a [`PropPath`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathSegment {
    /// A dict key or struct field name.
    Key(Cow<'static, str>),
    /// A list index.
    Index(usize),
}

impl From<&'static str> for PathSegment {
    fn from(value: &'static str) -> Self {
        PathSegment::Key(value.into())
    }
}
impl From<String> for PathSegment {
    fn from(value: String) -> Self {
        PathSegment::Key(value.into())
    }
}
impl From<usize> for PathSegment {
    fn from(value: usize) -> Self {
        PathSegment::Index(value)
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSegment::Key(key) => f.write_str(key),
            PathSegment::Index(i) => write!(f, "[{i}]"),
        }
    }
}

/// A path into a nested value, such as `a.b[0].c`. The empty path refers to the value itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct PropPath(pub Vec<PathSegment>);

impl PropPath {
    /// The empty path.
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn new(segments: impl IntoIterator<Item = PathSegment>) -> Self {
        Self(segments.into_iter().collect())
    }

    /// Parses a path such as `a.b[0].c` or `[2].name`.
    pub fn parse(source: &str) -> Result<Self, PathParseError> {
        let err = |msg: &str| PathParseError(format!("{msg} in path '{source}'"));
        let mut segments = Vec::new();
        let mut rest = source.trim();
        let mut expect_key = true;

        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('[') {
                let end = after.find(']').ok_or_else(|| err("unclosed '['"))?;
                let index = after[..end]
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| err("invalid index"))?;
                segments.push(PathSegment::Index(index));
                rest = &after[end + 1..];
                expect_key = false;
                continue;
            }
            if !expect_key {
                rest = rest.strip_prefix('.').ok_or_else(|| err("expected '.' or '['"))?;
            }
            let end = rest.find(['.', '[']).unwrap_or(rest.len());
            let key = rest[..end].trim();
            if key.is_empty() {
                return Err(err("empty key"));
            }
            segments.push(PathSegment::Key(key.to_owned().into()));
            rest = &rest[end..];
            expect_key = false;
        }

        Ok(Self(segments))
    }

    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, segment: impl Into<PathSegment>) {
        self.0.push(segment.into());
    }

    /// Returns a new path with `segment` appended.
    pub fn join(&self, segment: impl Into<PathSegment>) -> Self {
        let mut path = self.clone();
        path.push(segment);
        path
    }
}

impl std::ops::Deref for PropPath {
    type Target = [PathSegment];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for PropPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 && matches!(segment, PathSegment::Key(_)) {
                f.write_str(".")?;
            }
            segment.fmt(f)?;
        }
        Ok(())
    }
}

impl FromStr for PropPath {
    type Err = PathParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl From<Vec<PathSegment>> for PropPath {
    fn from(value: Vec<PathSegment>) -> Self {
        Self(value)
    }
}

/// Conversion into a [`PropPath`], so APIs can take either `"a.b[0]"` or an already built path.
pub trait AsPropPath {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError>;
}

impl AsPropPath for PropPath {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError> {
        Ok(Cow::Borrowed(self))
    }
}
impl AsPropPath for &PropPath {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError> {
        Ok(Cow::Borrowed(*self))
    }
}
impl AsPropPath for str {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError> {
        PropPath::parse(self).map(Cow::Owned)
    }
}
impl AsPropPath for &str {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError> {
        PropPath::parse(self).map(Cow::Owned)
    }
}
impl AsPropPath for String {
    fn as_prop_path(&self) -> Result<Cow<'_, PropPath>, PathParseError> {
        PropPath::parse(self).map(Cow::Owned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathParseError(pub String);

impl fmt::Display for PathParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PathParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let path = PropPath::parse("a.b[0].c").unwrap();
        assert_eq!(
            path.segments(),
            &[
                PathSegment::from("a"),
                PathSegment::from("b"),
                PathSegment::Index(0),
                PathSegment::from("c"),
            ]
        );
        assert_eq!(path.to_string(), "a.b[0].c");

        let nested = PropPath::parse("[2][3].name").unwrap();
        assert_eq!(
            nested.segments(),
            &[PathSegment::Index(2), PathSegment::Index(3), PathSegment::from("name")]
        );
        assert_eq!(nested.to_string(), "[2][3].name");

        assert!(PropPath::parse("").unwrap().is_empty());
    }

    #[test]
    fn parse_errors() {
        assert!(PropPath::parse("a..b").is_err());
        assert!(PropPath::parse("a[").is_err());
        assert!(PropPath::parse("a[x]").is_err());
        assert!(PropPath::parse("a[0]b").is_err());
        assert!(PropPath::parse(".a").is_err());
    }
}
