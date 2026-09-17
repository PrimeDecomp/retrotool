use super::{error, Node, Region, Shape};
use crate::{
    ir::{BlockId, Function, Terminator},
    program::Diagnostic,
};

struct LoopContext {
    header: BlockId,
    exit: Option<BlockId>,
    continues: bool,
    breaks: bool,
}

impl Shape {
    /// Check every represented path and lexical loop transfer against the CFG,
    /// independently of dominance, natural-loop discovery and region building.
    pub fn verify(&self, function: &Function) -> Result<(), Diagnostic> {
        match self {
            Self::Regions(body) => {
                if region(body, function, Some(BlockId(0)), &mut Vec::new())?.is_some() {
                    return Err(error("structured program falls through"));
                }
            }
            Self::Dispatch(prefix) => prefix.verify(function)?,
        }
        Ok(())
    }
}

fn region(
    body: &Region,
    function: &Function,
    mut at: Option<BlockId>,
    loops: &mut Vec<LoopContext>,
) -> Result<Option<BlockId>, Diagnostic> {
    for node in &body.0 {
        at = match node {
            Node::Block { block, arms } => {
                if at != Some(*block) {
                    return Err(error("structured path does not follow the CFG"));
                }
                let terminator = function
                    .blocks
                    .get(block.0)
                    .and_then(|block| block.terminator.as_ref())
                    .ok_or_else(|| error("structured block is missing"))?;
                match (terminator, arms) {
                    (Terminator::Jump(edge), None) => Some(edge.target),
                    (Terminator::Return(_), None) => None,
                    (Terminator::Branch { yes, no, .. }, Some([a, b])) => {
                        let a = region(a, function, Some(yes.target), loops)?;
                        let b = region(b, function, Some(no.target), loops)?;
                        match (a, b) {
                            (Some(a), Some(b)) if a != b => {
                                return Err(error("structured branch has incompatible joins"));
                            }
                            (Some(a), _) | (_, Some(a)) => Some(a),
                            (None, None) => None,
                        }
                    }
                    _ => return Err(error("structured terminator does not match the CFG")),
                }
            }
            Node::Loop { header, body, exit } => {
                if at != Some(*header) {
                    return Err(error("structured loop does not start at its header"));
                }
                // A continue is valid only after an actual CFG edge. Without
                // executing the header first, an empty body could invent a
                // backedge and silently replace the program with an infinite loop.
                if !matches!(body.0.first(), Some(Node::Block { block, .. }) if block == header) {
                    return Err(error("structured loop does not execute its header"));
                }
                loops.push(LoopContext {
                    header: *header,
                    exit: *exit,
                    continues: false,
                    breaks: false,
                });
                let fallthrough = region(body, function, at, loops)?;
                let context = loops.pop().unwrap();
                if fallthrough.is_some() || !context.continues || context.breaks != exit.is_some() {
                    return Err(error("structured loop has incompatible transfers"));
                }
                *exit
            }
            Node::Break | Node::Continue => {
                let context =
                    loops.last_mut().ok_or_else(|| error("loop transfer outside a loop"))?;
                let destination = match node {
                    Node::Break => {
                        context.breaks = true;
                        context.exit
                    }
                    Node::Continue => {
                        context.continues = true;
                        Some(context.header)
                    }
                    _ => unreachable!(),
                };
                if at.is_none() || at != destination {
                    return Err(error("loop transfer does not follow the CFG edge"));
                }
                None
            }
        };
    }
    Ok(at)
}
