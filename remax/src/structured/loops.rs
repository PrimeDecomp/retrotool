//! Dominance identifies natural backedges. Reverse predecessor closure collects
//! each loop's members; exits and nesting constrain the lexical representation.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::{
    algo::dominators::simple_fast,
    graph::{DiGraph, NodeIndex},
};

use crate::ir::{BlockId, Function};

pub(super) struct Loop {
    pub members: BTreeSet<BlockId>,
    pub exit: Option<BlockId>,
}

pub(super) type Loops = BTreeMap<BlockId, Loop>;

/// A loop may have several backedges and several edges to the same exit. Loops
/// with distinct exit destinations require additional control representation.
pub(super) fn discover(function: &Function, graph: &DiGraph<(), ()>) -> Option<Loops> {
    let dominators = simple_fast(graph, NodeIndex::new(0));
    let dominates = |header: BlockId, block: BlockId| {
        dominators
            .dominators(NodeIndex::new(block.0))
            .is_some_and(|mut path| path.any(|node| node.index() == header.0))
    };
    let mut incoming = vec![Vec::new(); function.blocks.len()];
    let mut backedges = BTreeMap::<BlockId, Vec<BlockId>>::new();
    for (index, block) in function.blocks.iter().enumerate() {
        let source = BlockId(index);
        for edge in block.terminator.as_ref()?.edges() {
            incoming[edge.target.0].push(source);
            if dominates(edge.target, source) {
                backedges.entry(edge.target).or_default().push(source);
            }
        }
    }
    let mut loops = Loops::new();
    for (header, mut pending) in backedges {
        let mut members = BTreeSet::from([header]);
        while let Some(block) = pending.pop() {
            if members.insert(block) {
                if !dominates(header, block) {
                    return None;
                }
                pending.extend(&incoming[block.0]);
            }
        }
        let exits = members
            .iter()
            .flat_map(|block| function.blocks[block.0].terminator.as_ref().unwrap().edges())
            .map(|edge| edge.target)
            .filter(|target| !members.contains(target))
            .collect::<BTreeSet<_>>();
        if exits.len() > 1 {
            return None;
        }
        loops.insert(header, Loop { members, exit: exits.first().copied() });
    }
    // Lexical loops must be disjoint or nested. A remaining irreducible cycle
    // will also fail bounded region construction, retaining the dispatcher.
    for (index, a) in loops.values().enumerate() {
        for b in loops.values().skip(index + 1) {
            if !a.members.is_disjoint(&b.members)
                && !a.members.is_subset(&b.members)
                && !b.members.is_subset(&a.members)
            {
                return None;
            }
        }
    }
    Some(loops)
}
