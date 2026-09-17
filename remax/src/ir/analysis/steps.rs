//! Upper bounds on executed checkpoints, counting instructions and transfers.
//! Recognized bitset loops have a finite bound; other cycles prevent bounds for
//! themselves and their successors. Neither case erases preceding acyclic facts.
//! These scalar bounds do not establish uniformity or GPU reconvergence.

use std::collections::VecDeque;

use crate::ir::{analysis::Constants, BlockId, Function};

mod loops;

pub(crate) struct StepBounds {
    before: Vec<Option<u64>>,
    maximum: Option<u64>,
    acyclic: bool,
}

impl StepBounds {
    /// Requires a verified, reachable CFG. No branch feasibility is assumed;
    /// unrecognized cycles keep their incoming dependency and remain unbounded.
    pub fn new(function: &Function, constants: &Constants) -> Self {
        let count = function.blocks.len();
        let iterations = (0..count)
            .map(|index| loops::bitset_iterations(function, constants, BlockId(index)))
            .collect::<Vec<_>>();
        let bounded_backedge =
            |source: usize, target: usize| source == target && iterations[source].is_some();
        let mut predecessors = vec![0usize; count];
        for (index, block) in function.blocks.iter().enumerate() {
            for edge in block.terminator.as_ref().unwrap().edges() {
                if !bounded_backedge(index, edge.target.0) {
                    predecessors[edge.target.0] += 1;
                }
            }
        }
        let mut pending = predecessors
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(index))
            .collect::<VecDeque<_>>();
        let mut arrivals = vec![Some(0u64); count];
        let mut before = vec![None; count];
        let mut maximum = Some(0);
        let mut visited = 0;
        while let Some(index) = pending.pop_front() {
            let block = &function.blocks[index];
            let cost = u64::try_from(block.instructions.len()).ok().and_then(|n| n.checked_add(1));
            let visits = iterations[index].unwrap_or(1);
            // before[] bounds every visit, including the last loop iteration.
            // A bounded activation can still be part of an unbounded outer cycle;
            // its other incoming edges must be processed before reaching here.
            before[index] = arrivals[index]
                .zip(cost)
                .and_then(|(arrival, cost)| cost.checked_mul(visits - 1)?.checked_add(arrival));
            let departure = before[index].zip(cost).and_then(|(a, b)| a.checked_add(b));
            maximum = maximum.zip(departure).map(|(a, b)| a.max(b));
            visited += 1;
            for edge in block.terminator.as_ref().unwrap().edges() {
                let next = edge.target.0;
                if bounded_backedge(index, next) {
                    continue;
                }
                arrivals[next] = arrivals[next].zip(departure).map(|(a, b)| a.max(b));
                predecessors[next] -= 1;
                if predecessors[next] == 0 {
                    pending.push_back(next);
                }
            }
        }
        Self {
            before,
            maximum: if visited == count { maximum } else { None },
            acyclic: visited == count && iterations.iter().all(Option::is_none),
        }
    }

    /// Checkpoints consumed before entry to this block, over every path. None
    /// includes paths through unproven cycles and arithmetic overflow; it is not zero.
    pub fn before(&self, block: BlockId) -> Option<u64> { self.before[block.0] }

    /// Checkpoints through completion. None does not imply nontermination.
    pub fn maximum(&self) -> Option<u64> { self.maximum }

    pub fn is_acyclic(&self) -> bool { self.acyclic }
}
