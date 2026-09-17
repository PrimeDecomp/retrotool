//! Finite input domains for symbolic attribute addresses. Known bits restrict
//! candidates without specializing uniform contents or asserting availability.

use super::{Dependencies, KnownBits};
use crate::ir::{Definition, Function, Operation, Type, ValueId};

pub(crate) struct AttributeInputs {
    /// Bit positions index Function::indexed_attributes (at most 128 words).
    candidates: Vec<u128>,
}

impl AttributeInputs {
    pub(super) fn analyze(
        function: &Function,
        dependencies: &Dependencies,
        bits: &KnownBits,
    ) -> Self {
        let candidates = dependencies.solve(
            0,
            |a, b| a | b,
            |facts, value, operands| {
                if function.values[value.0].ty != Type::AttributeAddress {
                    return 0;
                }
                if let Definition::Instruction { block, index } =
                    function.values[value.0].definition
                {
                    if let Operation::AttributeAddress { base, offset } =
                        function.blocks[block.0].instructions[index].operation
                    {
                        let mut mask = 0;
                        for (slot, &attribute) in function.indexed_attributes.iter().enumerate() {
                            let Some(relative) = attribute.checked_sub(base) else { continue };
                            if bits.get(offset).permits(u32::from(relative)) {
                                mask |= 1u128 << slot;
                            }
                        }
                        return mask;
                    }
                }
                operands.iter().fold(0, |mask, source| mask | facts[source.0])
            },
        );
        Self { candidates }
    }

    pub fn permits(&self, address: ValueId, slot: usize) -> bool {
        self.candidates[address.0] & (1u128 << slot) != 0
    }
}
