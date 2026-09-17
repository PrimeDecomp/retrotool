//! The uniquely entered, non-repeating prefix before unresolved control flow.
//! A predicated instruction temporarily narrows participation and rejoins the
//! skipped path. That provenance permits continuing the prefix past its guard;
//! a scalar branch with the same shape does not establish the same fact.

use crate::{
    ir::{convergence::BranchKind, BlockId, Function, Terminator},
    program::{Diagnostic, DiagnosticKind, Phase},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Block(BlockId),
    Predicated { guard: BlockId, body: BlockId },
}

impl Step {
    pub fn entry(self) -> BlockId {
        match self {
            Self::Block(block) => block,
            Self::Predicated { guard, .. } => guard,
        }
    }

    fn blocks(self) -> impl Iterator<Item = BlockId> {
        let body = match self {
            Self::Block(_) => None,
            Self::Predicated { body, .. } => Some(body),
        };
        std::iter::once(self.entry()).chain(body)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EntryPrefix {
    pub steps: Vec<Step>,
}

impl EntryPrefix {
    /// Requires a verified, reachable CFG with no incoming edge to block zero.
    pub fn new(function: &Function) -> Self {
        let incoming = predecessors(function);
        let mut steps = Vec::new();
        let mut at = BlockId(0);
        loop {
            let term = function.blocks[at.0].terminator.as_ref().unwrap();
            if let Terminator::Branch { kind: BranchKind::Predication, yes, no, .. } = term {
                // The verifier establishes the body's unconditional rejoin.
                // Neither the body nor the join may be entered from the tail.
                if incoming[yes.target.0] == [at]
                    && same_predecessors(&incoming[no.target.0], &[at, yes.target])
                {
                    steps.push(Step::Predicated { guard: at, body: yes.target });
                    at = no.target;
                    continue;
                }
            }
            steps.push(Step::Block(at));
            match term {
                Terminator::Jump(edge) if incoming[edge.target.0] == [at] => at = edge.target,
                _ => break,
            }
        }
        Self { steps }
    }

    pub fn last(&self) -> BlockId { self.steps.last().expect("nonempty prefix").entry() }

    pub fn contains(&self, block: BlockId) -> bool {
        self.steps.iter().any(|step| step.blocks().any(|candidate| candidate == block))
    }

    /// Guard bodies remain conditional. In particular, an implicit sample in
    /// a predicated body cannot inherit the guard's full entry participation.
    pub fn has_entry_participation(&self, block: BlockId) -> bool {
        self.steps.iter().any(|step| step.entry() == block)
    }

    /// Independently check the supplied steps and all their incoming edges.
    /// Following one path from entry alone would not establish non-repetition.
    pub fn verify(&self, function: &Function) -> Result<(), Diagnostic> {
        let error =
            |message| Diagnostic::new(Phase::Structure, None, DiagnosticKind::InvalidIr, message);
        if self.steps.first().map(|step| step.entry()) != Some(BlockId(0)) {
            return Err(error("execution prefix does not start at entry"));
        }
        if !matches!(self.steps.last(), Some(Step::Block(_))) {
            return Err(error("execution prefix has no final unconditional block"));
        }
        let incoming = predecessors(function);
        let mut visited = vec![false; function.blocks.len()];
        let mut expected_entry = BlockId(0);
        let mut expected_incoming = Vec::new();
        for (index, &step) in self.steps.iter().enumerate() {
            for block in step.blocks() {
                let seen = visited.get_mut(block.0).ok_or_else(|| error("invalid prefix block"))?;
                if std::mem::replace(seen, true) {
                    return Err(error("execution prefix repeats a block"));
                }
            }
            let entry = step.entry();
            if entry != expected_entry {
                return Err(error("execution prefix does not follow its continuation"));
            }
            if !same_predecessors(&incoming[entry.0], &expected_incoming) {
                return Err(error("execution prefix has another incoming edge"));
            }
            let term = function.blocks[entry.0].terminator.as_ref().unwrap();
            match step {
                Step::Block(_) if index + 1 == self.steps.len() => {}
                Step::Block(_) => {
                    let Terminator::Jump(edge) = term else {
                        return Err(error("execution prefix does not follow unconditional edges"));
                    };
                    expected_entry = edge.target;
                    expected_incoming = vec![entry];
                }
                Step::Predicated { body, .. } => {
                    let Terminator::Branch { kind: BranchKind::Predication, yes, no, .. } = term
                    else {
                        return Err(error("execution prefix has no predication provenance"));
                    };
                    if yes.target != body || incoming[body.0] != [entry] {
                        return Err(error("execution prefix has an invalid predicated body"));
                    }
                    expected_entry = no.target;
                    expected_incoming = vec![entry, body];
                }
            }
        }
        Ok(())
    }
}

fn predecessors(function: &Function) -> Vec<Vec<BlockId>> {
    let mut incoming = vec![Vec::new(); function.blocks.len()];
    for (index, block) in function.blocks.iter().enumerate() {
        for edge in block.terminator.as_ref().unwrap().edges() {
            incoming[edge.target.0].push(BlockId(index));
        }
    }
    incoming
}

fn same_predecessors(actual: &[BlockId], expected: &[BlockId]) -> bool {
    actual.len() == expected.len() && expected.iter().all(|block| actual.contains(block))
}
