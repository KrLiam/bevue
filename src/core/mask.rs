//! [`ReadMask`]: the parts of the props a template reads.
//!
//! The compiler collects every path a template can read (in bindings, `v-for` keys and event
//! handler arguments) into a mask. Syncing Rust data into props with
//! [`Props::sync_value_masked`](crate::core::props::Props::sync_value_masked) then copies and
//! diffs only those parts, so fields no template shows cost nothing.

use std::borrow::Cow;

use crate::core::{
    compile::{CompiledHandler, CompiledTemplate, Dep, ScopeRef, Slot, deps_of},
    path::{PathSegment, PropPath},
    template::NodeId,
};

/// A segment of a masked path.
#[derive(Debug, Clone, PartialEq)]
pub enum MaskSegment {
    /// A named member: a struct field or dict key.
    Key(Cow<'static, str>),
    /// Any element of a list.
    Elements,
}

impl From<&PathSegment> for MaskSegment {
    fn from(segment: &PathSegment) -> Self {
        match segment {
            PathSegment::Key(key) => MaskSegment::Key(key.clone()),
            PathSegment::Index(_) => MaskSegment::Elements,
        }
    }
}

/// The parts of a value that are read, as a trie of member names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadMask {
    /// The whole value is read.
    all: bool,
    /// Masks of the named members that are read.
    fields: Vec<(Cow<'static, str>, ReadMask)>,
    /// The mask of every element of a list, if elements are read.
    elements: Option<Box<ReadMask>>,
}

static ALL: ReadMask = ReadMask { all: true, fields: Vec::new(), elements: None };

impl ReadMask {
    /// A mask reading the whole value.
    pub fn all() -> Self {
        ALL.clone()
    }

    /// A reference to a mask reading the whole value.
    pub fn all_ref() -> &'static ReadMask {
        &ALL
    }

    /// Returns true if the whole value is read.
    pub fn is_all(&self) -> bool {
        self.all
    }

    /// The mask of member `name`, or `None` if it is not read.
    pub fn field(&self, name: &str) -> Option<&ReadMask> {
        if self.all {
            return Some(&ALL);
        }
        self.fields.iter().find(|(key, _)| key == name).map(|(_, mask)| mask)
    }

    /// The mask of the elements of a list, or `None` if they are not read.
    pub fn elements(&self) -> Option<&ReadMask> {
        if self.all {
            return Some(&ALL);
        }
        self.elements.as_deref()
    }

    /// The mask at `path`, or `None` if nothing at or below it is read.
    pub fn at(&self, path: &[PathSegment]) -> Option<&ReadMask> {
        path.iter().try_fold(self, |mask, segment| match segment {
            PathSegment::Key(key) => mask.field(key),
            PathSegment::Index(_) => mask.elements(),
        })
    }

    /// Marks the whole value at `path` as read.
    pub fn insert(&mut self, path: &[MaskSegment]) {
        if let Some(node) = self.node_mut(path) {
            *node = ReadMask::all();
        }
    }

    /// Marks the value at `path` as present without reading its contents: a list iterated by
    /// `v-for` needs its length, but only the parts of its elements that the items read.
    pub fn touch(&mut self, path: &[MaskSegment]) {
        self.node_mut(path);
    }

    /// Adds everything `other` reads.
    pub fn merge(&mut self, other: &ReadMask) {
        if self.all {
            return;
        }
        if other.all {
            *self = ReadMask::all();
            return;
        }
        for (key, mask) in &other.fields {
            self.node_mut(&[MaskSegment::Key(key.clone())]).unwrap().merge(mask);
        }
        if let Some(elements) = &other.elements {
            self.node_mut(&[MaskSegment::Elements]).unwrap().merge(elements);
        }
    }

    /// The node at `path`, created if missing. `None` if an ancestor is already read whole.
    fn node_mut(&mut self, path: &[MaskSegment]) -> Option<&mut ReadMask> {
        let mut node = self;
        for segment in path {
            if node.all {
                return None;
            }
            node = match segment {
                MaskSegment::Key(key) => {
                    let index = match node.fields.iter().position(|(k, _)| k == key) {
                        Some(index) => index,
                        None => {
                            node.fields.push((key.clone(), ReadMask::default()));
                            node.fields.len() - 1
                        }
                    };
                    &mut node.fields[index].1
                }
                MaskSegment::Elements => node.elements.get_or_insert_with(Default::default),
            };
        }
        if node.all { None } else { Some(node) }
    }

    /// Collects what `compiled` reads from its props.
    pub fn of(compiled: &CompiledTemplate) -> Self {
        let mut mask = ReadMask::default();
        if let Some(root) = compiled.root {
            let mut levels = Vec::new();
            collect(compiled, root, &mut levels, &mut mask);
        }
        // Queries sort their rows by these paths, even if no binding shows them.
        for query in &compiled.queries {
            let Ok(name) = PropPath::parse(&query.name) else { continue };
            if mask.at(&name).is_none() {
                continue;
            }
            for key in &query.order_by {
                let mut path: Vec<MaskSegment> = name.iter().map(MaskSegment::from).collect();
                path.push(MaskSegment::Elements);
                path.extend(key.path.iter().map(MaskSegment::from));
                mask.insert(&path);
            }
        }
        mask
    }
}

/// Adds what node `id` and its subtree read. `levels[n]` is the masked path of the elements of the
/// enclosing loop of level `n`, or `None` if its iterable is not a prop path.
fn collect(compiled: &CompiledTemplate, id: NodeId, levels: &mut Vec<Option<Vec<MaskSegment>>>, mask: &mut ReadMask) {
    let node = compiled.get(id);
    let depth = levels.len();

    if let Some(list) = &node.list {
        // The iterable is resolved in the enclosing scope.
        let elements = match &list.route {
            Some(route) => resolve(route, levels).map(|mut path| {
                mask.touch(&path);
                path.push(MaskSegment::Elements);
                path
            }),
            None => {
                add_deps(&list.deps, levels, mask);
                None
            }
        };
        levels.truncate(list.level);
        levels.push(elements);
        if let Some(key) = &list.key {
            add_deps(&deps_of([key]), levels, mask);
        }
    }

    add_deps(&node.text_deps, levels, mask);
    add_deps(&node.class_deps, levels, mask);
    add_deps(&node.style_deps, levels, mask);
    for listener in &node.listeners {
        if let Some(CompiledHandler::Call { args, .. }) = &listener.handler {
            add_deps(&deps_of(args.iter()), levels, mask);
        }
    }

    for slot in &node.slots {
        match slot {
            Slot::Static(child) | Slot::For(child) => collect(compiled, *child, levels, mask),
            Slot::If(chain) => {
                add_deps(&chain.1, levels, mask);
                for (_, child) in &chain.0 {
                    collect(compiled, *child, levels, mask);
                }
            }
        }
    }
    levels.truncate(depth);
}

fn add_deps(deps: &[Dep], levels: &[Option<Vec<MaskSegment>>], mask: &mut ReadMask) {
    for dep in deps {
        if let Some(path) = resolve(dep, levels) {
            mask.insert(&path);
        }
    }
}

/// The masked path of `dep` from the props root, if it reads props.
fn resolve(dep: &Dep, levels: &[Option<Vec<MaskSegment>>]) -> Option<Vec<MaskSegment>> {
    let mut path = match dep.root {
        ScopeRef::Props => Vec::new(),
        ScopeRef::Item(level) => levels.get(level)?.clone()?,
        ScopeRef::Index(_) | ScopeRef::Event => return None,
    };
    path.extend(dep.path.iter().map(MaskSegment::from));
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::path::PropPath, parser::parse_template};

    fn mask_of(source: &str) -> ReadMask {
        ReadMask::of(&CompiledTemplate::compile(&parse_template(source).unwrap()).unwrap())
    }

    fn read(mask: &ReadMask, path: &str) -> Option<bool> {
        mask.at(&PropPath::parse(path).unwrap()).map(ReadMask::is_all)
    }

    #[test]
    fn bindings_read_their_paths() {
        let mask = mask_of(r#"<div :class="{ on: user.active }"><p>{{ user.name }}</p><p v-if="flags.show">x</p></div>"#);
        assert_eq!(read(&mask, "user"), Some(false));
        assert_eq!(read(&mask, "user.name"), Some(true));
        assert_eq!(read(&mask, "user.active"), Some(true));
        assert_eq!(read(&mask, "user.email"), None);
        assert_eq!(read(&mask, "flags.show"), Some(true));
    }

    #[test]
    fn lists_read_what_their_items_read() {
        let mask = mask_of(
            r#"<div>
                <div v-for="row in table.rows" :key="row.id" @click="pick(row.value)">
                    <span v-for="cell in row.cells">{{ cell.text }}</span>
                </div>
            </div>"#,
        );
        assert_eq!(read(&mask, "table.rows"), Some(false));
        assert_eq!(read(&mask, "table.rows[0].id"), Some(true));
        assert_eq!(read(&mask, "table.rows[0].value"), Some(true));
        assert_eq!(read(&mask, "table.rows[3].cells"), Some(false));
        assert_eq!(read(&mask, "table.rows[3].cells[1].text"), Some(true));
        assert_eq!(read(&mask, "table.rows[3].cells[1].style"), None);
        assert_eq!(read(&mask, "table.title"), None);
    }

    #[test]
    fn whole_values_and_dynamic_paths() {
        let mask = mask_of(r#"<div><p>{{ f(inv) }}</p><p>{{ groups[g].name }}</p><p v-for="x in sorted(list)">{{ x.a }}</p></div>"#);
        assert_eq!(read(&mask, "inv"), Some(true));
        assert_eq!(read(&mask, "inv.anything"), Some(true));
        assert_eq!(read(&mask, "groups"), Some(true));
        assert_eq!(read(&mask, "g"), Some(true));
        assert_eq!(read(&mask, "list"), Some(true));
    }

    #[test]
    fn merge_and_insert() {
        let mut a = ReadMask::default();
        a.insert(&[MaskSegment::Key("x".into()), MaskSegment::Key("y".into())]);
        let mut b = ReadMask::default();
        b.insert(&[MaskSegment::Key("x".into())]);
        a.merge(&b);
        assert_eq!(read(&a, "x"), Some(true));
        // Inserting below a node read whole changes nothing.
        a.insert(&[MaskSegment::Key("x".into()), MaskSegment::Key("z".into())]);
        assert_eq!(read(&a, "x"), Some(true));
    }
}
