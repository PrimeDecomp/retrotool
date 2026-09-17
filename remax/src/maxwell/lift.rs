//! Maxwell instruction semantics into SSA, independently of WGSL emission.
//! Native control discovery precedes SSA construction and retains synchronization
//! contexts. Scalar value flow does not establish GPU lane participation.

use std::collections::BTreeSet;

use super::{
    alu::{PredicateOperand, ValueSource},
    compare::PredicateLogic,
    integer::{IntegerOperation, LogicOperation},
    integer_compare::{IntegerComparison, IntegerComparisonInstruction, IntegerComparisonOperands},
    Instruction, MaxwellCode, Operation as MachineOperation,
};
use crate::{
    ir::{
        ssa::Builder, BlockId, Edge, Function, Logic, Operation, Relation, Terminator, Type,
        ValueId,
    },
    program::{ConstantAddress, Diagnostic, DiagnosticKind, Phase, Register, Signature, Stage},
};

mod attributes;
mod control;
mod float;
mod integer;
mod local;
mod memory;
mod opaque;
mod raster;
mod texture;
mod xmad;

fn error(pc: Option<usize>, kind: DiagnosticKind, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(Phase::Lift, pc, kind, message)
}

fn unsupported(pc: usize, message: impl Into<String>) -> Diagnostic {
    error(Some(pc), DiagnosticKind::Unsupported, message)
}

pub(crate) fn lift(code: MaxwellCode<'_>, signature: &Signature) -> Result<Function, Diagnostic> {
    let code = MaxwellCode::parse(code.bytes)
        .map_err(|e| error(None, DiagnosticKind::InvalidInput, e.to_string()))?;
    let mut seen = BTreeSet::new();
    for &register in &signature.inputs {
        if matches!(register, Register::Gpr(255) | Register::Predicate(7..))
            || !seen.insert(register)
        {
            return Err(error(
                None,
                DiagnosticKind::InvalidInput,
                format!("invalid or duplicate input {register:?}"),
            ));
        }
    }
    if signature.outputs.iter().any(|r| matches!(r, Register::Predicate(8..))) {
        return Err(error(None, DiagnosticKind::InvalidInput, "invalid output predicate"));
    }
    for &register in signature.inputs.iter().chain(&signature.outputs) {
        if let Register::OutputAttribute(offset) = register {
            if signature.stage != Stage::Vertex || offset >= 0x400 || !offset.is_multiple_of(4) {
                return Err(error(
                    None,
                    DiagnosticKind::InvalidInput,
                    "invalid output attribute interface",
                ));
            }
        }
    }
    let graph = control::Graph::build(code)?;
    let mut builder = Builder::new(&signature.inputs, &signature.outputs);
    builder.function.stage = signature.stage;
    builder.function.local_memory_bytes = signature.local_memory_bytes;
    let mut indexed_attributes = BTreeSet::new();
    for &attribute in &signature.indexed_attributes {
        if signature.stage == Stage::Scalar
            || !(0x80..0x280).contains(&attribute)
            || !attribute.is_multiple_of(4)
            || !indexed_attributes.insert(attribute)
        {
            return Err(error(
                None,
                DiagnosticKind::InvalidInput,
                "invalid or duplicate indexed attribute declaration",
            ));
        }
    }
    builder.function.indexed_attributes = indexed_attributes.into_iter().collect();
    let mut pairs = attributes::PendingPairs::default();
    let entry = builder.block(None);
    let blocks = graph
        .nodes
        .iter()
        .map(|node| {
            let block = builder.block(Some(node.instruction.offset));
            builder.function.blocks[block.0].convergence = node.convergence.clone();
            block
        })
        .collect::<Vec<_>>();
    let edge = |link: control::Link| Edge {
        target: blocks[link.target.0],
        arguments: vec![],
        convergence: link.convergence,
    };
    builder.function.blocks[entry.0].terminator =
        Some(Terminator::Jump(Edge::new(blocks[0], vec![])));
    for (node, &block) in graph.nodes.iter().zip(&blocks) {
        let term = match node.action {
            control::Action::Execute { next } => {
                lower_instruction(&mut builder, block, node.instruction, &mut pairs)?;
                Terminator::Jump(edge(next))
            }
            control::Action::Jump(next) => Terminator::Jump(edge(next)),
            control::Action::Branch { predicate: condition, taken, otherwise, kind } => {
                let condition = predicate(&mut builder, block, node.instruction.offset, condition);
                Terminator::Branch { condition, yes: edge(taken), no: edge(otherwise), kind }
            }
            control::Action::Return => Terminator::Return(
                signature.outputs.iter().map(|&r| builder.read(block, r)).collect(),
            ),
        };
        builder.function.blocks[block.0].terminator = Some(term);
    }
    let (mut function, remap) = builder.finish();
    opaque::resolve(&mut function, &pairs.retained_values(&remap))?;
    pairs.verify(&function, &remap)?;
    Ok(function)
}

fn emit(builder: &mut Builder, block: BlockId, pc: usize, ty: Type, op: Operation) -> ValueId {
    builder.function.append(block, pc, ty, op)
}

fn predicate(builder: &mut Builder, block: BlockId, pc: usize, p: PredicateOperand) -> ValueId {
    let value = builder.read(block, Register::Predicate(p.register));
    if p.inverted {
        emit(builder, block, pc, Type::Predicate, Operation::PredicateNot(value))
    } else {
        value
    }
}

fn source(
    builder: &mut Builder,
    block: BlockId,
    pc: usize,
    source: ValueSource,
) -> Result<ValueId, Diagnostic> {
    Ok(match source {
        ValueSource::Register(r) => builder.read(block, Register::Gpr(r)),
        ValueSource::Immediate(bits) => builder.word(bits),
        ValueSource::Constant { buffer, byte_offset } => {
            if byte_offset < 0 {
                return Err(unsupported(pc, "negative constant word address"));
            }
            emit(
                builder,
                block,
                pc,
                Type::Bits32,
                Operation::ReadConstant(ConstantAddress {
                    bank: buffer,
                    byte_offset: byte_offset as u16,
                }),
            )
        }
    })
}

fn lower_instruction(
    b: &mut Builder,
    block: BlockId,
    i: Instruction,
    pairs: &mut attributes::PendingPairs,
) -> Result<(), Diagnostic> {
    let pc = i.offset;
    match i.operation() {
        MachineOperation::Nop => {}
        MachineOperation::DependencyBarrier(wait) => {
            if wait.unrecognized_bits != 0 || (wait.less_equal && wait.scoreboard >= 6) {
                return Err(unsupported(pc, "unrecognized dependency-wait bits or scoreboard"));
            }
            // DEPBAR waits on register-dependency scoreboards. All currently
            // lifted accesses produce their results synchronously, so the wait
            // is already satisfied. This is neither a memory fence nor a
            // workgroup/reconvergence barrier. Retain the execution point/PC.
        }
        MachineOperation::Attribute(attribute) => {
            attributes::lower(b, block, pc, attribute, pairs)?
        }
        MachineOperation::PixelLoad(load) => raster::load(b, block, pc, load)?,
        MachineOperation::ConstantLoad(load) => memory::load(b, block, pc, load)?,
        MachineOperation::GlobalLoad(load) => memory::global(b, block, pc, load)?,
        MachineOperation::LocalMemory(instruction) => local::lower(b, block, pc, instruction)?,
        MachineOperation::FragmentKill(kill) => {
            if b.function.stage != Stage::Fragment {
                return Err(unsupported(pc, "KIL requires a fragment stage"));
            }
            if kill.condition != 15 || kill.unrecognized_bits != 0 {
                return Err(unsupported(pc, "unestablished KIL condition or modifier bits"));
            }
            b.function.append_effect(block, pc, Operation::Demote);
        }
        MachineOperation::Xmad(op) => xmad::lower(b, block, pc, op)?,
        op @ (MachineOperation::TextureSample(_)
        | MachineOperation::TextureSampleContiguous(_)
        | MachineOperation::TextureLoad(_)) => {
            texture::lower(b, block, pc, op)?;
        }
        MachineOperation::Alu(alu) => float::alu(b, block, pc, alu)?,
        MachineOperation::Conversion(convert) => float::convert(b, block, pc, convert)?,
        MachineOperation::FloatComparison(compare) => float::compare(b, block, pc, compare)?,
        MachineOperation::Integer(args) => integer::lower(b, block, pc, args)?,
        MachineOperation::IntegerComparison(compare) => lower_comparison(b, block, pc, compare)?,
        _ => return Err(unsupported(pc, "operation has no semantic lowering")),
    }
    Ok(())
}

fn comparison(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    args: IntegerComparisonOperands,
) -> Result<(ValueId, ValueId, Logic), Diagnostic> {
    if args.unrecognized_bits != 0 || args.extended {
        return Err(unsupported(pc, "unrecognized comparison bits or extended carry"));
    }
    let a = source(b, block, pc, args.sources[0])?;
    let c = source(b, block, pc, args.sources[1])?;
    let relation = match args.comparison {
        IntegerComparison::False => b.predicate(false),
        IntegerComparison::True => b.predicate(true),
        comparison => emit(b, block, pc, Type::Predicate, Operation::Compare {
            op: match comparison {
                IntegerComparison::Less => Relation::Less,
                IntegerComparison::Equal => Relation::Equal,
                IntegerComparison::LessEqual => Relation::LessEqual,
                IntegerComparison::Greater => Relation::Greater,
                IntegerComparison::NotEqual => Relation::NotEqual,
                IntegerComparison::GreaterEqual => Relation::GreaterEqual,
                _ => unreachable!(),
            },
            a,
            b: c,
            signed: args.signed,
        }),
    };
    let accumulator = predicate(b, block, pc, args.accumulator);
    let logic = predicate_logic(pc, args.logic)?;
    Ok((relation, accumulator, logic))
}

fn predicate_logic(pc: usize, logic: Option<PredicateLogic>) -> Result<Logic, Diagnostic> {
    Ok(match logic {
        Some(PredicateLogic::And) => Logic::And,
        Some(PredicateLogic::Or) => Logic::Or,
        Some(PredicateLogic::Xor) => Logic::Xor,
        None => return Err(unsupported(pc, "unknown comparison predicate logic")),
    })
}

enum ComparisonDestination {
    Register { destination: u8, boolean_float: bool },
    Predicates { destination: u8, secondary: u8 },
}

fn lower_comparison(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    compare: IntegerComparisonInstruction,
) -> Result<(), Diagnostic> {
    let (destination, operands) = match compare {
        IntegerComparisonInstruction::Set {
            destination,
            boolean_float,
            write_condition_code,
            operands,
        } => {
            if write_condition_code {
                return Err(unsupported(pc, "comparison condition-code write"));
            }
            (ComparisonDestination::Register { destination, boolean_float }, operands)
        }
        IntegerComparisonInstruction::SetPredicates {
            destination,
            secondary_destination,
            operands,
        } => (
            ComparisonDestination::Predicates { destination, secondary: secondary_destination },
            operands,
        ),
    };
    let values = comparison(b, block, pc, operands)?;
    comparison_outputs(b, block, pc, destination, values)
}

fn comparison_outputs(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    destination: ComparisonDestination,
    (relation, accumulator, op): (ValueId, ValueId, Logic),
) -> Result<(), Diagnostic> {
    match destination {
        ComparisonDestination::Register { destination, boolean_float } => {
            let condition = emit(b, block, pc, Type::Predicate, Operation::PredicateLogic {
                op,
                a: relation,
                b: accumulator,
            });
            let yes = b.word(if boolean_float { 1.0f32.to_bits() } else { u32::MAX });
            let no = b.word(0);
            let result = emit(b, block, pc, Type::Bits32, Operation::Select { condition, yes, no });
            b.write(block, Register::Gpr(destination), result);
        }
        ComparisonDestination::Predicates { destination, secondary } => {
            if destination != 7 && destination == secondary {
                return Err(unsupported(pc, "aliased predicate destinations"));
            }
            // Both results consume the old accumulator, including when it is
            // also a destination or the instruction's execution predicate.
            let first = emit(b, block, pc, Type::Predicate, Operation::PredicateLogic {
                op,
                a: relation,
                b: accumulator,
            });
            let inverse = emit(b, block, pc, Type::Predicate, Operation::PredicateNot(relation));
            let second = emit(b, block, pc, Type::Predicate, Operation::PredicateLogic {
                op,
                a: inverse,
                b: accumulator,
            });
            b.write(block, Register::Predicate(destination), first);
            b.write(block, Register::Predicate(secondary), second);
        }
    }
    Ok(())
}
