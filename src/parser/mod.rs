use std::borrow::Cow;

use tl::{HTMLTag, Node, Parser, ParserOptions, VDom};



pub enum HtmlNode<'a> {
    OpenTag(&'a HTMLTag<'a>),
    CloseTag(&'a HTMLTag<'a>),
    Text(Cow<'a, str>), // Changed to 'a to avoid string allocations
}

pub struct HtmlIterator<'a> {
    parser: &'a Parser<'a>,
    nodes: Vec<(&'a Node<'a>, bool)>,
}

impl<'a> Iterator for HtmlIterator<'a> {
    type Item = HtmlNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, close)) = self.nodes.pop() {
            match node {
                Node::Tag(htmltag) if close => {
                    return Some(HtmlNode::CloseTag(htmltag));
                }
                Node::Tag(htmltag) => {
                    // Push self back as the closing tag so we encounter it after children
                    self.nodes.push((node, true));
                    
                    // Push children in reverse order so the first child is popped first
                    if let Some(children) = node.children() {
                        let start_len = self.nodes.len();
                        for handle in children.top().iter() {
                            if let Some(child_node) = handle.get(self.parser) {
                                self.nodes.push((child_node, false));
                            }
                        }
                        self.nodes[start_len..].reverse();
                    }
                    
                    return Some(HtmlNode::OpenTag(htmltag));
                }
                Node::Raw(bytes) => {
                    let s = bytes.as_utf8_str();
                    let trimmed = s.trim().to_owned();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // Yield borrowed text to avoid allocations
                    // (If you truly need 'static, you can do Cow::Owned(trimmed.to_string()))
                    return Some(HtmlNode::Text(trimmed.into()));
                }
                _ => continue,
            }
        }
        
        None
    }
}

/// Creates an iterator over the HTML nodes.
/// The `VDom` must be created beforehand so the iterator can borrow from it.
pub fn traverse_dom<'a>(dom: &'a VDom<'a>) -> HtmlIterator<'a> {
    let parser = dom.parser();
    
    // Grab top-level nodes and map them to our stack tuple format
    let nodes: Vec<_> = dom.children().iter()
        .filter_map(|handle| handle.get(parser).map(|n| (n, false)))
        .rev()
        .collect();
    
    HtmlIterator {
        parser,
        nodes,
    }
}

pub mod expr;
pub use expr::{
    parse_expression, parse_for, parse_template_text, try_parse_expression, try_parse_for,
    try_parse_template_text, ExprParseError, ForParts,
};

pub use crate::core::template::{
    BevueTemplate, ConditionDirective, ConditionKind, Expression, Identifier, ListDirective, NodeId, TemplateNode,
    TemplateRef, TemplateStyle, TemplateText, TextFragment,
};

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    InvalidHtml(String),
    InvalidExpression(String),
    InvalidDirective(String),
    Custom(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidHtml(msg) => write!(f, "Invalid HTML: {msg}"),
            ParseError::InvalidExpression(msg) => write!(f, "Invalid expression: {msg}"),
            ParseError::InvalidDirective(msg) => write!(f, "Invalid directive: {msg}"),
            ParseError::Custom(msg) => write!(f, "Parse error: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<ExprParseError> for ParseError {
    fn from(err: ExprParseError) -> Self {
        ParseError::InvalidExpression(err.0)
    }
}

pub type Result<T> = std::result::Result<T, ParseError>;

/// Parses a template source string into a `BevueTemplate`.
/// Supports `<template>` wrapper and `<style src="...">` tags.
pub fn parse_template(source: &str) -> Result<BevueTemplate> {
    let dom = tl::parse(source, ParserOptions::default())
        .map_err(|e| ParseError::InvalidHtml(format!("{e:?}")))?;

    let mut nodes: Vec<TemplateNode> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut depth: usize = 0;
    let mut in_style = false;
    let mut current_style_src: Option<String> = None;
    let mut current_style_inline = String::new();
    let mut parsed_style: Option<TemplateStyle> = None;
    let mut is_top_level_template = false;

    for node in traverse_dom(&dom) {
        match node {
            HtmlNode::OpenTag(tag) => {
                let tag_name = tag.name().as_utf8_str();

                if tag_name.eq_ignore_ascii_case("style") {
                    in_style = true;
                    current_style_inline.clear();
                    current_style_src = None;
                    for (name, value) in tag.attributes().iter() {
                        if name == "src" || (name == "href" && current_style_src.is_none()) {
                            current_style_src = value.map(|b| b.to_string());
                        }
                    }
                    continue;
                }

                if tag_name.eq_ignore_ascii_case("param") {
                    depth += 1;
                    continue;
                }

                if depth > 0 || in_style {
                    depth += 1;
                    continue;
                }

                if stack.is_empty() && tag_name.eq_ignore_ascii_case("template") {
                    is_top_level_template = true;
                    continue;
                }

                let mut template_node = TemplateNode::new(tag_name.to_string());
                let mut list_directive: Option<ListDirective> = None;
                let mut condition_directive: Option<ConditionDirective> = None;

                for (name, value) in tag.attributes().iter() {
                    let val_str = value.as_deref();
                    if name == "v-for" {
                        let val = val_str.ok_or_else(|| {
                            ParseError::InvalidDirective("v-for requires a value".to_owned())
                        })?;
                        let (target, index, iterable) = try_parse_for(val)?;
                        let mut directive = ListDirective::new(target, iterable);
                        directive.index = index;
                        list_directive = Some(directive);
                    } else if name == "v-if" || name == "v-else-if" {
                        let val = val_str.ok_or_else(|| {
                            ParseError::InvalidDirective(format!("{name} requires a value"))
                        })?;
                        let condition = try_parse_expression(val)?;
                        condition_directive = Some(if name == "v-if" {
                            ConditionDirective::new(condition)
                        } else {
                            ConditionDirective::else_if(condition)
                        });
                    } else if name == "v-else" {
                        condition_directive = Some(ConditionDirective::otherwise());
                    } else if let Some(stripped) = name.strip_prefix(':') {
                        if let Some(val) = val_str {
                            let expr = try_parse_expression(val)?;
                            template_node.attributes.push((stripped.to_string().into(), expr));
                        }
                    } else if let Some(stripped) = name.strip_prefix("v-bind:") {
                        if let Some(val) = val_str {
                            let expr = try_parse_expression(val)?;
                            template_node.attributes.push((stripped.to_string().into(), expr));
                        }
                    } else if let Some(val) = val_str {
                        template_node
                            .attributes
                            .push((name.to_string().into(), Expression::String(val.to_string().into())));
                    } else {
                        // Boolean attribute (e.g. disabled)
                        template_node
                            .attributes
                            .push((name.to_string().into(), Expression::Bool(true)));
                    }
                }

                // `:key` belongs to the list directive, not to the element.
                if let Some(directive) = &mut list_directive
                    && let Some(pos) = template_node.attributes.iter().position(|(name, _)| name == "key")
                {
                    directive.key = Some(template_node.attributes.remove(pos).1);
                }

                template_node.list = list_directive;
                template_node.condition = condition_directive;

                let node_idx = nodes.len();
                nodes.push(template_node);

                if let Some(&parent_idx) = stack.last() {
                    nodes[parent_idx].children.push(node_idx.into());
                }

                stack.push(node_idx);
            }
            HtmlNode::CloseTag(tag) => {
                let tag_name = tag.name().as_utf8_str();
                if tag_name.eq_ignore_ascii_case("style") && in_style {
                    in_style = false;
                    parsed_style = Some(TemplateStyle {
                        src: current_style_src.take().map(Into::into),
                        inline: current_style_inline.trim().to_string().into(),
                    });
                    continue;
                }
                if depth > 0 {
                    depth -= 1;
                    continue;
                }
                if tag_name.eq_ignore_ascii_case("template") && is_top_level_template && stack.is_empty() {
                    is_top_level_template = false;
                    continue;
                }
                stack.pop();
            }
            HtmlNode::Text(text) => {
                if in_style {
                    current_style_inline.push_str(&text);
                    continue;
                }
                if depth > 0 {
                    continue;
                }
                if let Some(&current_idx) = stack.last() {
                    let parsed_text = try_parse_template_text(&text)?;
                    if !parsed_text.0.is_empty() {
                        if let Some(existing) = &mut nodes[current_idx].text {
                            existing.0.extend(parsed_text.0);
                        } else {
                            nodes[current_idx].text = Some(parsed_text);
                        }
                    }
                }
            }
        }
    }

    let mut template = BevueTemplate::new(nodes);
    if let Some(style) = parsed_style
        && let Some(root_idx) = template.root_index()
        && let Some(node) = template.get_mut(root_idx)
    {
        node.style = Some(style);
    }

    Ok(template)
}

#[cfg(test)]
mod test {
    use bevy::{
        app::App,
        ecs::{
            hierarchy::{ChildOf, Children},
        },
        MinimalPlugins,
    };

    use crate::core::template::NodeId;

use super::*;

    const TEMPLATE: &str = r#"
        <layout>
            <param
                name="menu"
                source="resource"
                type="example::types::ContextMenu"
            />

            <style href="css/context_menu.css"/>
            
            <div class="context-menu-root">
                <div class="wrapper">
                    <div class="separator">
                        <p class="context-menu-title">{{menu.title}}</p>
                    </div>
                    <button v-for="action in menu.actions" :key="action.id">
                        <span>{{action.label}}</span>
                    </button>
                </div>
            </div>
        </layout>
    "#;

    #[test]
    fn simple() {
        let result = parse_template(TEMPLATE).expect("Failed to parse template");
        assert_eq!(result.nodes.len(), 7);

        // Node 0: <layout>
        assert_eq!(result.nodes[0].element, "layout".into());
        assert_eq!(result.nodes[0].children, vec![NodeId(1)]);

        // Node 1: <div class="context-menu-root">
        assert_eq!(result.nodes[1].element, "div".into());
        assert_eq!(
            result.nodes[1].attributes,
            vec![("class".into(), Expression::String("context-menu-root".into()))]
        );
        assert_eq!(result.nodes[1].children, vec![NodeId(2)]);

        // Node 2: <div class="wrapper">
        assert_eq!(result.nodes[2].element, "div".into());
        assert_eq!(result.nodes[2].children, vec![NodeId(3), NodeId(5)]);

        // Node 3: <div class="separator">
        assert_eq!(result.nodes[3].element, "div".into());
        assert_eq!(result.nodes[3].children, vec![NodeId(4)]);

        // Node 4: <p class="context-menu-title">{{menu.title}}</p>
        assert_eq!(result.nodes[4].element, "p".into());
        assert_eq!(result.nodes[4].children, vec![]);
        assert_eq!(
            result.nodes[4].text,
            Some(TemplateText(vec![TextFragment::Interpolated(
                Expression::Access {
                    target: Box::new(Expression::Identifier("menu".into())),
                    field: "title".into(),
                }
            )]))
        );

        // Node 5: <button v-for="action in menu.actions" :key="action.id">
        assert_eq!(result.nodes[5].element, "button".into());
        assert_eq!(
            result.nodes[5].list,
            Some(ListDirective::new(
                "action",
                Expression::Access {
                    target: Box::new(Expression::Identifier("menu".into())),
                    field: "actions".into(),
                }
            ).with_key(Expression::Access {
                target: Box::new(Expression::Identifier("action".into())),
                field: "id".into(),
            }))
        );
        assert_eq!(result.nodes[5].attributes, vec![]);
        assert_eq!(result.nodes[5].children, vec![NodeId(6)]);

        // Node 6: <span>{{action.label}}</span>
        assert_eq!(result.nodes[6].element, "span".into());
        assert_eq!(result.nodes[6].children, vec![]);
        assert_eq!(
            result.nodes[6].text,
            Some(TemplateText(vec![TextFragment::Interpolated(
                Expression::Access {
                    target: Box::new(Expression::Identifier("action".into())),
                    field: "label".into(),
                }
            )]))
        );

        // Verify helper methods on BevueTemplate
        assert_eq!(result.children(NodeId(0)), &[NodeId(1)]);
        assert_eq!(result.children(NodeId(2)), &[NodeId(3), NodeId(5)]);
        assert_eq!(result.children(NodeId(6)), &[]);
        assert_eq!(result.root_index(), Some(NodeId(0)));
        assert_eq!(result.root().unwrap().element, "layout".into());
    }

    #[test]
    fn test_spawn() {
        let template = parse_template(TEMPLATE).expect("Failed to parse template");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let mut commands = app.world_mut().commands();
        let root = template.spawn(&mut commands);

        app.update();

        let world = app.world();
        assert!(world.get::<bevy::ui::Node>(root).is_some());

        let children = world.get::<Children>(root).expect("Root should have children");
        assert_eq!(children.len(), 1);

        let div_root = children[0];
        assert!(world.get::<bevy::ui::Node>(div_root).is_some());
        assert_eq!(
            world.get::<ChildOf>(div_root).expect("ChildOf parent").parent(),
            root
        );
    }

    #[test]
    fn test_parse_conditions() {
        let source = r#"
            <div>
                <p v-if="a">A</p>
                <p v-else-if="b">B</p>
                <p v-else>C</p>
                <span v-for="(item, i) in items">{{item}}</span>
            </div>
        "#;
        let template = parse_template(source).expect("Failed to parse template");
        assert_eq!(template.nodes[1].condition, Some(ConditionDirective::new(Identifier::new("a"))));
        assert_eq!(template.nodes[2].condition, Some(ConditionDirective::else_if(Identifier::new("b"))));
        assert_eq!(template.nodes[3].condition, Some(ConditionDirective::otherwise()));
        assert_eq!(
            template.nodes[4].list,
            Some(ListDirective::new("item", Identifier::new("items")).with_index("i"))
        );
    }

    #[test]
    fn test_parse_template_with_style_and_wrapper() {
        let source = r#"
            <template>
                <div class="root">
                    <p>Hello!</p>
                    <div class="inner">
                        <span>You</span>
                    </div>
                </div>
            </template>

            <style src="css/simple.css"></style>
        "#;

        let template = parse_template(source).expect("Failed to parse template");
        let root = template.root().expect("Root node should exist");
        assert_eq!(root.element, "div".into());
        assert_eq!(
            root.attributes,
            vec![("class".into(), Expression::String("root".into()))]
        );
        assert_eq!(
            root.style,
            Some(TemplateStyle {
                src: Some("css/simple.css".into()),
                inline: "".into(),
            })
        );
        assert_eq!(root.children.len(), 2);
    }
}