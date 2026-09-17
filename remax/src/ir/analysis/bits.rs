//! Per-bit integer invariants over all incoming paths. A bit fact describes a
//! value when available; it never establishes availability or removes effects.
//! Pending is the empty set of observed values, not a usable numeric fact.

use super::{Constants, Dependencies};
use crate::ir::{Definition, Function, Logic, Operation, Type, ValueId};

mod value;
pub(crate) use value::Bits;

pub(crate) struct KnownBits {
    facts: Vec<Option<Bits>>,
}

impl KnownBits {
    pub(super) fn analyze(
        function: &Function,
        dependencies: &Dependencies,
        constants: &Constants,
    ) -> Self {
        let facts = dependencies.solve(None, join, |facts, value, operands| {
            if let Some(word) = constants.word(value) {
                return Some(Bits::constant(word));
            }
            let definition = &function.values[value.0];
            if definition.ty != Type::Bits32 {
                return Some(Bits::UNKNOWN);
            }
            match definition.definition {
                Definition::Word(word) => Some(Bits::constant(word)),
                Definition::Parameter { .. } => {
                    operands.iter().fold(None, |a, b| join(a, facts[b.0]))
                }
                Definition::Instruction { block, index } => {
                    let operation = &function.blocks[block.0].instructions[index].operation;
                    evaluate(operation, facts, constants)
                }
                _ => Some(Bits::UNKNOWN),
            }
        });
        Self { facts }
    }

    pub fn get(&self, value: ValueId) -> Bits { self.facts[value.0].unwrap_or(Bits::UNKNOWN) }
}

fn join(a: Option<Bits>, b: Option<Bits>) -> Option<Bits> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.join(b)),
        (None, value) | (value, None) => value,
    }
}

fn evaluate(operation: &Operation, facts: &[Option<Bits>], constants: &Constants) -> Option<Bits> {
    let get = |value: ValueId| facts[value.0];
    Some(match *operation {
        Operation::Copy(value) => get(value)?,
        Operation::NotBits(value) => get(value)?.not(),
        Operation::Negate(value) => get(value)?.not().add(Bits::constant(1)),
        Operation::ReverseBits(value) => get(value)?.reverse(),
        Operation::CountLeadingZeros(value) => get(value)?.leading_zeros(),
        Operation::Add(a, b) => get(a)?.add(get(b)?),
        Operation::Multiply(a, b) => get(a)?.multiply(get(b)?),
        Operation::Logic { op, a, b } => {
            let (a, b) = (get(a)?, get(b)?);
            match op {
                Logic::And => a.and(b),
                Logic::Or => a.or(b),
                Logic::Xor => a.xor(b),
            }
        }
        Operation::Shift { a, b, right, signed, wrap } => {
            let (a, b) = (get(a)?, get(b)?);
            b.exact().map_or(Bits::UNKNOWN, |amount| a.shift(amount, right, signed, wrap))
        }
        Operation::ExtractBits { value, offset, count, signed } => {
            let (value, offset, count) = (get(value)?, get(offset)?, get(count)?);
            match (offset.exact(), count.exact()) {
                (Some(offset), Some(count)) => value.extract(offset, count, signed),
                _ => Bits::UNKNOWN,
            }
        }
        Operation::Select { condition, yes, no } => {
            return match constants.predicate(condition) {
                Some(true) => get(yes),
                Some(false) => get(no),
                None => join(get(yes), get(no)),
            };
        }
        Operation::LoadConstantWord { bytes: bytes @ (1 | 2), signed: false, .. } => {
            Bits::unsigned_range(0, (1u32 << (bytes * 8)) - 1)
        }
        // No inferred contents for uniforms, textures, incoming words or floating
        // operations. Resource checks and availability remain separate obligations.
        _ => Bits::UNKNOWN,
    })
}
