use petgraph::{
    algo::dominators::simple_fast,
    graph::{DiGraph, NodeIndex},
};

use super::{loops::Loops, Node, Region};
use crate::ir::{BlockId, Function, Terminator};

struct Builder<'a> {
    function: &'a Function,
    joins: Vec<Option<BlockId>>,
    loops: Loops,
    budget: usize,
}

pub(super) fn build(
    function: &Function,
    mut graph: DiGraph<(), ()>,
    loops: Loops,
) -> Option<Region> {
    graph.reverse();
    let exit = NodeIndex::new(function.blocks.len());
    let postdominators = simple_fast(&graph, exit);
    let joins = (0..function.blocks.len())
        .map(|index| {
            postdominators
                .immediate_dominator(NodeIndex::new(index))
                .filter(|&node| node != exit)
                .map(|node| BlockId(node.index()))
        })
        .collect();
    // Bound both duplication and host recursion independently of runtime fuel.
    Builder { function, joins, loops, budget: function.blocks.len().saturating_mul(4) }.region(
        BlockId(0),
        None,
        None,
        0,
        false,
    )
}

impl Builder<'_> {
    fn region(
        &mut self,
        mut at: BlockId,
        stop: Option<BlockId>,
        active_loop: Option<BlockId>,
        depth: usize,
        mut entering_loop: bool,
    ) -> Option<Region> {
        if depth > 128 {
            return None;
        }
        let mut result = Region::default();
        while Some(at) != stop {
            self.budget = self.budget.checked_sub(1)?;
            if let Some(header) = active_loop {
                let loop_info = &self.loops[&header];
                if at == header && !entering_loop {
                    result.0.push(Node::Continue);
                    return Some(result);
                }
                if Some(at) == loop_info.exit {
                    result.0.push(Node::Break);
                    return Some(result);
                }
                if !loop_info.members.contains(&at) {
                    return None;
                }
            }
            entering_loop = false;
            if active_loop != Some(at) && self.loops.contains_key(&at) {
                let exit = self.loops[&at].exit;
                let body = self.region(at, None, Some(at), depth + 1, true)?;
                result.0.push(Node::Loop { header: at, body, exit });
                let Some(exit) = exit else { return Some(result) };
                at = exit;
                continue;
            }
            let terminator = self.function.blocks.get(at.0)?.terminator.as_ref()?;
            let join = self.joins[at.0].or(active_loop);
            let arms = if let Terminator::Branch { yes, no, .. } = terminator {
                let yes = yes.target;
                let no = no.target;
                Some([
                    self.region(yes, join, active_loop, depth + 1, false)?,
                    self.region(no, join, active_loop, depth + 1, false)?,
                ])
            } else {
                None
            };
            result.0.push(Node::Block { block: at, arms });
            at = match self.function.blocks[at.0].terminator.as_ref()? {
                Terminator::Jump(edge) => edge.target,
                Terminator::Branch { .. } => match join {
                    Some(join) => join,
                    None => return Some(result),
                },
                Terminator::Return(_) => return Some(result),
            };
        }
        Some(result)
    }
}
