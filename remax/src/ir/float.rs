//! Binary32 semantic operations. Modifiers act on bits; arithmetic retains its
//! rounding and result controls until interpretation or target legalization.
//! APFloat is a software comparison model, not an SM53 implementation. No
//! arithmetic reassociation, contraction or NaN-payload assumptions are implied.

use rustc_apfloat::{
    ieee::{Double, Single},
    Float, FloatConvert, Round,
};

use super::{Relation, Type, ValueId};
use crate::program::{
    Diagnostic, DiagnosticKind, F2iNanBehavior, FloatModel, Inputs, InterpretOptions, Phase,
    SpecialFunctionModel, Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Rounding {
    NearestEven,
    NegativeInfinity,
    PositiveInfinity,
    Zero,
}

impl Rounding {
    fn apfloat(self) -> Round {
        match self {
            Self::NearestEven => Round::NearestTiesToEven,
            Self::NegativeInfinity => Round::TowardNegative,
            Self::PositiveInfinity => Round::TowardPositive,
            Self::Zero => Round::TowardZero,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Binary {
    Add,
    /// Scale the exact product by 2^exponent before rounding to binary32.
    Multiply {
        exponent: i8,
    },
}

/// Approximate hardware operations, kept distinct from basic IEEE arithmetic.
/// No reciprocal/division or reciprocal-square-root identities are implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Approximate {
    Reciprocal,
    ReciprocalSqrt,
    Sqrt,
    Log2,
}

/// Distinct opaque domains produced by Maxwell range reduction. The captured
/// argument is an interpreter/backend model, not the native intermediate bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Reduction {
    Exp2,
    SinCos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReducedFunction {
    Exp2,
    Sin,
    Cos,
}

impl ReducedFunction {
    pub fn reduction(self) -> Reduction {
        match self {
            Self::Exp2 => Reduction::Exp2,
            Self::Sin | Self::Cos => Reduction::SinCos,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ResultControl {
    pub rounding: Rounding,
    /// Result flushing; input flushing is represented by Modify operations.
    /// Multiply tests exact-product tininess before rounding, as measured on SM75.
    pub flush_to_zero: bool,
    pub saturate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Operation {
    Modify {
        value: ValueId,
        absolute: bool,
        negate: bool,
        flush_to_zero: bool,
    },
    Binary {
        op: Binary,
        a: ValueId,
        b: ValueId,
        control: ResultControl,
    },
    /// IEEE minimumNumber/maximumNumber: prefer a number to a NaN, order
    /// -0 below +0, and preserve the selected number's bits. Both-NaN results
    /// use the measured SM75 payload 0x7fffffff in the comparison model;
    /// the corresponding SM53 payload remains unverified.
    MinMax {
        a: ValueId,
        b: ValueId,
        minimum: bool,
    },
    Fma {
        a: ValueId,
        b: ValueId,
        c: ValueId,
        control: ResultControl,
    },
    /// MUFU flushes binary32 subnormal inputs/results. Input modifiers and
    /// flushing are explicit Modify nodes; output saturation follows the SFU.
    Approximate {
        op: Approximate,
        value: ValueId,
        saturate: bool,
    },
    /// Complete RRO/MUFU pair. Sin/cos arguments are in radians. Producer
    /// modifiers precede reduction; flushing/saturation follow the consumer.
    Reduced {
        function: ReducedFunction,
        value: ValueId,
        saturate: bool,
    },
    Compare {
        op: Relation,
        a: ValueId,
        b: ValueId,
        unordered: bool,
    },
    IsNan(ValueId),
    /// Saturating float-to-integer conversion. Narrow signed results are sign
    /// extended to the containing word; unsigned results are zero extended.
    ToInteger {
        value: ValueId,
        bits: u8,
        signed: bool,
        rounding: Rounding,
    },
    FromInteger {
        value: ValueId,
        bits: u8,
        signed: bool,
        rounding: Rounding,
    },
    /// Round binary32 to an integral binary32 value, retaining the floating
    /// representation and signed zero. This is not an integer conversion.
    RoundIntegral {
        value: ValueId,
        control: ResultControl,
    },
}

impl Operation {
    pub fn operands(&self) -> Vec<ValueId> {
        match *self {
            Self::Reduced { value, .. } => vec![value],
            Self::Modify { value, .. }
            | Self::ToInteger { value, .. }
            | Self::FromInteger { value, .. }
            | Self::RoundIntegral { value, .. }
            | Self::Approximate { value, .. }
            | Self::IsNan(value) => vec![value],
            Self::Binary { a, b, .. } | Self::MinMax { a, b, .. } | Self::Compare { a, b, .. } => {
                vec![a, b]
            }
            Self::Fma { a, b, c, .. } => vec![a, b, c],
        }
    }

    pub fn map_operands(&mut self, mut f: impl FnMut(ValueId) -> ValueId) {
        match self {
            Self::Reduced { value, .. } => *value = f(*value),
            Self::Modify { value, .. }
            | Self::ToInteger { value, .. }
            | Self::FromInteger { value, .. }
            | Self::RoundIntegral { value, .. }
            | Self::Approximate { value, .. }
            | Self::IsNan(value) => *value = f(*value),
            Self::Binary { a, b, .. } | Self::MinMax { a, b, .. } | Self::Compare { a, b, .. } => {
                *a = f(*a);
                *b = f(*b);
            }
            Self::Fma { a, b, c, .. } => {
                *a = f(*a);
                *b = f(*b);
                *c = f(*c);
            }
        }
    }

    pub fn result_type(&self) -> Type {
        match self {
            Self::Compare { .. } | Self::IsNan(_) => Type::Predicate,
            _ => Type::Bits32,
        }
    }

    pub fn operand_type(&self) -> Type {
        if let Self::Reduced { function, .. } = self {
            Type::Reduced(function.reduction())
        } else {
            Type::Bits32
        }
    }

    pub fn evaluate(
        &self,
        args: &[Value],
        options: InterpretOptions,
        inputs: &Inputs,
        pc: usize,
    ) -> Result<Value, Diagnostic> {
        let error = |kind, message| Diagnostic::new(Phase::Interpret, Some(pc), kind, message);
        let word = |i: usize| match args[i] {
            Value::Word(bits) => bits,
            _ => unreachable!("verified float operand"),
        };
        let number = |i| Single::from_bits(u128::from(word(i)));
        let software = || {
            if options.float_model != FloatModel::SoftwareIeee {
                Err(error(
                    DiagnosticKind::Unsupported,
                    "operation requires the software IEEE comparison model",
                ))
            } else {
                Ok(())
            }
        };
        let result = match *self {
            // The interpreter unwraps the opaque input separately. Ordinary
            // numeric operands must never expose its symbolic payload as bits.
            Self::Reduced { .. } => unreachable!("opaque special-function operand"),
            Self::Modify { absolute, negate, flush_to_zero, .. } => {
                let mut bits = word(0);
                if absolute {
                    bits &= 0x7fffffff;
                }
                if negate {
                    bits ^= 0x80000000;
                }
                if flush_to_zero {
                    bits = flush(bits);
                }
                bits
            }
            Self::IsNan(_) => return Ok(Value::Predicate(is_nan(word(0)))),
            Self::Compare { op, unordered, .. } => {
                let (a, b) = (word(0), word(1));
                let nan = is_nan(a) || is_nan(b);
                let equal = a == b || ((a | b) & 0x7fffffff) == 0;
                let less = !equal && order_key(a) < order_key(b);
                let greater = !equal && order_key(a) > order_key(b);
                let relation = match op {
                    Relation::Less => less,
                    Relation::Equal => equal,
                    Relation::LessEqual => less || equal,
                    Relation::Greater => greater,
                    Relation::NotEqual => !equal,
                    Relation::GreaterEqual => greater || equal,
                };
                return Ok(Value::Predicate(if nan { unordered } else { relation }));
            }
            Self::Binary { op, control, .. } => {
                software()?;
                let (a, b) = (number(0), number(1));
                let value = match op {
                    Binary::Add => a.add_r(b, control.rounding.apfloat()),
                    Binary::Multiply { exponent } => {
                        return Ok(Value::Word(multiply(a, b, exponent, control)));
                    }
                }
                .value;
                finish(value.to_bits() as u32, control)
            }
            Self::MinMax { minimum, .. } => {
                software()?;
                min_max(word(0), word(1), minimum)
            }
            Self::Fma { control, .. } => {
                software()?;
                let value =
                    number(0).mul_add_r(number(1), number(2), control.rounding.apfloat()).value;
                finish(value.to_bits() as u32, control)
            }
            Self::Approximate { op, saturate: sat, .. } => {
                if options.special_functions != SpecialFunctionModel::HostF32 {
                    return Err(error(
                        DiagnosticKind::Unsupported,
                        "MUFU requires an explicit special-function comparison model",
                    ));
                }
                let input = f32::from_bits(word(0));
                let result = match op {
                    Approximate::Reciprocal => input.recip(),
                    Approximate::ReciprocalSqrt => input.sqrt().recip(),
                    Approximate::Sqrt => input.sqrt(),
                    Approximate::Log2 => input.log2(),
                };
                // Host libm supplies the approximation, not instruction controls.
                let bits = flush(result.to_bits());
                if sat {
                    saturate(bits)
                } else {
                    bits
                }
            }
            Self::ToInteger { bits, signed, rounding, .. } => {
                software()?;
                let value = number(0);
                if value.is_nan() {
                    match inputs.f2i_nan_behavior.ok_or_else(|| {
                        error(
                            DiagnosticKind::MissingInput,
                            "F2I of NaN requires the shader-control NaN behavior",
                        )
                    })? {
                        F2iNanBehavior::Zero => 0,
                        F2iNanBehavior::Indefinite => 1 << (bits - 1),
                    }
                } else if signed {
                    value.to_i128_r(bits as usize, rounding.apfloat(), &mut false).value as u32
                } else {
                    value.to_u128_r(bits as usize, rounding.apfloat(), &mut false).value as u32
                }
            }
            Self::FromInteger { bits, signed, rounding, .. } => {
                software()?;
                let shift = 32 - bits;
                let value = if signed {
                    Single::from_i128_r(
                        i128::from((word(0) << shift) as i32 >> shift),
                        rounding.apfloat(),
                    )
                } else {
                    Single::from_u128_r(u128::from((word(0) << shift) >> shift), rounding.apfloat())
                }
                .value;
                value.to_bits() as u32
            }
            Self::RoundIntegral { control, .. } => {
                software()?;
                let value = number(0).round_to_integral(control.rounding.apfloat()).value;
                finish(value.to_bits() as u32, control)
            }
        };
        Ok(Value::Word(result))
    }
}

pub(super) fn reduced(
    function: ReducedFunction,
    argument: u32,
    saturate_result: bool,
    options: InterpretOptions,
    pc: usize,
) -> Result<u32, Diagnostic> {
    if options.special_functions != SpecialFunctionModel::HostF32 {
        return Err(Diagnostic::new(
            Phase::Interpret,
            Some(pc),
            DiagnosticKind::Unsupported,
            "range-reduced function requires an explicit special-function comparison model",
        ));
    }
    let argument = f32::from_bits(argument);
    let result = match function {
        ReducedFunction::Exp2 => argument.exp2(),
        ReducedFunction::Sin => argument.sin(),
        ReducedFunction::Cos => argument.cos(),
    };
    let result = flush(result.to_bits());
    Ok(if saturate_result { saturate(result) } else { result })
}

fn is_nan(bits: u32) -> bool { bits & 0x7fffffff > 0x7f800000 }

pub(crate) fn interpolate_plane(
    plane: crate::program::interpolation::Plane,
    offset: [f32; 2],
    model: FloatModel,
) -> u32 {
    match model {
        FloatModel::HostF32 => {
            (plane.center + plane.dx * offset[0] + plane.dy * offset[1]).to_bits()
        }
        FloatModel::SoftwareIeee => {
            let value = |f: f32| Single::from_bits(u128::from(f.to_bits()));
            let r = Round::NearestTiesToEven;
            let x = value(plane.dx).mul_r(value(offset[0]), r).value;
            let y = value(plane.dy).mul_r(value(offset[1]), r).value;
            value(plane.center).add_r(x, r).value.add_r(y, r).value.to_bits() as u32
        }
        FloatModel::Reject => unreachable!("plane evaluation requires a numeric model"),
    }
}

fn order_key(bits: u32) -> u32 {
    if bits & 0x80000000 != 0 {
        !bits
    } else {
        bits | 0x80000000
    }
}

fn min_max(a: u32, b: u32, minimum: bool) -> u32 {
    if is_nan(a) {
        return if is_nan(b) { 0x7fffffff } else { b };
    }
    if is_nan(b) {
        return a;
    }
    let take_a = if minimum { order_key(a) < order_key(b) } else { order_key(a) > order_key(b) };
    if take_a {
        a
    } else {
        b
    }
}

/// Two binary32 significands have at most 48 product bits. Binary64 retains
/// that product exactly, including the full exponent range after an i8 scale.
/// Conversion to Single is the only rounding point. This is the comparison
/// model; the native SM53 scaling/NaN edge behavior remains unmeasured.
fn multiply(a: Single, b: Single, exponent: i8, control: ResultControl) -> u32 {
    let mut loses_info = false;
    let a: Double = a.convert(&mut loses_info).value;
    let b: Double = b.convert(&mut loses_info).value;
    let product = a.mul_r(b, Round::NearestTiesToEven).value.scalbn(i32::from(exponent));
    let minimum_normal = Double::from_bits(897u128 << 52); // 2^-126
    let bits = if control.flush_to_zero && product.abs() < minimum_normal {
        if product.is_negative() {
            0x80000000
        } else {
            0
        }
    } else {
        let rounded: Single = product.convert_r(control.rounding.apfloat(), &mut loses_info).value;
        rounded.to_bits() as u32
    };
    finish(bits, control)
}

fn flush(bits: u32) -> u32 {
    if bits & 0x7f800000 == 0 {
        bits & 0x80000000
    } else {
        bits
    }
}

fn saturate(bits: u32) -> u32 {
    if is_nan(bits) || bits & 0x80000000 != 0 {
        0
    } else {
        bits.min(0x3f800000)
    }
}

fn finish(mut bits: u32, control: ResultControl) -> u32 {
    if control.saturate {
        bits = saturate(bits);
    }
    if control.flush_to_zero {
        bits = flush(bits);
    }
    bits
}

pub(super) fn interpolate(word: u32, multiplier: Option<u32>, sat: bool, model: FloatModel) -> u32 {
    match model {
        FloatModel::SoftwareIeee => {
            let mut bits = word;
            if let Some(multiplier) = multiplier {
                bits = Single::from_bits(u128::from(word))
                    .mul_r(Single::from_bits(u128::from(multiplier)), Round::NearestTiesToEven)
                    .value
                    .to_bits() as u32;
            }
            if sat {
                bits = saturate(bits);
            }
            bits
        }
        FloatModel::HostF32 => {
            let mut value = f32::from_bits(word);
            if let Some(multiplier) = multiplier {
                value *= f32::from_bits(multiplier);
            }
            if sat {
                value = value.clamp(0.0, 1.0);
            }
            value.to_bits()
        }
        FloatModel::Reject => unreachable!("interpolation model checked by interpreter"),
    }
}
