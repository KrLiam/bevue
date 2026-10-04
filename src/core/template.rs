use std::borrow::Cow;

use bevy::{
    asset::Asset,
    ecs::{entity::Entity, hierarchy::ChildOf, system::Commands},
    reflect::TypePath,
};


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
    pub children: Vec<usize>,
    pub text: Option<TemplateText>,
    pub list: Option<ListDirective>,
    pub condition: Option<ConditionDirective>,
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
        }
    }

    pub fn with_field(mut self, name: Cow<'static, str>, value: Expression) -> Self {
        self.attributes.push((name, value));
        self
    }

    pub fn with_child(mut self, child_index: usize) -> Self {
        self.children.push(child_index);
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Identifier(Identifier),
    Access { target: Box<Expression>, field: Identifier },
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


#[derive(Debug, Clone, PartialEq)]
pub struct ListDirective {
    pub target: Identifier,
    pub iterable: Expression,
}

impl ListDirective {
    pub fn new(target: impl Into<Identifier>, iterable: impl Into<Expression>) -> Self {
        Self { target: target.into(), iterable: iterable.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConditionDirective(pub Expression);

impl ConditionDirective {
    pub fn new(condition: impl Into<Expression>) -> Self {
        Self(condition.into())
    }
}

/// Alias for `TemplateNode` for backwards compatibility.
pub type TemplateNodeData = TemplateNode;

#[derive(Asset, TypePath, Debug, Clone, Default, PartialEq)]
pub struct BevueTemplate {
    pub nodes: Vec<TemplateNode>,
}

impl BevueTemplate {
    pub fn new(nodes: Vec<TemplateNode>) -> Self {
        Self { nodes }
    }

    /// Gets a reference to the node at the given index.
    pub fn get(&self, index: usize) -> Option<&TemplateNode> {
        self.nodes.get(index)
    }

    /// Gets a mutable reference to the node at the given index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut TemplateNode> {
        self.nodes.get_mut(index)
    }

    /// Returns the indices of the children of the node at `index`.
    pub fn children(&self, index: usize) -> &[usize] {
        self.nodes.get(index).map(|n| n.children.as_slice()).unwrap_or(&[])
    }

    /// Returns an iterator yielding `(child_index, &child_node)` for the node at `index`.
    pub fn child_nodes(&self, index: usize) -> impl Iterator<Item = (usize, &TemplateNode)> + '_ {
        self.children(index)
            .iter()
            .copied()
            .filter_map(|child_idx| self.nodes.get(child_idx).map(|node| (child_idx, node)))
    }

    /// Returns an iterator yielding the child indices of the node at `index`.
    pub fn child_node_indices(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.children(index).iter().copied()
    }

    /// Finds and returns the indices of all root nodes (nodes that are not children of any other node).
    pub fn roots(&self) -> Vec<usize> {
        let mut is_child = vec![false; self.nodes.len()];
        for node in &self.nodes {
            for &child in &node.children {
                if child < is_child.len() {
                    is_child[child] = true;
                }
            }
        }
        is_child
            .into_iter()
            .enumerate()
            .filter_map(|(i, child)| if !child { Some(i) } else { None })
            .collect()
    }

    /// Returns the index of the primary root node (defaults to 0, or first unparented node).
    pub fn root_index(&self) -> Option<usize> {
        if self.nodes.is_empty() {
            None
        } else {
            Some(self.roots().first().copied().unwrap_or(0))
        }
    }

    /// Returns a reference to the primary root node.
    pub fn root(&self) -> Option<&TemplateNode> {
        let root_idx = self.root_index()?;
        self.nodes.get(root_idx)
    }

    /// Spawns the template node tree into Bevy UI nodes.
    pub fn spawn(&self, commands: &mut Commands) -> Entity {
        if self.nodes.is_empty() {
            return commands.spawn_empty().id();
        }

        let entities: Vec<Entity> = (0..self.nodes.len())
            .map(|_| commands.spawn_empty().id())
            .collect();

        for (i, node) in self.nodes.iter().enumerate() {
            let entity = entities[i];
            let mut entity_commands = commands.entity(entity);
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
            for &child_idx in &node.children {
                if child_idx < entities.len() {
                    commands.entity(entities[child_idx]).insert(ChildOf(entity));
                }
            }
        }

        let root_idx = self.root_index().unwrap_or(0);
        entities[root_idx]
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
        BevueTemplate, ConditionDirective, Expression, Identifier, ListDirective, TemplateNode,
        TemplateText,
    };

    #[test]
    fn build() {
        let template = BevueTemplate::new(vec![
            TemplateNode::new("div")
                .with_condition(ConditionDirective::new(Identifier::new("active")))
                .with_child(1),
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

        assert_eq!(template.root_index(), Some(0));
        assert_eq!(template.children(0), &[1]);
        assert_eq!(template.children(1), &[]);
        let child_nodes: Vec<_> = template.child_nodes(0).collect();
        assert_eq!(child_nodes.len(), 1);
        assert_eq!(child_nodes[0].0, 1);
        assert_eq!(child_nodes[0].1.element, "p".into());
    }
}