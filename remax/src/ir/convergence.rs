//! Native execution provenance alongside scalar value flow. Predication marks
//! an instruction's guarded execution and rejoin. Synchronization contexts
//! record pending joins; transfers preserve, enter or leave one region. These
//! facts support execution analysis, not an arbitrary host warp scheduler.

use super::{Function, Terminator};
use crate::program::{Diagnostic, DiagnosticKind, Phase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BranchKind {
    ControlFlow,
    /// A guarded instruction: the taken block executes conditionally, then
    /// rejoins the skipped path with the guard's original lane participation.
    /// This is frontend provenance, not an inference from a scalar diamond.
    Predication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Region {
    pub setup_pc: usize,
    pub join_pc: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Transfer {
    #[default]
    Preserve,
    Enter(Region),
    Leave,
}

impl Function {
    /// Check region nesting and every edge's context change independently of
    /// native control discovery. The general verifier checks edge indices first.
    pub(super) fn verify_convergence(&self) -> Result<(), Diagnostic> {
        let error =
            |pc, message| Diagnostic::new(Phase::Verify, pc, DiagnosticKind::InvalidIr, message);
        if !self.blocks[0].convergence.is_empty() {
            return Err(error(None, "entry block has a pending convergence region"));
        }
        for block in &self.blocks {
            for (index, region) in block.convergence.iter().enumerate() {
                let Some(pc) = block.pc else {
                    return Err(error(None, "convergence context has no source location"));
                };
                if !(region.setup_pc < pc && pc < region.join_pc) {
                    return Err(error(block.pc, "block lies outside its convergence region"));
                }
                if index > 0 {
                    let outer = block.convergence[index - 1];
                    if region.setup_pc <= outer.setup_pc || region.join_pc > outer.join_pc {
                        return Err(error(block.pc, "convergence regions are not nested"));
                    }
                }
            }
            let terminator = block.terminator.as_ref().expect("verified terminator");
            if let Terminator::Branch { kind: BranchKind::Predication, yes, no, .. } = terminator {
                let body = &self.blocks[yes.target.0];
                let valid_body = matches!(&body.terminator,
                    Some(Terminator::Jump(edge))
                        if edge.target == no.target && edge.convergence == Transfer::Preserve);
                if yes.target == no.target
                    || !valid_body
                    || block.pc.is_none()
                    || block.pc != body.pc
                    || yes.convergence != Transfer::Preserve
                    || no.convergence != Transfer::Preserve
                {
                    return Err(error(block.pc, "invalid predicated instruction region"));
                }
            }
            if matches!(terminator, Terminator::Return(_)) && !block.convergence.is_empty() {
                return Err(error(block.pc, "return leaves a pending convergence region"));
            }
            for edge in terminator.edges() {
                let target = &self.blocks[edge.target.0];
                let mut expected = block.convergence.clone();
                match edge.convergence {
                    Transfer::Preserve => {}
                    Transfer::Enter(region) => {
                        if !matches!(terminator, Terminator::Jump(_))
                            || block.pc != Some(region.setup_pc)
                            || region.setup_pc >= region.join_pc
                        {
                            return Err(error(block.pc, "invalid convergence setup transfer"));
                        }
                        expected.push(region);
                    }
                    Transfer::Leave => {
                        let Some(region) = expected.pop() else {
                            return Err(error(block.pc, "convergence transfer has no region"));
                        };
                        if target.pc != Some(region.join_pc) {
                            return Err(error(block.pc, "convergence transfer misses its join"));
                        }
                    }
                }
                if target.convergence != expected {
                    return Err(error(block.pc, "edge changes convergence context incorrectly"));
                }
            }
        }
        Ok(())
    }
}
