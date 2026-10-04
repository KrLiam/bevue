use std::borrow::Cow;

use bevy::{
    asset::Asset,
    ecs::{component::Component, entity::Entity, hierarchy::ChildOf, system::Commands},
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


#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct TemplateNode {
    /// The element type.
    pub element: ElementType,
    pub attributes: Vec<(Cow<'static, str>, Expression)>,
}

impl TemplateNode {
    pub fn new(element_type: impl Into<ElementType>) -> Self {
        Self {
            element: element_type.into(),
            attributes: Vec::default()
        }
    }

    pub fn with_field(mut self, name: Cow<'static, str>, value: Expression) {
        self.attributes.push((name, value));
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


#[derive(Component, Debug, Clone, PartialEq)]
pub struct TemplateText(pub Vec<TextFragment>);

impl TemplateText {
    pub fn new(fragments: impl IntoIterator<Item=TextFragment>) -> Self {
        Self(fragments.into_iter().collect())
    }
}


#[derive(Component, Debug, Clone, PartialEq)]
pub struct ListDirective {
    pub target: Identifier,
    pub iterable: Expression,
}

impl ListDirective {
    pub fn new(target: impl Into<Identifier>, iterable: impl Into<Expression>) -> Self {
        Self { target: target.into(), iterable: iterable.into() }
    }
}

#[derive(Component, Debug, Clone, PartialEq)]
pub struct ConditionDirective(pub Expression);

impl ConditionDirective {
    pub fn new(condition: impl Into<Expression>) -> Self {
        Self(condition.into())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateNodeData {
    pub node: TemplateNode,
    pub children: Vec<usize>,
    pub text: Option<TemplateText>,
    pub list: Option<ListDirective>,
    pub condition: Option<ConditionDirective>,
}

impl TemplateNodeData {
    pub fn new(node: TemplateNode) -> Self {
        Self {
            node,
            children: Vec::new(),
            text: None,
            list: None,
            condition: None,
        }
    }
}

#[derive(Asset, TypePath, Debug, Clone, Default, PartialEq)]
pub struct BevueTemplate {
    pub nodes: Vec<TemplateNodeData>,
}

impl BevueTemplate {
    pub fn new(nodes: Vec<TemplateNodeData>) -> Self {
        Self { nodes }
    }

    pub fn spawn(&self, commands: &mut Commands) -> Entity {
        if self.nodes.is_empty() {
            return commands.spawn_empty().id();
        }

        let entities: Vec<Entity> = (0..self.nodes.len())
            .map(|_| commands.spawn_empty().id())
            .collect();

        for (i, data) in self.nodes.iter().enumerate() {
            let entity = entities[i];
            let mut entity_commands = commands.entity(entity);
            entity_commands.insert(data.node.clone());
            if let Some(text) = &data.text {
                entity_commands.insert(text.clone());
            }
            if let Some(list) = &data.list {
                entity_commands.insert(list.clone());
            }
            if let Some(condition) = &data.condition {
                entity_commands.insert(condition.clone());
            }
            for &child_idx in &data.children {
                if child_idx < entities.len() {
                    commands.entity(entities[child_idx]).insert(ChildOf(entity));
                }
            }
        }

        entities[0]
    }
}

impl std::ops::Deref for BevueTemplate {
    type Target = Vec<TemplateNodeData>;

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

impl std::ops::DerefMut for BevueTemplate {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.nodes
    }
}

impl From<Vec<TemplateNodeData>> for BevueTemplate {
    fn from(nodes: Vec<TemplateNodeData>) -> Self {
        Self { nodes }
    }
}


#[cfg(test)]
mod test {
    use bevy::ecs::children;

    use crate::core::template::{ConditionDirective, Expression, Identifier, ListDirective, TemplateNode, TemplateText};

    #[test]
    fn build() {
        let _ = (
            TemplateNode::new("div"),
            ConditionDirective::new(Identifier::new("active")),
            children! [
                (
                    TemplateNode::new("p"),
                    ListDirective::new("item", Identifier::new("items")),
                    TemplateText::new([
                        "hello ".into(),
                        Expression::Access {
                            target: Box::new(Expression::Identifier("item".into())),
                            field: "name".into(),
                        }.into(),
                        "!".into(),
                    ]),
                ),
            ],
        );
    }
}