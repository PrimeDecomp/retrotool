use crate::ir::{analysis::FloatFacts, float, Function, Operation};

/// Eliminate input flushing only when it cannot change any available word.
/// Preserve sign modifiers, producer operations and every execution checkpoint.
pub(super) fn remove_redundant_flushes(function: &mut Function) {
    let facts = FloatFacts::new(function);
    for instruction in function.blocks.iter_mut().flat_map(|block| &mut block.instructions) {
        let Operation::Float(float::Operation::Modify {
            value,
            absolute,
            negate,
            flush_to_zero: true,
        }) = instruction.operation
        else {
            continue;
        };
        if facts.excludes_subnormal(value) {
            instruction.operation = if absolute || negate {
                Operation::Float(float::Operation::Modify {
                    value,
                    absolute,
                    negate,
                    flush_to_zero: false,
                })
            } else {
                Operation::Copy(value)
            };
        }
    }
}
