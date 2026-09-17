use super::{emit, source, unsupported};
use crate::{
    ir::{ssa::Builder, BlockId, Logic, Operation, Type, ValueId},
    maxwell::xmad::{XmadAccumulator, XmadInstruction},
    program::{Diagnostic, Register},
};

pub(super) fn lower(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    op: XmadInstruction,
) -> Result<(), Diagnostic> {
    if op.unrecognized_bits != 0 || op.extended || op.write_condition_code {
        return Err(unsupported(pc, "XMAD has unknown bits or carry/condition-code state"));
    }
    let mode = match op.accumulator {
        Some(XmadAccumulator::SignCorrection) | None => {
            return Err(unsupported(pc, "XMAD accumulator mode has no established lowering"));
        }
        Some(mode) => mode,
    };
    // Capture all operands before writing: packing commonly overwrites C.
    let raw = [
        source(b, block, pc, op.sources[0])?,
        source(b, block, pc, op.sources[1])?,
        source(b, block, pc, op.sources[2])?,
    ];
    let mask = b.word(0xffff);
    let mut half = [raw[0], raw[1]];
    for (i, part) in half.iter_mut().enumerate() {
        *part = if op.high_half[i] {
            shift(b, block, pc, *part, true, false)
        } else {
            emit(b, block, pc, Type::Bits32, Operation::Logic { op: Logic::And, a: *part, b: mask })
        };
        if op.signed[i] {
            *part = shift(b, block, pc, *part, false, false);
            *part = shift(b, block, pc, *part, true, true);
        }
    }
    let mut product = emit(b, block, pc, Type::Bits32, Operation::Multiply(half[0], half[1]));
    if op.product_shift_left {
        product = shift(b, block, pc, product, false, false);
    }
    let accumulator = match mode {
        XmadAccumulator::Low => emit(b, block, pc, Type::Bits32, Operation::Logic {
            op: Logic::And,
            a: raw[2],
            b: mask,
        }),
        XmadAccumulator::High => shift(b, block, pc, raw[2], true, false),
        _ => raw[2],
    };
    let mut result = emit(b, block, pc, Type::Bits32, Operation::Add(product, accumulator));
    if mode == XmadAccumulator::MergeCarry {
        let carry = shift(b, block, pc, raw[1], false, false);
        result = emit(b, block, pc, Type::Bits32, Operation::Add(result, carry));
    }
    if op.merge {
        let low = emit(b, block, pc, Type::Bits32, Operation::Logic {
            op: Logic::And,
            a: result,
            b: mask,
        });
        let high = shift(b, block, pc, raw[1], false, false);
        result =
            emit(b, block, pc, Type::Bits32, Operation::Logic { op: Logic::Or, a: low, b: high });
    }
    b.write(block, Register::Gpr(op.destination), result);
    Ok(())
}

fn shift(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    value: ValueId,
    right: bool,
    signed: bool,
) -> ValueId {
    let amount = b.word(16);
    emit(b, block, pc, Type::Bits32, Operation::Shift {
        a: value,
        b: amount,
        right,
        signed,
        wrap: false,
    })
}
