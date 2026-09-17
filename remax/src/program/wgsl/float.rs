use super::{
    emit::{name, Emitter},
    instruction::relation,
    unsupported,
};
use crate::{
    ir::{
        float::{Approximate, Binary, Operation, ReducedFunction, ResultControl, Rounding},
        ValueId,
    },
    program::{Diagnostic, F2iNanBehavior},
};

fn rounding_code(rounding: Rounding) -> u8 {
    match rounding {
        Rounding::NearestEven => 0,
        Rounding::NegativeInfinity => 1,
        Rounding::PositiveInfinity => 2,
        Rounding::Zero => 3,
    }
}

fn apply_result_control(value: String, control: ResultControl) -> String {
    let value = if control.saturate { format!("remax_saturate({value})") } else { value };
    if control.flush_to_zero {
        format!("remax_flush({value})")
    } else {
        value
    }
}

fn binary(
    op: Binary,
    a: ValueId,
    b: ValueId,
    control: ResultControl,
    pc: usize,
) -> Result<String, Diagnostic> {
    let expression = match op {
        Binary::Multiply { exponent }
            if exponent != 0 || control.rounding != Rounding::NearestEven =>
        {
            // Scaling belongs to the exact product, before rounding. A second
            // floating multiply can overflow or flush early. Directed rounding
            // uses the same integer path, including for an unscaled product.
            format!(
                "remax_scaled_multiply({}, {}, {exponent}, {}u, {})",
                name(a),
                name(b),
                rounding_code(control.rounding),
                control.flush_to_zero,
            )
        }
        _ => {
            if control.rounding != Rounding::NearestEven {
                return Err(unsupported(pc, "directed arithmetic rounding has no WGSL lowering"));
            }
            // Preserve the existing approximate host arithmetic for addition
            // and unscaled nearest-even multiplication.
            let operator = match op {
                Binary::Add => "+",
                Binary::Multiply { .. } => "*",
            };
            format!("bitcast<u32>(bitcast<f32>({}) {operator} bitcast<f32>({}))", name(a), name(b))
        }
    };
    Ok(apply_result_control(expression, control))
}

impl Emitter<'_> {
    pub(super) fn arithmetic(&mut self, pc: usize) -> Result<(), Diagnostic> {
        if !self.options.approximate_arithmetic {
            return Err(unsupported(
                pc,
                "WGSL arithmetic requires an explicit approximation policy",
            ));
        }
        self.kernel.approximate_arithmetic = true;
        Ok(())
    }

    pub(super) fn float(
        &mut self,
        operation: &Operation,
        pc: usize,
        valid: &str,
    ) -> Result<String, Diagnostic> {
        let value = match *operation {
            Operation::Reduced { function, value, saturate } => {
                if !self.options.approximate_special_functions {
                    return Err(unsupported(
                        pc,
                        "WGSL range-reduced functions require an explicit approximation policy",
                    ));
                }
                self.kernel.approximate_special_functions = true;
                let function = match function {
                    ReducedFunction::Exp2 => "exp2",
                    ReducedFunction::Sin => "sin",
                    ReducedFunction::Cos => "cos",
                };
                let value = format!(
                    "remax_flush(bitcast<u32>({function}(bitcast<f32>({}.argument))))",
                    name(value)
                );
                if saturate {
                    format!("remax_saturate({value})")
                } else {
                    value
                }
            }
            Operation::Modify { value, absolute, negate, flush_to_zero } => {
                let mut expression = name(value);
                if absolute {
                    expression = format!("({expression} & 0x7fffffffu)");
                }
                if negate {
                    expression = format!("({expression} ^ 0x80000000u)");
                }
                if flush_to_zero {
                    expression = format!("remax_flush({expression})");
                }
                expression
            }
            Operation::Binary { op, a, b, control } => {
                self.arithmetic(pc)?;
                binary(op, a, b, control, pc)?
            }
            Operation::MinMax { a, b, minimum } => {
                // The integer helper preserves number bits, including signed
                // zeros and subnormals. Both-NaN payloads remain a model choice.
                self.arithmetic(pc)?;
                format!("remax_min_max({}, {}, {minimum})", name(a), name(b))
            }
            Operation::Fma { a, b, c, control } => {
                self.arithmetic(pc)?;
                if control.rounding != Rounding::NearestEven {
                    return Err(unsupported(pc, "directed FMA rounding has no WGSL lowering"));
                }
                apply_result_control(
                    format!(
                        "bitcast<u32>(fma(bitcast<f32>({}), bitcast<f32>({}), bitcast<f32>({})))",
                        name(a),
                        name(b),
                        name(c)
                    ),
                    control,
                )
            }
            Operation::Approximate { op, value, saturate } => {
                if !self.options.approximate_special_functions {
                    return Err(unsupported(
                        pc,
                        "WGSL special functions require an explicit approximation policy",
                    ));
                }
                self.kernel.approximate_special_functions = true;
                let value = format!("bitcast<f32>({})", name(value));
                let call = match op {
                    Approximate::Reciprocal => format!("1.0 / {value}"),
                    Approximate::ReciprocalSqrt => format!("inverseSqrt({value})"),
                    Approximate::Sqrt => format!("sqrt({value})"),
                    Approximate::Log2 => format!("log2({value})"),
                };
                let value = format!("remax_flush(bitcast<u32>({call}))");
                if saturate {
                    format!("remax_saturate({value})")
                } else {
                    value
                }
            }
            Operation::IsNan(value) => format!("remax_nan({})", name(value)),
            Operation::Compare { op, a, b, unordered } => {
                use crate::ir::Relation::*;
                let (a, b) = (name(a), name(b));
                let equal = format!("remax_equal({a}, {b})");
                let compare = match op {
                    Equal => equal.clone(),
                    NotEqual => format!("!{equal}"),
                    Less | Greater => {
                        format!("!{equal} && remax_order({a}) {} remax_order({b})", relation(op))
                    }
                    LessEqual | GreaterEqual => {
                        format!("{equal} || remax_order({a}) {} remax_order({b})", relation(op))
                    }
                };
                let has_nan = format!("remax_nan({a}) || remax_nan({b})");
                if unordered {
                    format!("({has_nan}) || ({compare})")
                } else {
                    format!("!({has_nan}) && ({compare})")
                }
            }
            Operation::ToInteger { value, bits, signed, rounding } => {
                self.kernel.f2i_nan_behavior = self.options.f2i_nan_behavior;
                if self.options.f2i_nan_behavior.is_none() {
                    self.fault(&format!("({valid}) && remax_nan({})", name(value)), "64u", pc);
                }
                let nan_result: u32 = match self.options.f2i_nan_behavior {
                    Some(F2iNanBehavior::Indefinite) => 1 << (bits - 1),
                    _ => 0,
                };
                let converted = format!(
                    "remax_f2i({}, {signed}, {}u, {nan_result}u)",
                    name(value),
                    rounding_code(rounding)
                );
                if bits == 32 {
                    converted
                } else if signed {
                    let clipped =
                        format!("bitcast<u32>(clamp(bitcast<i32>({converted}), -32768i, 32767i))");
                    // The indefinite NaN result is 0x00008000, even for S16;
                    // only finite signed results are sign extended.
                    if nan_result != 0 {
                        format!("select({clipped}, {nan_result}u, remax_nan({}))", name(value))
                    } else {
                        clipped
                    }
                } else {
                    format!("min({converted}, 65535u)")
                }
            }
            Operation::FromInteger { value, bits, signed, rounding } => {
                let mut expression = name(value);
                if bits < 32 {
                    let shift = 32 - bits;
                    expression = if signed {
                        format!("bitcast<u32>(bitcast<i32>({expression} << {shift}u) >> {shift}u)")
                    } else {
                        format!("({expression} & {}u)", (1u32 << bits) - 1)
                    };
                }
                format!("remax_i2f({expression}, {signed}, {}u)", rounding_code(rounding))
            }
            Operation::RoundIntegral { value, control } => {
                // The bit algorithm avoids host flushing and integer-range
                // limits. NaN quieting/payloads remain the IEEE model's choice.
                self.arithmetic(pc)?;
                apply_result_control(
                    format!(
                        "remax_round_integral({}, {}u)",
                        name(value),
                        rounding_code(control.rounding)
                    ),
                    control,
                )
            }
        };
        Ok(value)
    }
}
