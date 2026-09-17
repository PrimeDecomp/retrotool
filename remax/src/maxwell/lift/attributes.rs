use super::{emit, source, unsupported};
use crate::{
    ir::{
        attribute::Input,
        ssa::{Builder, ValueRemap},
        transport::Transport,
        BlockId, Definition, Function, InterpolationLocation, Operation, Type, ValueId,
    },
    maxwell::{
        alu::ValueSource,
        attribute::{
            AttributeAccess, AttributeAddress, AttributeInstruction, Interpolation,
            InterpolationLocation as MachineLocation,
        },
    },
    program::{Diagnostic, InterpolationMode, Register, Stage},
};

#[derive(Default)]
pub(super) struct PendingPairs(Vec<Pair>);

struct Pair {
    pc: usize,
    address: ValueId,
    predicate: ValueId,
    retained_base: u16,
}

impl PendingPairs {
    pub fn retained_values(&self, remap: &ValueRemap) -> Vec<ValueId> {
        self.0
            .iter()
            .flat_map(|pair| [remap.get(pair.address), remap.get(pair.predicate)])
            .collect()
    }

    /// Frontend-only obligations survive SSA sealing through its value remap;
    /// no native predicate operand or retained encoding enters the logical IR.
    pub fn verify(self, function: &Function, remap: &ValueRemap) -> Result<(), Diagnostic> {
        let transport = Transport::new(function);
        for pair in self.0 {
            let address = remap.get(pair.address);
            let predicate = remap.get(pair.predicate);
            if function.values[address.0].ty != Type::AttributeAddress
                || !transport.equivalent(function, address, predicate)
            {
                return Err(unsupported(
                    pair.pc,
                    "indexed IPA requires matching AL2P address/predicate provenance",
                ));
            }
            let origins = transport.origins(address);
            if origins.is_empty()
                || origins.iter().any(|origin| {
                    let Definition::Instruction { block, index } =
                        function.values[origin.0].definition
                    else {
                        return true;
                    };
                    !matches!(function.blocks[block.0].instructions[index].operation,
                    Operation::AttributeAddress { base, .. } if base & !15 == pair.retained_base)
                })
            {
                return Err(unsupported(
                    pair.pc,
                    "indexed IPA retained base disagrees with its AL2P producers",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn lower(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    attribute: AttributeInstruction,
    pairs: &mut PendingPairs,
) -> Result<(), Diagnostic> {
    match attribute {
        AttributeInstruction::Load(access) => access_attribute(b, block, pc, access, true),
        AttributeInstruction::Store(access) => access_attribute(b, block, pc, access, false),
        AttributeInstruction::Interpolate(ipa) => interpolate(b, block, pc, ipa, pairs),
        AttributeInstruction::Address(address) => attribute_address(b, block, pc, address),
    }
}

fn attribute_address(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    address: AttributeAddress,
) -> Result<(), Diagnostic> {
    if b.function.stage == Stage::Scalar
        || address.unrecognized_bits != 0
        || address.output
        || address.components != 1
        || !(0x80..0x280).contains(&address.byte_offset)
        || address.byte_offset % 4 != 0
    {
        return Err(unsupported(pc, "AL2P requires a single generic input component"));
    }
    let offset = b.read(block, Register::Gpr(address.address_register));
    let value = emit(b, block, pc, Type::AttributeAddress, Operation::AttributeAddress {
        base: address.byte_offset as u16,
        offset,
    });
    b.write(block, Register::Gpr(address.destination), value);
    // The same symbolic provenance represents the two native outputs. It is
    // neither an integer address nor a boolean predicate usable by other code.
    b.write(block, Register::Predicate(address.predicate_destination), value);
    Ok(())
}

fn access_attribute(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    access: AttributeAccess,
    load: bool,
) -> Result<(), Diagnostic> {
    if b.function.stage != Stage::Vertex {
        return Err(unsupported(pc, "ALD/AST requires the current-vertex interface"));
    }
    if access.unrecognized_bits != 0 || access.patch || access.vertex_register != 255 {
        return Err(unsupported(pc, "patch, cross-vertex or unrecognized attribute access"));
    }
    if access.address_register != 255 {
        if !load || access.output_flag || access.byte_offset != 0 || access.components != 1 {
            return Err(unsupported(pc, "indirect ALD requires a single generic input component"));
        }
        let address = b.read(block, Register::Gpr(access.address_register));
        let value =
            emit(b, block, pc, Type::Bits32, Operation::ReadAttribute(Input::Indexed(address)));
        b.write(block, Register::Gpr(access.data_register), value);
        return Ok(());
    }
    if !access.byte_offset.is_multiple_of(4)
        || u32::from(access.byte_offset) + u32::from(access.components) * 4 > 0x400
        || u16::from(access.data_register) + u16::from(access.components) > 256
    {
        return Err(unsupported(pc, "misaligned or overflowing attribute/register group"));
    }
    for component in 0..access.components {
        let offset = access.byte_offset + u16::from(component) * 4;
        let register = access.data_register + component;
        if load {
            let value = if access.output_flag {
                b.read(block, Register::OutputAttribute(offset))
            } else {
                emit(b, block, pc, Type::Bits32, Operation::ReadAttribute(Input::Direct(offset)))
            };
            let value = emit(b, block, pc, Type::Bits32, Operation::Copy(value));
            b.write(block, Register::Gpr(register), value);
        } else {
            let value = b.read(block, Register::Gpr(register));
            let value = emit(b, block, pc, Type::Bits32, Operation::Copy(value));
            // Output attributes are per-invocation state, including ALD.O reads.
            // Only the interface's returned values become externally observable.
            b.write(block, Register::OutputAttribute(offset), value);
        }
    }
    Ok(())
}

fn interpolate(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    ipa: Interpolation,
    pairs: &mut PendingPairs,
) -> Result<(), Diagnostic> {
    if b.function.stage != Stage::Fragment {
        return Err(unsupported(pc, "IPA requires a fragment interpolation provider"));
    }
    if ipa.unrecognized_bits != 0 || ipa.predicate.inverted {
        return Err(unsupported(pc, "unrecognized or inverted IPA operand predicate"));
    }
    let attribute = if ipa.indexed {
        if ipa.mode != InterpolationMode::Perspective {
            return Err(unsupported(
                pc,
                "indexed IPA currently requires established perspective interpolation",
            ));
        }
        if ipa.address_register == 255 || ipa.predicate.register == 7 {
            return Err(unsupported(pc, "indexed IPA requires an AL2P address/predicate pair"));
        }
        let address = b.read(block, Register::Gpr(ipa.address_register));
        let predicate = b.read(block, Register::Predicate(ipa.predicate.register));
        pairs.0.push(Pair { pc, address, predicate, retained_base: ipa.byte_offset });
        Input::Indexed(address)
    } else {
        if ipa.address_register != 255 || ipa.predicate.register != 7 {
            return Err(unsupported(pc, "direct IPA requires RZ and PT operands"));
        }
        Input::Direct(ipa.byte_offset)
    };
    if !ipa.byte_offset.is_multiple_of(4) || ipa.mode == InterpolationMode::State {
        return Err(unsupported(pc, "misaligned or state-selected interpolation"));
    }
    let location = match ipa.location {
        Some(MachineLocation::Default) => InterpolationLocation::Default,
        Some(MachineLocation::Centroid) => InterpolationLocation::Centroid,
        Some(MachineLocation::Offset) if ipa.mode != InterpolationMode::Constant => {
            InterpolationLocation::Offset(source(
                b,
                block,
                pc,
                ValueSource::Register(ipa.offset_register),
            )?)
        }
        _ => {
            return Err(unsupported(
                pc,
                "unknown location or unverified constant-mode offset interpolation",
            ))
        }
    };
    let multiplier = if ipa.mode == InterpolationMode::Perspective {
        Some(source(b, block, pc, ValueSource::Register(ipa.multiplier_register))?)
    } else {
        None
    };
    let value = emit(b, block, pc, Type::Bits32, Operation::Interpolate {
        attribute,
        mode: ipa.mode,
        location,
        multiplier,
        saturate: ipa.saturate,
    });
    b.write(block, Register::Gpr(ipa.destination), value);
    Ok(())
}
