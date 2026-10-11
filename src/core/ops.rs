//! What the operators of template expressions compute.
//!
//! They follow JavaScript where the types of props allow it. An operation that makes no sense for
//! its operands gives `null` (arithmetic) or `false` (ordering) instead of failing, since props
//! are often missing while data loads.

use std::cmp::Ordering;

use crate::core::{
    template::{BinaryOp, UnaryOp},
    value::{Value, ValueType},
};

/// Applies a prefix operator.
pub fn unary(op: UnaryOp, operand: &Value) -> Value {
    match op {
        UnaryOp::Not => Value::from(!operand.is_truthy()),
        UnaryOp::Neg => operand.as_number().map_or_else(Value::null, |n| Value::number(-n)),
        UnaryOp::Pos => operand.as_number().map_or_else(Value::null, Value::number),
    }
}

/// Applies an infix operator to two evaluated operands.
///
/// `&&` and `||` return one of their operands. The evaluator does not call this for them, as it
/// must not evaluate the right operand when the left one decides the result.
pub fn binary(op: BinaryOp, left: &Value, right: &Value) -> Value {
    match op {
        BinaryOp::Or => if left.is_truthy() { left } else { right }.clone(),
        BinaryOp::And => if left.is_truthy() { right } else { left }.clone(),
        BinaryOp::Eq => Value::from(left == right),
        BinaryOp::Ne => Value::from(left != right),
        BinaryOp::Lt => Value::from(compare(left, right) == Some(Ordering::Less)),
        BinaryOp::Le => Value::from(matches!(compare(left, right), Some(Ordering::Less | Ordering::Equal))),
        BinaryOp::Gt => Value::from(compare(left, right) == Some(Ordering::Greater)),
        BinaryOp::Ge => Value::from(matches!(compare(left, right), Some(Ordering::Greater | Ordering::Equal))),
        // With a string on either side, `+` joins the operands as text.
        BinaryOp::Add if left.as_string().is_some() || right.as_string().is_some() => {
            Value::from(left.to_text() + &right.to_text())
        }
        BinaryOp::Add => arithmetic(left, right, |a, b| a + b),
        BinaryOp::Sub => arithmetic(left, right, |a, b| a - b),
        BinaryOp::Mul => arithmetic(left, right, |a, b| a * b),
        BinaryOp::Div => arithmetic(left, right, |a, b| a / b),
        BinaryOp::Rem => arithmetic(left, right, |a, b| a % b),
    }
}

fn arithmetic(left: &Value, right: &Value, f: impl FnOnce(f64, f64) -> f64) -> Value {
    match (left.as_number(), right.as_number()) {
        (Some(a), Some(b)) => Value::number(f(a, b)),
        _ => Value::null(),
    }
}

/// Orders two numbers, or two strings alphabetically. Anything else has no order.
fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left.value_type(), right.value_type()) {
        (ValueType::Number(a), ValueType::Number(b)) => a.partial_cmp(b),
        (ValueType::String(a), ValueType::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;

    #[test]
    fn binary_operators() {
        use BinaryOp::*;
        let null = Value::null;
        for (op, left, right, expected) in [
            (Add, value!(1), value!(2), value!(3)),
            (Add, value!("Item #"), value!(7), value!("Item #7")),
            (Add, value!(1.5), value!("px"), value!("1.5px")),
            (Add, value!("a"), null(), value!("a")),
            (Add, value!(1), null(), null()),
            (Add, value!(true), value!(1), null()),
            (Sub, value!(5), value!(7), value!(-2)),
            (Sub, value!("5"), value!(1), null()),
            (Mul, value!(3), value!(0.5), value!(1.5)),
            (Div, value!(7), value!(2), value!(3.5)),
            (Div, value!(1), value!(0), value!(f64::INFINITY)),
            (Rem, value!(7), value!(3), value!(1)),
            (Eq, value!(1), value!(1.0), value!(true)),
            (Eq, value!(1), value!("1"), value!(false)),
            (Eq, value!([1, { "a": 2 }]), value!([1, { "a": 2 }]), value!(true)),
            (Eq, null(), null(), value!(true)),
            (Ne, value!("a"), value!("b"), value!(true)),
            (Lt, value!(1), value!(2), value!(true)),
            (Lt, value!(2), value!(2), value!(false)),
            (Le, value!(2), value!(2), value!(true)),
            (Gt, value!("b"), value!("a"), value!(true)),
            (Ge, value!("a"), value!("b"), value!(false)),
            // No order between different types, or for anything but numbers and strings.
            (Lt, value!(1), value!("2"), value!(false)),
            (Ge, value!(1), value!("2"), value!(false)),
            (Le, null(), null(), value!(false)),
            (Or, value!(0), value!("x"), value!("x")),
            (Or, value!("a"), value!("x"), value!("a")),
            (And, value!(""), value!("x"), value!("")),
            (And, value!(1), value!("x"), value!("x")),
        ] {
            assert_eq!(binary(op, &left, &right), expected, "{left} {op:?} {right}");
        }
    }

    #[test]
    fn unary_operators() {
        assert_eq!(unary(UnaryOp::Not, &value!(0)), value!(true));
        assert_eq!(unary(UnaryOp::Not, &value!("a")), value!(false));
        assert_eq!(unary(UnaryOp::Not, &Value::null()), value!(true));
        assert_eq!(unary(UnaryOp::Neg, &value!(2)), value!(-2));
        assert_eq!(unary(UnaryOp::Neg, &value!("2")), Value::null());
        assert_eq!(unary(UnaryOp::Pos, &value!(2)), value!(2));
        assert_eq!(unary(UnaryOp::Pos, &value!(true)), Value::null());
    }
}
