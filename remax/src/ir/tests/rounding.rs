use Register::{Gpr as R, Predicate as P};

use super::{bytes, lift, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, FloatModel, Inputs, InterpretOptions, Program, Register, Signature, Value,
    },
    resources::ConstantMemory,
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: crate::program::SpecialFunctionModel::Reject,
};

fn instruction(mode: u8, controls: u8) -> u64 {
    0x5ca8040000170a00
        | (u64::from(mode) << 39)
        | (u64::from(controls & 1) << 44)
        | (u64::from((controls >> 1) & 1) << 45)
        | (u64::from((controls >> 2) & 1) << 49)
        | (u64::from((controls >> 3) & 1) << 50)
}

#[test]
fn integral_float_rounding_keeps_signed_zeros_ties_and_large_values() {
    for (input, expected) in [
        (0x80000000, [0x80000000; 4]),
        (0x00000001, [0, 0, 0x3f800000, 0]),
        (0x80000001, [0x80000000, 0xbf800000, 0x80000000, 0x80000000]),
        (0x3f000000, [0, 0, 0x3f800000, 0]),
        (0xbf000000, [0x80000000, 0xbf800000, 0x80000000, 0x80000000]),
        (0x3fc00000, [0x40000000, 0x3f800000, 0x40000000, 0x3f800000]),
        (0x40200000, [0x40000000, 0x40000000, 0x40400000, 0x40000000]),
        (0xc0200000, [0xc0000000, 0xc0400000, 0xc0000000, 0xc0000000]),
        (0x7f7fffff, [0x7f7fffff; 4]),
        (0x7f800000, [0x7f800000; 4]),
    ] {
        for (mode, expected) in expected.into_iter().enumerate() {
            let p = lift(&[instruction(mode as u8, 0), EXIT], &[R(1)], &[R(0)]);
            let input =
                Inputs { registers: [(R(1), Value::Word(input))].into(), ..Default::default() };
            assert_eq!(
                p.interpret_with_options(&input, OPTIONS)
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation"),
                [Value::Word(expected)]
            );
        }
    }
}

#[test]
fn floor_applies_input_modifiers_before_rounding_and_saturation_afterward() {
    for (controls, input, expected) in [
        (1, 0x80000001, 0x80000000), // FTZ before floor.
        (2, 0x3fc00000, 0xc0000000), // floor(-1.5) = -2.
        (6, 0xbfc00000, 0xc0000000), // absolute before negate.
        (8, 0x3fc00000, 0x3f800000), // Saturate the rounded result.
        (8, 0x80000000, 0),
        (8, 0x7fc12345, 0),
    ] {
        let p = lift(&[instruction(1, controls), EXIT], &[R(1)], &[R(0)]);
        let input = Inputs { registers: [(R(1), Value::Word(input))].into(), ..Default::default() };
        assert_eq!(
            p.interpret_with_options(&input, OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn decoding_more_float_conversions_does_not_authorize_unresolved_forms() {
    for word in [
        0x5ca8000000170a00, // Same-width non-integral NaN handling.
        instruction(0, 0) | (1 << 47),
        instruction(0, 0) | (1 << 41),
        instruction(0, 0) ^ (1 << 8),
        instruction(0, 0) | (1 << 12),
    ] {
        assert_eq!(
            Program::lift(
                MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
                &Signature::default()
            )
            .unwrap_err()
            .kind,
            DiagnosticKind::Unsupported
        );
    }
    let p = lift(&[instruction(1, 0) & !(7 << 16), EXIT], &[R(0), R(1), P(0)], &[R(0)]);
    let mut input = Inputs {
        registers: [(R(0), Value::Word(0x1234)), (P(0), Value::Predicate(false))].into(),
        ..Default::default()
    };
    assert_eq!(p.interpret(&input, 1000).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(0x1234)
    ]);
    input.registers.insert(P(0), Value::Predicate(true));
    assert!(p.interpret_with_options(&input, OPTIONS).is_err());
    input.registers.insert(R(1), Value::Word(1.5f32.to_bits()));
    assert_eq!(p.interpret(&input, 1000).unwrap_err().kind, DiagnosticKind::Unsupported);
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut words = vec![
        0, 0x80000000, 1, 0x80000001, 0x7fffff, 0x807fffff, 0x7f800000, 0xff800000, 0x7fc12345,
        0xffc12345, 0x7f812345, 0xff812345,
    ];
    for exponent in 0..255 {
        for sign in [0, 0x80000000] {
            for fraction in [0, 1, 0x400000, 0x7fffff] {
                words.push(sign | (exponent << 23) | fraction);
            }
        }
    }
    let cases = words
        .into_iter()
        .map(|w| Inputs { registers: [(R(1), Value::Word(w))].into(), ..Default::default() })
        .collect::<Vec<_>>();
    let mut result = vec![];
    for mode in 0..4 {
        for controls in 0..16 {
            result.push((
                lift(&[instruction(mode, controls), EXIT], &[R(1)], &[R(0)]),
                cases.clone(),
            ));
        }
    }
    // Original source31 cbuf floor, with an explicit test input snapshot.
    let mut constant = vec![0; 0x98];
    constant[0x94..].copy_from_slice(&2.75f32.to_le_bytes());
    let input =
        Inputs { constants: ConstantMemory::new(&[(4, &constant)]).unwrap(), ..Default::default() };
    result.push((lift(&[0x4ca8149002570a01, EXIT], &[], &[R(1)]), vec![input]));
    // nvdisasm independently spells the compact immediate as 1.5.
    result.push((lift(&[0x38a8043fc0070a00, EXIT], &[], &[R(0)]), vec![Inputs::default()]));
    result
}
