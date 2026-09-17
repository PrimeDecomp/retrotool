//! Resolve opaque attribute and range-reduction transport after SSA sealing.
//! Registers do not carry types, and loop/join parameters are unknown while lifting
//! instructions. Only closed networks of copies, selects and block arguments
//! may transport an opaque value. Consumer path facts first bypass transport
//! with a unique dominating origin; demand then removes unobserved alternatives.
//! Observable mixed representations still fail.

use super::unsupported;
use crate::{
    ir::{
        attribute::Input,
        float,
        transport::{specialize_uses, Demand, Transport},
        Definition, Function, InterpolationLocation, Operation, Terminator, Type, ValueId,
    },
    program::Diagnostic,
};

pub(super) fn resolve(function: &mut Function, retained: &[ValueId]) -> Result<(), Diagnostic> {
    specialize_uses(function);
    if let Some(demand) = Demand::analyze(function, retained) {
        Transport::filtered(function, |result, source| demand.retains(result, source))
            .refine(function)
            .map_err(|value| {
                unsupported(value_pc(function, value), "transport mixes incompatible opaque values")
            })?;
        demand.erase_unobserved(function);
    }
    Transport::new(function).refine(function).map_err(|value| {
        unsupported(value_pc(function, value), "transport mixes incompatible opaque values")
    })?;
    validate_uses(function)
}

fn value_pc(function: &Function, value: ValueId) -> usize {
    match function.values[value.0].definition {
        Definition::Instruction { block, index } => function.blocks[block.0].instructions[index].pc,
        Definition::Parameter { block, .. } => function.blocks[block.0].pc.unwrap_or(0),
        _ => 0,
    }
}

fn validate_uses(function: &Function) -> Result<(), Diagnostic> {
    let reduction = |value: ValueId| match function.values[value.0].ty {
        Type::Reduced(kind) => Some(kind),
        _ => None,
    };
    let opaque = |value: ValueId| function.values[value.0].ty.is_opaque();
    let opaque_type = |value: ValueId| opaque(value).then_some(function.values[value.0].ty);
    for block in &function.blocks {
        for instruction in &block.instructions {
            let allowed = match instruction.operation {
                Operation::Copy(_) => true,
                Operation::Select { condition, yes, no } => {
                    !opaque(condition) && opaque_type(yes) == opaque_type(no)
                }
                Operation::Float(float::Operation::Reduced { function, value, .. }) => {
                    if reduction(value) != Some(function.reduction()) {
                        let kind = function.reduction();
                        let message = format!(
                            "{function:?} requires a {kind:?} reduction on every incoming path"
                        );
                        return Err(unsupported(instruction.pc, message));
                    }
                    true
                }
                Operation::ReadAttribute(Input::Indexed(address)) => {
                    function.values[address.0].ty == Type::AttributeAddress
                }
                Operation::Interpolate {
                    attribute: Input::Indexed(address),
                    multiplier,
                    location,
                    ..
                } => {
                    let offset = match location {
                        InterpolationLocation::Offset(value) => Some(value),
                        _ => None,
                    };
                    function.values[address.0].ty == Type::AttributeAddress
                        && !multiplier.into_iter().chain(offset).any(opaque)
                }
                _ => !instruction.operation.operands().into_iter().any(opaque),
            };
            if !allowed {
                return Err(unsupported(
                    instruction.pc,
                    "opaque value used as raw bits or mixed with an ordinary value",
                ));
            }
        }
        let terminator = block.terminator.as_ref().unwrap();
        let pc = block.pc.unwrap_or(0);
        match terminator {
            Terminator::Return(values) if values.iter().copied().any(opaque) => {
                return Err(unsupported(pc, "opaque result is observable at shader exit"));
            }
            Terminator::Branch { condition, .. } if opaque(*condition) => {
                return Err(unsupported(pc, "opaque value used as a branch condition"));
            }
            _ => {}
        }
        for edge in terminator.edges() {
            for (&argument, &parameter) in
                edge.arguments.iter().zip(&function.blocks[edge.target.0].parameters)
            {
                if opaque_type(argument) != opaque_type(parameter) {
                    let target_pc = function.blocks[edge.target.0].pc.unwrap_or(pc);
                    return Err(unsupported(
                        target_pc,
                        "control-flow join mixes an opaque value with ordinary or unavailable bits",
                    ));
                }
            }
        }
    }
    Ok(())
}
