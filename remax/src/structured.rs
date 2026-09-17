//! Control structure separate from operations and target syntax. Regions retain
//! common joins and can contain single-exit natural loops. Irreducible graphs,
//! unsupported exits and excessive duplication retain a scalar dispatcher.
//! Structuring preserves scalar paths; it does not establish GPU reconvergence.

use petgraph::{algo::toposort, graph::DiGraph};

use crate::{
    ir::{analysis::EntryPrefix, BlockId, Function, Terminator},
    program::{Diagnostic, DiagnosticKind, Phase},
};

mod loops;
mod regions;
mod verify;

#[derive(Clone, Copy)]
pub(crate) enum Strategy {
    /// Keep the original cyclic dispatcher as a checked comparison path.
    Acyclic,
    NaturalLoops,
}

#[derive(Debug)]
pub(crate) enum Shape {
    Regions(Region),
    /// Execute the prefix once, then dispatch from its final terminator.
    /// No tail edge can return to the prefix's instructions.
    Dispatch(EntryPrefix),
}

#[derive(Debug, Default)]
pub(crate) struct Region(pub Vec<Node>);

#[derive(Debug)]
pub(crate) enum Node {
    /// Operations, conditions, edge arguments and returns remain in SSA. Arms
    /// start after the corresponding edge's simultaneous parameter transfer.
    Block {
        block: BlockId,
        arms: Option<[Region; 2]>,
    },
    Loop {
        header: BlockId,
        body: Region,
        exit: Option<BlockId>,
    },
    /// The preceding SSA edge has already transferred its arguments. These
    /// nodes change only lexical control and must target the innermost loop.
    Break,
    Continue,
}

fn error(message: &str) -> Diagnostic {
    Diagnostic::new(Phase::Structure, None, DiagnosticKind::InvalidIr, message)
}

/// Requires verified SSA. The independently checked prefix supplies the part
/// that a dispatcher may execute before entering its state loop.
pub(crate) fn build(
    function: &Function,
    prefix: &EntryPrefix,
    strategy: Strategy,
) -> Result<Shape, Diagnostic> {
    prefix.verify(function)?;
    let mut graph = DiGraph::<(), ()>::new();
    let nodes = (0..=function.blocks.len()).map(|_| graph.add_node(())).collect::<Vec<_>>();
    let exit = nodes[function.blocks.len()];
    for (index, block) in function.blocks.iter().enumerate() {
        let terminator = block.terminator.as_ref().expect("verified terminator");
        for edge in terminator.edges() {
            graph.add_edge(nodes[index], nodes[edge.target.0], ());
        }
        if matches!(terminator, Terminator::Return(_)) {
            graph.add_edge(nodes[index], exit, ());
        }
    }
    let loops = match strategy {
        Strategy::Acyclic if toposort(&graph, None).is_err() => None,
        Strategy::Acyclic => Some(Default::default()),
        Strategy::NaturalLoops => loops::discover(function, &graph),
    };
    let shape = loops
        .and_then(|loops| regions::build(function, graph, loops))
        .map(Shape::Regions)
        .unwrap_or_else(|| Shape::Dispatch(prefix.clone()));
    shape.verify(function)?;
    Ok(shape)
}
