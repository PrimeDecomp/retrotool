//! Narrow F2I saturation, extension and NaN policy. The numeric reference uses
//! binary64 rounding and exact 16-bit bounds, independently of APFloat/WGSL.

use super::{lift, Inputs, Program, Register, Value, EXIT};
use crate::program::{F2iNanBehavior, FloatModel, InterpretOptions};

pub(super) struct ConversionCases {
    pub program: Program,
    pub inputs: Vec<Inputs>,
    pub expected: Vec<u32>,
    pub nan_behavior: F2iNanBehavior,
}

pub(super) fn cases() -> Vec<ConversionCases> {
    let mut values = vec![
        0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x807fffff, 0x7f800000, 0xff800000, 0x7f7fffff,
        0xff7fffff, 0x7fc12345, 0xff800001,
    ];
    for base in [-32769., -32768., -1., 0., 1., 32767., 32768., 65535., 65536.] {
        for quarter in -4..=4 {
            values.push((base + quarter as f32 * 0.25).to_bits());
        }
    }
    let mut random = 0x53u32;
    for _ in 0..512 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        values.push(random);
    }
    let mut cases = Vec::new();
    for signed in [false, true] {
        for ftz in [false, true] {
            for round in 0..4 {
                let word = 0x5cb0000000170900
                    | (u64::from(signed) << 12)
                    | (u64::from(ftz) << 44)
                    | (round << 39);
                for nan_behavior in [F2iNanBehavior::Zero, F2iNanBehavior::Indefinite] {
                    let program = lift(&[word, EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)]);
                    let inputs = values
                        .iter()
                        .map(|&value| Inputs {
                            registers: [(Register::Gpr(1), Value::Word(value))].into(),
                            f2i_nan_behavior: Some(nan_behavior),
                            ..Default::default()
                        })
                        .collect();
                    let expected = values
                        .iter()
                        .map(|&value| reference(value, signed, ftz, round, nan_behavior))
                        .collect();
                    cases.push(ConversionCases { program, inputs, expected, nan_behavior });
                }
            }
        }
    }
    cases
}

fn reference(mut bits: u32, signed: bool, ftz: bool, round: u64, nan: F2iNanBehavior) -> u32 {
    if ftz && bits & 0x7f800000 == 0 {
        bits &= 0x80000000;
    }
    let value = f64::from(f32::from_bits(bits));
    if value.is_nan() {
        return match nan {
            F2iNanBehavior::Zero => 0,
            F2iNanBehavior::Indefinite => 0x8000,
        };
    }
    let rounded = match round {
        0 => value.round_ties_even(),
        1 => value.floor(),
        2 => value.ceil(),
        3 => value.trunc(),
        _ => unreachable!(),
    };
    let (low, high) = if signed { (-32768., 32767.) } else { (0., 65535.) };
    rounded.clamp(low, high) as i32 as u32
}

#[test]
fn narrow_float_to_integer_matches_independent_numeric_reference() {
    for case in cases() {
        for (input, expected) in case.inputs.iter().zip(case.expected) {
            let actual = case
                .program
                .interpret_with_options(input, InterpretOptions {
                    step_budget: 100,
                    float_model: FloatModel::SoftwareIeee,
                    ..Default::default()
                })
                .unwrap()
                .into_outputs()
                .unwrap();
            assert_eq!(actual, [Value::Word(expected)], "{input:?}");
        }
    }
}
