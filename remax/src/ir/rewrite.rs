//! Value substitution and compaction shared by SSA construction and optimization.
//! Callers establish type equivalence and dominance, and remove replaced
//! definitions from instructions/parameters before applying the substitution.

use super::{BlockId, Definition, Function, ValueId};

pub(crate) struct Aliases(Vec<ValueId>);

/// Remaps references retained outside a function while its values are compacted.
pub(crate) struct ValueRemap(Vec<ValueId>);

impl ValueRemap {
    pub fn get(&self, value: ValueId) -> ValueId { self.0[value.0] }
}

impl Aliases {
    pub fn new(function: &Function) -> Self {
        Self((0..function.values.len()).map(ValueId).collect())
    }

    pub fn resolve(&self, mut value: ValueId) -> ValueId {
        while self.0[value.0] != value {
            value = self.0[value.0];
        }
        value
    }

    pub fn replace(&mut self, value: ValueId, replacement: ValueId) {
        assert_eq!(self.0[value.0], value, "definition already replaced");
        let replacement = self.resolve(replacement);
        assert_ne!(value, replacement, "cyclic substitution");
        self.0[value.0] = replacement;
    }

    pub fn apply(self, function: &mut Function) -> ValueRemap {
        let mut retained = vec![ValueId(usize::MAX); self.0.len()];
        for (index, value) in std::mem::take(&mut function.values).into_iter().enumerate() {
            if self.0[index] == ValueId(index) {
                retained[index] = ValueId(function.values.len());
                function.values.push(value);
            }
        }
        let resolve = |value| retained[self.resolve(value).0];
        for (block, data) in function.blocks.iter_mut().enumerate() {
            for (index, value) in data.parameters.iter_mut().enumerate() {
                *value = resolve(*value);
                function.values[value.0].definition =
                    Definition::Parameter { block: BlockId(block), index };
            }
            for instruction in &mut data.instructions {
                instruction.result = instruction.result.map(resolve);
                instruction.operation.map_operands(resolve);
            }
            data.terminator.as_mut().unwrap().map_operands(resolve);
        }
        ValueRemap((0..self.0.len()).map(|index| resolve(ValueId(index))).collect())
    }
}
