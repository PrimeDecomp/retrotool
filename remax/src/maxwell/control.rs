//! Maxwell relative branches and reconvergence regions. Encodings follow uam
//! GM107 emitBRA/SSY/SYNC (5a5afc2) and Mesa NAK sm50.rs (822120d6).
//! A region graph describes per-invocation value flow. It does not emulate warp
//! scheduling, cross-lane operations or derivative execution in divergent flow.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{bail, ensure, Context, Result};
use serde_derive::Serialize;

use crate::maxwell::{alu::PredicateOperand, Instruction, MaxwellCode};

/// KIL suppresses fragment outputs while retaining helper-lane execution. It is
/// not a control-flow terminator: NAK emits it for NIR demote/demote_if.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FragmentKill {
    pub condition: u8,
    pub unrecognized_bits: u64,
}

impl FragmentKill {
    pub fn decode(word: u64) -> Option<Self> {
        (word >> 48 == 0xe330).then_some(Self {
            condition: (word & 31) as u8,
            unrecognized_bits: word & !(0xffff000000000000 | 0xf0000 | 0x1f),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum StackKind {
    Sync,
    Break,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FlowTarget {
    /// Signed byte displacement from the instruction address plus eight.
    Relative(i32),
    Constant {
        buffer: u8,
        byte_offset: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ControlOperation {
    Branch { target: FlowTarget, condition: u8, limit: bool, uniform: bool },
    Push { kind: StackKind, target: FlowTarget, low_bits: u8 },
    Pop { kind: StackKind, condition: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ControlInstruction {
    pub operation: ControlOperation,
    pub unrecognized_bits: u64,
}

impl ControlInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let opcode = word >> 48;
        let push = match opcode {
            0xe290 => Some(StackKind::Sync),
            0xe2a0 => Some(StackKind::Break),
            0xe2b0 => Some(StackKind::Continue),
            _ => None,
        };
        let pop = match opcode {
            0xf0f8 => Some(StackKind::Sync),
            0xe340 => Some(StackKind::Break),
            0xe350 => Some(StackKind::Continue),
            _ => None,
        };
        if opcode != 0xe240 && push.is_none() && pop.is_none() {
            return None;
        }
        let mut used = 0xffff000000000000u64 | 0x1f;
        let operation = if let Some(kind) = pop {
            used |= 0xf0000;
            ControlOperation::Pop { kind, condition: (word & 31) as u8 }
        } else {
            used |= 1 << 5;
            let target = if word & (1 << 5) != 0 {
                used |= 0x1fffff << 20;
                FlowTarget::Constant {
                    buffer: ((word >> 36) & 31) as u8,
                    byte_offset: ((word >> 20) & 0xffff) as u16,
                }
            } else {
                used |= 0xffffff << 20;
                FlowTarget::Relative((((word >> 20) as u32) << 8) as i32 >> 8)
            };
            if let Some(kind) = push {
                // uam/native pushes have zero predicate bits. NAK also writes a
                // low condition nibble; retain it, without interpreting it as P0.
                ControlOperation::Push { kind, target, low_bits: (word & 31) as u8 }
            } else {
                used |= 0xf0000 | (3 << 6);
                ControlOperation::Branch {
                    target,
                    condition: (word & 31) as u8,
                    limit: word & (1 << 6) != 0,
                    uniform: word & (1 << 7) != 0,
                }
            }
        };
        Some(Self { operation, unrecognized_bits: word & !used })
    }
}

impl Instruction {
    pub fn control_flow(&self) -> Option<ControlInstruction> {
        ControlInstruction::decode(self.word)
    }
}

impl FlowTarget {
    pub fn instruction_offset(self, from: usize, code_len: usize) -> Result<usize> {
        let Self::Relative(delta) = self else {
            bail!("constant-buffer control target unresolved");
        };
        let raw = i64::try_from(from)? + 8 + i64::from(delta);
        ensure!(raw >= 0 && raw % 8 == 0, "unaligned/negative control target {raw:#x}");
        let raw = usize::try_from(raw)?;
        // Native branches can land on the scheduling word preceding a group.
        let target = raw
            .checked_add(if raw % 32 == 0 { 8 } else { 0 })
            .context("control target overflow")?;
        ensure!(target < code_len, "control target {target:#x} exceeds code");
        Ok(target)
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum RegionEdge {
    /// None leaves the reconvergence region at its resolved end offset.
    Jump(Option<usize>),
    Branch {
        predicate: PredicateOperand,
        taken: Option<usize>,
        otherwise: Option<usize>,
    },
}

impl RegionEdge {
    pub fn targets(&self) -> Vec<Option<usize>> {
        match *self {
            Self::Jump(t) => vec![t],
            Self::Branch { taken, otherwise, .. } => vec![taken, otherwise],
        }
    }

    fn map(&self, f: impl Fn(usize) -> usize) -> Self {
        match *self {
            Self::Jump(t) => Self::Jump(t.map(&f)),
            Self::Branch { predicate, taken, otherwise } => {
                Self::Branch { predicate, taken: taken.map(&f), otherwise: otherwise.map(f) }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegionBlock {
    pub offset: usize,
    pub instructions: Vec<Instruction>,
    pub edge: RegionEdge,
}

#[derive(Clone, Debug)]
pub struct ReconvergenceRegion {
    pub entry: usize,
    pub end_offset: usize,
    pub blocks: Vec<RegionBlock>,
    pub instruction_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct State {
    offset: usize,
    stack: Vec<usize>,
}
struct Node {
    instruction: Instruction,
    edge: RegionEdge,
    terminator: bool,
}

impl ReconvergenceRegion {
    /// A closed SSY region, including nested SSY/SYNC and relative BRA loops.
    /// Edges may leave only by popping the initial sync frame at its target.
    /// Other stack kinds, exits/warp branches and outside jumps remain explicit
    /// errors. This is a structural requirement, not a material/shader allowlist.
    pub fn build(code: MaxwellCode<'_>, start: Instruction) -> Result<Self> {
        let flow = start.control_flow().context("region must start with SSY")?;
        ensure!(flow.unrecognized_bits == 0, "unrecognized SSY bits");
        let ControlOperation::Push { kind: StackKind::Sync, target, low_bits: 0 } = flow.operation
        else {
            bail!("unsupported reconvergence setup");
        };
        let end_offset = target.instruction_offset(start.offset, code.bytes.len())?;
        ensure!(end_offset > start.offset, "reconvergence target must follow its setup");
        let instructions = code.instructions().map(|i| (i.offset, i)).collect::<BTreeMap<_, _>>();
        let next = |offset: usize| -> Result<usize> {
            let raw = offset.checked_add(8).context("instruction address overflow")?;
            Ok(if raw % 32 == 0 { raw + 8 } else { raw })
        };
        let first = State { offset: next(start.offset)?, stack: vec![end_offset] };
        let mut states = vec![first.clone()];
        let mut ids = BTreeMap::from([(first, 0)]);
        let mut nodes = Vec::<Node>::new();
        let mut work = VecDeque::from([0usize]);
        while let Some(index) = work.pop_front() {
            ensure!(index == nodes.len(), "internal region traversal order");
            let state = states[index].clone();
            ensure!(
                state.offset > start.offset && state.offset < end_offset,
                "control flow escapes SSY region without synchronization"
            );
            let i =
                *instructions.get(&state.offset).context("missing instruction at region target")?;
            let fallthrough = State { offset: next(i.offset)?, stack: state.stack.clone() };
            let mut register = |s: State| -> Result<Option<usize>> {
                if s.stack.is_empty() {
                    ensure!(s.offset == end_offset, "sync exits at wrong target");
                    return Ok(None);
                }
                ensure!(
                    s.stack.len() <= 32 && states.len() <= instructions.len() * 32,
                    "control stack requires unsupported unbounded expansion"
                );
                ensure!(
                    s.offset > start.offset && s.offset < end_offset,
                    "control flow leaves active reconvergence region"
                );
                if let Some(&id) = ids.get(&s) {
                    return Ok(Some(id));
                }
                let id = states.len();
                ids.insert(s.clone(), id);
                states.push(s);
                work.push_back(id);
                Ok(Some(id))
            };
            let mut terminator = false;
            let edge = if let Some(flow) = i.control_flow() {
                ensure!(
                    flow.unrecognized_bits == 0,
                    "unrecognized control bits at {:#x}",
                    i.offset
                );
                match flow.operation {
                    ControlOperation::Push { kind: StackKind::Sync, target, low_bits: 0 } => {
                        let end = target.instruction_offset(i.offset, code.bytes.len())?;
                        ensure!(
                            end > i.offset && end <= end_offset,
                            "nested SSY target outside region"
                        );
                        let mut s = fallthrough;
                        s.stack.push(end);
                        RegionEdge::Jump(register(s)?)
                    }
                    ControlOperation::Branch {
                        target,
                        condition: 15,
                        limit: false,
                        uniform: false,
                    } => {
                        terminator = true;
                        let taken = State {
                            offset: target.instruction_offset(i.offset, code.bytes.len())?,
                            stack: state.stack.clone(),
                        };
                        Self::conditional(i, taken, fallthrough, &mut register)?
                    }
                    ControlOperation::Pop { kind: StackKind::Sync, condition: 15 } => {
                        terminator = true;
                        let mut stack = state.stack.clone();
                        let offset = stack.pop().context("SYNC stack underflow")?;
                        Self::conditional(i, State { offset, stack }, fallthrough, &mut register)?
                    }
                    _ => bail!(
                        "unsupported control operation at {:#x}: {:?}",
                        i.offset,
                        flow.operation
                    ),
                }
            } else {
                ensure!(
                    i.word & !0xf0000 != 0xe30000000000000f,
                    "EXIT inside a reconvergence region requires additional active-lane handling"
                );
                RegionEdge::Jump(register(fallthrough)?)
            };
            nodes.push(Node { instruction: i, edge, terminator });
        }
        let mut predecessors = vec![0usize; nodes.len()];
        let mut leaders = BTreeSet::from([0usize]);
        for node in &nodes {
            for target in node.edge.targets().into_iter().flatten() {
                predecessors[target] += 1;
                if node.terminator {
                    leaders.insert(target);
                }
            }
        }
        for (id, &count) in predecessors.iter().enumerate() {
            if count != 1 {
                leaders.insert(id);
            }
        }
        let mut blocks = Vec::new();
        let mut owners = vec![usize::MAX; nodes.len()];
        for &leader in &leaders {
            let block_id = blocks.len();
            let mut id = leader;
            let mut body = Vec::new();
            loop {
                ensure!(owners[id] == usize::MAX, "overlapping control blocks");
                owners[id] = block_id;
                let node = &nodes[id];
                if node.instruction.control_flow().is_none() {
                    body.push(node.instruction);
                }
                let RegionEdge::Jump(Some(next)) = node.edge else {
                    break;
                };
                if node.terminator || leaders.contains(&next) {
                    break;
                }
                id = next;
            }
            blocks.push(RegionBlock {
                offset: nodes[leader].instruction.offset,
                instructions: body,
                edge: nodes[id].edge.clone(),
            });
        }
        ensure!(owners.iter().all(|&o| o != usize::MAX), "unassigned control-flow cycle");
        for block in &mut blocks {
            block.edge = block.edge.map(|id| owners[id]);
        }
        ensure!(
            blocks.iter().any(|b| b.edge.targets().contains(&None)),
            "reconvergence region has no exit"
        );
        Ok(Self { entry: owners[0], end_offset, blocks, instruction_count: nodes.len() + 1 })
    }

    fn conditional(
        i: Instruction,
        taken: State,
        otherwise: State,
        register: &mut impl FnMut(State) -> Result<Option<usize>>,
    ) -> Result<RegionEdge> {
        if i.predicate == 7 {
            return Ok(RegionEdge::Jump(register(if i.predicate_inverted {
                otherwise
            } else {
                taken
            })?));
        }
        Ok(RegionEdge::Branch {
            predicate: PredicateOperand { register: i.predicate, inverted: i.predicate_inverted },
            taken: register(taken)?,
            otherwise: register(otherwise)?,
        })
    }
}

/// Per-invocation scoreboard wait; not a memory fence or workgroup barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DependencyBarrier {
    pub less_equal: bool,
    pub scoreboard: u8,
    pub count: u8,
    pub mask: u8,
    pub unrecognized_bits: u64,
}
impl DependencyBarrier {
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xffff000000000000 != 0xf0f0000000000000 {
            return None;
        }
        let used = 0xffff000000000000u64 | 0xf0000 | 0x3f | (0x3ff << 20);
        Some(Self {
            less_equal: word & (1 << 29) != 0,
            scoreboard: ((word >> 26) & 7) as u8,
            count: ((word >> 20) & 63) as u8,
            mask: (word & 63) as u8,
            unrecognized_bits: word & !used,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offset(index: usize) -> usize { index / 3 * 32 + (index % 3 + 1) * 8 }
    fn jump(op: u64, from: usize, to: usize, predicate: u64) -> u64 {
        op | ((((offset(to) as i64 - offset(from) as i64 - 8) as u64) & 0xffffff) << 20) | predicate
    }
    fn code(words: &[u64]) -> Vec<u8> {
        words
            .chunks(3)
            .flat_map(|g| {
                [
                    0x001f8000fc0007e0,
                    g[0],
                    *g.get(1).unwrap_or(&0x50b0000000070f00),
                    *g.get(2).unwrap_or(&0x50b0000000070f00),
                ]
                .into_iter()
                .flat_map(u64::to_le_bytes)
            })
            .collect()
    }
    #[test]
    fn nested_sync_paths_keep_the_outer_frame() {
        let bytes = code(&[
            jump(0xe290000000000000, 0, 7, 0),
            jump(0xe290000000000000, 1, 5, 0),
            0xf0f800000000000f,
            0x010000000017f000,
            0xf0f800000007000f,
            0x010000000027f001,
            0xf0f800000007000f,
            0xe30000000007000f,
        ]);
        let c = MaxwellCode::parse(&bytes).unwrap();
        let region = ReconvergenceRegion::build(c, c.instructions().next().unwrap()).unwrap();
        assert_eq!(region.end_offset, offset(7));
        assert_eq!(region.blocks.iter().filter(|b| b.edge.targets().contains(&None)).count(), 1);
        let k = crate::wgsl::lower_scalar(c).unwrap();
        assert!(k.defined_registers_on_exit.contains(&1));
        assert!(!k.defined_registers_on_exit.contains(&0));
        assert_eq!(k.input_predicates, [0]);
        assert!(k.uses_memory_status);
        assert_eq!(k.constant_memory_binding, None);
    }
    #[test]
    fn relative_targets_preserve_schedule_entry_and_reject_invalid_addresses() {
        assert_eq!(FlowTarget::Relative(-24).instruction_offset(0x58, 128).unwrap(), 0x48);
        assert_eq!(FlowTarget::Relative(-32).instruction_offset(0x58, 128).unwrap(), 0x48);
        assert!(FlowTarget::Relative(-256).instruction_offset(8, 128).is_err());
        assert!(FlowTarget::Relative(1).instruction_offset(8, 128).is_err());
        assert!(FlowTarget::Relative(112).instruction_offset(8, 128).is_err());
        assert!(FlowTarget::Constant { buffer: 3, byte_offset: 16 }
            .instruction_offset(8, 128)
            .is_err());
    }
    #[test]
    fn branches_cannot_bypass_the_sync_pop() {
        let bytes = code(&[
            jump(0xe290000000000000, 0, 3, 0),
            jump(0xe240000000000000, 1, 3, 0x7000f),
            0xf0f800000007000f,
            0xe30000000007000f,
        ]);
        let c = MaxwellCode::parse(&bytes).unwrap();
        assert!(ReconvergenceRegion::build(c, c.instructions().next().unwrap()).is_err());
    }

    #[test]
    fn branch_definitions_do_not_hide_inputs_on_the_other_path() {
        let mut words = [
            jump(0xe290000000000000, 0, 6, 0),
            jump(0xe240000000000000, 1, 4, 0xf), // @P0
            0x010000000017f000,                  // MOV32I R0,1
            jump(0xe240000000000000, 3, 5, 0x7000f),
            0x5c98078000070001, // MOV R1,R0: needs incoming R0 on this path
            0xf0f800000007000f,
            0x5c98078000070002, // MOV R2,R0 after join
            0xe30000000007000f,
        ];
        let bytes = code(&words);
        let k = crate::wgsl::lower_scalar(MaxwellCode::parse(&bytes).unwrap()).unwrap();
        assert_eq!(k.input_registers, [0]);
        assert_eq!(k.input_predicates, [0]);
        assert_eq!(k.defined_registers_on_exit, [2]);
        words[4] = 0x010000000027f000; // Both paths now assign R0.
        let bytes = code(&words);
        let k = crate::wgsl::lower_scalar(MaxwellCode::parse(&bytes).unwrap()).unwrap();
        assert!(k.input_registers.is_empty());
        assert_eq!(k.defined_registers_on_exit, [0, 2]);
    }

    #[test]
    fn control_modes_require_explicit_semantics() {
        let ssy = jump(0xe290000000000000, 0, 3, 0);
        let bra = jump(0xe240000000000000, 1, 2, 0x7000f);
        for (setup, body) in [
            (ssy | 15, bra),           // NAK low-condition form is retained but not executed.
            (ssy, bra | 1 << 6),       // .LMT
            (ssy, bra | 1 << 7),       // .U
            (ssy, bra & !15),          // CC condition, distinct from execution predicate
            (ssy, 0xe30000000007000f), // EXIT while a sync frame is active
        ] {
            let bytes = code(&[setup, body, 0xf0f800000007000f, 0xe30000000007000f]);
            let c = MaxwellCode::parse(&bytes).unwrap();
            assert!(ReconvergenceRegion::build(c, c.instructions().next().unwrap()).is_err());
        }
    }
}
