use super::{Logic, Operation, Relation};
use crate::program::Value;

/// Fold only exact scalar integer/predicate operations. Floating arithmetic,
/// opaque reductions and resource reads require their own models.
pub(super) fn evaluate(op: &Operation, args: &[Value]) -> Option<Value> {
    let word = |index| match args[index] {
        Value::Word(bits) => bits,
        _ => unreachable!("verified word operand"),
    };
    let predicate = |index| match args[index] {
        Value::Predicate(b) => b,
        _ => unreachable!("verified predicate operand"),
    };
    Some(match *op {
        Operation::Add(..) => Value::Word(word(0).wrapping_add(word(1))),
        Operation::Multiply(..) => Value::Word(word(0).wrapping_mul(word(1))),
        Operation::Negate(_) => Value::Word(word(0).wrapping_neg()),
        Operation::NotBits(_) => Value::Word(!word(0)),
        Operation::ReverseBits(_) => Value::Word(word(0).reverse_bits()),
        Operation::CountLeadingZeros(_) => Value::Word(word(0).leading_zeros()),
        Operation::ExtractBits { signed, .. } => {
            let (value, offset, count) = (word(0), word(1), word(2));
            if count == 0 {
                return Some(Value::Word(0));
            }
            let sign_bit = offset.saturating_add(count - 1).min(31);
            let padding = if signed && value & (1 << sign_bit) != 0 { u32::MAX } else { 0 };
            if offset >= 32 {
                return Some(Value::Word(padding));
            }
            let width = count.min(32 - offset);
            let mask = u32::MAX >> (32 - width);
            Value::Word(((value >> offset) & mask) | (padding & !mask))
        }
        Operation::Logic { op, .. } => Value::Word(match op {
            Logic::And => word(0) & word(1),
            Logic::Or => word(0) | word(1),
            Logic::Xor => word(0) ^ word(1),
        }),
        Operation::Shift { right, signed, wrap, .. } => {
            let (a, b) = (word(0), word(1));
            Value::Word(if !wrap && b >= 32 {
                if right && signed {
                    ((a as i32) >> 31) as u32
                } else {
                    0
                }
            } else if right && signed {
                ((a as i32) >> (b & 31)) as u32
            } else if right {
                a >> (b & 31)
            } else {
                a << (b & 31)
            })
        }
        Operation::Compare { op, signed, .. } => {
            let (a, b) = if signed {
                (i64::from(word(0) as i32), i64::from(word(1) as i32))
            } else {
                (i64::from(word(0)), i64::from(word(1)))
            };
            Value::Predicate(match op {
                Relation::Less => a < b,
                Relation::Equal => a == b,
                Relation::LessEqual => a <= b,
                Relation::Greater => a > b,
                Relation::NotEqual => a != b,
                Relation::GreaterEqual => a >= b,
            })
        }
        Operation::PredicateNot(_) => Value::Predicate(!predicate(0)),
        Operation::PredicateLogic { op, .. } => Value::Predicate(match op {
            Logic::And => predicate(0) & predicate(1),
            Logic::Or => predicate(0) | predicate(1),
            Logic::Xor => predicate(0) ^ predicate(1),
        }),
        Operation::Copy(_) => args[0],
        Operation::Select { .. } => args[if predicate(0) { 1 } else { 2 }],
        Operation::ReadConstant(_)
        | Operation::Reduce { .. }
        | Operation::Component { .. }
        | Operation::Texture(_)
        | Operation::GlobalLoad(_)
        | Operation::Nop
        | Operation::Demote
        | Operation::LocalLoad(_)
        | Operation::LocalStore(_)
        | Operation::LoadConstantWord { .. }
        | Operation::ReadSampleIndex
        | Operation::Float(_)
        | Operation::ReadAttribute(_)
        | Operation::AttributeAddress { .. }
        | Operation::Interpolate { .. } => return None,
    })
}
