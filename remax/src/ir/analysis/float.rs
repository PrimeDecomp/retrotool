//! Binary32 facts about word values, independent of their availability. These
//! follow explicit instruction controls, not a host's treatment of subnormals.

use super::{Constants, Dependencies};
use crate::ir::{float, Definition, Function, Operation, Type, ValueId};

pub(crate) struct FloatFacts {
    may_be_subnormal: Vec<bool>,
}

impl FloatFacts {
    pub fn new(function: &Function) -> Self {
        let dependencies = Dependencies::new(function);
        let constants = Constants::analyze(function, &dependencies);
        // Propagate possible subnormals from inputs and producers. This also
        // preserves inductive loop facts; a later unflushed incoming edge
        // permanently revokes the fact for every dependent value.
        let may_be_subnormal = dependencies.solve(
            false,
            |a, b| a || b,
            |facts, value, operands| {
                if let Some(word) = constants.word(value) {
                    return word & 0x7f800000 == 0 && word & 0x007fffff != 0;
                }
                let value = &function.values[value.0];
                if value.ty != Type::Bits32 {
                    return true;
                }
                match value.definition {
                    Definition::Parameter { .. } => operands.iter().any(|v| facts[v.0]),
                    Definition::Instruction { block, index } => {
                        let operation = &function.blocks[block.0].instructions[index].operation;
                        may_be_subnormal(operation, facts, &constants)
                    }
                    _ => true,
                }
            },
        );
        Self { may_be_subnormal }
    }

    /// When available, the word is zero, normal, infinite or NaN. This says
    /// nothing about its sign, NaN payload, availability or resource obligations.
    pub fn excludes_subnormal(&self, value: ValueId) -> bool { !self.may_be_subnormal[value.0] }
}

fn may_be_subnormal(operation: &Operation, facts: &[bool], constants: &Constants) -> bool {
    match *operation {
        Operation::Copy(value) => facts[value.0],
        Operation::Select { condition, yes, no } => match constants.predicate(condition) {
            Some(true) => facts[yes.0],
            Some(false) => facts[no.0],
            None => facts[yes.0] || facts[no.0],
        },
        Operation::Float(ref operation) => match *operation {
            float::Operation::Modify { value, flush_to_zero, .. } => {
                !flush_to_zero && facts[value.0]
            }
            float::Operation::Binary { control, .. } | float::Operation::Fma { control, .. } => {
                !control.flush_to_zero
            }
            float::Operation::MinMax { a, b, .. } => facts[a.0] || facts[b.0],
            float::Operation::Approximate { .. } | float::Operation::Reduced { .. } => false,
            // Integer-valued floating results cannot be subnormal. An integer
            // bit pattern from ToInteger has no such guarantee when reinterpreted.
            float::Operation::FromInteger { .. } | float::Operation::RoundIntegral { .. } => false,
            _ => true,
        },
        _ => true,
    }
}
