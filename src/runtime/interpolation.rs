//! Evaluation of compiled template expressions against props.

use std::borrow::Cow;

use crate::core::{
    compile::{CompiledExpr, CompiledFragment, CompiledSegment, CompiledText, ScopeRef},
    path::{PathSegment, PropPath},
    value::{Value, ValueType},
};

/// A `v-for` frame resolved for evaluation: the absolute path of the current element in props and
/// its index.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub path: PropPath,
    pub index: usize,
}

static NULL: Value = Value::NULL;

/// Evaluates `expr`. Paths that do not exist evaluate to `null`.
pub fn eval<'a>(expr: &'a CompiledExpr, frames: &[Frame], props: &'a Value) -> Cow<'a, Value> {
    match expr {
        CompiledExpr::Literal(value) => Cow::Borrowed(value),
        CompiledExpr::Path { root: ScopeRef::Index(level), .. } => match frames.get(*level) {
            Some(frame) => Cow::Owned(Value::from(frame.index)),
            None => Cow::Borrowed(&NULL),
        },
        CompiledExpr::Path { .. } => match eval_path(expr, frames, props) {
            Some(path) => Cow::Borrowed(props.at(&path).unwrap_or(&NULL)),
            None => Cow::Borrowed(&NULL),
        },
    }
}

/// Resolves a path expression to an absolute path in props. Returns `None` for literals, loop
/// indices and dynamic indices that do not evaluate to a number or string.
pub fn eval_path(expr: &CompiledExpr, frames: &[Frame], props: &Value) -> Option<PropPath> {
    let CompiledExpr::Path { root, segments } = expr else { return None };
    let mut path = match root {
        ScopeRef::Props => PropPath::root(),
        ScopeRef::Item(level) => frames.get(*level)?.path.clone(),
        ScopeRef::Index(_) => return None,
    };
    for segment in segments {
        match segment {
            CompiledSegment::Static(s) => path.push(s.clone()),
            CompiledSegment::Dynamic(index) => path.push(value_segment(&eval(index, frames, props))?),
        }
    }
    Some(path)
}

/// Converts a value used as an index into a path segment.
fn value_segment(value: &Value) -> Option<PathSegment> {
    match value.value_type() {
        ValueType::String(s) => Some(PathSegment::Key(s.clone().into())),
        ValueType::Number(n) if *n >= 0.0 && n.fract() == 0.0 => Some(PathSegment::Index(*n as usize)),
        _ => None,
    }
}

/// Renders interpolated text.
pub fn render_text(text: &CompiledText, frames: &[Frame], props: &Value) -> String {
    let mut out = String::new();
    for fragment in &text.0 {
        match fragment {
            CompiledFragment::Literal(s) => out.push_str(s),
            CompiledFragment::Expr(expr) => out.push_str(&eval(expr, frames, props).to_text()),
        }
    }
    out
}

/// Evaluates class attributes like Vue: a string is a whitespace separated list, a list holds
/// class names and a dict maps class names to whether they are active.
pub fn class_names(classes: &[CompiledExpr], frames: &[Frame], props: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for expr in classes {
        push_classes(&eval(expr, frames, props), &mut out);
    }
    out
}

fn push_classes(value: &Value, out: &mut Vec<String>) {
    match value.value_type() {
        ValueType::String(s) => out.extend(s.split_whitespace().map(str::to_owned)),
        ValueType::List(list) => list.iter().for_each(|v| push_classes(v, out)),
        ValueType::Dict(map) => {
            let mut active: Vec<_> = map.iter().filter(|(_, v)| v.is_truthy()).map(|(k, _)| k.clone()).collect();
            // Dict iteration order is unspecified; keep the class list stable.
            active.sort();
            out.extend(active);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{compile::CompiledTemplate, template::NodeId},
        parser::parse_template,
        value,
    };

    fn first_text(source: &str) -> (CompiledTemplate, NodeId) {
        let template = parse_template(source).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let id = (0..compiled.nodes.len()).map(NodeId).find(|id| compiled.get(*id).text.is_some()).unwrap();
        (compiled, id)
    }

    #[test]
    fn text() {
        let props = value!({ "user": { "name": "Ana", "tags": ["a", "b"] }, "n": 3 });
        let (compiled, id) = first_text(r#"<p>Hi {{user.name}}, {{n}} {{user.tags[1]}}{{missing.x}}!</p>"#);
        assert_eq!(render_text(compiled.get(id).text.as_ref().unwrap(), &[], &props), "Hi Ana, 3 b!");
    }

    #[test]
    fn loop_frames() {
        let props = value!({ "rows": [{ "id": "r0" }, { "id": "r1" }], "grid": ["g0", "g1"] });
        let (compiled, id) = first_text(r#"<div><p v-for="(row, i) in rows">{{row.id}}:{{i}}:{{grid[i]}}</p></div>"#);
        let frames = [Frame { path: PropPath::parse("rows[1]").unwrap(), index: 1 }];
        assert_eq!(render_text(compiled.get(id).text.as_ref().unwrap(), &frames, &props), "r1:1:g1");
    }

    #[test]
    fn classes() {
        let props = value!({ "s": "a b", "l": ["c", "d"], "m": { "e": true, "f": false } });
        let template = parse_template(r#"<div class="x" :class="s"><p :class="l"></p><p :class="m"></p></div>"#).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        // Attribute order is not preserved by the HTML parser, and does not matter for classes.
        let mut root = class_names(&compiled.get(NodeId(0)).classes, &[], &props);
        root.sort();
        assert_eq!(root, vec!["a", "b", "x"]);
        assert_eq!(class_names(&compiled.get(NodeId(1)).classes, &[], &props), vec!["c", "d"]);
        assert_eq!(class_names(&compiled.get(NodeId(2)).classes, &[], &props), vec!["e"]);
    }
}
