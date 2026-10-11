use std::borrow::Cow;

use winnow::ascii::multispace0;
use winnow::combinator::{alt, delimited, opt};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{literal, one_of, take_while};

use crate::core::template::{BinaryOp, Expression, Identifier, TemplateText, TextFragment, UnaryOp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprParseError(pub String);

impl std::fmt::Display for ExprParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ExprParseError {}

/// Parses an identifier string: [a-zA-Z_$][a-zA-Z0-9_$]*
fn ident_str<'s>(input: &mut &'s str) -> ModalResult<&'s str> {
    let start = *input;
    let head = one_of(('a'..='z', 'A'..='Z', '_', '$')).parse_next(input)?;
    let tail: &str = take_while(0.., ('a'..='z', 'A'..='Z', '0'..='9', '_', '$')).parse_next(input)?;
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

/// Parses a template string: `` `text ${expr} more` ``. `\`` and `\$` stand for themselves.
fn parse_template_string(input: &mut &str) -> ModalResult<TemplateText> {
    let _ = literal("`").parse_next(input)?;
    let mut fragments = Vec::new();
    let mut text = String::new();
    loop {
        let Some(ch) = input.chars().next() else {
            return Err(winnow::error::ErrMode::Cut(ContextError::new()));
        };
        *input = &input[ch.len_utf8()..];
        match ch {
            '`' => break,
            '\\' => {
                let esc = one_of(|_| true).parse_next(input)?;
                text.push(match esc {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            '$' if input.starts_with('{') => {
                *input = &input[1..];
                if !text.is_empty() {
                    fragments.push(TextFragment::Literal(std::mem::take(&mut text).into()));
                }
                fragments.push(TextFragment::Interpolated(parse_expr_internal.parse_next(input)?));
                let _ = literal("}").parse_next(input).map_err(|e: winnow::error::ErrMode<ContextError>| e.cut())?;
            }
            other => text.push(other),
        }
    }
    if !text.is_empty() {
        fragments.push(TextFragment::Literal(text.into()));
    }
    Ok(TemplateText::new(fragments))
}

/// Parses float: [0-9]+\.[0-9]+([eE][+-]?[0-9]+)?. A sign is a unary operator.
fn parse_float_literal(input: &mut &str) -> ModalResult<f32> {
    let checkpoint = *input;
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

/// Parses integer: [0-9]+. A sign is a unary operator.
fn parse_int_literal(input: &mut &str) -> ModalResult<i32> {
    let checkpoint = *input;
    let _digits: &str = take_while(1.., '0'..='9').parse_next(input)?;
    let int_slice_len = checkpoint.len() - input.len();
    let slice = &checkpoint[..int_slice_len];
    slice.parse::<i32>().map_err(|_| winnow::error::ErrMode::Backtrack(ContextError::new()))
}

/// Parses an atom (primary expression before any dot-access):
/// - parenthesized expression
/// - string literal
/// - template string `` `text ${expr}` ``
/// - float literal
/// - integer literal
/// - object literal `{ key: expr, 'key': expr }`
/// - function call `ident(expr, ...)`
/// - boolean or identifier
fn parse_atom(input: &mut &str) -> ModalResult<Expression> {
    let _: &str = multispace0.parse_next(input)?;
    
    // Parentheses
    if input.starts_with('(') {
        let expr = delimited('(', parse_expr_internal, ')').parse_next(input)?;
        return Ok(expr);
    }
    
    // Object literal
    if input.starts_with('{') {
        return parse_object.parse_next(input).map(Expression::Object);
    }

    // String literal
    if input.starts_with('"') || input.starts_with('\'') {
        let s = parse_string_literal.parse_next(input)?;
        return Ok(Expression::String(Cow::Owned(s)));
    }

    // Template string
    if input.starts_with('`') {
        return parse_template_string.parse_next(input).map(Expression::TemplateString);
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
    
    // Identifier, bool or call
    let id_str = ident_str.parse_next(input)?;
    let after_ident = *input;
    let _: &str = multispace0.parse_next(input)?;
    if input.starts_with('(') {
        if matches!(id_str, "true" | "false") {
            return Err(winnow::error::ErrMode::Cut(ContextError::new()));
        }
        let args = parse_call_args.parse_next(input)?;
        return Ok(Expression::Call { function: Identifier::new(id_str.to_string()), args });
    }
    *input = after_ident;
    match id_str {
        "true" => Ok(Expression::Bool(true)),
        "false" => Ok(Expression::Bool(false)),
        other => Ok(Expression::Identifier(Identifier::new(other.to_string()))),
    }
}

/// Parses a parenthesized, comma separated and possibly empty argument list: `(a, b.c, 1)`.
fn parse_call_args(input: &mut &str) -> ModalResult<Vec<Expression>> {
    let _ = literal("(").parse_next(input)?;
    let mut args = Vec::new();
    let _: &str = multispace0.parse_next(input)?;
    if input.starts_with(')') {
        let _ = literal(")").parse_next(input)?;
        return Ok(args);
    }
    loop {
        args.push(parse_expr_internal.parse_next(input)?);
        let _: &str = multispace0.parse_next(input)?;
        if input.starts_with(',') {
            let _ = literal(",").parse_next(input)?;
        } else {
            let _ = literal(")").parse_next(input)?;
            return Ok(args);
        }
    }
}

/// Parses an object literal: `{ a: 1, 'b-c': d.e }`. Keys are identifiers or quoted strings and a
/// trailing comma is allowed.
fn parse_object(input: &mut &str) -> ModalResult<Vec<(Cow<'static, str>, Expression)>> {
    let _ = literal("{").parse_next(input)?;
    let mut entries = Vec::new();
    loop {
        let _: &str = multispace0.parse_next(input)?;
        if input.starts_with('}') {
            let _ = literal("}").parse_next(input)?;
            return Ok(entries);
        }
        let key = if input.starts_with('"') || input.starts_with('\'') {
            parse_string_literal.parse_next(input)?
        } else {
            ident_str.parse_next(input)?.to_owned()
        };
        let _: &str = multispace0.parse_next(input)?;
        let _ = literal(":").parse_next(input)?;
        entries.push((Cow::Owned(key), parse_expr_internal.parse_next(input)?));
        let _: &str = multispace0.parse_next(input)?;
        if input.starts_with(',') {
            let _ = literal(",").parse_next(input)?;
        } else {
            let _ = literal("}").parse_next(input)?;
            return Ok(entries);
        }
    }
}

/// Parses a whole expression, with surrounding whitespace.
///
/// From loosest to tightest: `? :`, `||`, `&&`, `==` `!=`, `<` `<=` `>` `>=`, `+` `-`,
/// `*` `/` `%`, the prefix operators `!` `-` `+`, then member access, indexing and calls.
fn parse_expr_internal(input: &mut &str) -> ModalResult<Expression> {
    let _: &str = multispace0.parse_next(input)?;
    let condition = parse_binary(input, 1)?;
    let _: &str = multispace0.parse_next(input)?;
    if !input.starts_with('?') {
        return Ok(condition);
    }
    // Both branches are whole expressions, which makes the operator right-associative.
    let _ = literal("?").parse_next(input)?;
    let then = parse_expr_internal.parse_next(input)?;
    let _ = literal(":").parse_next(input).map_err(|e: winnow::error::ErrMode<ContextError>| e.cut())?;
    let otherwise = parse_expr_internal.parse_next(input)?;
    Ok(Expression::Conditional {
        condition: Box::new(condition),
        then: Box::new(then),
        otherwise: Box::new(otherwise),
    })
}

/// The infix operator at the start of `input`, with its length.
fn peek_binary_op(input: &str) -> Option<(BinaryOp, usize)> {
    // Two-character operators first: `<=` is not `<` followed by `=`.
    const OPERATORS: [(&str, BinaryOp); 13] = [
        ("||", BinaryOp::Or),
        ("&&", BinaryOp::And),
        ("==", BinaryOp::Eq),
        ("!=", BinaryOp::Ne),
        ("<=", BinaryOp::Le),
        (">=", BinaryOp::Ge),
        ("<", BinaryOp::Lt),
        (">", BinaryOp::Gt),
        ("+", BinaryOp::Add),
        ("-", BinaryOp::Sub),
        ("*", BinaryOp::Mul),
        ("/", BinaryOp::Div),
        ("%", BinaryOp::Rem),
    ];
    OPERATORS.iter().find(|(token, _)| input.starts_with(token)).map(|(token, op)| (*op, token.len()))
}

/// Parses infix operators that bind at least as tightly as `min_precedence`, left-associatively.
fn parse_binary(input: &mut &str, min_precedence: u8) -> ModalResult<Expression> {
    let mut left = parse_unary.parse_next(input)?;
    loop {
        let _: &str = multispace0.parse_next(input)?;
        let Some((op, len)) = peek_binary_op(input) else { break };
        if op.precedence() < min_precedence {
            break;
        }
        *input = &input[len..];
        let right = parse_binary(input, op.precedence() + 1)?;
        left = Expression::Binary { op, left: Box::new(left), right: Box::new(right) };
    }
    Ok(left)
}

/// Parses prefix operators: `!a`, `-a`, `+a`.
fn parse_unary(input: &mut &str) -> ModalResult<Expression> {
    let _: &str = multispace0.parse_next(input)?;
    let op = match input.chars().next() {
        Some('!') => UnaryOp::Not,
        Some('-') => UnaryOp::Neg,
        Some('+') => UnaryOp::Pos,
        _ => return parse_postfix.parse_next(input),
    };
    *input = &input[1..];
    let operand = parse_unary.parse_next(input)?;
    Ok(Expression::Unary { op, operand: Box::new(operand) })
}

/// Parses an atom with chained member access and indexing: `target.field1[0].field2`.
/// A numeric member such as `target.0` is the same as `target[0]`.
fn parse_postfix(input: &mut &str) -> ModalResult<Expression> {
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
                    let digits: ModalResult<&str> = take_while(1.., '0'..='9').parse_next(input);
                    match digits.ok().and_then(|digits| digits.parse::<i32>().ok()) {
                        Some(index) => {
                            current = Expression::Index {
                                target: Box::new(current),
                                index: Box::new(Expression::Integer(index)),
                            };
                        }
                        None => {
                            // Not a member after dot; rollback
                            *input = checkpoint;
                            break;
                        }
                    }
                }
            }
        } else if input.starts_with('[') {
            let index = delimited('[', parse_expr_internal, ']').parse_next(input)?;
            current = Expression::Index {
                target: Box::new(current),
                index: Box::new(index),
            };
        } else {
            break;
        }
    }
    
    let _: &str = multispace0.parse_next(input)?;
    Ok(current)
}

/// Parses the left-hand side of a `v-for`: `item` or `(item, index)`.
fn parse_for_aliases(input: &mut &str) -> ModalResult<(Identifier, Option<Identifier>)> {
    let _: &str = multispace0.parse_next(input)?;
    if !input.starts_with('(') {
        return Ok((parse_identifier.parse_next(input)?, None));
    }
    let _ = literal("(").parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    let target = parse_identifier.parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    let mut index = None;
    if input.starts_with(',') {
        let _ = literal(",").parse_next(input)?;
        let _: &str = multispace0.parse_next(input)?;
        index = Some(parse_identifier.parse_next(input)?);
        let _: &str = multispace0.parse_next(input)?;
    }
    let _ = literal(")").parse_next(input)?;
    Ok((target, index))
}

fn parse_for_internal(input: &mut &str) -> ModalResult<(Identifier, Option<Identifier>, Expression)> {
    let (target, index) = parse_for_aliases.parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    let _ = alt((literal("in"), literal("of"))).parse_next(input)?;
    // The keyword must be followed by whitespace, so `item inside` is not `item in side`.
    let _: &str = take_while(1.., (' ', '\t', '\n', '\r')).parse_next(input)?;
    let iterable = parse_expr_internal.parse_next(input)?;
    let _: &str = multispace0.parse_next(input)?;
    Ok((target, index, iterable))
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

/// The parts of a `v-for` attribute: element alias, optional index alias and iterable.
pub type ForParts = (Identifier, Option<Identifier>, Expression);

/// Parses `<identifier> in <expression>` or `(<identifier>, <identifier>) in <expression>` from
/// `v-for` attributes.
pub fn try_parse_for(value: &str) -> Result<ForParts, ExprParseError> {
    let mut input = value.trim();
    match parse_for_internal.parse_next(&mut input) {
        Ok(res) if input.is_empty() => Ok(res),
        Ok(_) => Err(ExprParseError(format!("Trailing characters in v-for: '{input}'"))),
        Err(e) => Err(ExprParseError(format!("Failed to parse v-for '{value}': {e}"))),
    }
}

/// Parses `<identifier> in <expression>` from `v-for` attributes. Panics on syntax error.
pub fn parse_for(value: &str) -> ForParts {
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
        if !after_open.contains("}}") {
            return Err(ExprParseError(format!("Unclosed interpolation in: '{value}'")));
        }
        // The expression is parsed before looking for `}}`, as it may contain braces itself.
        let mut input = after_open;
        let expr = match parse_expr_internal.parse_next(&mut input) {
            Ok(expr) if input.starts_with("}}") => expr,
            _ => {
                let end = after_open.find("}}").unwrap_or(after_open.len());
                return Err(ExprParseError(format!("Failed to parse expression '{}'", after_open[..end].trim())));
            }
        };
        fragments.push(TextFragment::Interpolated(expr));

        remaining = &input[2..];
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
        // A sign is an operator, folded into the literal when the template is compiled.
        assert_eq!(
            parse_expression("-7"),
            Expression::Unary { op: UnaryOp::Neg, operand: Box::new(Expression::Integer(7)) }
        );
        assert_eq!(parse_expression("3.5"), Expression::Float(3.5));
        assert_eq!(parse_expression("\"hello world\""), Expression::String("hello world".into()));
        assert_eq!(parse_expression("'single quote'"), Expression::String("single quote".into()));
    }

    #[test]
    fn test_for() {
        let (id, index, expr) = parse_for("action in menu.actions");
        assert_eq!(id, Identifier::new("action"));
        assert_eq!(index, None);
        assert_eq!(
            expr,
            Expression::Access {
                target: Box::new(Expression::Identifier("menu".into())),
                field: "actions".into(),
            }
        );
    }

    #[test]
    fn test_for_with_index() {
        let (id, index, expr) = parse_for("( action , i ) of actions");
        assert_eq!(id, Identifier::new("action"));
        assert_eq!(index, Some(Identifier::new("i")));
        assert_eq!(expr, Expression::Identifier("actions".into()));

        assert!(try_parse_for("item inside").is_err());
        assert!(try_parse_for("(a, b in list").is_err());
    }

    #[test]
    fn test_dollar_identifiers() {
        assert_eq!(parse_expression("$event"), Expression::Identifier("$event".into()));
        assert_eq!(parse_expression("a$b"), Expression::Identifier("a$b".into()));
    }

    /// An expression fully parenthesised, to check how operators group.
    fn show(expr: &Expression) -> String {
        match expr {
            Expression::Identifier(ident) => ident.as_str().to_owned(),
            Expression::Integer(i) => i.to_string(),
            Expression::Float(f) => f.to_string(),
            Expression::Bool(b) => b.to_string(),
            Expression::String(s) => format!("'{s}'"),
            Expression::Access { target, field } => format!("{}.{}", show(target), field.as_str()),
            Expression::Index { target, index } => format!("{}[{}]", show(target), show(index)),
            Expression::Call { function, args } => {
                format!("{}({})", function.as_str(), args.iter().map(show).collect::<Vec<_>>().join(", "))
            }
            Expression::Object(entries) => {
                let entries: Vec<_> = entries.iter().map(|(k, v)| format!("{k}: {}", show(v))).collect();
                format!("{{{}}}", entries.join(", "))
            }
            Expression::TemplateString(text) => {
                let parts: Vec<_> = text
                    .0
                    .iter()
                    .map(|fragment| match fragment {
                        TextFragment::Literal(s) => format!("{s:?}"),
                        TextFragment::Interpolated(expr) => show(expr),
                    })
                    .collect();
                format!("`{}`", parts.join(" "))
            }
            Expression::Unary { op, operand } => format!("({op:?} {})", show(operand)),
            Expression::Binary { op, left, right } => format!("({} {op:?} {})", show(left), show(right)),
            Expression::Conditional { condition, then, otherwise } => {
                format!("({} ? {} : {})", show(condition), show(then), show(otherwise))
            }
        }
    }

    #[test]
    fn test_operators() {
        for (source, expected) in [
            ("1 + 2 * 3", "(1 Add (2 Mul 3))"),
            ("(1 + 2) * 3", "((1 Add 2) Mul 3)"),
            ("a - b - c", "((a Sub b) Sub c)"),
            ("a / b % c * d", "(((a Div b) Rem c) Mul d)"),
            ("!a && b || c", "(((Not a) And b) Or c)"),
            ("a || b && c", "(a Or (b And c))"),
            ("a == b != c", "((a Eq b) Ne c)"),
            ("a < b == c >= d", "((a Lt b) Eq (c Ge d))"),
            ("a + 1 <= b * 2", "((a Add 1) Le (b Mul 2))"),
            ("a<b", "(a Lt b)"),
            ("a>b&&c<=d", "((a Gt b) And (c Le d))"),
            ("-a.b", "(Neg a.b)"),
            ("- - a", "(Neg (Neg a))"),
            ("!!a", "(Not (Not a))"),
            ("a - -1", "(a Sub (Neg 1))"),
            ("a -1", "(a Sub 1)"),
            ("a+1", "(a Add 1)"),
            ("+a * -b", "((Pos a) Mul (Neg b))"),
            ("1.5 + 2", "(1.5 Add 2)"),
            ("'a' + b", "('a' Add b)"),
            ("(a || b).name[i + 1]", "(a Or b).name[(i Add 1)]"),
            ("f(a + 1, !b)", "f((a Add 1), (Not b))"),
            ("rows[i - 1].x * 2", "(rows[(i Sub 1)].x Mul 2)"),
            ("{ a: b + 1, c: !d }", "{a: (b Add 1), c: (Not d)}"),
            ("a ? b : c", "(a ? b : c)"),
            ("a ? b : c ? d : e", "(a ? b : (c ? d : e))"),
            ("a ? b ? c : d : e", "(a ? (b ? c : d) : e)"),
            ("a || b ? c + 1 : d && e", "((a Or b) ? (c Add 1) : (d And e))"),
            ("{ k: a ? b : c, j: d }", "{k: (a ? b : c), j: d}"),
            ("f(a ? 1 : 2)", "f((a ? 1 : 2))"),
            ("(a ? b : c).d", "(a ? b : c).d"),
        ] {
            assert_eq!(show(&parse_expression(source)), expected, "{source}");
        }
        for source in ["a +", "* a", "a ? b", "a ? b :", "a ? : c", "a &&& b", "a = b", "a ! b", "1 2"] {
            assert!(try_parse_expression(source).is_err(), "{source} should not parse");
        }
    }

    #[test]
    fn test_template_strings() {
        for (source, expected) in [
            ("``", "``"),
            ("`plain`", r#"`"plain"`"#),
            ("`Hello, ${user.name}!`", r#"`"Hello, " user.name "!"`"#),
            ("`${a}${ b + 1 }`", "`a (b Add 1)`"),
            ("`${ {k: 1}.k }%`", r#"`{k: 1}.k "%"`"#),
            ("`a ${ `b ${c}` } d`", r#"`"a " `"b " c` " d"`"#),
            // The expected text is the `Debug` form of the string: a backslash and a newline.
            (r"`\`\${x} \\ $5 \n`", r#"`"`${x} \\ $5 \n"`"#),
            ("`${n}px` + suffix", r#"(`n "px"` Add suffix)"#),
            ("f(`#${id}`, 'x')", r##"f(`"#" id`, 'x')"##),
            ("{ width: `${w * 100}%` }", r#"{width: `(w Mul 100) "%"`}"#),
        ] {
            assert_eq!(show(&parse_expression(source)), expected, "{source}");
        }
        for source in ["`open", "`${a`", "`${}`", "`${a b}`", "`a` `b`"] {
            assert!(try_parse_expression(source).is_err(), "{source} should not parse");
        }

        let text = parse_template_text("{{ `${a}` }} and {{ `}}` }}");
        assert_eq!(text.0.len(), 3);
        assert!(matches!(&text.0[2], TextFragment::Interpolated(Expression::TemplateString(_))));
    }

    #[test]
    fn test_interpolation_end() {
        let text = parse_template_text("{{ {a: {b: 1}} }} and {{ x < y }}!");
        assert_eq!(text.0.len(), 4);
        assert!(matches!(&text.0[0], TextFragment::Interpolated(Expression::Object(_))));
        assert!(matches!(&text.0[2], TextFragment::Interpolated(Expression::Binary { op: BinaryOp::Lt, .. })));
        assert!(try_parse_template_text("{{ a b }}").is_err());
        assert!(try_parse_template_text("{{ a ").is_err());
        assert!(try_parse_template_text("{{ a } }").is_err());
    }

    #[test]
    fn test_objects() {
        assert_eq!(parse_expression("{}"), Expression::Object(vec![]));
        assert_eq!(
            parse_expression("{ backgroundColor: theme.color, 'border-radius': \"4px\", n: f({ a: 1 }), }"),
            Expression::Object(vec![
                (
                    "backgroundColor".into(),
                    Expression::Access { target: Box::new(Expression::Identifier("theme".into())), field: "color".into() },
                ),
                ("border-radius".into(), Expression::String("4px".into())),
                (
                    "n".into(),
                    Expression::Call {
                        function: "f".into(),
                        args: vec![Expression::Object(vec![("a".into(), Expression::Integer(1))])],
                    },
                ),
            ])
        );
        assert!(try_parse_expression("{ a 1 }").is_err());
        assert!(try_parse_expression("{ a: 1").is_err());
        assert!(try_parse_expression("{ a: 1 b: 2 }").is_err());
        assert!(try_parse_expression("{ 1: a }").is_err());
    }

    #[test]
    fn test_calls() {
        assert_eq!(parse_expression("f()"), Expression::Call { function: "f".into(), args: vec![] });
        assert_eq!(
            parse_expression("f( a.b , 1, \"x\" )"),
            Expression::Call {
                function: "f".into(),
                args: vec![
                    Expression::Access {
                        target: Box::new(Expression::Identifier("a".into())),
                        field: "b".into(),
                    },
                    Expression::Integer(1),
                    Expression::String("x".into()),
                ],
            }
        );
        assert_eq!(
            parse_expression("f(g(x))[0].name"),
            Expression::Access {
                target: Box::new(Expression::Index {
                    target: Box::new(Expression::Call {
                        function: "f".into(),
                        args: vec![Expression::Call {
                            function: "g".into(),
                            args: vec![Expression::Identifier("x".into())],
                        }],
                    }),
                    index: Box::new(Expression::Integer(0)),
                }),
                field: "name".into(),
            }
        );
        assert_eq!(
            parse_expression("$emit('buy', $event)"),
            Expression::Call {
                function: "$emit".into(),
                args: vec![Expression::String("buy".into()), Expression::Identifier("$event".into())],
            }
        );
        assert!(try_parse_expression("f(a,)").is_err());
        assert!(try_parse_expression("f(a").is_err());
        assert!(try_parse_expression("true(1)").is_err());
    }

    #[test]
    fn test_index() {
        let expr = parse_expression("a.b[0].c");
        assert_eq!(
            expr,
            Expression::Access {
                target: Box::new(Expression::Index {
                    target: Box::new(Expression::Access {
                        target: Box::new(Expression::Identifier("a".into())),
                        field: "b".into(),
                    }),
                    index: Box::new(Expression::Integer(0)),
                }),
                field: "c".into(),
            }
        );

        let dynamic = parse_expression("rows[ i ][\"k\"]");
        assert_eq!(
            dynamic,
            Expression::Index {
                target: Box::new(Expression::Index {
                    target: Box::new(Expression::Identifier("rows".into())),
                    index: Box::new(Expression::Identifier("i".into())),
                }),
                index: Box::new(Expression::String("k".into())),
            }
        );

        assert!(try_parse_expression("a[0").is_err());

        // Numeric members are indices.
        assert_eq!(parse_expression("a.b.0.c"), expr);
        assert_eq!(parse_expression("rows.0.1"), parse_expression("rows[0][1]"));
        assert_eq!(parse_expression("f(x).0"), parse_expression("f(x)[0]"));
        assert!(try_parse_expression("a.").is_err());
        assert!(try_parse_expression("a.0x").is_err());
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
