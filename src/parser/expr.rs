use std::borrow::Cow;

use winnow::ascii::multispace0;
use winnow::combinator::{delimited, opt};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{literal, one_of, take_while};

use crate::core::template::{Expression, Identifier, TemplateText, TextFragment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprParseError(pub String);

impl std::fmt::Display for ExprParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ExprParseError {}

/// Parses an identifier string: [a-zA-Z_][a-zA-Z0-9_]*
fn ident_str<'s>(input: &mut &'s str) -> ModalResult<&'s str> {
    let start = *input;
    let head = one_of(('a'..='z', 'A'..='Z', '_')).parse_next(input)?;
    let tail: &str = take_while(0.., ('a'..='z', 'A'..='Z', '0'..='9', '_')).parse_next(input)?;
    let len = head.len_utf8() + tail.len();
    Ok(&start[..len])
}

fn parse_identifier(input: &mut &str) -> ModalResult<Identifier> {
    let s: &str = ident_str.parse_next(input)?;
    Ok(Identifier::new(s.to_string()))
}

/// Parses string escape sequences and characters inside quotes
fn parse_string_literal(input: &mut &str) -> ModalResult<String> {
    let quote = one_of(('"', '\'')).parse_next(input)?;
    let mut out = String::new();
    loop {
        if input.is_empty() {
            return Err(winnow::error::ErrMode::Backtrack(ContextError::new()));
        }
        let ch = one_of(|_| true).parse_next(input)?;
        if ch == quote {
            break;
        }
        if ch == '\\' {
            let esc = one_of(|_| true).parse_next(input)?;
            match esc {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

/// Parses float: [+-]?[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?
fn parse_float_literal(input: &mut &str) -> ModalResult<f32> {
    let checkpoint = *input;
    let _sign: Option<char> = opt(one_of(('+', '-'))).parse_next(input)?;
    let _int_part: &str = take_while(1.., '0'..='9').parse_next(input)?;
    let _dot = literal(".").parse_next(input)?;
    let _frac_part: &str = take_while(1.., '0'..='9').parse_next(input)?;
    
    // Optional exponent
    let exp: Option<char> = opt(one_of(('e', 'E'))).parse_next(input)?;
    if exp.is_some() {
        let _: Option<char> = opt(one_of(('+', '-'))).parse_next(input)?;
        let _: &str = take_while(1.., '0'..='9').parse_next(input)?;
    }
    
    let float_slice_len = checkpoint.len() - input.len();
    let slice = &checkpoint[..float_slice_len];
    slice.parse::<f32>().map_err(|_| winnow::error::ErrMode::Backtrack(ContextError::new()))
}

/// Parses integer: [+-]?[0-9]+
fn parse_int_literal(input: &mut &str) -> ModalResult<i32> {
    let checkpoint = *input;
    let _sign: Option<char> = opt(one_of(('+', '-'))).parse_next(input)?;
    let _digits: &str = take_while(1.., '0'..='9').parse_next(input)?;
    let int_slice_len = checkpoint.len() - input.len();
    let slice = &checkpoint[..int_slice_len];
    slice.parse::<i32>().map_err(|_| winnow::error::ErrMode::Backtrack(ContextError::new()))
}

/// Parses an atom (primary expression before any dot-access):
/// - parenthesized expression
/// - string literal
/// - float literal
/// - integer literal
/// - boolean or identifier
fn parse_atom(input: &mut &str) -> ModalResult<Expression> {
    let _: &str = multispace0.parse_next(input)?;
    
    // Parentheses
    if input.starts_with('(') {
        let expr = delimited('(', parse_expr_internal, ')').parse_next(input)?;
        return Ok(expr);
    }
    
    // String literal
    if input.starts_with('"') || input.starts_with('\'') {
        let s = parse_string_literal.parse_next(input)?;
        return Ok(Expression::String(Cow::Owned(s)));
    }
    
    // Try float before int
    let checkpoint = *input;
    if let Ok(f) = parse_float_literal.parse_next(input) {
        return Ok(Expression::Float(f));
    }
    *input = checkpoint;
    
    // Try int
    if let Ok(i) = parse_int_literal.parse_next(input) {
        return Ok(Expression::Integer(i));
    }
    *input = checkpoint;
    
    // Identifier or bool
    let id_str = ident_str.parse_next(input)?;
    match id_str {
        "true" => Ok(Expression::Bool(true)),
        "false" => Ok(Expression::Bool(false)),
        other => Ok(Expression::Identifier(Identifier::new(other.to_string()))),
    }
}

/// Parses expression with chained member access: `target.field1.field2`
fn parse_expr_internal(input: &mut &str) -> ModalResult<Expression> {
    let _: &str = multispace0.parse_next(input)?;
    let mut current = parse_atom.parse_next(input)?;
    
    loop {
        let _: &str = multispace0.parse_next(input)?;
        if input.starts_with('.') {
            // Make sure the dot is member access and not something else
            let checkpoint = *input;
            let _ = literal(".").parse_next(input)?;
            let _: &str = multispace0.parse_next(input)?;
            match ident_str.parse_next(input) {
                Ok(field_name) => {
                    current = Expression::Access {
                        target: Box::new(current),
                        field: Identifier::new(field_name.to_string()),
                    };
                }
                Err(_) => {
                    // Not an identifier after dot; rollback
                    *input = checkpoint;
                    break;
                }
            }
        } else {
            break;
        }
    }
    
    let _: &str = multispace0.parse_next(input)?;
    Ok(current)
}

fn parse_for_internal(input: &mut &str) -> ModalResult<(Identifier, Expression)> {
    let target = parse_identifier.parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    let _ = literal("in").parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    let iterable = parse_expr_internal.parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    Ok((target, iterable))
}

/// Parses expression from string, ensuring all input is consumed.
pub fn try_parse_expression(value: &str) -> Result<Expression, ExprParseError> {
    let mut input = value.trim();
    match parse_expr_internal.parse_next(&mut input) {
        Ok(expr) if input.is_empty() => Ok(expr),
        Ok(_) => Err(ExprParseError(format!("Trailing characters after expression in: '{value}'"))),
        Err(e) => Err(ExprParseError(format!("Failed to parse expression '{value}': {e}"))),
    }
}

/// Parses an expression. Panics with a helpful message on syntax error.
pub fn parse_expression(value: &str) -> Expression {
    try_parse_expression(value).unwrap_or_else(|e| panic!("{e}"))
}

/// Parses `<identifier> in <expression>` from `v-for` attributes.
pub fn try_parse_for(value: &str) -> Result<(Identifier, Expression), ExprParseError> {
    let mut input = value.trim();
    match parse_for_internal.parse_next(&mut input) {
        Ok(res) if input.is_empty() => Ok(res),
        Ok(_) => Err(ExprParseError(format!("Trailing characters in v-for: '{input}'"))),
        Err(e) => Err(ExprParseError(format!("Failed to parse v-for '{value}': {e}"))),
    }
}

/// Parses `<identifier> in <expression>` from `v-for` attributes. Panics on syntax error.
pub fn parse_for(value: &str) -> (Identifier, Expression) {
    try_parse_for(value).unwrap_or_else(|e| panic!("{e}"))
}

/// Parses a template text containing potential `{{expr}}` interpolations.
pub fn try_parse_template_text(value: &str) -> Result<TemplateText, ExprParseError> {
    let mut fragments = Vec::new();
    let mut remaining = value;

    while let Some(start_idx) = remaining.find("{{") {
        let literal_part = &remaining[..start_idx];
        if !literal_part.is_empty() {
            fragments.push(TextFragment::Literal(literal_part.to_string().into()));
        }

        let after_open = &remaining[start_idx + 2..];
        let end_idx = after_open
            .find("}}")
            .ok_or_else(|| ExprParseError(format!("Unclosed interpolation in: '{value}'")))?;

        let expr_str = &after_open[..end_idx];
        let expr = try_parse_expression(expr_str)?;
        fragments.push(TextFragment::Interpolated(expr));

        remaining = &after_open[end_idx + 2..];
    }

    if !remaining.is_empty() {
        fragments.push(TextFragment::Literal(remaining.to_string().into()));
    }

    Ok(TemplateText::new(fragments))
}

/// Parses template text containing potential `{{expr}}` interpolations. Panics on syntax error.
pub fn parse_template_text(value: &str) -> TemplateText {
    try_parse_template_text(value).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ident_and_access() {
        let expr = parse_expression("menu.title");
        assert_eq!(
            expr,
            Expression::Access {
                target: Box::new(Expression::Identifier("menu".into())),
                field: "title".into(),
            }
        );

        let expr2 = parse_expression("menu.actions");
        assert_eq!(
            expr2,
            Expression::Access {
                target: Box::new(Expression::Identifier("menu".into())),
                field: "actions".into(),
            }
        );

        let chained = parse_expression("a.b.c");
        assert_eq!(
            chained,
            Expression::Access {
                target: Box::new(Expression::Access {
                    target: Box::new(Expression::Identifier("a".into())),
                    field: "b".into(),
                }),
                field: "c".into(),
            }
        );
    }

    #[test]
    fn test_literals() {
        assert_eq!(parse_expression("true"), Expression::Bool(true));
        assert_eq!(parse_expression("false"), Expression::Bool(false));
        assert_eq!(parse_expression("42"), Expression::Integer(42));
        assert_eq!(parse_expression("-7"), Expression::Integer(-7));
        assert_eq!(parse_expression("3.5"), Expression::Float(3.5));
        assert_eq!(parse_expression("\"hello world\""), Expression::String("hello world".into()));
        assert_eq!(parse_expression("'single quote'"), Expression::String("single quote".into()));
    }

    #[test]
    fn test_for() {
        let (id, expr) = parse_for("action in menu.actions");
        assert_eq!(id, Identifier::new("action"));
        assert_eq!(
            expr,
            Expression::Access {
                target: Box::new(Expression::Identifier("menu".into())),
                field: "actions".into(),
            }
        );
    }

    #[test]
    fn test_template_text() {
        let text = parse_template_text("{{menu.title}}");
        assert_eq!(
            text.0,
            vec![TextFragment::Interpolated(Expression::Access {
                target: Box::new(Expression::Identifier("menu".into())),
                field: "title".into(),
            })]
        );

        let mixed = parse_template_text("hello {{item.name}}!");
        assert_eq!(
            mixed.0,
            vec![
                TextFragment::Literal("hello ".into()),
                TextFragment::Interpolated(Expression::Access {
                    target: Box::new(Expression::Identifier("item".into())),
                    field: "name".into(),
                }),
                TextFragment::Literal("!".into()),
            ]
        );
    }
}
