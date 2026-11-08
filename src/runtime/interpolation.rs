//! Evaluation of compiled template expressions against props.

use std::{borrow::Cow, sync::Arc};

use bevy::{ecs::entity::Entity, log::warn};
use bevy_flair::prelude::InlineStyle;

use crate::core::{
    ops,
    template::BinaryOp,
    compile::{CompiledExpr, CompiledFragment, CompiledSegment, CompiledText, ScopeRef},
    path::PathSegment,
    value::{Value, ValueType},
};

/// The element of a `v-for` frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameElement<'p> {
    /// The element itself, for iterables read from props.
    Ref(&'p Value),
    /// The list computed by a call iterable such as `sorted(items)`; the element is at the
    /// frame's index.
    Computed(Arc<[Value]>),
}

/// A `v-for` frame resolved for evaluation: the current element and its index.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame<'p> {
    pub element: FrameElement<'p>,
    pub index: usize,
}

impl<'p> Frame<'p> {
    /// A frame for `element`, read from props, at `index` of its list.
    pub fn new(element: &'p Value, index: usize) -> Self {
        Self { element: FrameElement::Ref(element), index }
    }

    /// A frame for the element at `index` of a computed list.
    pub fn computed(list: Arc<[Value]>, index: usize) -> Self {
        Self { element: FrameElement::Computed(list), index }
    }

    /// The current element.
    pub fn value(&self) -> &Value {
        match &self.element {
            FrameElement::Ref(value) => value,
            FrameElement::Computed(list) => list.get(self.index).unwrap_or(&NULL),
        }
    }
}

/// What an expression can reach outside of its props: registered functions and the components of
/// entities.
pub trait Host {
    /// Calls the registered function `name`.
    fn call(&mut self, name: &str, args: Vec<Value>) -> Value;

    /// Reads the component named `component` of `entity`, then follows as much of `path` into it
    /// as the component's data allows. Returns the value reached and how many segments of `path`
    /// were followed, or `None` if the entity or component does not exist. The default reads
    /// nothing.
    fn read_entity(&mut self, entity: Entity, component: &str, path: &[PathSegment]) -> Option<(Value, usize)> {
        let _ = (entity, component, path);
        None
    }
}

impl<F: FnMut(&str, Vec<Value>) -> Value> Host for F {
    fn call(&mut self, name: &str, args: Vec<Value>) -> Value {
        self(name, args)
    }
}

/// Everything an expression can read while being evaluated.
pub struct EvalCtx<'p, 'f, 'c> {
    pub frames: &'f [Frame<'p>],
    pub props: &'p Value,
    /// The arguments of the event being handled, for `$event` and `$args`.
    pub event_args: Option<&'p [Value]>,
    /// Calls registered functions and reads entities.
    pub call: &'c mut dyn Host,
}

impl<'p, 'f, 'c> EvalCtx<'p, 'f, 'c> {
    pub fn new(frames: &'f [Frame<'p>], props: &'p Value, call: &'c mut dyn Host) -> Self {
        Self { frames, props, event_args: None, call }
    }
}

/// A `call` implementation for contexts where functions are not available.
pub fn no_calls(name: &str, _args: Vec<Value>) -> Value {
    warn!("function '{name}' called where functions are not available");
    Value::null()
}

static NULL: Value = Value::NULL;

/// Evaluates `expr`. Paths that do not exist evaluate to `null`.
pub fn eval<'r, 'p: 'r>(expr: &'r CompiledExpr, ctx: &mut EvalCtx<'p, '_, '_>) -> Cow<'r, Value> {
    match expr {
        CompiledExpr::Literal(value) => Cow::Borrowed(value),
        CompiledExpr::Path { root, segments } => match root {
            ScopeRef::Index(level) => match ctx.frames.get(*level) {
                Some(frame) => Cow::Owned(Value::from(frame.index)),
                None => Cow::Borrowed(&NULL),
            },
            ScopeRef::Props => navigate(ctx.props, segments, ctx).unwrap_or(Cow::Borrowed(&NULL)),
            ScopeRef::Item(level) => {
                let frames = ctx.frames;
                match frames.get(*level) {
                    Some(Frame { element: FrameElement::Ref(start), .. }) => {
                        navigate(start, segments, ctx).unwrap_or(Cow::Borrowed(&NULL))
                    }
                    Some(frame) => {
                        let start = frame.value();
                        Cow::Owned(navigate(start, segments, ctx).map(Cow::into_owned).unwrap_or_default())
                    }
                    None => Cow::Borrowed(&NULL),
                }
            }
            ScopeRef::Event => {
                let args = ctx.event_args.unwrap_or(&[]);
                match segments.split_first() {
                    None => Cow::Owned(Value::list(args.to_vec())),
                    Some((CompiledSegment::Static(PathSegment::Index(i)), rest)) => match args.get(*i) {
                        Some(start) => navigate(start, rest, ctx).unwrap_or(Cow::Borrowed(&NULL)),
                        None => Cow::Borrowed(&NULL),
                    },
                    Some(_) => {
                        let list = Value::list(args.to_vec());
                        Cow::Owned(navigate(&list, segments, ctx).map(Cow::into_owned).unwrap_or_default())
                    }
                }
            }
        },
        CompiledExpr::Call { function, args, segments } => {
            let args: Vec<Value> = args.iter().map(|arg| eval(arg, ctx).into_owned()).collect();
            let result = ctx.call.call(function, args);
            if segments.is_empty() {
                return Cow::Owned(result);
            }
            Cow::Owned(navigate(&result, segments, ctx).map(Cow::into_owned).unwrap_or_default())
        }
        CompiledExpr::Text(text) => Cow::Owned(Value::from(render_text(text, ctx))),
        CompiledExpr::Unary { op, operand } => Cow::Owned(ops::unary(*op, &eval(operand, ctx))),
        CompiledExpr::Binary { op: BinaryOp::And, left, right } => {
            let left = eval(left, ctx);
            if left.is_truthy() { eval(right, ctx) } else { left }
        }
        CompiledExpr::Binary { op: BinaryOp::Or, left, right } => {
            let left = eval(left, ctx);
            if left.is_truthy() { left } else { eval(right, ctx) }
        }
        CompiledExpr::Binary { op, left, right } => {
            let left = eval(left, ctx);
            let right = eval(right, ctx);
            Cow::Owned(ops::binary(*op, &left, &right))
        }
        CompiledExpr::Conditional { condition, then, otherwise } => {
            if eval(condition, ctx).is_truthy() { eval(then, ctx) } else { eval(otherwise, ctx) }
        }
        CompiledExpr::Access { target, segments } => {
            let target = eval(target, ctx);
            Cow::Owned(navigate(&target, segments, ctx).map(Cow::into_owned).unwrap_or_default())
        }
        CompiledExpr::Object(entries) => {
            let mut dict = Value::empty_dict();
            for (key, value) in entries {
                dict.insert(key.as_ref(), eval(value, ctx).into_owned());
            }
            Cow::Owned(dict)
        }
    }
}

/// Follows `segments` from `start`, evaluating dynamic indices. A key after an entity names one
/// of its components, which is read through [`Host::read_entity`]; the value is then owned.
fn navigate<'v>(
    start: &'v Value,
    segments: &[CompiledSegment],
    ctx: &mut EvalCtx<'_, '_, '_>,
) -> Option<Cow<'v, Value>> {
    let mut current = Cow::Borrowed(start);
    let mut i = 0;
    while i < segments.len() {
        let segment = match &segments[i] {
            CompiledSegment::Static(s) => Cow::Borrowed(s),
            CompiledSegment::Dynamic(index) => Cow::Owned(value_segment(&eval(index, ctx))?),
        };
        i += 1;
        if let Some(entity) = current.as_entity() {
            let PathSegment::Key(component) = &*segment else { return None };
            // The static segments that follow are read inside the component.
            let rest: Vec<PathSegment> = segments[i..]
                .iter()
                .map_while(|s| match s {
                    CompiledSegment::Static(s) => Some(s.clone()),
                    CompiledSegment::Dynamic(_) => None,
                })
                .collect();
            let (value, followed) = ctx.call.read_entity(entity, component, &rest)?;
            current = Cow::Owned(value);
            i += followed;
            continue;
        }
        current = match current {
            Cow::Borrowed(value) => Cow::Borrowed(value.child(&segment)?),
            Cow::Owned(value) => Cow::Owned(value.child(&segment)?.clone()),
        };
    }
    Some(current)
}

/// Resolves a path expression to the value it names in props, borrowed. Returns `None` for
/// literals, calls, loop indices, event arguments, elements of computed lists and paths that do
/// not exist.
pub fn resolve<'p>(expr: &CompiledExpr, ctx: &mut EvalCtx<'p, '_, '_>) -> Option<&'p Value> {
    let CompiledExpr::Path { root, segments } = expr else { return None };
    let frames = ctx.frames;
    let start: &'p Value = match root {
        ScopeRef::Props => ctx.props,
        ScopeRef::Item(level) => match &frames.get(*level)?.element {
            FrameElement::Ref(value) => value,
            FrameElement::Computed(_) => return None,
        },
        ScopeRef::Index(_) | ScopeRef::Event => return None,
    };
    // Values read through entities are not in props.
    match navigate(start, segments, ctx)? {
        Cow::Borrowed(value) => Some(value),
        Cow::Owned(_) => None,
    }
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
pub fn render_text(text: &CompiledText, ctx: &mut EvalCtx<'_, '_, '_>) -> String {
    let mut out = String::new();
    write_text(text, ctx, &mut out);
    out
}

/// Renders interpolated text, appending it to `out`.
pub fn write_text(text: &CompiledText, ctx: &mut EvalCtx<'_, '_, '_>, out: &mut String) {
    for fragment in &text.0 {
        match fragment {
            CompiledFragment::Literal(s) => out.push_str(s),
            CompiledFragment::Expr(expr) => eval(expr, ctx).write_text(out),
        }
    }
}

/// Evaluates class attributes like Vue: a string is a whitespace separated list, a list holds
/// class names and a dict maps class names to whether they are active.
pub fn class_names(classes: &[CompiledExpr], ctx: &mut EvalCtx<'_, '_, '_>) -> Vec<String> {
    let mut out = Vec::new();
    for expr in classes {
        push_classes(&eval(expr, ctx), &mut out);
    }
    out
}

fn push_classes(value: &Value, out: &mut Vec<String>) {
    match value.value_type() {
        ValueType::String(s) => out.extend(s.split_whitespace().map(str::to_owned)),
        ValueType::List(list) => list.iter().for_each(|v| push_classes(v, out)),
        ValueType::Dict(_) | ValueType::Struct(_) => {
            let entries = value.entries().into_iter().flatten();
            let mut active: Vec<_> = entries.filter(|(_, v)| v.is_truthy()).map(|(k, _)| k.to_owned()).collect();
            // Dict iteration order is unspecified; keep the class list stable.
            active.sort();
            out.extend(active);
        }
        _ => {}
    }
}

/// Evaluates style attributes like Vue: a string holds CSS declarations, an object or dict maps
/// property names to values and a list holds any of these. Later declarations of a property win.
pub fn inline_style(styles: &[CompiledExpr], ctx: &mut EvalCtx<'_, '_, '_>) -> InlineStyle {
    let mut out = InlineStyle::default();
    for expr in styles {
        match expr {
            // Read entry by entry: a dict would lose the order the declarations were written in.
            CompiledExpr::Object(entries) => {
                for (key, value) in entries {
                    set_declaration(&mut out, key, &eval(value, ctx));
                }
            }
            _ => push_styles(&eval(expr, ctx), &mut out),
        }
    }
    out
}

fn push_styles(value: &Value, out: &mut InlineStyle) {
    match value.value_type() {
        ValueType::String(s) => {
            for declaration in s.split(';') {
                if let Some((name, value)) = declaration.split_once(':') {
                    out.set(name.trim(), value.trim().to_owned());
                }
            }
        }
        ValueType::List(list) => list.iter().for_each(|v| push_styles(v, out)),
        ValueType::Dict(_) | ValueType::Struct(_) => {
            // Dict iteration order is unspecified; sorting keeps the style stable and puts a
            // shorthand before its longhands (`margin` before `margin-left`).
            let mut entries: Vec<_> = value.entries().into_iter().flatten().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (key, value) in entries {
                set_declaration(out, key, value);
            }
        }
        _ => {}
    }
}

/// Sets the property `key`, or removes it if the value is `null` or `false`.
fn set_declaration(out: &mut InlineStyle, key: &str, value: &Value) {
    let name = css_property_name(key);
    match value.value_type() {
        ValueType::Null | ValueType::Boolean(false) => out.remove(&name),
        _ => out.set(name.into_owned(), value.to_text()),
    }
}

/// Converts `backgroundColor` and `background_color` to `background-color`. Custom properties
/// (`--name`) are case sensitive and returned unchanged.
pub fn css_property_name(key: &str) -> Cow<'_, str> {
    if key.starts_with("--") || !key.contains(|c: char| c == '_' || c.is_ascii_uppercase()) {
        return Cow::Borrowed(key);
    }
    let mut out = String::with_capacity(key.len() + 2);
    for c in key.chars() {
        match c {
            '_' => out.push('-'),
            c if c.is_ascii_uppercase() => {
                out.push('-');
                out.push(c.to_ascii_lowercase());
            }
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            compile::{CompiledHandler, CompiledTemplate},
            template::NodeId,
        },
        parser::parse_template,
        value,
    };

    fn first_text(source: &str) -> (CompiledTemplate, NodeId) {
        let template = parse_template(source).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let id = (0..compiled.nodes.len()).map(NodeId).find(|id| compiled.get(*id).text.is_some()).unwrap();
        (compiled, id)
    }

    fn text_of(compiled: &CompiledTemplate, id: NodeId, frames: &[Frame], props: &Value) -> String {
        render_text(compiled.get(id).text.as_ref().unwrap(), &mut EvalCtx::new(frames, props, &mut no_calls))
    }

    #[test]
    fn text() {
        let props = value!({ "user": { "name": "Ana", "tags": ["a", "b"] }, "n": 3 });
        let (compiled, id) = first_text(r#"<p>Hi {{user.name}}, {{n}} {{user.tags[1]}}{{missing.x}}!</p>"#);
        assert_eq!(text_of(&compiled, id, &[], &props), "Hi Ana, 3 b!");
    }

    #[test]
    fn loop_frames() {
        let props = value!({ "rows": [{ "id": "r0" }, { "id": "r1" }], "grid": ["g0", "g1"] });
        let (compiled, id) = first_text(r#"<div><p v-for="(row, i) in rows">{{row.id}}:{{i}}:{{grid[i]}}</p></div>"#);
        let row = props.at(&crate::core::path::PropPath::parse("rows[1]").unwrap()).unwrap();
        let frames = [Frame::new(row, 1)];
        assert_eq!(text_of(&compiled, id, &frames, &props), "r1:1:g1");

        let computed: Arc<[Value]> = Arc::from(vec![value!({ "id": "c0" }), value!({ "id": "c1" })]);
        let frames = [Frame::computed(computed, 0)];
        assert_eq!(text_of(&compiled, id, &frames, &props), "c0:0:g0");
    }

    #[test]
    fn classes() {
        let props = value!({ "s": "a b", "l": ["c", "d"], "m": { "e": true, "f": false } });
        let template = parse_template(r#"<div class="x" :class="s"><p :class="l"></p><p :class="m"></p></div>"#).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let classes = |id: usize| {
            class_names(&compiled.get(NodeId(id)).classes, &mut EvalCtx::new(&[], &props, &mut no_calls))
        };
        // Attribute order is not preserved by the HTML parser, and does not matter for classes.
        let mut root = classes(0);
        root.sort();
        assert_eq!(root, vec!["a", "b", "x"]);
        assert_eq!(classes(1), vec!["c", "d"]);
        assert_eq!(classes(2), vec!["e"]);
    }

    #[test]
    fn calls() {
        let props = value!({ "name": "ana", "items": [1, 2, 3] });
        let (compiled, id) =
            first_text(r#"<p>{{upper(name)}} {{pick(items)[1]}} {{sum(items[0], 10)}} {{items[len(items)]}}</p>"#);
        let mut calls = Vec::new();
        let mut call = |name: &str, args: Vec<Value>| {
            calls.push(name.to_owned());
            match name {
                "upper" => Value::from(args[0].to_text().to_uppercase()),
                "pick" => args[0].clone(),
                "sum" => Value::from(args.iter().filter_map(Value::as_number).sum::<f64>()),
                "len" => Value::from(2),
                _ => Value::null(),
            }
        };
        let text = render_text(compiled.get(id).text.as_ref().unwrap(), &mut EvalCtx::new(&[], &props, &mut call));
        assert_eq!(text, "ANA 2 11 3");
        assert_eq!(calls, vec!["upper", "pick", "sum", "len"]);
    }

    #[test]
    fn event_args() {
        let template = parse_template(r#"<div><p @click="f($event, $args, $args[1].x, $event.y)">x</p></div>"#).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let Some(CompiledHandler::Call { args, .. }) = &compiled.get(NodeId(1)).listener("click").unwrap().handler else {
            panic!("expected a call handler")
        };
        let props = Value::empty_dict();
        let event = [value!({ "y": 7 }), value!({ "x": 5 })];
        let mut call = no_calls;
        let mut ctx = EvalCtx::new(&[], &props, &mut call);
        ctx.event_args = Some(&event);
        let values: Vec<Value> = args.iter().map(|a| eval(a, &mut ctx).into_owned()).collect();
        assert_eq!(
            values,
            vec![value!({ "y": 7 }), value!([{ "y": 7 }, { "x": 5 }]), value!(5), value!(7)]
        );
    }
}
