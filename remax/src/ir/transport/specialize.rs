//! Bypass ambiguous transport when a consumer's path conditions prove a unique,
//! dominating origin. This preserves every instruction and checkpoint, including
//! select-condition observations and resource reads. It neither speculates an
//! origin nor gives an opaque value an ordinary bit representation.

use std::collections::BTreeMap;

use petgraph::{
    algo::dominators::simple_fast,
    graph::{DiGraph, NodeIndex},
};

use super::{paths::Paths, Transport};
use crate::ir::{BlockId, Definition, Function, Operation, Terminator, ValueId};

/// Requires sealed, structurally valid SSA. Types may still reflect untyped
/// native-register transport. Metadata references and transport definitions are
/// unchanged; cyclic graphs are left to an iteration-aware analysis.
pub(crate) fn specialize_uses(function: &mut Function) {
    let Some(paths) = Paths::analyze(function) else { return };
    let transport = Transport::new(function);
    let mut graph = DiGraph::<(), ()>::new();
    for _ in &function.blocks {
        graph.add_node(());
    }
    for (index, block) in function.blocks.iter().enumerate() {
        for edge in block.terminator.as_ref().unwrap().edges() {
            graph.add_edge(NodeIndex::new(index), NodeIndex::new(edge.target.0), ());
        }
    }
    let dominators = simple_fast(&graph, NodeIndex::new(0));
    let mut ambiguous = BTreeMap::new();
    let mut origins = BTreeMap::new();
    for (block_index, block) in function.blocks.iter_mut().enumerate() {
        let id = BlockId(block_index);
        let dominating_blocks =
            dominators.dominators(NodeIndex::new(block_index)).unwrap().collect::<Vec<_>>();
        let mut replacement = |value: ValueId, position: usize| {
            // Ordinary copy elimination belongs to the optimization pipeline.
            // Here only a path-dependent choice needs specialization.
            if !*ambiguous.entry(value).or_insert_with(|| transport.origins(value).len() > 1) {
                return value;
            }
            let origin =
                *origins.entry((id, value)).or_insert_with(|| paths.unique_origin(value, id));
            let Some(origin) = origin else { return value };
            let available = match function.values[origin.0].definition {
                Definition::Parameter { block, .. } => {
                    dominating_blocks.contains(&NodeIndex::new(block.0))
                }
                Definition::Instruction { block, index } => {
                    dominating_blocks.contains(&NodeIndex::new(block.0))
                        && (block != id || index < position)
                }
                _ => true,
            };
            if available {
                origin
            } else {
                value
            }
        };
        for (position, instruction) in block.instructions.iter_mut().enumerate() {
            match &mut instruction.operation {
                Operation::Copy(_) => {}
                Operation::Select { condition, .. } => {
                    *condition = replacement(*condition, position);
                }
                operation => operation.map_operands(|value| replacement(value, position)),
            }
        }
        let position = block.instructions.len();
        match block.terminator.as_mut().unwrap() {
            Terminator::Branch { condition, .. } => *condition = replacement(*condition, position),
            Terminator::Return(values) => {
                for value in values {
                    *value = replacement(*value, position);
                }
            }
            Terminator::Jump(_) => {}
        }
    }
}
