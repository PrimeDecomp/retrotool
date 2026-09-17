use std::collections::BTreeSet;

use super::{emit, source, unsupported};
use crate::{
    ir::{
        ssa::Builder,
        texture::{FetchLevel, Kind, Operation as TextureOperation, SampleLevel},
        BlockId, Logic, Operation, Type, ValueId,
    },
    maxwell::{alu::ValueSource, texture::TextureLod, Operation as MachineOperation},
    program::{Diagnostic, Register},
};

pub(super) fn lower(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    machine: MachineOperation,
) -> Result<(), Diagnostic> {
    let (dimension, lod, sources, destinations, mask, outputs, inputs, descriptor, nodep) =
        match machine {
            MachineOperation::TextureSampleContiguous(t) => {
                if t.sparse_predicate != 7 || t.non_divergent || t.offset || t.lod_clamp {
                    return Err(unsupported(pc, "TEX residency, derivative, offset or clamp mode"));
                }
                (
                    t.dimension.ok_or_else(|| unsupported(pc, "unsupported TEX dimension"))?,
                    t.lod.ok_or_else(|| unsupported(pc, "unsupported TEX LOD mode"))?,
                    t.sources,
                    [t.destination, 255],
                    Some(t.component_mask),
                    t.component_registers(),
                    t.operand_registers(),
                    t.texture_index,
                    t.nodep,
                )
            }
            MachineOperation::TextureSample(t) => {
                let mode = t.mode.ok_or_else(|| unsupported(pc, "unknown TEXS mode"))?;
                (
                    mode.dimension,
                    mode.lod,
                    t.sources,
                    t.destinations,
                    t.component_mask,
                    t.component_registers(),
                    t.operand_registers(),
                    t.texture_index,
                    t.nodep,
                )
            }
            MachineOperation::TextureLoad(t) => {
                let mode = t.mode.ok_or_else(|| unsupported(pc, "unknown TLDS mode"))?;
                (
                    mode.dimension,
                    mode.lod,
                    t.sources,
                    t.destinations,
                    t.component_mask,
                    t.component_registers(),
                    t.operand_registers(),
                    t.texture_index,
                    t.nodep,
                )
            }
            _ => unreachable!(),
        };
    let inputs = inputs.ok_or_else(|| unsupported(pc, "invalid texture source register span"))?;
    let outputs = outputs.ok_or_else(|| unsupported(pc, "invalid texture result mask"))?;
    let count = mask.expect("validated mask").count_ones();
    let split = !matches!(machine, MachineOperation::TextureSampleContiguous(_));
    if split
        && (sources
            .iter()
            .zip(inputs.source_widths)
            .any(|(base, width)| width == 2 && base % 2 != 0)
            || (count >= 2 && destinations[0] % 2 != 0)
            || (count >= 4 && destinations[1] % 2 != 0))
    {
        return Err(unsupported(pc, "unaligned paired texture register group"));
    }
    let mut seen = BTreeSet::new();
    if outputs.iter().flatten().any(|r| !seen.insert(*r)) {
        return Err(unsupported(pc, "overlapping texture result groups"));
    }
    // All sources are captured before the sample and before any result write.
    let mut read = |r| source(b, block, pc, ValueSource::Register(r));
    let coordinates =
        inputs.coordinates.into_iter().flatten().map(&mut read).collect::<Result<Vec<_>, _>>()?;
    let layer = inputs.layer.map(&mut read).transpose()?;
    let level = inputs.lod.map(&mut read).transpose()?;
    let comparison = inputs.comparison.map(&mut read).transpose()?;
    let offset = inputs.texel_offset.map(&mut read).transpose()?;
    let sample = inputs.sample_index.map(&mut read).transpose()?;
    let layer = layer.map(|value| {
        let mask = b.word(0xffff);
        emit(b, block, pc, Type::Bits32, Operation::Logic { op: Logic::And, a: value, b: mask })
    });
    let kind = match machine {
        MachineOperation::TextureSample(_) | MachineOperation::TextureSampleContiguous(_) => {
            Kind::Sample {
                level: match lod {
                    TextureLod::Implicit => SampleLevel::Implicit,
                    TextureLod::Zero => SampleLevel::Explicit(b.word(0)),
                    TextureLod::Explicit => {
                        SampleLevel::Explicit(level.expect("decoded explicit LOD"))
                    }
                },
                comparison,
            }
        }
        MachineOperation::TextureLoad(_) => Kind::Load {
            level: if let Some(sample) = sample {
                FetchLevel::Sample(sample)
            } else {
                FetchLevel::Mip(level.unwrap_or_else(|| b.word(0)))
            },
            offset: offset.map(|value| {
                [offset_component(b, block, pc, value, 0), offset_component(b, block, pc, value, 4)]
            }),
        },
        _ => unreachable!(),
    };
    let op =
        TextureOperation { descriptor, dimension, coordinates, layer, kind, skip_helpers: nodep };
    if !op.valid(b.function.stage) {
        return Err(unsupported(pc, "texture operation requires a compatible stage/interface"));
    }
    let vector = emit(b, block, pc, Type::Words4, Operation::Texture(op));
    for (index, register) in outputs.into_iter().enumerate() {
        if let Some(register) = register {
            let value = emit(b, block, pc, Type::Bits32, Operation::Component {
                vector,
                index: index as u8,
            });
            b.write(block, Register::Gpr(register), value);
        }
    }
    Ok(())
}

fn offset_component(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    value: ValueId,
    start: u32,
) -> ValueId {
    let left = b.word(28 - start);
    let value = emit(b, block, pc, Type::Bits32, Operation::Shift {
        a: value,
        b: left,
        right: false,
        signed: false,
        wrap: false,
    });
    let right = b.word(28);
    emit(b, block, pc, Type::Bits32, Operation::Shift {
        a: value,
        b: right,
        right: true,
        signed: true,
        wrap: false,
    })
}
