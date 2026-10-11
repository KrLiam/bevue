use std::borrow::Cow;

use bevy::{
    asset::{Asset, AssetServer, Handle},
    ecs::{component::Component, entity::Entity, hierarchy::ChildOf, system::Commands},
    prelude::{Deref, DerefMut},
    reflect::TypePath,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateStyle {
    pub src: Option<Cow<'static, str>>,
    pub inline: Cow<'static, str>,
}

impl TemplateStyle {
    pub fn new(src: Option<impl Into<Cow<'static, str>>>, inline: impl Into<Cow<'static, str>>) -> Self {
        Self {
            src: src.map(Into::into),
            inline: inline.into(),
        }
    }

    pub fn from_src(src: impl Into<Cow<'static, str>>) -> Self {
        Self {
            src: Some(src.into()),
            inline: Cow::Borrowed(""),
        }
    }

    pub fn from_inline(inline: impl Into<Cow<'static, str>>) -> Self {
        Self {
            src: None,
            inline: inline.into(),
        }
    }
}

/// The type of a template node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementType {
    /// Native HTML element such as `div`,`p`, `span`, `button` etc.
    Native(Cow<'static, str>),
    /// A user defined component.
    Component(Cow<'static, str>),
}

impl Default for ElementType {
    fn default() -> Self {
        ElementType::Native("div".into())
    }
}

impl From<String> for ElementType {
    fn from(value: String) -> Self {
        let ch = value.chars().next().expect("Element type cannot be an empty name.");
        if ch.is_uppercase() {
            ElementType::Component(value.into())
        }
        else {
            ElementType::Native(value.into())
        }
    }
}

impl From<&'static str> for ElementType {
    fn from(value: &'static str) -> Self {
        let ch = value.chars().next().expect("Element type cannot be an empty name.");
        if ch.is_uppercase() {
            ElementType::Component(value.into())
        }
        else {
            ElementType::Native(value.into())
        }
    }
}


#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateNode {
    /// The element type.
    pub element: ElementType,
    pub attributes: Vec<(Cow<'static, str>, Expression)>,
    pub children: Vec<NodeId>,
    pub text: Option<TemplateText>,
    pub list: Option<ListDirective>,
    pub condition: Option<ConditionDirective>,
    pub style: Option<TemplateStyle>,
    /// Event listeners from `@name="handler"` / `v-on:name="handler"`. A listener without a
    /// handler (`@click`) still makes the node interactive.
    pub events: Vec<(Cow<'static, str>, Option<Expression>)>,
}

impl TemplateNode {
    pub fn new(element_type: impl Into<ElementType>) -> Self {
        Self {
            element: element_type.into(),
            attributes: Vec::default(),
            children: Vec::default(),
            text: None,
            list: None,
            condition: None,
            style: None,
            events: Vec::default(),
        }
    }

    pub fn with_field(mut self, name: Cow<'static, str>, value: Expression) -> Self {
        self.attributes.push((name, value));
        self
    }

    pub fn with_child(mut self, child_index: impl Into<NodeId>) -> Self {
        self.children.push(child_index.into());
        self
    }

    pub fn with_text(mut self, text: impl Into<TemplateText>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_list(mut self, list: ListDirective) -> Self {
        self.list = Some(list);
        self
    }

    pub fn with_condition(mut self, condition: ConditionDirective) -> Self {
        self.condition = Some(condition);
        self
    }

    pub fn with_event(mut self, name: impl Into<Cow<'static, str>>, handler: Option<Expression>) -> Self {
        self.events.push((name.into(), handler));
        self
    }

    pub fn with_style(mut self, style: TemplateStyle) -> Self {
        self.style = Some(style);
        self
    }
}


#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identifier(Cow<'static, str>);

impl From<&'static str> for Identifier {
    fn from(value: &'static str) -> Self {
        Identifier(value.into())
    }
}
impl From<String> for Identifier {
    fn from(value: String) -> Self {
        Identifier(value.into())
    }
}
impl From<Cow<'static, str>> for Identifier {
    fn from(value: Cow<'static, str>) -> Self {
        Identifier(value)
    }
}

impl Identifier {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A prefix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `!a`
    Not,
    /// `-a`
    Neg,
    /// `+a`
    Pos,
}

/// An infix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl BinaryOp {
    /// How tightly the operator binds; a higher value binds tighter.
    pub fn precedence(self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq | BinaryOp::Ne => 3,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 4,
            BinaryOp::Add | BinaryOp::Sub => 5,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Identifier(Identifier),
    Access { target: Box<Expression>, field: Identifier },
    Index { target: Box<Expression>, index: Box<Expression> },
    /// A call to a registered function: `function(args...)`.
    Call { function: Identifier, args: Vec<Expression> },
    /// An object literal: `{ key: expr, 'quoted-key': expr }`, in source order.
    Object(Vec<(Cow<'static, str>, Expression)>),
    Unary { op: UnaryOp, operand: Box<Expression> },
    Binary { op: BinaryOp, left: Box<Expression>, right: Box<Expression> },
    /// `condition ? then : otherwise`
    Conditional { condition: Box<Expression>, then: Box<Expression>, otherwise: Box<Expression> },
    /// A template string: `` `Hello, ${user.name}!` ``.
    TemplateString(TemplateText),
    Float(f32),
    Integer(i32),
    String(Cow<'static, str>),
    Bool(bool),
}

impl From<Identifier> for Expression {
    fn from(value: Identifier) -> Self {
        Expression::Identifier(value)
    }
}

impl From<f32> for Expression {
    fn from(value: f32) -> Self {
        Expression::Float(value)
    }
}

impl From<i32> for Expression {
    fn from(value: i32) -> Self {
        Expression::Integer(value)
    }
}

impl From<&'static str> for Expression {
    fn from(value: &'static str) -> Self {
        Expression::String(value.into())
    }
}
impl From<String> for Expression {
    fn from(value: String) -> Self {
        Expression::String(value.into())
    }
}
impl From<Cow<'static, str>> for Expression {
    fn from(value: Cow<'static, str>) -> Self {
        Expression::String(value)
    }
}

impl From<bool> for Expression {
    fn from(value: bool) -> Self {
        Expression::Bool(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TextFragment {
    Literal(Cow<'static, str>),
    Interpolated(Expression),
}

impl From<Cow<'static, str>> for TextFragment {
    fn from(value: Cow<'static, str>) -> Self {
        TextFragment::Literal(value)
    }
}
impl From<&'static str> for TextFragment {
    fn from(value: &'static str) -> Self {
        TextFragment::Literal(value.into())
    }
}
impl From<String> for TextFragment {
    fn from(value: String) -> Self {
        TextFragment::Literal(value.into())
    }
}
impl From<Expression> for TextFragment {
    fn from(value: Expression) -> Self {
        TextFragment::Interpolated(value)
    }
}


#[derive(Debug, Clone, PartialEq)]
pub struct TemplateText(pub Vec<TextFragment>);

impl TemplateText {
    pub fn new(fragments: impl IntoIterator<Item=TextFragment>) -> Self {
        Self(fragments.into_iter().collect())
    }
}


/// A `v-for="item in iterable"` or `v-for="(item, index) in iterable"` directive.
#[derive(Debug, Clone, PartialEq)]
pub struct ListDirective {
    /// The alias bound to each element.
    pub target: Identifier,
    /// The optional alias bound to each element's index.
    pub index: Option<Identifier>,
    pub iterable: Expression,
    /// The `:key` expression used to match elements across updates.
    pub key: Option<Expression>,
}

impl ListDirective {
    pub fn new(target: impl Into<Identifier>, iterable: impl Into<Expression>) -> Self {
        Self { target: target.into(), index: None, iterable: iterable.into(), key: None }
    }

    pub fn with_index(mut self, index: impl Into<Identifier>) -> Self {
        self.index = Some(index.into());
        self
    }

    pub fn with_key(mut self, key: impl Into<Expression>) -> Self {
        self.key = Some(key.into());
        self
    }
}

/// The kind of a conditional rendering directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionKind {
    /// `v-if`, starts a chain.
    If,
    /// `v-else-if`, continues the chain started by a preceding sibling.
    ElseIf,
    /// `v-else`, ends the chain started by a preceding sibling.
    Else,
}

/// A `v-if`, `v-else-if` or `v-else` directive.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionDirective {
    pub kind: ConditionKind,
    /// The condition; `None` only for `v-else`.
    pub expr: Option<Expression>,
}

impl ConditionDirective {
    /// Creates a `v-if` directive.
    pub fn new(condition: impl Into<Expression>) -> Self {
        Self { kind: ConditionKind::If, expr: Some(condition.into()) }
    }

    /// Creates a `v-else-if` directive.
    pub fn else_if(condition: impl Into<Expression>) -> Self {
        Self { kind: ConditionKind::ElseIf, expr: Some(condition.into()) }
    }

    /// Creates a `v-else` directive.
    pub fn otherwise() -> Self {
        Self { kind: ConditionKind::Else, expr: None }
    }
}

#[derive(Debug, Clone, Copy, Deref, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

impl From<usize> for NodeId {
    fn from(value: usize) -> Self {
        NodeId(value)
    }
}

impl NodeId {
    pub fn root() -> NodeId {
        NodeId(0)
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateRef {
    pub root: Entity,
    pub node: NodeId,
}

impl TemplateRef {
    pub fn new(root: Entity, node: impl Into<NodeId>) -> Self {
        Self {
            root,
            node: node.into(),
        }
    }
}

/// Where the data of a [`PropsDeclaration`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropsSourceKind {
    /// `<resource>`: a resource.
    Resource,
    /// `<component>`: a component of the template entity.
    Component,
}

/// A `<resource>` or `<component>` element of the template's `<props>`: a registered `Props` type
/// the template mounts in its props.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropsDeclaration {
    pub source: PropsSourceKind,
    /// The key the type is mounted at. `None` uses the type's `Props::name`.
    pub name: Option<Cow<'static, str>>,
    /// The type, by its `Props::type_path` or `Props::short_type_path`.
    pub type_path: Cow<'static, str>,
}

impl PropsDeclaration {
    pub fn resource(type_path: impl Into<Cow<'static, str>>) -> Self {
        Self { source: PropsSourceKind::Resource, name: None, type_path: type_path.into() }
    }

    pub fn component(type_path: impl Into<Cow<'static, str>>) -> Self {
        Self { source: PropsSourceKind::Component, name: None, type_path: type_path.into() }
    }

    pub fn with_name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// A component imported by a template with `<import src="item_row.html"/>`, usable in that
/// template as `<ItemRow/>`.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateImport {
    /// The element name of the component.
    pub name: Cow<'static, str>,
    /// The path of the component's file, as written.
    pub src: Cow<'static, str>,
    /// The component's template. The asset loader sets it; a template parsed in code has `None`
    /// until [`BevueTemplate::load_imports`] is called.
    pub handle: Option<Handle<BevueTemplate>>,
}

impl TemplateImport {
    /// The component name of a file: its stem in PascalCase, so `ui/item_row.html` and
    /// `item-row.html` are both `ItemRow`. Returns `None` if that does not start with a letter.
    pub fn component_name(src: &str) -> Option<String> {
        let file = src.rsplit(['/', '\\']).next().unwrap_or(src);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let mut name = String::with_capacity(stem.len());
        for part in stem.split(|c: char| !c.is_alphanumeric()) {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                name.extend(first.to_uppercase());
                name.push_str(chars.as_str());
            }
        }
        name.starts_with(|c: char| c.is_ascii_uppercase()).then_some(name)
    }
}

#[derive(Asset, TypePath, Debug, Clone, Default, PartialEq, Deref, DerefMut)]
pub struct BevueTemplate {
    #[deref]
    pub nodes: Vec<TemplateNode>,
    /// The props declared in the template's `<props>` element.
    pub props: Vec<PropsDeclaration>,
    /// The components imported with `<import>` elements, which only this template can use.
    pub imports: Vec<TemplateImport>,
}

impl BevueTemplate {
    pub fn new(nodes: Vec<TemplateNode>) -> Self {
        Self { nodes, props: Vec::new(), imports: Vec::new() }
    }

    /// Makes the template `handle` usable in this template as the component `<name/>`, replacing
    /// an import of the same name.
    pub fn with_import(mut self, name: impl Into<Cow<'static, str>>, handle: Handle<BevueTemplate>) -> Self {
        let name = name.into();
        self.imports.retain(|import| import.name != name);
        self.imports.push(TemplateImport { name, src: Cow::Borrowed(""), handle: Some(handle) });
        self
    }

    /// Loads the `<import>`s that have no handle yet, with their paths taken from the asset root.
    /// The asset loader does this for template files; call it for templates parsed in code.
    pub fn load_imports(&mut self, asset_server: &AssetServer) {
        for import in &mut self.imports {
            if import.handle.is_none() {
                import.handle = Some(asset_server.load(import.src.to_string()));
            }
        }
    }

    pub fn with_props(mut self, props: impl IntoIterator<Item = PropsDeclaration>) -> Self {
        self.props.extend(props);
        self
    }

    /// Gets a reference to the node at the given index.
    pub fn get(&self, id: impl Into<NodeId>) -> Option<&TemplateNode> {
        self.nodes.get(*id.into())
    }

    /// Gets a mutable reference to the node at the given index.
    pub fn get_mut(&mut self, id: impl Into<NodeId>) -> Option<&mut TemplateNode> {
        self.nodes.get_mut(*id.into())
    }

    /// Returns the indices of the children of the node at `index`.
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.nodes.get(*id).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    /// Returns an iterator yielding `(child_index, &child_node)` for the node at `index`.
    pub fn child_nodes(&self, id: NodeId) -> impl Iterator<Item = (NodeId, &TemplateNode)> + '_ {
        self.children(id)
            .iter()
            .copied()
            .filter_map(|child_id| self.nodes.get(*child_id).map(|node| (child_id, node)))
    }

    /// Returns an iterator yielding the child indices of the node at `index`.
    pub fn child_node_indices(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id).iter().copied()
    }

    /// Finds and returns the indices of all root nodes (nodes that are not children of any other node).
    pub fn roots(&self) -> Vec<NodeId> {
        let mut is_child = vec![false; self.nodes.len()];
        for node in &self.nodes {
            for &child in &node.children {
                if *child < is_child.len() {
                    is_child[*child] = true;
                }
            }
        }
        is_child
            .into_iter()
            .enumerate()
            .filter_map(|(i, child)| if !child { Some(NodeId(i)) } else { None })
            .collect()
    }

    /// Returns the index of the primary root node (defaults to 0, or first unparented node).
    pub fn root_index(&self) -> Option<NodeId> {
        if self.nodes.is_empty() {
            None
        } else {
            Some(self.roots().first().copied().unwrap_or(NodeId(0)))
        }
    }

    /// Returns a reference to the primary root node.
    pub fn root(&self) -> Option<&TemplateNode> {
        let root_id = self.root_index()?;
        self.nodes.get(*root_id)
    }

    /// Spawns the template node tree into Bevy UI nodes.
    pub fn spawn(&self, commands: &mut Commands) -> Entity {
        if self.nodes.is_empty() {
            return commands.spawn_empty().id();
        }

        let entities: Vec<Entity> = (0..self.nodes.len())
            .map(|_| commands.spawn_empty().id())
            .collect();

        let root_id = self.root_index().unwrap_or(NodeId(0));
        let root_entity = entities[*root_id];

        for (i, node) in self.nodes.iter().enumerate() {
            let entity = entities[i];
            let mut entity_commands = commands.entity(entity);
            entity_commands.insert(TemplateRef {
                root: root_entity,
                node: NodeId(i),
            });
            // Components are only expanded by the runtime; here they are empty nodes.
            entity_commands.insert(bevy::ui::Node::default());
            if let Some(text) = &node.text {
                let mut acc = String::new();
                for fragment in &text.0 {
                    if let TextFragment::Literal(s) = fragment {
                        acc.push_str(s);
                    }
                }
                entity_commands.insert(bevy::ui::widget::Text::new(acc));
            }
            for &child_id in &node.children {
                if *child_id < entities.len() {
                    commands.entity(entities[*child_id]).insert(ChildOf(entity));
                }
            }
        }

        let root_id = self.root_index().unwrap_or(NodeId(0));
        entities[*root_id]
    }
}

impl From<Vec<TemplateNode>> for BevueTemplate {
    fn from(nodes: Vec<TemplateNode>) -> Self {
        Self::new(nodes)
    }
}


#[cfg(test)]
mod test {
    use crate::core::template::{
        BevueTemplate, ConditionDirective, Expression, Identifier, ListDirective, TemplateNode, NodeId, TemplateText,
    };

    #[test]
    fn component_names() {
        use crate::core::template::TemplateImport;
        for (src, expected) in [
            ("button.html", Some("Button")),
            ("ui/item_row.html", Some("ItemRow")),
            ("./item-row.html", Some("ItemRow")),
            ("mem://a/b/contextMenu.v2.bevue", Some("ContextMenuV2")),
            ("Panel", Some("Panel")),
            ("3d.html", None),
            ("ui/", None),
        ] {
            assert_eq!(TemplateImport::component_name(src).as_deref(), expected, "{src}");
        }
    }

    #[test]
    fn build() {
        let template = BevueTemplate::new(vec![
            TemplateNode::new("div")
                .with_condition(ConditionDirective::new(Identifier::new("active")))
                .with_child(NodeId(1)),
            TemplateNode::new("p")
                .with_list(ListDirective::new("item", Identifier::new("items")))
                .with_text(TemplateText::new([
                    "hello ".into(),
                    Expression::Access {
                        target: Box::new(Expression::Identifier("item".into())),
                        field: "name".into(),
                    }
                    .into(),
                    "!".into(),
                ])),
        ]);

        assert_eq!(template.root_index(), Some(NodeId(0)));
        assert_eq!(template.children(NodeId(0)), &[NodeId(1)]);
        assert_eq!(template.children(NodeId(1)), &[]);
        let child_nodes: Vec<_> = template.child_nodes(NodeId(0)).collect();
        assert_eq!(child_nodes.len(), 1);
        assert_eq!(child_nodes[0].0, NodeId(1));
        assert_eq!(child_nodes[0].1.element, "p".into());
    }
}