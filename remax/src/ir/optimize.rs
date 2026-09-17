//! Semantics-preserving transformations over verified IR. These are independent
//! of WGSL and bindings; the checked execution path retains the original IR.

use std::borrow::Cow;

use super::{analysis::LocalSlots, ssa::Builder, BlockId, Function, Operation};
use crate::program::Diagnostic;

mod blocks;
mod float;
mod reuse;

/// Shared production transformations. Keep the checked program intact so its
/// original control flow remains available for differential validation.
pub(crate) fn for_rendering(function: &Function) -> Result<Function, Diagnostic> {
    let mut result = promote_local_memory(function)?.into_owned();
    float::remove_redundant_flushes(&mut result);
    blocks::coalesce(&mut result);
    reuse::eliminate(&mut result);
    result.verify()?;
    Ok(result)
}

/// Promote fixed local words through ordinary SSA construction. Retained memory
/// operations may touch other words of a partially promoted wide access.
pub(crate) fn promote_local_memory(function: &Function) -> Result<Cow<'_, Function>, Diagnostic> {
    function.verify()?;
    let slots = LocalSlots::analyze(function);
    if slots.is_empty() {
        return Ok(Cow::Borrowed(function));
    }
    let mut builder = Builder::extend(function.clone());
    for (block_index, block) in function.blocks.iter().enumerate() {
        let id = BlockId(block_index);
        for (index, instruction) in block.instructions.iter().enumerate() {
            let operation = match &instruction.operation {
                Operation::LocalLoad(load) => {
                    let Some(word) = slots.word(load.access, load.part) else { continue };
                    Operation::Copy(builder.read_local(id, word))
                }
                Operation::LocalStore(store) => {
                    let mut promoted_all = true;
                    for (part, &value) in store.values.iter().enumerate() {
                        if let Some(word) = slots.word(store.access, part as u8 * 4) {
                            builder.write_local(id, word, value);
                        } else {
                            promoted_all = false;
                        }
                    }
                    if !promoted_all {
                        continue;
                    }
                    // Keep source-budget accounting even when the memory effect
                    // disappears. No code is emitted once the bound is proven.
                    Operation::Nop
                }
                _ => continue,
            };
            builder.function.blocks[block_index].instructions[index].operation = operation;
        }
    }
    let (result, _) = builder.finish();
    result.verify()?;
    Ok(Cow::Owned(result))
}
