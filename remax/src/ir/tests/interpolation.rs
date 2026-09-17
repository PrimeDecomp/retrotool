use Register::{Gpr as R, Predicate as P};

use super::{bytes, mov, offset, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        interpolation::{Attribute, OffsetModel, Plane, Planes},
        DiagnosticKind, FloatModel, Inputs, Interpolant, InterpolationLocation, InterpolationMode,
        InterpretOptions, Program, Register, Signature, Stage, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: crate::program::SpecialFunctionModel::Reject,
};
pub(super) const ATTRIBUTE: Attribute =
    Attribute { byte_offset: 0x80, mode: InterpolationMode::Pass };
pub(super) const PLANE: Plane = Plane { center: 2., dx: 4., dy: -8. };

pub(super) fn ipa(mode: u8, location: u8) -> u64 {
    0xe000000000000000
        | (255 << 8)
        | (7 << 16)
        | (5 << 20)
        | (0x80 << 28)
        | (6 << 39)
        | (7 << 47)
        | (u64::from(location) << 52)
        | (u64::from(mode) << 54)
}
pub(super) fn program(words: &[u64], inputs: &[Register], outputs: &[Register]) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
    })
    .unwrap()
}
fn planes(mode: InterpolationMode) -> Planes {
    Planes::new(OffsetModel::SignedSixteenths, &[(Attribute { mode, ..ATTRIBUTE }, PLANE)]).unwrap()
}

#[test]
fn planes_require_explicit_numeric_policy_and_never_replace_exact_requests() {
    let p = program(&[ipa(0, 2), EXIT], &[R(6)], &[R(0)]);
    let mut input = Inputs {
        registers: [(R(6), Value::Word(0))].into(),
        offset_interpolation: Some(planes(InterpolationMode::Pass)),
        ..Default::default()
    };
    assert_eq!(p.interpret(&input, 100).unwrap_err().kind, DiagnosticKind::Unsupported);
    assert_eq!(
        p.interpret_with_options(&input, OPTIONS).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(2f32.to_bits())]
    );
    input.interpolants.insert(
        Interpolant {
            byte_offset: 0x80,
            mode: InterpolationMode::Pass,
            location: InterpolationLocation::Offset(0),
        },
        0x7fc12345,
    );
    assert_eq!(p.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(0x7fc12345)
    ]);
    for location in [0, 1] {
        let p = program(&[ipa(0, location), EXIT], &[], &[R(0)]);
        assert_eq!(
            p.interpret_with_options(&input, OPTIONS).unwrap_err().kind,
            DiagnosticKind::MissingInput
        );
    }
}

#[test]
fn plane_keys_reject_invalid_modes_addresses_and_duplicates() {
    for attribute in [
        Attribute { byte_offset: 0x400, ..ATTRIBUTE },
        Attribute { byte_offset: 0x81, ..ATTRIBUTE },
        Attribute { mode: InterpolationMode::Constant, ..ATTRIBUTE },
        Attribute { mode: InterpolationMode::State, ..ATTRIBUTE },
    ] {
        assert!(Planes::new(OffsetModel::SignedSixteenths, &[(attribute, PLANE)]).is_err());
    }
    assert!(Planes::new(OffsetModel::SignedSixteenths, &[(ATTRIBUTE, PLANE); 2]).is_err());
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut result = Vec::new();
    for (mode_code, mode) in [(0, InterpolationMode::Pass), (1, InterpolationMode::Perspective)] {
        // Destination aliases the dynamic offset. Perspective multiplies only
        // after plane evaluation, then saturation applies to that final value.
        let word = ipa(mode_code, 2) | 6 | if mode_code == 1 { 1 << 51 } else { 0 };
        let p = program(&[word, EXIT], &[R(6), R(5)], &[R(6)]);
        let cases = (0..256u32)
            .flat_map(|byte| {
                [0, 0xf00f00ff].map(move |extra| Inputs {
                    registers: [
                        (R(6), Value::Word((byte << 20) | (byte << 8) | extra)),
                        (R(5), Value::Word(0.5f32.to_bits())),
                    ]
                    .into(),
                    offset_interpolation: Some(planes(mode)),
                    ..Default::default()
                })
            })
            .collect();
        result.push((p, cases));
    }
    // The offset is selected at runtime; the plane is still relative to center.
    let p =
        program(&[mov(6, 0x0cc0cc00, 7), mov(6, 0x04404400, 0), ipa(0, 2), EXIT], &[P(0)], &[R(0)]);
    let cases = [false, true]
        .map(|enabled| Inputs {
            registers: [(P(0), Value::Predicate(enabled))].into(),
            offset_interpolation: Some(planes(InterpolationMode::Pass)),
            ..Default::default()
        })
        .to_vec();
    result.push((p, cases));
    // An untaken IPA reads neither offsets nor plane inputs. A taken one must
    // fault at IPA even when its result is discarded.
    let p = program(&[(ipa(0, 2) | 255) & !(7 << 16), EXIT], &[P(0), R(6)], &[]);
    let cases = [false, true]
        .into_iter()
        .flat_map(|enabled| {
            [false, true].map(move |offset| Inputs {
                registers: std::iter::once((P(0), Value::Predicate(enabled)))
                    .chain(offset.then_some((R(6), Value::Word(0))))
                    .collect(),
                ..Default::default()
            })
        })
        .collect();
    result.push((p, cases));
    result
}

#[test]
fn all_sample_offsets_and_ignored_bits_match_an_independent_affine_plane() {
    for (index, (p, cases)) in comparison_cases().into_iter().enumerate() {
        for (case, input) in cases.into_iter().enumerate() {
            let actual = p.interpret_with_options(&input, OPTIONS);
            if index < 2 {
                let byte = (case / 2) as i8;
                let x = (byte.wrapping_shl(4) >> 4) as f32 / 16.;
                let y = (byte >> 4) as f32 / 16.;
                let v = 2. + 4. * x - 8. * y;
                let expected = if index == 1 { (v * 0.5).clamp(0., 1.) } else { v };
                assert_eq!(actual.unwrap().into_outputs().expect("live invocation"), [
                    Value::Word(expected.to_bits())
                ]);
            } else if index == 2 {
                assert_eq!(actual.unwrap().into_outputs().expect("live invocation"), [
                    Value::Word(if case == 0 { 3f32 } else { 1f32 }.to_bits())
                ]);
            } else if case < 2 {
                assert_eq!(actual.unwrap().into_outputs().expect("live invocation"), []);
            } else {
                let error = actual.unwrap_err();
                assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(0))));
            }
        }
    }
}
