use super::{emit, source, unsupported};
use crate::{
    ir::{ssa::Builder, BlockId, Operation, Relation, Type},
    maxwell::{
        alu::ValueSource,
        memory::{ConstantLoad, GlobalLoad, GlobalLoadCache, GlobalLoadType, LdcMode, LdcType},
    },
    program::{Diagnostic, Register},
};

pub(super) fn load(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    load: ConstantLoad,
) -> Result<(), Diagnostic> {
    if load.unrecognized_bits != 0 || load.mode != LdcMode::Indexed {
        return Err(unsupported(
            pc,
            "LDC has unknown bits or unresolved segmented/linear addressing",
        ));
    }
    let ty = load.data_type.ok_or_else(|| unsupported(pc, "unknown LDC access size"))?;
    let bytes = ty.byte_len();
    if bytes == 8 && load.destination == 254 {
        return Err(unsupported(pc, "LDC register span crosses RZ"));
    }
    let base = source(b, block, pc, ValueSource::Register(load.address_register))?;
    let displacement = b.word(i32::from(load.displacement) as u32);
    let address = emit(b, block, pc, Type::Bits32, Operation::Add(base, displacement));
    for part in (0..bytes).step_by(4) {
        let value = emit(b, block, pc, Type::Bits32, Operation::LoadConstantWord {
            bank: load.buffer,
            address,
            bytes,
            part,
            signed: matches!(ty, LdcType::I8 | LdcType::I16),
        });
        if load.destination != 255 {
            b.write(block, Register::Gpr(load.destination + part / 4), value);
        }
    }
    Ok(())
}

pub(super) fn global(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    load: GlobalLoad,
) -> Result<(), Diagnostic> {
    if load.unrecognized_bits != 0 {
        return Err(unsupported(pc, "unrecognized global-load bits"));
    }
    let bytes: u8 = match load.data_type {
        GlobalLoadType::U8 | GlobalLoadType::I8 => 1,
        GlobalLoadType::U16 | GlobalLoadType::I16 => 2,
        GlobalLoadType::B32 => 4,
        GlobalLoadType::B64 => 8,
        GlobalLoadType::B128 | GlobalLoadType::B128U => 16,
    };
    let count = bytes.div_ceil(4);
    if count > 1
        && (!load.destination.is_multiple_of(count)
            || u16::from(load.destination) + u16::from(count) > 255)
    {
        return Err(unsupported(pc, "unaligned global-load result group or span crossing RZ"));
    }
    let pair = load.extended_address && load.address_register != 255;
    if pair && (!load.address_register.is_multiple_of(2) || load.address_register >= 254) {
        return Err(unsupported(pc, "unresolved global address register pair"));
    }
    let low = source(b, block, pc, ValueSource::Register(load.address_register))?;
    let high = if pair {
        source(b, block, pc, ValueSource::Register(load.address_register + 1))?
    } else {
        b.word(0)
    };
    // Zero-extend a 32-bit base first, then add the signed displacement to the
    // 64-bit address. RZ's decoded displacement is an unsigned absolute address.
    // Matches the Maxwell address construction in yuzu 6bb3815b's memory lifter.
    let offset = b.word(load.displacement as u32);
    let low_result = emit(b, block, pc, Type::Bits32, Operation::Add(low, offset));
    let carry = emit(b, block, pc, Type::Predicate, Operation::Compare {
        op: Relation::Less,
        a: low_result,
        b: low,
        signed: false,
    });
    let one = b.word(1);
    let zero = b.word(0);
    let carry = emit(b, block, pc, Type::Bits32, Operation::Select {
        condition: carry,
        yes: one,
        no: zero,
    });
    let upper_offset = b.word(if load.displacement < 0 { u32::MAX } else { 0 });
    let high_result = emit(b, block, pc, Type::Bits32, Operation::Add(high, upper_offset));
    let high_result = emit(b, block, pc, Type::Bits32, Operation::Add(high_result, carry));
    let result = emit(
        b,
        block,
        pc,
        Type::Words4,
        Operation::GlobalLoad(crate::ir::memory::Load {
            address: [low_result, high_result],
            bytes,
            signed: matches!(load.data_type, GlobalLoadType::I8 | GlobalLoadType::I16),
            cache: match load.cache {
                GlobalLoadCache::Default => crate::program::memory::Cache::Default,
                GlobalLoadCache::Cg => crate::program::memory::Cache::Cg,
                GlobalLoadCache::Ci => crate::program::memory::Cache::Ci,
                GlobalLoadCache::Cv => crate::program::memory::Cache::Cv,
            },
            native_u128: load.data_type == GlobalLoadType::B128U,
        }),
    );
    if load.destination != 255 {
        for index in 0..count {
            let word =
                emit(b, block, pc, Type::Bits32, Operation::Component { vector: result, index });
            b.write(block, Register::Gpr(load.destination + index), word);
        }
    }
    Ok(())
}
