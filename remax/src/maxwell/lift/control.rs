//! Discover native control before constructing values. Execution points include
//! predication state and the pending synchronization stack, so a shared PC is
//! not accidentally merged across different control contexts.
//!
//! The supported stack subset is closed, forward SSY regions with relative BRA
//! and SYNC. Resolving these targets establishes scalar flow only. The semantic
//! IR retains region transitions for separate GPU execution analysis.

use std::collections::BTreeMap;

use super::{error, unsupported};
use crate::{
    ir::convergence::{BranchKind, Region, Transfer},
    maxwell::{
        alu::PredicateOperand,
        control::{ControlOperation, FlowTarget, StackKind},
        Instruction, MaxwellCode, Operation,
    },
    program::{Diagnostic, DiagnosticKind},
};

// This bounds host analysis work, independently of any guest execution budget.
const MAX_EXECUTION_POINTS: usize = 65_536;

#[derive(Clone, Copy, Debug)]
pub(super) struct NodeId(pub usize);

#[derive(Clone, Copy, Debug)]
pub(super) struct Link {
    pub target: NodeId,
    pub convergence: Transfer,
}

#[derive(Debug)]
pub(super) enum Action {
    Execute { next: Link },
    Jump(Link),
    Branch { predicate: PredicateOperand, taken: Link, otherwise: Link, kind: BranchKind },
    Return,
}

#[derive(Debug)]
pub(super) struct Node {
    pub instruction: Instruction,
    pub convergence: Vec<Region>,
    pub action: Action,
}

pub(super) struct Graph {
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    pc: usize,
    execute: bool,
    convergence: Vec<Region>,
}

impl Point {
    fn at(&self, pc: usize) -> Self {
        Self { pc, execute: false, convergence: self.convergence.clone() }
    }
}

struct Target {
    point: Point,
    convergence: Transfer,
}

impl Target {
    fn preserve(point: Point) -> Self { Self { point, convergence: Transfer::Preserve } }
}

struct Discovery {
    instructions: BTreeMap<usize, Instruction>,
    code_len: usize,
    points: Vec<Point>,
    ids: BTreeMap<Point, NodeId>,
}

impl Graph {
    pub fn build(code: MaxwellCode<'_>) -> Result<Self, Diagnostic> {
        let instructions = code.instructions().map(|i| (i.offset, i)).collect::<BTreeMap<_, _>>();
        let first = instructions
            .keys()
            .next()
            .copied()
            .ok_or_else(|| error(None, DiagnosticKind::InvalidInput, "empty program"))?;
        let mut discovery = Discovery {
            instructions,
            code_len: code.bytes.len(),
            points: vec![],
            ids: BTreeMap::new(),
        };
        discovery.link(Target::preserve(Point {
            pc: first,
            execute: false,
            convergence: vec![],
        }))?;
        let mut nodes = Vec::new();
        while let Some(point) = discovery.points.get(nodes.len()).cloned() {
            let instruction = discovery.instructions[&point.pc];
            let action = discovery.action(&point, instruction)?;
            nodes.push(Node { instruction, convergence: point.convergence, action });
        }
        Ok(Self { nodes })
    }
}

impl Discovery {
    fn link(&mut self, target: Target) -> Result<Link, Diagnostic> {
        let point = target.point;
        if !self.instructions.contains_key(&point.pc) {
            return Err(error(
                Some(point.pc),
                DiagnosticKind::InvalidInput,
                "control flow reaches beyond the instruction stream",
            ));
        }
        if point
            .convergence
            .iter()
            .any(|region| point.pc <= region.setup_pc || point.pc >= region.join_pc)
        {
            return Err(unsupported(
                point.pc,
                "control flow leaves an active synchronization region",
            ));
        }
        let id = if let Some(&id) = self.ids.get(&point) {
            id
        } else {
            if self.points.len() >= MAX_EXECUTION_POINTS {
                return Err(unsupported(point.pc, "control graph exceeds the analysis size limit"));
            }
            let id = NodeId(self.points.len());
            self.points.push(point.clone());
            self.ids.insert(point, id);
            id
        };
        Ok(Link { target: id, convergence: target.convergence })
    }

    fn branch(
        &mut self,
        predicate: PredicateOperand,
        taken: Target,
        otherwise: Point,
    ) -> Result<Action, Diagnostic> {
        if predicate.register == 7 {
            let target = if predicate.inverted { Target::preserve(otherwise) } else { taken };
            return Ok(Action::Jump(self.link(target)?));
        }
        Ok(Action::Branch {
            predicate,
            taken: self.link(taken)?,
            otherwise: self.link(Target::preserve(otherwise))?,
            kind: BranchKind::ControlFlow,
        })
    }

    fn relative_target(&self, from: usize, target: FlowTarget) -> Result<usize, Diagnostic> {
        if !matches!(target, FlowTarget::Relative(_)) {
            return Err(unsupported(from, "unresolved indirect control target"));
        }
        target
            .instruction_offset(from, self.code_len)
            .map_err(|e| error(Some(from), DiagnosticKind::InvalidInput, e.to_string()))
    }

    fn action(&mut self, point: &Point, instruction: Instruction) -> Result<Action, Diagnostic> {
        let pc = instruction.offset;
        let operation = instruction.operation();
        let next_pc = if (pc + 8).is_multiple_of(32) { pc + 16 } else { pc + 8 };
        let next = point.at(next_pc);
        let predicate = PredicateOperand {
            register: instruction.predicate,
            inverted: instruction.predicate_inverted,
        };
        if let Operation::Control(flow) = operation {
            if flow.unrecognized_bits != 0 {
                return Err(unsupported(pc, "unrecognized control bits"));
            }
            return match flow.operation {
                ControlOperation::Branch {
                    target,
                    condition: 15,
                    limit: false,
                    uniform: false,
                } => {
                    let target = point.at(self.relative_target(pc, target)?);
                    self.branch(predicate, Target::preserve(target), next)
                }
                ControlOperation::Push { kind: StackKind::Sync, target, low_bits: 0 } => {
                    // SSY has no execution predicate. Its zero predicate-shaped
                    // bits must not introduce an incoming P0 requirement.
                    let join_pc = self.relative_target(pc, target)?;
                    if join_pc <= pc
                        || point.convergence.last().is_some_and(|outer| join_pc > outer.join_pc)
                    {
                        return Err(unsupported(
                            pc,
                            "synchronization target is outside its region",
                        ));
                    }
                    let region = Region { setup_pc: pc, join_pc };
                    let mut target = next;
                    target.convergence.push(region);
                    Ok(Action::Jump(
                        self.link(Target { point: target, convergence: Transfer::Enter(region) })?,
                    ))
                }
                ControlOperation::Pop { kind: StackKind::Sync, condition: 15 } => {
                    if predicate.register == 7 && predicate.inverted {
                        return Ok(Action::Jump(self.link(Target::preserve(next))?));
                    }
                    let mut target = point.clone();
                    let region = target.convergence.pop().ok_or_else(|| {
                        error(Some(pc), DiagnosticKind::InvalidInput, "SYNC has no pending SSY")
                    })?;
                    target.pc = region.join_pc;
                    target.execute = false;
                    self.branch(
                        predicate,
                        Target { point: target, convergence: Transfer::Leave },
                        next,
                    )
                }
                _ => Err(unsupported(
                    pc,
                    "control operation requires an established convergence/condition model",
                )),
            };
        }
        // Predication is known for these families. Other encodings must not
        // acquire semantics merely because their generic predicate bits match.
        if !matches!(
            operation,
            Operation::Alu(_)
                | Operation::Integer(_)
                | Operation::IntegerComparison(_)
                | Operation::FloatComparison(_)
                | Operation::Conversion(_)
                | Operation::Attribute(_)
                | Operation::PixelLoad(_)
                | Operation::ConstantLoad(_)
                | Operation::GlobalLoad(_)
                | Operation::LocalMemory(_)
                | Operation::DependencyBarrier(_)
                | Operation::Xmad(_)
                | Operation::TextureSample(_)
                | Operation::TextureSampleContiguous(_)
                | Operation::TextureLoad(_)
                | Operation::FragmentKill(_)
                | Operation::Nop
                | Operation::Exit
        ) {
            return Err(unsupported(
                pc,
                format!(
                    "semantic operation is not implemented: {operation:?} ({:#018x})",
                    instruction.word
                ),
            ));
        }
        if point.execute || (predicate.register == 7 && !predicate.inverted) {
            if matches!(operation, Operation::Exit) {
                if !point.convergence.is_empty() {
                    return Err(unsupported(
                        pc,
                        "EXIT inside a synchronization region needs lane-exit analysis",
                    ));
                }
                Ok(Action::Return)
            } else {
                Ok(Action::Execute { next: self.link(Target::preserve(next))? })
            }
        } else {
            let taken = Point { execute: true, ..point.clone() };
            let mut action = self.branch(predicate, Target::preserve(taken), next)?;
            if !matches!(operation, Operation::Exit) {
                if let Action::Branch { kind, .. } = &mut action {
                    *kind = BranchKind::Predication;
                }
            }
            Ok(action)
        }
    }
}
