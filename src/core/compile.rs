//! Compiles a [`BevueTemplate`] into a form the runtime can evaluate and track dependencies of.
//!
//! Compilation resolves every identifier against the enclosing `v-for` scopes, so `item.label`
//! inside `v-for="item in menu.items"` becomes "field `label` of the element of loop level 0"
//! instead of a prop lookup. It also groups sibling `v-if`/`v-else-if`/`v-else` nodes into chains.
//!
//! Component elements (`<Foo :item="it"/>`) are expanded here: the nodes of the component's
//! template are compiled into the host's node list, with every identifier naming an attribute
//! replaced by the host's expression for it. The runtime never sees a component, so using one
//! costs the same as writing its elements in place.

use std::{borrow::Cow, fmt};

use crate::core::{
    path::PathSegment,
    template::{
        BevueTemplate, ConditionKind, ElementType, Expression, Identifier, NodeId, PropsDeclaration, TemplateNode,
        TemplateStyle, TextFragment,
    },
    value::Value,
};

/// The root a compiled path starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeRef {
    /// The template's `BevueProps`.
    Props,
    /// The current element of the `v-for` loop at this nesting level (0 is the outermost).
    Item(usize),
    /// The current index of the `v-for` loop at this nesting level.
    Index(usize),
    /// The arguments of the event being handled, as a list. Only valid in event handlers, where
    /// `$args` is this list and `$event` its first element.
    Event,
}

/// A path segment that may need evaluating.
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledSegment {
    Static(PathSegment),
    /// An index computed from an expression, e.g. `rows[i]`.
    Dynamic(Box<CompiledExpr>),
}

/// An expression with its identifiers resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledExpr {
    Literal(Value),
    Path { root: ScopeRef, segments: Vec<CompiledSegment> },
    /// A call to a registered function; `segments` are applied to its result.
    Call { function: Cow<'static, str>, args: Vec<CompiledExpr>, segments: Vec<CompiledSegment> },
}

impl CompiledExpr {
    pub fn is_literal(&self) -> bool {
        matches!(self, CompiledExpr::Literal(_))
    }

    /// Appends the paths this expression reads. A dynamic segment cuts the path there, so
    /// `rows[i].name` depends on all of `rows` plus whatever `i` depends on.
    ///
    /// Functions are assumed pure: a call depends only on its arguments. Event arguments are not
    /// reactive and add no dependency.
    pub fn dependencies(&self, out: &mut Vec<(ScopeRef, Vec<PathSegment>)>) {
        match self {
            CompiledExpr::Literal(_) => {}
            CompiledExpr::Path { root, segments } => {
                let mut prefix = Vec::new();
                for segment in segments {
                    match segment {
                        CompiledSegment::Static(s) => prefix.push(s.clone()),
                        CompiledSegment::Dynamic(index) => {
                            index.dependencies(out);
                            break;
                        }
                    }
                }
                if *root != ScopeRef::Event {
                    out.push((*root, prefix));
                }
            }
            CompiledExpr::Call { args, segments, .. } => {
                for arg in args {
                    arg.dependencies(out);
                }
                for segment in segments {
                    if let CompiledSegment::Dynamic(index) = segment {
                        index.dependencies(out);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompiledFragment {
    Literal(Cow<'static, str>),
    Expr(CompiledExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledText(pub Vec<CompiledFragment>);

impl CompiledText {
    /// Returns true if the text contains no interpolations.
    pub fn is_static(&self) -> bool {
        self.0.iter().all(|f| matches!(f, CompiledFragment::Literal(_)))
    }
}

/// A `v-if`/`v-else-if`/`v-else` chain. Branches are tried in order; a `None` condition always
/// matches.
#[derive(Debug, Clone, PartialEq)]
pub struct IfChain(pub Vec<(Option<CompiledExpr>, NodeId)>);

/// What a template child position renders to.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    /// A plain element, always rendered once.
    Static(NodeId),
    /// The first branch of the chain whose condition is truthy, if any.
    If(IfChain),
    /// One element per item of the node's `v-for` iterable.
    For(NodeId),
}

/// A compiled `v-for` directive.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledList {
    /// The iterable, resolved in the scope enclosing the loop.
    pub iterable: CompiledExpr,
    /// The `:key` expression, resolved in the loop's own scope.
    pub key: Option<CompiledExpr>,
    /// The nesting level of this loop; its items are `ScopeRef::Item(level)`.
    pub level: usize,
}

/// What an event listener does.
#[derive(Debug, Clone, PartialEq)]
pub enum CompiledHandler {
    /// `@click="add_user(item.id, $event)"`: calls the function with the evaluated arguments.
    Call { function: Cow<'static, str>, args: Vec<CompiledExpr> },
    /// `@click="add_user"`: calls the function with the event's arguments.
    Function(Cow<'static, str>),
}

/// An `@name` / `v-on:name` listener. Without a handler it only makes the node interactive.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledListener {
    pub name: Cow<'static, str>,
    pub handler: Option<CompiledHandler>,
    /// The root node of the component the listener was written in, where its `$emit` triggers
    /// events. `None` for listeners of the template itself, which emit on the template root.
    pub emit_root: Option<NodeId>,
}

/// Events that Bevue triggers from `Interaction`.
pub const INTERACTION_EVENTS: [&str; 3] = ["click", "mouseover", "mouseleave"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompiledNode {
    pub text: Option<CompiledText>,
    /// All `class` and `:class` attributes, merged at render time.
    pub classes: Vec<CompiledExpr>,
    pub slots: Vec<Slot>,
    pub list: Option<CompiledList>,
    pub listeners: Vec<CompiledListener>,
}

impl CompiledNode {
    /// Returns true if any class attribute needs evaluating.
    pub fn has_dynamic_classes(&self) -> bool {
        self.classes.iter().any(|c| !c.is_literal())
    }

    /// Returns true if the node listens to an event triggered from `Interaction`.
    pub fn interactive(&self) -> bool {
        self.listeners.iter().any(|l| INTERACTION_EVENTS.contains(&l.name.as_ref()))
    }

    /// The first listener for the event `name`.
    pub fn listener(&self, name: &str) -> Option<&CompiledListener> {
        self.listeners.iter().find(|l| l.name == name)
    }

    /// Every listener for the event `name`. The root element of a component has its own listeners
    /// followed by those of the component element.
    pub fn listeners_for<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a CompiledListener> + 'a {
        self.listeners.iter().filter(move |l| l.name == name)
    }
}

/// What a component name refers to.
pub enum ResolvedComponent<'a> {
    /// The component's template.
    Ready(&'a BevueTemplate),
    /// The component exists but its template is not available yet. An empty node is compiled in
    /// its place.
    Pending,
    /// No component has this name, which is a compile error.
    Unknown,
}

/// Finds the templates of the components a template uses.
pub trait ComponentResolver {
    fn resolve(&self, name: &str) -> ResolvedComponent<'_>;
}

/// A [`ComponentResolver`] for which every component is [`ResolvedComponent::Pending`].
pub struct NoComponents;

impl ComponentResolver for NoComponents {
    fn resolve(&self, _name: &str) -> ResolvedComponent<'_> {
        ResolvedComponent::Pending
    }
}

/// The compiled form of a template. Its own nodes are indexed by [`NodeId`] like
/// [`BevueTemplate::nodes`]; the nodes of the components it uses come after them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompiledTemplate {
    pub root: Option<NodeId>,
    pub nodes: Vec<CompiledNode>,
    /// The styles of the components used, followed by the template's own.
    pub styles: Vec<TemplateStyle>,
    /// The props declared by the template and by the components used.
    pub props: Vec<PropsDeclaration>,
}

impl CompiledTemplate {
    pub fn get(&self, id: NodeId) -> &CompiledNode {
        &self.nodes[*id]
    }

    /// Compiles `template` without its components: an empty node takes the place of each one.
    pub fn compile(template: &BevueTemplate) -> Result<Self, CompileError> {
        Self::compile_with(template, &NoComponents)
    }

    /// Compiles `template`, expanding the components found by `components`.
    pub fn compile_with(template: &BevueTemplate, components: &dyn ComponentResolver) -> Result<Self, CompileError> {
        let mut compiled = Self {
            root: template.root_index(),
            nodes: vec![CompiledNode::default(); template.nodes.len()],
            styles: Vec::new(),
            props: template.props.clone(),
        };
        let Some(root) = compiled.root else {
            return Ok(compiled);
        };

        let root_node = &template.nodes[*root];
        if root_node.list.is_some() || root_node.condition.is_some() {
            return Err(CompileError::new(root, "the root element cannot have v-if or v-for"));
        }
        if let ElementType::Component(name) = &root_node.element {
            return Err(CompileError::new(root, format!("the root element cannot be a component: <{name}>")));
        }

        let mut compiler = Compiler {
            template,
            components,
            offset: 0,
            scopes: Vec::new(),
            visited: vec![false; template.nodes.len()],
            in_handler: false,
            envs: Vec::new(),
        };
        compiler.compile_node(root, &mut compiled)?;
        // Last, so the template's own rules win over those of its components.
        if let Some(style) = &root_node.style {
            compiled.add_style(style);
        }
        Ok(compiled)
    }

    fn add_style(&mut self, style: &TemplateStyle) {
        if !self.styles.contains(style) {
            self.styles.push(style.clone());
        }
    }
}

/// A `v-for` alias frame.
struct ScopeFrame {
    item: Identifier,
    index: Option<Identifier>,
}

/// The component whose template is being expanded.
struct ComponentEnv<'a> {
    name: &'a str,
    /// The attributes of the component element, compiled in the scope of the element.
    props: Vec<(String, CompiledExpr)>,
    /// The first `v-for` frame the component can see: those of its own template.
    visible_from: usize,
    /// The compiled root node of this use of the component.
    root: NodeId,
}

struct Compiler<'a> {
    /// The template being compiled: the host's, or that of the component being expanded.
    template: &'a BevueTemplate,
    components: &'a dyn ComponentResolver,
    /// Where the nodes of `template` start in the compiled node list.
    offset: usize,
    scopes: Vec<ScopeFrame>,
    visited: Vec<bool>,
    /// Set while compiling an event handler, where `$event`, `$args` and `$emit` are allowed.
    in_handler: bool,
    /// The components being expanded, outermost first.
    envs: Vec<ComponentEnv<'a>>,
}

impl<'a> Compiler<'a> {
    /// The compiled id of node `id` of the template being compiled.
    fn out_id(&self, id: NodeId) -> NodeId {
        NodeId(self.offset + *id)
    }

    fn visit(&mut self, id: NodeId) -> Result<(), CompileError> {
        if std::mem::replace(&mut self.visited[*id], true) {
            return Err(CompileError::new(id, "node is the child of more than one element"));
        }
        Ok(())
    }

    /// Compiles the child `id` and returns its compiled id: its own, or for a component element
    /// the root of the expanded component.
    fn compile_child(&mut self, id: NodeId, out: &mut CompiledTemplate) -> Result<NodeId, CompileError> {
        let template = self.template;
        match &template.nodes[*id].element {
            ElementType::Component(name) => self.expand(id, name, out).map_err(|e| e.at(id)),
            ElementType::Native(_) => {
                self.compile_node(id, out)?;
                Ok(self.out_id(id))
            }
        }
    }

    /// Compiles the template of the component element `id` in its place.
    fn expand(&mut self, id: NodeId, name: &'a str, out: &mut CompiledTemplate) -> Result<NodeId, CompileError> {
        self.visit(id)?;
        let node = &self.template.nodes[*id];
        if !node.children.is_empty() || node.text.is_some() {
            return Err(CompileError::unplaced(format!("component elements cannot have children: <{name}>")));
        }
        let component = match self.components.resolve(name) {
            ResolvedComponent::Ready(component) => component,
            ResolvedComponent::Pending => return Ok(self.out_id(id)),
            ResolvedComponent::Unknown => {
                return Err(CompileError::unplaced(format!(
                    "unknown component <{name}>; register it with `app.register_component`"
                )));
            }
        };
        if self.envs.iter().any(|env| env.name == name) {
            return Err(CompileError::unplaced(format!("component <{name}> contains itself")));
        }
        let Some(root) = component.root_index() else { return Ok(self.out_id(id)) };
        let root_node = &component.nodes[*root];
        if root_node.list.is_some() || root_node.condition.is_some() {
            return Err(CompileError::unplaced(format!("the root element of <{name}> cannot have v-if or v-for")));
        }
        if matches!(root_node.element, ElementType::Component(_)) {
            return Err(CompileError::unplaced(format!("the root element of <{name}> cannot be a component")));
        }

        // Everything written on the element belongs to the scope the element is in.
        let mut props = Vec::new();
        let mut classes = Vec::new();
        for (attribute, expr) in &node.attributes {
            let expr = self.expr(expr)?;
            if attribute == "class" {
                classes.push(expr);
            } else {
                props.push((attribute.replace('-', "_"), expr));
            }
        }
        let listeners = self.listeners(node)?;

        let offset = out.nodes.len();
        out.nodes.resize(offset + component.nodes.len(), CompiledNode::default());
        let target = NodeId(offset + *root);

        self.envs.push(ComponentEnv { name, props, visible_from: self.scopes.len(), root: target });
        let template = std::mem::replace(&mut self.template, component);
        let host_offset = std::mem::replace(&mut self.offset, offset);
        let visited = std::mem::replace(&mut self.visited, vec![false; component.nodes.len()]);
        let result = self.compile_node(root, out);
        self.template = template;
        self.offset = host_offset;
        self.visited = visited;
        self.envs.pop();
        result.map_err(|e| e.in_component(name))?;

        let compiled = &mut out.nodes[*target];
        compiled.classes.extend(classes);
        compiled.listeners.extend(listeners);
        if let Some(style) = &root_node.style {
            out.add_style(style);
        }
        for declaration in &component.props {
            if !out.props.contains(declaration) {
                out.props.push(declaration.clone());
            }
        }
        Ok(target)
    }

    fn listeners(&mut self, node: &TemplateNode) -> Result<Vec<CompiledListener>, CompileError> {
        let emit_root = self.envs.last().map(|env| env.root);
        self.in_handler = true;
        let listeners = node
            .events
            .iter()
            .map(|(name, handler)| {
                let handler = handler.as_ref().map(|h| self.handler(h)).transpose()?;
                Ok(CompiledListener { name: name.clone(), handler, emit_root })
            })
            .collect();
        self.in_handler = false;
        listeners
    }

    /// Compiles `id` in the current scope, which already includes its own `v-for` frame if any.
    fn compile_node(&mut self, id: NodeId, out: &mut CompiledTemplate) -> Result<(), CompileError> {
        self.visit(id)?;
        let node = &self.template.nodes[*id];

        let text = match &node.text {
            Some(text) => Some(CompiledText(
                text.0
                    .iter()
                    .map(|fragment| match fragment {
                        TextFragment::Literal(s) => Ok(CompiledFragment::Literal(s.clone())),
                        TextFragment::Interpolated(expr) => self.expr(expr).map(CompiledFragment::Expr),
                    })
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.at(id))?,
            )),
            None => None,
        };
        let classes = node
            .attributes
            .iter()
            .filter(|(name, _)| name == "class")
            .map(|(_, expr)| self.expr(expr))
            .collect::<Result<_, _>>()
            .map_err(|e| e.at(id))?;

        let listeners = self.listeners(node).map_err(|e| e.at(id))?;

        let mut slots = Vec::new();
        let mut open_chain: Option<IfChain> = None;
        for &child_id in &node.children {
            let child = &self.template.nodes[*child_id];
            let condition = child.condition.as_ref();

            if let Some(condition) = condition
                && condition.kind != ConditionKind::If
            {
                let Some(chain) = open_chain.as_mut() else {
                    return Err(CompileError::new(child_id, "v-else or v-else-if without a preceding v-if"));
                };
                if child.list.is_some() {
                    return Err(CompileError::new(child_id, "v-for on a v-else/v-else-if element is not supported"));
                }
                let expr = condition.expr.as_ref().map(|e| self.expr(e)).transpose().map_err(|e| e.at(child_id))?;
                let target = self.compile_child(child_id, out)?;
                chain.0.push((expr, target));
                if condition.kind == ConditionKind::Else {
                    slots.push(Slot::If(open_chain.take().unwrap()));
                }
                continue;
            }

            if let Some(chain) = open_chain.take() {
                slots.push(Slot::If(chain));
            }

            if let Some(condition) = condition {
                if child.list.is_some() {
                    return Err(CompileError::new(
                        child_id,
                        "v-if and v-for on the same element are not supported, wrap the element instead",
                    ));
                }
                let expr = condition.expr.as_ref().map(|e| self.expr(e)).transpose().map_err(|e| e.at(child_id))?;
                let target = self.compile_child(child_id, out)?;
                open_chain = Some(IfChain(vec![(expr, target)]));
            } else if let Some(list) = &child.list {
                let iterable = self.expr(&list.iterable).map_err(|e| e.at(child_id))?;
                let level = self.scopes.len();
                self.scopes.push(ScopeFrame { item: list.target.clone(), index: list.index.clone() });
                let key = list.key.as_ref().map(|k| self.expr(k)).transpose().map_err(|e| e.at(child_id));
                let result = key.and_then(|key| {
                    let target = self.compile_child(child_id, out)?;
                    out.nodes[*target].list = Some(CompiledList { iterable, key, level });
                    Ok(target)
                });
                self.scopes.pop();
                slots.push(Slot::For(result?));
            } else {
                slots.push(Slot::Static(self.compile_child(child_id, out)?));
            }
        }
        if let Some(chain) = open_chain.take() {
            slots.push(Slot::If(chain));
        }

        let compiled = &mut out.nodes[self.offset + *id];
        compiled.text = text;
        compiled.classes = classes;
        compiled.slots = slots;
        compiled.listeners = listeners;
        Ok(())
    }

    fn handler(&self, expr: &Expression) -> Result<CompiledHandler, CompileError> {
        match expr {
            Expression::Call { function, args } => Ok(CompiledHandler::Call {
                function: function.as_str().to_owned().into(),
                args: args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?,
            }),
            Expression::Identifier(name) if !name.as_str().starts_with('$') => {
                Ok(CompiledHandler::Function(name.as_str().to_owned().into()))
            }
            _ => Err(CompileError::unplaced("event handler must be a function call or a function name")),
        }
    }

    fn expr(&self, expr: &Expression) -> Result<CompiledExpr, CompileError> {
        Ok(match expr {
            Expression::Float(f) => CompiledExpr::Literal(Value::from(*f)),
            Expression::Integer(i) => CompiledExpr::Literal(Value::from(*i)),
            Expression::String(s) => CompiledExpr::Literal(Value::from(s.as_ref())),
            Expression::Bool(b) => CompiledExpr::Literal(Value::from(*b)),
            Expression::Identifier(ident) => self.resolve(ident)?,
            Expression::Call { function, args } => {
                if function.as_str() == "$emit" && !self.in_handler {
                    return Err(CompileError::unplaced("$emit can only be used in event handlers"));
                }
                CompiledExpr::Call {
                    function: function.as_str().to_owned().into(),
                    args: args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?,
                    segments: Vec::new(),
                }
            }
            Expression::Access { target, field } => {
                self.extend(self.expr(target)?, CompiledSegment::Static(PathSegment::Key(field.as_str().to_owned().into())))?
            }
            Expression::Index { target, index } => {
                let target = self.expr(target)?;
                let segment = match self.expr(index)? {
                    CompiledExpr::Literal(value) => CompiledSegment::Static(literal_segment(&value)?),
                    dynamic => CompiledSegment::Dynamic(Box::new(dynamic)),
                };
                self.extend(target, segment)?
            }
        })
    }

    fn resolve(&self, ident: &Identifier) -> Result<CompiledExpr, CompileError> {
        if matches!(ident.as_str(), "$event" | "$args") {
            if !self.in_handler {
                return Err(CompileError::unplaced(format!("{ident} can only be used in event handlers")));
            }
            let segments = match ident.as_str() {
                "$event" => vec![CompiledSegment::Static(PathSegment::Index(0))],
                _ => Vec::new(),
            };
            return Ok(CompiledExpr::Path { root: ScopeRef::Event, segments });
        }
        // Inside a component, only the loops of its own template are in scope.
        let env = self.envs.last();
        let visible_from = env.map_or(0, |env| env.visible_from);
        for (level, frame) in self.scopes.iter().enumerate().skip(visible_from).rev() {
            if frame.item == *ident {
                return Ok(CompiledExpr::Path { root: ScopeRef::Item(level), segments: Vec::new() });
            }
            if frame.index.as_ref() == Some(ident) {
                return Ok(CompiledExpr::Path { root: ScopeRef::Index(level), segments: Vec::new() });
            }
        }
        // An attribute of the component element stands for the expression it was given.
        if let Some((_, expr)) = env.and_then(|env| env.props.iter().rev().find(|(name, _)| name == ident.as_str())) {
            return Ok(expr.clone());
        }
        Ok(CompiledExpr::Path {
            root: ScopeRef::Props,
            segments: vec![CompiledSegment::Static(PathSegment::Key(ident.as_str().to_owned().into()))],
        })
    }

    fn extend(&self, target: CompiledExpr, segment: CompiledSegment) -> Result<CompiledExpr, CompileError> {
        match target {
            CompiledExpr::Path { root: ScopeRef::Index(_), .. } => {
                Err(CompileError::unplaced("a loop index has no fields"))
            }
            CompiledExpr::Path { root, mut segments } => {
                segments.push(segment);
                Ok(CompiledExpr::Path { root, segments })
            }
            CompiledExpr::Call { function, args, mut segments } => {
                segments.push(segment);
                Ok(CompiledExpr::Call { function, args, segments })
            }
            CompiledExpr::Literal(_) => Err(CompileError::unplaced("cannot access a field of a literal")),
        }
    }
}

fn literal_segment(value: &Value) -> Result<PathSegment, CompileError> {
    if let Some(s) = value.as_string() {
        return Ok(PathSegment::Key(s.clone().into()));
    }
    match value.as_number() {
        Some(n) if n >= 0.0 && n.fract() == 0.0 => Ok(PathSegment::Index(n as usize)),
        _ => Err(CompileError::unplaced(format!("invalid index {value}"))),
    }
}

/// An error found while compiling a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub node: Option<NodeId>,
    pub message: String,
}

impl CompileError {
    fn new(node: NodeId, message: impl Into<String>) -> Self {
        Self { node: Some(node), message: message.into() }
    }

    fn unplaced(message: impl Into<String>) -> Self {
        Self { node: None, message: message.into() }
    }

    fn at(mut self, node: NodeId) -> Self {
        self.node.get_or_insert(node);
        self
    }

    /// Marks the error as found in the template of component `name`. The node is then the
    /// component element, as node ids of the component's template mean nothing to the host.
    fn in_component(mut self, name: &str) -> Self {
        self.message = format!("in <{name}>: {}", self.message);
        self.node = None;
        self
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node {
            Some(node) => write!(f, "{} (node {})", self.message, *node),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for CompileError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_template;

    fn key(k: &'static str) -> CompiledSegment {
        CompiledSegment::Static(PathSegment::Key(k.into()))
    }

    #[test]
    fn resolves_loop_scopes() {
        let template = parse_template(
            r#"
            <div>
                <p v-for="(row, i) in rows" :key="row.id">
                    <span v-for="cell in row.cells">{{cell.label}} {{row.id}} {{i}} {{title}}</span>
                </p>
            </div>
            "#,
        )
        .unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();

        assert_eq!(compiled.get(NodeId(0)).slots, vec![Slot::For(NodeId(1))]);

        let outer = compiled.get(NodeId(1)).list.as_ref().unwrap();
        assert_eq!(outer.level, 0);
        assert_eq!(outer.iterable, CompiledExpr::Path { root: ScopeRef::Props, segments: vec![key("rows")] });
        assert_eq!(outer.key, Some(CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![key("id")] }));

        let inner = compiled.get(NodeId(2)).list.as_ref().unwrap();
        assert_eq!(inner.level, 1);
        assert_eq!(inner.iterable, CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![key("cells")] });

        let text = compiled.get(NodeId(2)).text.as_ref().unwrap();
        let exprs: Vec<_> = text
            .0
            .iter()
            .filter_map(|f| match f {
                CompiledFragment::Expr(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            exprs,
            vec![
                CompiledExpr::Path { root: ScopeRef::Item(1), segments: vec![key("label")] },
                CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![key("id")] },
                CompiledExpr::Path { root: ScopeRef::Index(0), segments: vec![] },
                CompiledExpr::Path { root: ScopeRef::Props, segments: vec![key("title")] },
            ]
        );
    }

    #[test]
    fn groups_if_chains() {
        let template = parse_template(
            r#"
            <div>
                <p v-if="a">A</p>
                <p v-else-if="b">B</p>
                <p v-else>C</p>
                <p v-if="c">D</p>
                <span>E</span>
            </div>
            "#,
        )
        .unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let slots = &compiled.get(NodeId(0)).slots;
        assert_eq!(slots.len(), 3);
        let Slot::If(chain) = &slots[0] else { panic!("expected an if chain") };
        assert_eq!(chain.0.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![NodeId(1), NodeId(2), NodeId(3)]);
        assert!(chain.0[2].0.is_none());
        let Slot::If(chain) = &slots[1] else { panic!("expected an if chain") };
        assert_eq!(chain.0.len(), 1);
        assert_eq!(slots[2], Slot::Static(NodeId(5)));
    }

    #[test]
    fn indexes() {
        let template = parse_template(r#"<div><p v-for="(x, i) in xs">{{xs[0].a}} {{grid[i]}}</p></div>"#).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let text = compiled.get(NodeId(1)).text.as_ref().unwrap();
        assert_eq!(
            text.0[0],
            CompiledFragment::Expr(CompiledExpr::Path {
                root: ScopeRef::Props,
                segments: vec![key("xs"), CompiledSegment::Static(PathSegment::Index(0)), key("a")],
            })
        );
        assert_eq!(
            text.0[2],
            CompiledFragment::Expr(CompiledExpr::Path {
                root: ScopeRef::Props,
                segments: vec![
                    key("grid"),
                    CompiledSegment::Dynamic(Box::new(CompiledExpr::Path { root: ScopeRef::Index(0), segments: vec![] })),
                ],
            })
        );
    }

    #[test]
    fn calls_and_handlers() {
        let template = parse_template(
            r#"<div>
                <p v-for="item in items" @click="buy(item.id, $event)" @custom="log" @mouseover>{{fmt(item.price)[0]}}</p>
            </div>"#,
        )
        .unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        let node = compiled.get(NodeId(1));
        assert!(node.interactive());
        assert_eq!(
            node.listener("click").unwrap().handler,
            Some(CompiledHandler::Call {
                function: "buy".into(),
                args: vec![
                    CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![key("id")] },
                    CompiledExpr::Path { root: ScopeRef::Event, segments: vec![CompiledSegment::Static(PathSegment::Index(0))] },
                ],
            })
        );
        assert_eq!(node.listener("custom").unwrap().handler, Some(CompiledHandler::Function("log".into())));
        assert_eq!(node.listener("mouseover").unwrap().handler, None);

        let CompiledFragment::Expr(expr) = &node.text.as_ref().unwrap().0[0] else { panic!() };
        assert_eq!(
            *expr,
            CompiledExpr::Call {
                function: "fmt".into(),
                args: vec![CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![key("price")] }],
                segments: vec![CompiledSegment::Static(PathSegment::Index(0))],
            }
        );
        let mut deps = Vec::new();
        expr.dependencies(&mut deps);
        assert_eq!(deps, vec![(ScopeRef::Item(0), vec![PathSegment::Key("price".into())])]);

        let custom_only = parse_template(r#"<div><p @custom="f()">x</p></div>"#).unwrap();
        assert!(!CompiledTemplate::compile(&custom_only).unwrap().get(NodeId(1)).interactive());
    }

    #[test]
    fn handler_only_builtins() {
        for source in [
            r#"<div><p>{{$event}}</p></div>"#,
            r#"<div><p :class="$args">x</p></div>"#,
            r#"<div><p>{{$emit('x')}}</p></div>"#,
            r#"<div><p v-if="$emit('x')">x</p></div>"#,
            r#"<div><p @click="1">x</p></div>"#,
            r#"<div><p @click="a.b">x</p></div>"#,
            r#"<div><p @click="$event">x</p></div>"#,
        ] {
            let template = parse_template(source).unwrap();
            assert!(CompiledTemplate::compile(&template).is_err(), "{source} should not compile");
        }
        let ok = parse_template(r#"<div><p @click="$emit('buy', $args)">x</p></div>"#).unwrap();
        assert!(CompiledTemplate::compile(&ok).is_ok());
    }

    struct Components(Vec<(&'static str, BevueTemplate)>);

    impl Components {
        fn new(components: &[(&'static str, &str)]) -> Self {
            Self(components.iter().map(|(name, source)| (*name, parse_template(source).unwrap())).collect())
        }
    }

    impl ComponentResolver for Components {
        fn resolve(&self, name: &str) -> ResolvedComponent<'_> {
            match self.0.iter().find(|(n, _)| *n == name) {
                Some((_, template)) => ResolvedComponent::Ready(template),
                None => ResolvedComponent::Unknown,
            }
        }
    }

    fn compile_with(source: &str, components: &Components) -> Result<CompiledTemplate, CompileError> {
        CompiledTemplate::compile_with(&parse_template(source).unwrap(), components)
    }

    /// The compiled tree below `id` without node ids, to compare templates laid out differently.
    fn shape(compiled: &CompiledTemplate, id: NodeId) -> String {
        let node = compiled.get(id);
        let listeners: Vec<_> = node.listeners.iter().map(|l| (&l.name, &l.handler)).collect();
        let slots: Vec<String> = node
            .slots
            .iter()
            .map(|slot| match slot {
                Slot::Static(child) => shape(compiled, *child),
                Slot::For(child) => format!("for {}", shape(compiled, *child)),
                Slot::If(chain) => {
                    let branches: Vec<_> =
                        chain.0.iter().map(|(cond, child)| format!("{cond:?} => {}", shape(compiled, *child))).collect();
                    format!("if {branches:?}")
                }
            })
            .collect();
        format!("({:?} {:?} {:?} {listeners:?} {slots:?})", node.text, node.classes, node.list)
    }

    #[test]
    fn components_compile_like_inlined_markup() {
        let components = Components::new(&[
            (
                "Row",
                r#"<li class="row" @click="$emit('remove', item.id)">
                    <p>{{ item.label }} {{ n }} {{ title }} {{ fmt(total) }}</p>
                    <Tag v-for="tag in item.tags" :key="tag" :label="tag"/>
                    <b v-if="open">open</b>
                    <b v-else>{{ size.w }}</b>
                </li>"#,
            ),
            ("Tag", r#"<span :class="label">{{ label }}</span>"#),
        ]);
        let with_components = compile_with(
            r#"<div>
                <Row v-for="(it, i) in rows[0].items" :key="it.id" class="x" :class="it.kind"
                     :item="it" :n="i" title="T" open :total="sum(it.prices)" :size="sizes[i]"
                     @remove="remove(i, $event)"/>
            </div>"#,
            &components,
        )
        .unwrap();
        let inlined = compile_with(
            r#"<div>
                <li v-for="(it, i) in rows[0].items" :key="it.id" class="row" class="x" :class="it.kind"
                    @click="$emit('remove', it.id)" @remove="remove(i, $event)">
                    <p>{{ it.label }} {{ i }} {{ 'T' }} {{ fmt(sum(it.prices)) }}</p>
                    <span v-for="tag in it.tags" :key="tag" :class="tag">{{ tag }}</span>
                    <b v-if="true">open</b>
                    <b v-else>{{ sizes[i].w }}</b>
                </li>
            </div>"#,
            &components,
        )
        .unwrap();
        assert_eq!(shape(&with_components, NodeId(0)), shape(&inlined, NodeId(0)));

        // `$emit` in the component triggers on its root, the element's own handler does not.
        let Slot::For(row) = with_components.get(NodeId(0)).slots[0] else { panic!("expected a list") };
        assert!(*row >= 2, "component nodes come after the template's own");
        let listeners = &with_components.get(row).listeners;
        assert_eq!(listeners[0].emit_root, Some(row));
        assert_eq!(listeners[1].emit_root, None);
    }

    #[test]
    fn component_scope() {
        let components = Components::new(&[("Row", r#"<p>{{ it }} {{ item }} {{ menu.title }}</p>"#)]);
        let compiled =
            compile_with(r#"<div><Row v-for="it in items" :item-name="it" :item="it"/></div>"#, &components).unwrap();
        let Slot::For(row) = compiled.get(NodeId(0)).slots[0] else { panic!("expected a list") };
        let exprs: Vec<_> = compiled
            .get(row)
            .text
            .as_ref()
            .unwrap()
            .0
            .iter()
            .filter_map(|f| match f {
                CompiledFragment::Expr(e) => Some(e.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            exprs,
            vec![
                // The loop alias of the host is not visible: `it` is a prop of the host template.
                CompiledExpr::Path { root: ScopeRef::Props, segments: vec![key("it")] },
                CompiledExpr::Path { root: ScopeRef::Item(0), segments: vec![] },
                CompiledExpr::Path { root: ScopeRef::Props, segments: vec![key("menu"), key("title")] },
            ]
        );

        let kebab = Components::new(&[("Row", r#"<p>{{ item_name }}</p>"#)]);
        let compiled = compile_with(r#"<div><Row item-name="x"/></div>"#, &kebab).unwrap();
        let Slot::Static(row) = compiled.get(NodeId(0)).slots[0] else { panic!("expected an element") };
        assert_eq!(
            compiled.get(row).text.as_ref().unwrap().0,
            vec![CompiledFragment::Expr(CompiledExpr::Literal(Value::from("x")))]
        );
    }

    #[test]
    fn component_styles_and_props() {
        let components = Components::new(&[
            ("A", r#"<props><resource type="Menu"/></props><p>a</p><style>.a {}</style>"#),
            ("B", r#"<props><resource type="Menu"/><resource type="User"/></props><p>b</p><style src="b.css"></style>"#),
        ]);
        let compiled = compile_with(
            r#"<props><resource type="Host"/></props><div><A/><A/><B/></div><style>.host {}</style>"#,
            &components,
        )
        .unwrap();
        assert_eq!(
            compiled.styles,
            vec![TemplateStyle::from_inline(".a {}"), TemplateStyle::from_src("b.css"), TemplateStyle::from_inline(".host {}")]
        );
        assert_eq!(
            compiled.props,
            vec![PropsDeclaration::resource("Host"), PropsDeclaration::resource("Menu"), PropsDeclaration::resource("User")]
        );
    }

    #[test]
    fn component_errors() {
        let components = Components::new(&[
            ("Leaf", r#"<p>x</p>"#),
            ("Loop", r#"<div><Other/></div>"#),
            ("Other", r#"<div><Loop/></div>"#),
            ("Broken", r#"<div><p v-else>x</p></div>"#),
            ("Nested", r#"<Leaf/>"#),
        ]);
        for (source, expected) in [
            (r#"<div><Nope/></div>"#, "unknown component <Nope>"),
            (r#"<div><Leaf><p>x</p></Leaf></div>"#, "cannot have children"),
            (r#"<div><Leaf>text</Leaf></div>"#, "cannot have children"),
            (r#"<div><Loop/></div>"#, "in <Loop>: in <Other>: component <Loop> contains itself"),
            (r#"<div><Broken/></div>"#, "in <Broken>: v-else"),
            (r#"<div><Nested/></div>"#, "the root element of <Nested> cannot be a component"),
            (r#"<Leaf/>"#, "the root element cannot be a component"),
        ] {
            let error = compile_with(source, &components).unwrap_err();
            assert!(error.message.contains(expected), "{source}: {error}");
        }

        // Without a resolver, components are left out.
        let template = parse_template(r#"<div><Nope :a="b"/><p>x</p></div>"#).unwrap();
        let compiled = CompiledTemplate::compile(&template).unwrap();
        assert_eq!(compiled.get(NodeId(0)).slots, vec![Slot::Static(NodeId(1)), Slot::Static(NodeId(2))]);
        assert_eq!(*compiled.get(NodeId(1)), CompiledNode::default());
    }

    #[test]
    fn errors() {
        let orphan = parse_template(r#"<div><p v-else>x</p></div>"#).unwrap();
        assert!(CompiledTemplate::compile(&orphan).is_err());

        let both = parse_template(r#"<div><p v-if="a" v-for="x in xs">x</p></div>"#).unwrap();
        assert!(CompiledTemplate::compile(&both).is_err());

        let index_field = parse_template(r#"<div><p v-for="(x, i) in xs">{{i.a}}</p></div>"#).unwrap();
        assert!(CompiledTemplate::compile(&index_field).is_err());
    }
}
