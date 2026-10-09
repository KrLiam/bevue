use std::borrow::Cow;

use bevy::{
    asset::Asset,
    ecs::{component::Component, entity::Entity, hierarchy::ChildOf, system::Commands},
    prelude::Deref,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Identifier(Identifier),
    Access { target: Box<Expression>, field: Identifier },
    Index { target: Box<Expression>, index: Box<Expression> },
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

#[derive(Asset, TypePath, Debug, Clone, Default, PartialEq)]
pub struct BevueTemplate {
    pub nodes: Vec<TemplateNode>,
}

impl BevueTemplate {
    pub fn new(nodes: Vec<TemplateNode>) -> Self {
        Self { nodes }
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
            match &node.element {
                ElementType::Native(_) => {
                    entity_commands.insert(bevy::ui::Node::default());
                }
                ElementType::Component(_) => panic!("Components not supported yet."),
            }
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

impl std::ops::Deref for BevueTemplate {
    type Target = Vec<TemplateNode>;

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

impl std::ops::DerefMut for BevueTemplate {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.nodes
    }
}

impl From<Vec<TemplateNode>> for BevueTemplate {
    fn from(nodes: Vec<TemplateNode>) -> Self {
        Self { nodes }
    }
}


#[cfg(test)]
mod test {
    use crate::core::template::{
        BevueTemplate, ConditionDirective, Expression, Identifier, ListDirective, TemplateNode, NodeId, TemplateText,
    };

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