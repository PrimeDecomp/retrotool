use super::{
    comparison_outputs, emit, predicate, predicate_logic, source, unsupported,
    ComparisonDestination,
};
use crate::{
    ir::{
        float::{
            Approximate, Binary, Operation as FloatOperation, ReducedFunction, Reduction,
            ResultControl, Rounding,
        },
        ssa::Builder,
        BlockId, Logic, Operation, Relation, Type, ValueId,
    },
    maxwell::{
        alu::{
            AluInstruction, AluOperation, FloatOperand, FloatRounding, PredicateOperand,
            RangeReduction, SpecialFunction,
        },
        compare::{FloatComparison, FloatComparisonInstruction, FloatComparisonOperands},
        convert::{ConversionInstruction, ConversionOperation},
    },
    program::{Diagnostic, Register},
};

fn rounding(value: FloatRounding) -> Rounding {
    match value {
        FloatRounding::NearestEven => Rounding::NearestEven,
        FloatRounding::NegativeInfinity => Rounding::NegativeInfinity,
        FloatRounding::PositiveInfinity => Rounding::PositiveInfinity,
        FloatRounding::Zero => Rounding::Zero,
    }
}

fn operation(b: &mut Builder, block: BlockId, pc: usize, operation: FloatOperation) -> ValueId {
    emit(b, block, pc, operation.result_type(), Operation::Float(operation))
}

fn operand(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    operand: FloatOperand,
    flush_to_zero: bool,
) -> Result<ValueId, Diagnostic> {
    let value = source(b, block, pc, operand.source)?;
    Ok(if operand.absolute || operand.negate || flush_to_zero {
        operation(b, block, pc, FloatOperation::Modify {
            value,
            absolute: operand.absolute,
            negate: operand.negate,
            flush_to_zero,
        })
    } else {
        value
    })
}

pub(super) fn alu(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    alu: AluInstruction,
) -> Result<(), Diagnostic> {
    if alu.unrecognized_bits != 0 || alu.write_condition_code {
        return Err(unsupported(pc, "unrecognized ALU bits or condition-code write"));
    }
    let value = match alu.operation {
        AluOperation::Move { source: input, lane_mask: 15 } => {
            let value = source(b, block, pc, input)?;
            emit(b, block, pc, Type::Bits32, Operation::Copy(value))
        }
        AluOperation::FAdd { sources: [a, c], rounding: mode, flush_to_zero, saturate }
        | AluOperation::FMul {
            sources: [a, c],
            rounding: mode,
            flush_to_zero,
            saturate,
            dnz: false,
            scale_exponent: Some(_),
            ..
        } => {
            let a = operand(b, block, pc, a, flush_to_zero)?;
            let c = operand(b, block, pc, c, flush_to_zero)?;
            let op = match alu.operation {
                AluOperation::FAdd { .. } => Binary::Add,
                AluOperation::FMul { scale_exponent: Some(exponent), .. } => {
                    Binary::Multiply { exponent }
                }
                _ => unreachable!("matched binary arithmetic"),
            };
            operation(b, block, pc, FloatOperation::Binary {
                op,
                a,
                b: c,
                control: ResultControl { rounding: rounding(mode), flush_to_zero, saturate },
            })
        }
        AluOperation::FMinMax { sources, min, flush_to_zero } => {
            let [a, c] = sources;
            let a = operand(b, block, pc, a, flush_to_zero)?;
            let c = operand(b, block, pc, c, flush_to_zero)?;
            min_max(b, block, pc, [a, c], min)
        }
        AluOperation::FFma {
            sources: [a, c, d],
            rounding: mode,
            flush_to_zero,
            saturate,
            dnz: false,
        } => {
            let a = operand(b, block, pc, a, flush_to_zero)?;
            let c = operand(b, block, pc, c, flush_to_zero)?;
            let d = operand(b, block, pc, d, flush_to_zero)?;
            operation(b, block, pc, FloatOperation::Fma {
                a,
                b: c,
                c: d,
                control: ResultControl { rounding: rounding(mode), flush_to_zero, saturate },
            })
        }
        AluOperation::Rro { source, reduction } => {
            let value = operand(b, block, pc, source, false)?;
            let kind = match reduction {
                RangeReduction::Exp2 => Reduction::Exp2,
                RangeReduction::SinCos => Reduction::SinCos,
            };
            emit(b, block, pc, Type::Reduced(kind), Operation::Reduce { kind, value })
        }
        AluOperation::Mufu { source: input, function: Some(function), saturate, .. }
            if matches!(
                function,
                SpecialFunction::Exp2 | SpecialFunction::Sin | SpecialFunction::Cos
            ) =>
        {
            if input.absolute || input.negate {
                return Err(unsupported(pc, "modifiers on an opaque RRO result are unresolved"));
            }
            let value = source(b, block, pc, input.source)?;
            let function = match function {
                SpecialFunction::Exp2 => ReducedFunction::Exp2,
                SpecialFunction::Sin => ReducedFunction::Sin,
                SpecialFunction::Cos => ReducedFunction::Cos,
                _ => unreachable!("matched range-reduced function"),
            };
            operation(b, block, pc, FloatOperation::Reduced { function, value, saturate })
        }
        AluOperation::Mufu { source, function, saturate, .. } => {
            let op = match function {
                Some(SpecialFunction::Reciprocal) => Approximate::Reciprocal,
                Some(SpecialFunction::ReciprocalSqrt) => Approximate::ReciprocalSqrt,
                Some(SpecialFunction::Sqrt) => Approximate::Sqrt,
                Some(SpecialFunction::Log2) => Approximate::Log2,
                _ => return Err(unsupported(pc, "unknown or double-high MUFU operation")),
            };
            // NAK classifies binary32 MUFU as flushing; SM52+ also
            // supports SQRT. RRO's output is not an ordinary binary32 value.
            let value = operand(b, block, pc, source, true)?;
            operation(b, block, pc, FloatOperation::Approximate { op, value, saturate })
        }
        _ => {
            return Err(unsupported(
                pc,
                "ALU mode needs additional numeric or partial-lane semantics",
            ))
        }
    };
    b.write(block, Register::Gpr(alu.destination), value);
    Ok(())
}

fn min_max(
    builder: &mut Builder,
    block: BlockId,
    pc: usize,
    [a, b]: [ValueId; 2],
    selector: PredicateOperand,
) -> ValueId {
    // Keep scalar min/max in the semantic IR. A dynamic hardware selector
    // becomes an ordinary SSA select; PT only needs one operation.
    if selector.register == 7 {
        return operation(builder, block, pc, FloatOperation::MinMax {
            a,
            b,
            minimum: !selector.inverted,
        });
    }
    let condition = predicate(builder, block, pc, selector);
    let minimum = operation(builder, block, pc, FloatOperation::MinMax { a, b, minimum: true });
    let maximum = operation(builder, block, pc, FloatOperation::MinMax { a, b, minimum: false });
    emit(builder, block, pc, Type::Bits32, Operation::Select {
        condition,
        yes: minimum,
        no: maximum,
    })
}

pub(super) fn convert(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    c: ConversionInstruction,
) -> Result<(), Diagnostic> {
    if c.unrecognized_bits != 0 || c.write_condition_code || c.selector != 0 {
        return Err(unsupported(
            pc,
            "conversion has unknown bits, condition-code writes or source selection",
        ));
    }
    let op = match c.operation {
        ConversionOperation::FloatToFloat
            if c.source_bits == 32 && c.destination_bits == 32 && c.integer_rounding =>
        {
            let value = operand(
                b,
                block,
                pc,
                FloatOperand { source: c.source, absolute: c.absolute, negate: c.negate },
                c.flush_to_zero,
            )?;
            FloatOperation::RoundIntegral {
                value,
                control: ResultControl {
                    rounding: rounding(c.rounding),
                    flush_to_zero: c.flush_to_zero,
                    saturate: c.saturate,
                },
            }
        }
        ConversionOperation::FloatToInteger if c.source_bits == 32 && matches!(c.destination_bits, 16 | 32) => {
            let value = operand(
                b,
                block,
                pc,
                FloatOperand { source: c.source, absolute: c.absolute, negate: c.negate },
                c.flush_to_zero,
            )?;
            FloatOperation::ToInteger { value, bits: c.destination_bits, signed: c.signed, rounding: rounding(c.rounding) }
        }
        ConversionOperation::IntegerToFloat
            if matches!(c.source_bits, 8 | 16 | 32)
                && c.destination_bits == 32
                && !c.absolute
                && !c.negate =>
        {
            let value = source(b, block, pc, c.source)?;
            FloatOperation::FromInteger {
                value,
                bits: c.source_bits,
                signed: c.signed,
                rounding: rounding(c.rounding),
            }
        }
        _ => {
            return Err(unsupported(
                pc,
                format!(
                    "unsupported {:?} conversion: {} to {} bits, signed={}, rounding={:?}, integral={}, abs={}, negate={}",
                    c.operation, c.source_bits, c.destination_bits, c.signed, c.rounding,
                    c.integer_rounding, c.absolute, c.negate,
                ),
            ))
        }
    };
    let value = operation(b, block, pc, op);
    b.write(block, Register::Gpr(c.destination), value);
    Ok(())
}

fn relation(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    args: FloatComparisonOperands,
) -> Result<ValueId, Diagnostic> {
    if args.unrecognized_bits != 0 {
        return Err(unsupported(pc, "unrecognized floating comparison bits"));
    }
    let a = operand(b, block, pc, args.sources[0], args.flush_to_zero)?;
    let c = operand(b, block, pc, args.sources[1], args.flush_to_zero)?;
    use FloatComparison::*;
    Ok(match args.comparison {
        False => b.predicate(false),
        True => b.predicate(true),
        IsNumber | IsNan => {
            let a = operation(b, block, pc, FloatOperation::IsNan(a));
            let c = operation(b, block, pc, FloatOperation::IsNan(c));
            let nan = emit(b, block, pc, Type::Predicate, Operation::PredicateLogic {
                op: Logic::Or,
                a,
                b: c,
            });
            if args.comparison == IsNumber {
                emit(b, block, pc, Type::Predicate, Operation::PredicateNot(nan))
            } else {
                nan
            }
        }
        compare => operation(b, block, pc, FloatOperation::Compare {
            op: match compare {
                Less | LessUnordered => Relation::Less,
                Equal | EqualUnordered => Relation::Equal,
                LessEqual | LessEqualUnordered => Relation::LessEqual,
                Greater | GreaterUnordered => Relation::Greater,
                NotEqual | NotEqualUnordered => Relation::NotEqual,
                GreaterEqual | GreaterEqualUnordered => Relation::GreaterEqual,
                _ => unreachable!(),
            },
            a,
            b: c,
            unordered: matches!(
                compare,
                LessUnordered
                    | EqualUnordered
                    | LessEqualUnordered
                    | GreaterUnordered
                    | NotEqualUnordered
                    | GreaterEqualUnordered
            ),
        }),
    })
}

pub(super) fn compare(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    compare: FloatComparisonInstruction,
) -> Result<(), Diagnostic> {
    let (destination, args) = match compare {
        FloatComparisonInstruction::Set {
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
        FloatComparisonInstruction::SetPredicates {
            destination,
            secondary_destination,
            operands,
        } => (
            ComparisonDestination::Predicates { destination, secondary: secondary_destination },
            operands,
        ),
    };
    let relation = relation(b, block, pc, args)?;
    let accumulator = predicate(b, block, pc, args.accumulator);
    let logic = predicate_logic(pc, args.logic)?;
    comparison_outputs(b, block, pc, destination, (relation, accumulator, logic))
}
