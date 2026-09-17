//! Runtime budget accounting after control-flow analysis. Proven steps can be
//! charged together because operations cannot observe the remaining counter.
//! Potential exhaustion still checks each original checkpoint in order.

use super::{Options, Profile};
use crate::ir::{analysis::StepBounds, BlockId, Function};

pub(super) struct Plan {
    pub needs_counter: bool,
    proven_prefixes: Vec<u32>,
}

impl Plan {
    pub fn new(function: &Function, bounds: &StepBounds, options: Options) -> Self {
        let budget = u64::from(options.step_budget);
        // Checked keeps cyclic accounting independent of the loop proof, as it
        // keeps the original graph independent of production transformations.
        let needs_counter = (options.profile == Profile::Checked && !bounds.is_acyclic())
            || bounds.maximum().is_none_or(|bound| bound > budget);
        let proven_prefixes = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                if !needs_counter || options.profile == Profile::Checked {
                    return 0;
                }
                let Some(before) = bounds.before(BlockId(index)) else { return 0 };
                let Some(cost) =
                    u64::try_from(block.instructions.len()).ok().and_then(|n| n.checked_add(1))
                else {
                    return 0;
                };
                // At most the u32 budget can be charged. Paths shorter than the
                // bound retain extra steps for the later checked checkpoints.
                budget.saturating_sub(before).min(cost) as u32
            })
            .collect();
        Self { needs_counter, proven_prefixes }
    }

    pub fn proven_prefix(&self, block: BlockId) -> usize { self.proven_prefixes[block.0] as usize }
}
