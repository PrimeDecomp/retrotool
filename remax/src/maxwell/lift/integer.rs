//! Integer operations and the carry data flow used for wide addresses.
//! Carry is SSA state, so predicated producers merge with the incoming value.
//! Arithmetic flags other than carry still have no modeled consumers.

use super::*;
use crate::maxwell::integer::IntegerInstruction;

pub(super) fn lower(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    integer: IntegerInstruction,
) -> Result<(), Diagnostic> {
    if integer.unrecognized_bits != 0 {
        return Err(unsupported(pc, "unrecognized integer bits"));
    }
    if matches!(
        integer.operation,
        IntegerOperation::Add { .. } | IntegerOperation::ScaledAdd { .. }
    ) {
        return add(b, block, pc, integer);
    }
    if integer.extended || integer.write_condition_code {
        return Err(unsupported(
            pc,
            "integer operation requires unmodeled condition-code behavior",
        ));
    }
    let mut a = if matches!(integer.operation, IntegerOperation::Logic {
        operation: LogicOperation::PassB,
        ..
    }) {
        b.word(0)
    } else {
        source(b, block, pc, integer.sources[0])?
    };
    let mut c = source(b, block, pc, integer.sources[1])?;
    let mut predicate_write = None;
    let operation = match integer.operation {
        IntegerOperation::Logic { operation, invert, predicate_mode, predicate_destination } => {
            if predicate_mode == 1 || (predicate_mode == 0 && predicate_destination != 7) {
                return Err(unsupported(pc, "unsupported LOP predicate mode"));
            }
            if invert[0] {
                a = emit(b, block, pc, Type::Bits32, Operation::NotBits(a));
            }
            if invert[1] {
                c = emit(b, block, pc, Type::Bits32, Operation::NotBits(c));
            }
            if predicate_mode >= 2 && predicate_destination != 7 {
                predicate_write = Some((predicate_destination, predicate_mode == 3));
            }
            match operation {
                LogicOperation::PassB => Operation::Copy(c),
                LogicOperation::And => Operation::Logic { op: Logic::And, a, b: c },
                LogicOperation::Or => Operation::Logic { op: Logic::Or, a, b: c },
                LogicOperation::Xor => Operation::Logic { op: Logic::Xor, a, b: c },
            }
        }
        IntegerOperation::Shift { right, signed, wrap } => {
            Operation::Shift { a, b: c, right, signed, wrap }
        }
        IntegerOperation::BitFieldExtract { signed, reverse } => {
            if reverse {
                a = emit(b, block, pc, Type::Bits32, Operation::ReverseBits(a));
            }
            let byte_mask = b.word(255);
            let offset = emit(b, block, pc, Type::Bits32, Operation::Logic {
                op: Logic::And,
                a: c,
                b: byte_mask,
            });
            let eight = b.word(8);
            let high = emit(b, block, pc, Type::Bits32, Operation::Shift {
                a: c,
                b: eight,
                right: true,
                signed: false,
                wrap: false,
            });
            let count = emit(b, block, pc, Type::Bits32, Operation::Logic {
                op: Logic::And,
                a: high,
                b: byte_mask,
            });
            Operation::ExtractBits { value: a, offset, count, signed }
        }
        IntegerOperation::FindLeading { signed, shift_amount, invert } => {
            if invert {
                c = emit(b, block, pc, Type::Bits32, Operation::NotBits(c));
            }
            if signed {
                let zero = b.word(0);
                let negative = emit(b, block, pc, Type::Predicate, Operation::Compare {
                    op: Relation::Less,
                    a: c,
                    b: zero,
                    signed: true,
                });
                let inverted = emit(b, block, pc, Type::Bits32, Operation::NotBits(c));
                c = emit(b, block, pc, Type::Bits32, Operation::Select {
                    condition: negative,
                    yes: inverted,
                    no: c,
                });
            }
            let leading = emit(b, block, pc, Type::Bits32, Operation::CountLeadingZeros(c));
            if shift_amount {
                Operation::Copy(leading)
            } else {
                let highest = b.word(31);
                let negative = emit(b, block, pc, Type::Bits32, Operation::Negate(leading));
                Operation::Add(highest, negative)
            }
        }
        _ => return Err(unsupported(pc, "integer mode has no semantic lowering")),
    };
    let result = emit(b, block, pc, Type::Bits32, operation);
    b.write(block, Register::Gpr(integer.destination), result);
    if let Some((destination, nonzero)) = predicate_write {
        let zero = b.word(0);
        let value = emit(b, block, pc, Type::Predicate, Operation::Compare {
            op: if nonzero { Relation::NotEqual } else { Relation::Equal },
            a: result,
            b: zero,
            signed: false,
        });
        b.write(block, Register::Predicate(destination), value);
    }
    Ok(())
}

fn add(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    instruction: IntegerInstruction,
) -> Result<(), Diagnostic> {
    let (shift, negate) = match instruction.operation {
        IntegerOperation::Add { negate, saturate: false } => (None, negate),
        IntegerOperation::ScaledAdd { shift, negate } => (Some(shift), negate),
        _ => return Err(unsupported(pc, "saturated integer addition is not modeled")),
    };
    // IADD.X changes negate modifiers to bitwise complements. Carry on
    // subtraction, and the combined PO modifier, need their own lowering.
    if negate == [true, true]
        || (negate != [false, false]
            && (instruction.extended || instruction.write_condition_code || shift.is_some()))
    {
        return Err(unsupported(pc, "modified integer addition needs separate carry/PO semantics"));
    }
    let mut a = source(b, block, pc, instruction.sources[0])?;
    let mut c = source(b, block, pc, instruction.sources[1])?;
    if negate[0] {
        a = emit(b, block, pc, Type::Bits32, Operation::Negate(a));
    }
    if negate[1] {
        c = emit(b, block, pc, Type::Bits32, Operation::Negate(c));
    }
    if let Some(shift) = shift {
        let amount = b.word(u32::from(shift));
        a = emit(b, block, pc, Type::Bits32, Operation::Shift {
            a,
            b: amount,
            right: false,
            signed: false,
            wrap: false,
        });
    }
    let sum = emit(b, block, pc, Type::Bits32, Operation::Add(a, c));
    let result = if instruction.extended {
        let carry = b.read(block, Register::Carry);
        let one = b.word(1);
        let zero = b.word(0);
        let carry = emit(b, block, pc, Type::Bits32, Operation::Select {
            condition: carry,
            yes: one,
            no: zero,
        });
        emit(b, block, pc, Type::Bits32, Operation::Add(sum, carry))
    } else {
        sum
    };
    if instruction.write_condition_code {
        let mut carry = emit(b, block, pc, Type::Predicate, Operation::Compare {
            op: Relation::Less,
            a: sum,
            b: a,
            signed: false,
        });
        if instruction.extended {
            let extra = emit(b, block, pc, Type::Predicate, Operation::Compare {
                op: Relation::Less,
                a: result,
                b: sum,
                signed: false,
            });
            carry = emit(b, block, pc, Type::Predicate, Operation::PredicateLogic {
                op: Logic::Or,
                a: carry,
                b: extra,
            });
        }
        b.write(block, Register::Carry, carry);
    }
    b.write(block, Register::Gpr(instruction.destination), result);
    Ok(())
}
