//! Coalesce linear execution into basic blocks without changing checkpoints or
//! convergence provenance. This reduces dispatcher traffic for cyclic shaders.

use crate::ir::{
    convergence::{BranchKind, Transfer},
    BlockId, Definition, Function, Instruction, Operation, Terminator,
};

/// The caller supplies verified IR and verifies the result after a change.
pub(super) fn coalesce(function: &mut Function) -> bool {
    let count = function.blocks.len();
    let mut incoming = vec![0; count];
    let mut pinned = vec![false; count];
    for block in &function.blocks {
        let terminator = block.terminator.as_ref().expect("verified terminator");
        for edge in terminator.edges() {
            incoming[edge.target.0] += 1;
            // These block PCs identify a native join or guarded instruction;
            // retaining them also keeps the execution analysis independent.
            if edge.convergence == Transfer::Leave {
                pinned[edge.target.0] = true;
            }
        }
        if let Terminator::Branch { kind: BranchKind::Predication, yes, .. } = terminator {
            pinned[yes.target.0] = true;
        }
    }

    let mut removed = vec![false; count];
    for source in 0..count {
        if removed[source] || pinned[source] {
            continue;
        }
        loop {
            let block = &function.blocks[source];
            // A location-less entry checkpoint must keep its diagnostic PC.
            let Some(pc) = block.pc else { break };
            let Some(Terminator::Jump(edge)) = &block.terminator else { break };
            let target = edge.target.0;
            if target == source
                || incoming[target] != 1
                || pinned[target]
                || edge.convergence != Transfer::Preserve
                || !edge.arguments.is_empty()
                || !function.blocks[target].parameters.is_empty()
            {
                break;
            }
            debug_assert!(!removed[target]);
            debug_assert_eq!(block.convergence, function.blocks[target].convergence);

            let instructions = std::mem::take(&mut function.blocks[target].instructions);
            let terminator = function.blocks[target].terminator.take();
            let target_pc = function.blocks[target].pc;
            let block = &mut function.blocks[source];
            // The removed jump consumed a step. Retain that checkpoint, in
            // order, including its exhaustion PC even inside an unbounded loop.
            block.instructions.push(Instruction { result: None, operation: Operation::Nop, pc });
            block.instructions.extend(instructions);
            block.terminator = terminator;
            block.pc = target_pc;
            removed[target] = true;
        }
    }
    if !removed.iter().any(|&value| value) {
        return false;
    }

    let mut remap = vec![BlockId(usize::MAX); count];
    let mut next = 0;
    for (old, &removed) in removed.iter().enumerate() {
        if !removed {
            remap[old] = BlockId(next);
            next += 1;
        }
    }
    let mut old = 0;
    function.blocks.retain(|_| {
        let keep = !removed[old];
        old += 1;
        keep
    });
    for (block_index, block) in function.blocks.iter_mut().enumerate() {
        let id = BlockId(block_index);
        for (index, &value) in block.parameters.iter().enumerate() {
            function.values[value.0].definition = Definition::Parameter { block: id, index };
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            if let Some(value) = instruction.result {
                function.values[value.0].definition = Definition::Instruction { block: id, index };
            }
        }
        for edge in block.terminator.as_mut().expect("retained terminator").edges_mut() {
            edge.target = remap[edge.target.0];
        }
    }
    true
}
