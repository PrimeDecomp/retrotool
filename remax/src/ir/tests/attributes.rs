use super::{bytes, mov, offset, EXIT};
use crate::{
    ir::{Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, FloatModel, Inputs, Interpolant, InterpolationLocation, InterpolationMode,
        InterpretOptions, Program, Register, Signature, Stage, Value,
    },
};

fn lift(words: &[u64], stage: Stage, inputs: &[Register], outputs: &[Register]) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage,
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
    })
    .unwrap()
}

pub(super) fn access(load: bool, register: u8, offset: u16, components: u8) -> u64 {
    let opcode = if load { 0xefd8000000000000 } else { 0xeff0000000000000 };
    opcode
        | u64::from(register)
        | (255 << 8)
        | (7 << 16)
        | (u64::from(offset) << 20)
        | (255 << 39)
        | (u64::from(components - 1) << 47)
}

// R5/R6 deliberately occupy the multiplier/offset fields. Modes which do not
// consume them must work without these registers in the invocation's inputs.
fn ipa(mode: u8, location: u8) -> u64 {
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

fn interpolant(mode: InterpolationMode, location: InterpolationLocation) -> Interpolant {
    Interpolant { byte_offset: 0x80, mode, location }
}

#[test]
fn vertex_groups_preserve_bits_and_distinguish_input_from_output_storage() {
    use Register::{Gpr as R, OutputAttribute as O};
    // Load inputs, store outputs at the same addresses, overwrite one output,
    // then read both namespaces. Only R4..R7 and the overwritten output escape.
    let words = [
        access(true, 0, 0x80, 4),
        access(false, 0, 0x80, 4),
        mov(0, 19, 7),
        access(false, 0, 0x80, 1),
        access(true, 4, 0x80, 2) | (1 << 32), // ALD.O
        access(true, 6, 0x80, 2),
        EXIT,
    ];
    let program = lift(&words, Stage::Vertex, &[], &[R(4), R(5), R(6), R(7), O(0x80)]);
    let inputs = Inputs {
        attributes: [(0x80, 0x7fc12345), (0x84, 0x80000000), (0x88, 7), (0x8c, u32::MAX)].into(),
        ..Default::default()
    };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [19, 0x80000000, 0x7fc12345, 0x80000000, 19].map(Value::Word)
    );

    // The last component is RZ, not an out-of-bounds register or a wrapped R0.
    let program = lift(
        &[access(true, 254, 0x80, 2), access(false, 254, 0x3f8, 2), EXIT],
        Stage::Vertex,
        &[],
        &[R(254), R(255), O(0x3f8), O(0x3fc)],
    );
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [0x7fc12345, 0, 0x7fc12345, 0].map(Value::Word)
    );
}

#[test]
fn predicated_vertex_store_preserves_incoming_output_and_ald_o_observes_it() {
    use Register::{Gpr as R, OutputAttribute as O, Predicate as P};
    let words =
        [access(false, 0, 0x80, 1) & !(7 << 16), access(true, 1, 0x80, 1) | (1 << 32), EXIT];
    let mut inputs = Inputs {
        registers: [
            (R(0), Value::Word(23)),
            (O(0x80), Value::Word(41)),
            (P(0), Value::Predicate(false)),
        ]
        .into(),
        ..Default::default()
    };
    let program = lift(&words, Stage::Vertex, &[R(0), O(0x80), P(0)], &[R(1), O(0x80)]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(41); 2]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(23); 2]
    );

    let program = lift(&words, Stage::Vertex, &[R(0), P(0)], &[R(1)]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(23)]
    );
    inputs.registers.insert(P(0), Value::Predicate(false));
    // The map cannot silently turn undeclared initial output state into an input.
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
}

#[test]
fn predication_guards_vertex_input_reads() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(
        &[mov(0, 29, 7), access(true, 0, 0x80, 1) & !(7 << 16), EXIT],
        Stage::Vertex,
        &[P(0)],
        &[R(0)],
    );
    let mut inputs =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(29)]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    let error = program.interpret(&inputs, 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(1))));
}

#[test]
fn unsupported_vertex_addressing_and_invalid_interfaces_fail_explicitly() {
    let base = access(true, 0, 0x80, 1);
    for (word, stage) in [
        (base, Stage::Scalar),
        (base, Stage::Fragment),
        (base | (1 << 31), Stage::Vertex),    // patch
        (base & !(255 << 8), Stage::Vertex),  // address R0
        (base & !(255 << 39), Stage::Vertex), // vertex R0
        (base | (1 << 33), Stage::Vertex),    // unknown bit
        (access(true, 0, 0x81, 1), Stage::Vertex),
        (access(true, 0, 0x3fc, 2), Stage::Vertex),
        (access(false, 255, 0x80, 2), Stage::Vertex),
    ] {
        let error = Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
            indexed_attributes: vec![],
            stage,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)), "{word:016x}");
    }
    for (stage, output) in [(Stage::Vertex, 0x81), (Stage::Vertex, 0x400), (Stage::Fragment, 0x80)]
    {
        let error = Program::lift(MaxwellCode::parse(&bytes(&[EXIT])).unwrap(), &Signature {
            indexed_attributes: vec![],
            stage,
            outputs: vec![Register::OutputAttribute(output)],
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::InvalidInput);
    }
}

#[test]
fn interpolation_preserves_raw_words_and_requires_the_exact_location() {
    use InterpolationLocation::{Centroid, Default as Center};
    use InterpolationMode::{Constant, Pass};
    for (mode_bits, mode) in [(0, Pass), (2, Constant)] {
        for (location_bits, location) in [(0, Center), (1, Centroid)] {
            let program = lift(&[ipa(mode_bits, location_bits), EXIT], Stage::Fragment, &[], &[
                Register::Gpr(0),
            ]);
            let request = interpolant(mode, location);
            for word in [0x7fc12345, 0x80000000, u32::MAX] {
                let inputs =
                    Inputs { interpolants: [(request, word)].into(), ..Default::default() };
                assert_eq!(
                    program
                        .interpret(&inputs, 100)
                        .unwrap()
                        .into_outputs()
                        .expect("live invocation"),
                    [Value::Word(word)]
                );
            }
            let wrong_location = if location == Center { Centroid } else { Center };
            let inputs = Inputs {
                interpolants: [(interpolant(mode, wrong_location), 0)].into(),
                ..Default::default()
            };
            let error = program.interpret(&inputs, 100).unwrap_err();
            assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));
        }
    }
}

#[test]
fn perspective_multiplier_and_saturation_require_an_explicit_numeric_model() {
    use Register::Gpr as R;
    let options = InterpretOptions {
        step_budget: 100,
        float_model: FloatModel::HostF32,
        ..Default::default()
    };
    let request = interpolant(InterpolationMode::Perspective, InterpolationLocation::Default);
    let mut inputs = Inputs {
        registers: [(R(5), Value::Word(4.0f32.to_bits()))].into(),
        interpolants: [(request, 0.25f32.to_bits())].into(),
        ..Default::default()
    };
    let program = lift(&[ipa(1, 0), EXIT], Stage::Fragment, &[R(5)], &[R(0)]);
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::Unsupported);
    assert_eq!(
        program
            .interpret_with_options(&inputs, options)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(1.0f32.to_bits())]
    );
    inputs.registers.remove(&R(5));
    assert_eq!(
        program.interpret_with_options(&inputs, options).unwrap_err().kind,
        DiagnosticKind::MissingInput
    );
    let program = lift(&[ipa(1, 0), EXIT], Stage::Fragment, &[], &[R(0)]);
    assert_eq!(
        program.interpret_with_options(&inputs, options).unwrap_err().kind,
        DiagnosticKind::UnavailableValue
    );

    for mode in [0, 1] {
        let request = interpolant(
            if mode == 0 { InterpolationMode::Pass } else { InterpolationMode::Perspective },
            InterpolationLocation::Default,
        );
        let program = lift(&[ipa(mode, 0) | (1 << 51), EXIT], Stage::Fragment, &[R(5)], &[R(0)]);
        for (word, expected) in
            [(-2.0f32, 0.0f32), (0.125, if mode == 0 { 0.125 } else { 0.5 }), (2.0, 1.0)]
        {
            let inputs = Inputs {
                registers: [(R(5), Value::Word(4.0f32.to_bits()))].into(),
                interpolants: [(request, word.to_bits())].into(),
                ..Default::default()
            };
            assert_eq!(
                program.interpret(&inputs, 100).unwrap_err().kind,
                DiagnosticKind::Unsupported
            );
            assert_eq!(
                program
                    .interpret_with_options(&inputs, options)
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation"),
                [Value::Word(expected.to_bits())]
            );
        }
    }
}

#[test]
fn interpolation_guard_skips_provider_and_numeric_model_requirements() {
    use Register::{Gpr as R, Predicate as P};
    let program =
        lift(&[mov(0, 73, 7), ipa(1, 0) & !(7 << 16), EXIT], Stage::Fragment, &[P(0)], &[R(0)]);
    let inputs =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(73)]
    );
}

#[test]
fn software_interpolation_preserves_a_subnormal_product_without_ftz() {
    use Register::Gpr as R;
    let program = lift(&[ipa(1, 0), EXIT], Stage::Fragment, &[R(5)], &[R(0)]);
    let inputs = Inputs {
        registers: [(R(5), Value::Word(0.5f32.to_bits()))].into(),
        interpolants: [(
            interpolant(InterpolationMode::Perspective, InterpolationLocation::Default),
            0x00800000,
        )]
        .into(),
        ..Default::default()
    };
    let options = InterpretOptions {
        step_budget: 100,
        float_model: FloatModel::SoftwareIeee,
        ..Default::default()
    };
    assert_eq!(
        program
            .interpret_with_options(&inputs, options)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(0x00400000)]
    );
}

#[test]
fn unresolved_interpolation_modes_and_predicates_are_rejected() {
    for word in [
        ipa(0, 0) & !(255 << 8), // address register
        ipa(0, 0) | (1 << 38),   // indexed
        ipa(0, 0) & !(7 << 47),  // trailing P0, not the execution guard
        ipa(0, 0) | (1 << 50),   // trailing !PT
        ipa(0, 0) | (1 << 28),   // misaligned
        ipa(3, 0),               // state-selected
        ipa(2, 2),               // unverified constant-mode offset
        ipa(0, 3),               // invalid location
    ] {
        let error = Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
            indexed_attributes: vec![],
            stage: Stage::Fragment,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)), "{word:016x}");
    }
}

#[test]
fn verifier_checks_attribute_stage_and_interpolation_contract_independently() {
    let mut function =
        lift(&[access(true, 0, 0x80, 1), EXIT], Stage::Vertex, &[], &[Register::Gpr(0)]).function;
    function.stage = Stage::Fragment;
    assert!(function.verify().unwrap_err().message.contains("vertex attribute"));

    let function =
        lift(&[ipa(1, 0), EXIT], Stage::Fragment, &[Register::Gpr(5)], &[Register::Gpr(0)])
            .function;
    let mut bad = function.clone();
    bad.stage = Stage::Vertex;
    assert!(bad.verify().unwrap_err().message.contains("interpolation"));
    let mut bad = function.clone();
    let wrong_type = bad.value(Type::Predicate, Definition::Predicate(true));
    bad.blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::Interpolate { .. }))
        .unwrap()
        .operation = Operation::Interpolate {
        attribute: crate::ir::attribute::Input::Direct(0x80),
        mode: InterpolationMode::Perspective,
        location: crate::ir::InterpolationLocation::Default,
        multiplier: Some(wrong_type),
        saturate: false,
    };
    assert!(bad.verify().unwrap_err().message.contains("Bits32"));
    for change in 0..4 {
        let mut bad = function.clone();
        let operation = &mut bad
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.operation, Operation::Interpolate { .. }))
            .unwrap()
            .operation;
        let Operation::Interpolate { attribute, mode, multiplier, .. } = operation else {
            unreachable!()
        };
        match change {
            0 => *multiplier = None,
            1 => *attribute = crate::ir::attribute::Input::Direct(0x400),
            2 => *mode = InterpolationMode::State,
            _ => *mode = InterpolationMode::Pass,
        }
        assert!(bad.verify().unwrap_err().message.contains("interpolation"));
    }
}

#[test]
fn offset_interpolation_preserves_every_request_bit_and_reads_before_writing() {
    use Register::Gpr as R;
    // Destination aliases the offset; the multiplier is deliberately unavailable
    // because PASS does not consume it. No floating model is needed here.
    let program = lift(&[ipa(0, 2) | 6, EXIT], Stage::Fragment, &[R(6)], &[R(6)]);
    for packed in [0, 0x0cc0cc00, 0x80000000, 0x7fc12345, u32::MAX] {
        let request = interpolant(InterpolationMode::Pass, InterpolationLocation::Offset(packed));
        let mut inputs = Inputs {
            registers: [(R(6), Value::Word(packed))].into(),
            interpolants: [(request, 0x80000000)].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(0x80000000)]
        );
        inputs.registers.insert(R(6), Value::Word(packed ^ 1));
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));
    }
}

#[test]
fn offset_interpolation_tracks_merged_offset_and_multiplies_the_supplied_numerator() {
    use Register::{Gpr as R, Predicate as P};
    // The offset comes from a predicated write, exercising SSA edge arguments
    // and operand remapping. The provider resolves the location, not the divide.
    let words = [mov(6, 17, 7), mov(6, 29, 0), ipa(1, 2) | (1 << 51), EXIT];
    let program = lift(&words, Stage::Fragment, &[R(5), P(0)], &[R(0)]);
    let options = InterpretOptions {
        step_budget: 100,
        float_model: FloatModel::SoftwareIeee,
        ..Default::default()
    };
    for enabled in [false, true] {
        let inputs = Inputs {
            registers: [(P(0), Value::Predicate(enabled)), (R(5), Value::Word(4.0f32.to_bits()))]
                .into(),
            interpolants: [
                (
                    interpolant(InterpolationMode::Perspective, InterpolationLocation::Offset(17)),
                    0.125f32.to_bits(),
                ),
                (
                    interpolant(InterpolationMode::Perspective, InterpolationLocation::Offset(29)),
                    0.5f32.to_bits(),
                ),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            program
                .interpret_with_options(&inputs, options)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(if enabled { 1.0f32 } else { 0.5f32 }.to_bits())]
        );
    }
}

#[test]
fn executed_offset_read_is_required_even_when_the_destination_is_discarded() {
    use Register::{Gpr as R, Predicate as P};
    let words = [(ipa(0, 2) | 255) & !(7 << 16), EXIT];
    for declared in [false, true] {
        let registers = if declared { vec![P(0), R(6)] } else { vec![P(0)] };
        let program = lift(&words, Stage::Fragment, &registers, &[]);
        let mut inputs =
            Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            []
        );
        inputs.registers.insert(P(0), Value::Predicate(true));
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!(error.pc, Some(8));
        assert_eq!(
            error.kind,
            if declared { DiagnosticKind::MissingInput } else { DiagnosticKind::UnavailableValue }
        );
    }
    let program = lift(&[ipa(0, 2) | 255, EXIT], Stage::Fragment, &[R(6)], &[]);
    let inputs = Inputs { registers: [(R(6), Value::Word(0))].into(), ..Default::default() };
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
}

#[test]
fn verifier_rejects_offset_type_and_constant_mode_independently() {
    let function = lift(&[ipa(0, 2), EXIT], Stage::Fragment, &[Register::Gpr(6)], &[]).function;
    for invalid_type in [false, true] {
        let mut bad = function.clone();
        let predicate = bad.value(Type::Predicate, Definition::Predicate(true));
        let operation = &mut bad
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.operation, Operation::Interpolate { .. }))
            .unwrap()
            .operation;
        let Operation::Interpolate { mode, location, .. } = operation else { unreachable!() };
        if invalid_type {
            *location = crate::ir::InterpolationLocation::Offset(predicate);
        } else {
            *mode = InterpolationMode::Constant;
        }
        assert_eq!(bad.verify().unwrap_err().kind, DiagnosticKind::InvalidIr);
    }
}
