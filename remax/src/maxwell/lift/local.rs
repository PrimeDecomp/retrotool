use super::{emit, unsupported};
use crate::{
    ir::{
        memory::local::{Access, Load, Store},
        ssa::Builder,
        BlockId, Operation, Type,
    },
    maxwell::memory::{LocalMemoryInstruction, LocalMemoryKind, LocalMemoryType},
    program::{Diagnostic, Register},
};

pub(super) fn lower(
    builder: &mut Builder,
    block: BlockId,
    pc: usize,
    instruction: LocalMemoryInstruction,
) -> Result<(), Diagnostic> {
    let data_type = instruction
        .data_type
        .filter(|_| instruction.unrecognized_bits == 0)
        .ok_or_else(|| unsupported(pc, "unknown local-memory width or modifier"))?;
    let bytes = data_type.byte_len();
    let words = bytes.div_ceil(4);
    if !instruction.register.is_multiple_of(words)
        || u16::from(instruction.register) + u16::from(words) > 256
    {
        return Err(unsupported(pc, "unaligned or overflowing local-memory register group"));
    }
    let base = builder.read(block, Register::Gpr(instruction.address_register));
    let displacement = builder.word(instruction.displacement as u32);
    let address = emit(builder, block, pc, Type::Bits32, Operation::Add(base, displacement));
    let access = Access { address, bytes };
    match instruction.kind {
        LocalMemoryKind::Load(_) => {
            // Capture every result before overwriting any address/source GPR.
            let results = (0..words)
                .map(|part| {
                    emit(
                        builder,
                        block,
                        pc,
                        Type::Bits32,
                        Operation::LocalLoad(Load {
                            access,
                            part: part * 4,
                            signed: matches!(data_type, LocalMemoryType::I8 | LocalMemoryType::I16),
                        }),
                    )
                })
                .collect::<Vec<_>>();
            if instruction.register != 255 {
                for (part, value) in results.into_iter().enumerate() {
                    builder.write(block, Register::Gpr(instruction.register + part as u8), value);
                }
            }
        }
        LocalMemoryKind::Store(_) => {
            let values = (0..words)
                .map(|part| builder.read(block, Register::Gpr(instruction.register + part)))
                .collect();
            builder.function.append_effect(
                block,
                pc,
                Operation::LocalStore(Store { access, values }),
            );
        }
    }
    Ok(())
}
