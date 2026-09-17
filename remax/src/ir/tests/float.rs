use super::{bytes, lift, mov, offset, EXIT};
use crate::{
    ir::{float::Operation as FloatOperation, Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, F2iNanBehavior, FloatModel, Inputs, InterpretOptions, Program, Register,
        Signature, SpecialFunctionModel, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: SpecialFunctionModel::Reject,
};
const FADD: u64 = 0x5c58000000270100; // R0 = R1 + R2
const FMUL: u64 = 0x5c68000000270100;
const FFMA: u64 = 0x5980018000270100; // R0 = R1 * R2 + R3

fn execute(words: &[u64], arguments: &[u32]) -> u32 {
    let registers = (1..=arguments.len()).map(|i| Register::Gpr(i as u8)).collect::<Vec<_>>();
    let program = lift(words, &registers, &[Register::Gpr(0)]);
    let inputs = Inputs {
        registers: registers.into_iter().zip(arguments.iter().copied().map(Value::Word)).collect(),
        ..Default::default()
    };
    let [Value::Word(result)] = program
        .interpret_with_options(&inputs, OPTIONS)
        .unwrap()
        .into_outputs()
        .expect("live invocation")[..]
    else {
        panic!()
    };
    result
}

fn f2i(signed: bool, round: u8) -> u64 {
    0x5cb0000000170a00 | (u64::from(signed) << 12) | (u64::from(round) << 39)
}

fn i2f(signed: bool, source_size_code: u8, round: u8) -> u64 {
    0x5cb8000000170200
        | (u64::from(signed) << 13)
        | (u64::from(source_size_code) << 10)
        | (u64::from(round) << 39)
}

#[test]
fn float_arithmetic_requires_the_selected_interpreter_model() {
    let program = lift(&[FADD, EXIT], &[Register::Gpr(1), Register::Gpr(2)], &[Register::Gpr(0)]);
    let inputs = Inputs {
        registers: [(Register::Gpr(1), Value::Word(0)), (Register::Gpr(2), Value::Word(0))].into(),
        ..Default::default()
    };
    for float_model in [FloatModel::Reject, FloatModel::HostF32] {
        let error = program
            .interpret_with_options(&inputs, InterpretOptions { float_model, ..OPTIONS })
            .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

#[test]
fn addition_and_multiplication_keep_all_four_rounding_modes() {
    for (round, positive, negative, product) in [
        (0, 0x3f800000, 0xbf800000, 0x3f800002),
        (1, 0x3f800000, 0xbf800001, 0x3f800002),
        (2, 0x3f800001, 0xbf800000, 0x3f800003),
        (3, 0x3f800000, 0xbf800000, 0x3f800002),
    ] {
        // 1 + 2^-24 is exactly halfway. (1+2^-23)^2 is just above two ULPs.
        assert_eq!(execute(&[FADD | (round << 39), EXIT], &[0x3f800000, 0x33800000]), positive);
        assert_eq!(execute(&[FADD | (round << 39), EXIT], &[0xbf800000, 0xb3800000]), negative);
        assert_eq!(execute(&[FMUL | (round << 39), EXIT], &[0x3f800001, 0x3f800001]), product);
    }
}

#[test]
fn fused_multiply_add_retains_the_single_rounding_point() {
    // (1+2^-23)*(1-2^-23)-1 = -2^-46; separate multiplication rounds to 1.
    assert_eq!(execute(&[FFMA, EXIT], &[0x3f800001, 0x3f7ffffe, 0xbf800000]), 0xa8800000);
    // The exact product exceeds F32 range but the fused result does not.
    assert_eq!(execute(&[FFMA, EXIT], &[0x7f7fffff, 0x40000000, 0xff7fffff]), 0x7f7fffff);
    for (round, expected) in [(0, 0), (1, 0x80000000), (2, 0), (3, 0)] {
        assert_eq!(
            execute(&[FFMA | (round << 51), EXIT], &[0x3f800000, 0x3f800000, 0xbf800000]),
            expected
        );
    }
}

#[test]
fn long_fma_reads_the_old_destination_before_writing_its_result() {
    // R0 starts at -1. FFMA32I computes R0 = R1 * 2 + old R0.
    let instruction = 0x0c00000000070100 | (u64::from(2.0f32.to_bits()) << 20);
    assert_eq!(
        execute(&[mov(0, (-1.0f32).to_bits(), 7), instruction, EXIT], &[3.0f32.to_bits()]),
        5.0f32.to_bits()
    );
}

#[test]
fn ftz_applies_to_inputs_and_results_and_preserves_the_sign() {
    assert_eq!(execute(&[FMUL, EXIT], &[0x00400001, 0x40000000]), 0x00800002);
    assert_eq!(execute(&[FMUL | (1 << 44), EXIT], &[0x00400001, 0x40000000]), 0);
    for (source, small, zero) in [(0x00800000, 0x00400000, 0), (0x80800000, 0x80400000, 0x80000000)]
    {
        assert_eq!(execute(&[FMUL, EXIT], &[source, 0x3f000000]), small);
        assert_eq!(execute(&[FMUL | (1 << 44), EXIT], &[source, 0x3f000000]), zero);
    }
    // Absolute precedes negate, then FTZ. Both modifiers on -2 produce -2.
    assert_eq!(
        execute(&[FADD | (1 << 46) | (1 << 48), EXIT], &[0xc0000000, 0x3f800000]),
        0xbf800000
    );
}

#[test]
fn software_saturation_policy_is_explicit_for_nan_zero_and_infinity() {
    for (a, b, expected) in [
        (0x40000000, 0, 0x3f800000),
        (0xbf800000, 0, 0),
        (0x7f800000, 0, 0x3f800000),
        (0x7fc12345, 0, 0),
        (0x80000000, 0x80000000, 0),
        (0x3e800000, 0, 0x3e800000),
    ] {
        assert_eq!(execute(&[FADD | (1 << 50), EXIT], &[a, b]), expected);
    }
}

#[test]
fn float_to_integer_rounds_and_clips_without_host_casts() {
    for (signed, round, value, expected) in [
        (true, 0, 0.5f32, 0),
        (true, 0, 1.5, 2),
        (true, 0, 2.5, 2),
        (true, 0, -2.5, (-2i32) as u32),
        (true, 1, -2.25, (-3i32) as u32),
        (true, 2, -2.25, (-2i32) as u32),
        (true, 3, -2.75, (-2i32) as u32),
        (true, 0, 2147483648.0, 0x7fffffff),
        (true, 0, f32::NEG_INFINITY, 0x80000000),
        (false, 0, f32::INFINITY, u32::MAX),
        (false, 0, -1.0, 0),
    ] {
        assert_eq!(execute(&[f2i(signed, round), EXIT], &[value.to_bits()]), expected);
    }
    assert_eq!(execute(&[f2i(true, 2), EXIT], &[1]), 1);
    assert_eq!(execute(&[f2i(true, 2) | (1 << 44), EXIT], &[1]), 0);
    assert_eq!(execute(&[f2i(true, 1), EXIT], &[0x80000001]), u32::MAX);
}

#[test]
fn integer_to_float_respects_signed_width_and_directed_rounding() {
    for (signed, size, word, round, expected) in [
        (true, 0, 0xdeadbeff, 0, 0xbf800000),
        (false, 0, 0xdeadbeff, 0, 0x437f0000),
        (true, 1, 0xdead8000, 0, 0xc7000000),
        (false, 1, 0xdeadffff, 0, 0x477fff00),
        (false, 2, 0x01000001, 0, 0x4b800000),
        (false, 2, 0x01000001, 2, 0x4b800001),
        (true, 2, 0xfeffffff, 1, 0xcb800001),
        (true, 2, 0xfeffffff, 2, 0xcb800000),
        (false, 2, u32::MAX, 0, 0x4f800000),
        (false, 2, u32::MAX, 3, 0x4f7fffff),
    ] {
        assert_eq!(execute(&[i2f(signed, size, round), EXIT], &[word]), expected);
    }
}

#[test]
fn nan_conversion_requires_shader_control_only_on_the_executed_path() {
    use Register::{Gpr as R, Predicate as P};
    let words = [mov(0, 17, 7), f2i(true, 0) & !(7 << 16), EXIT];
    let program = lift(&words, &[R(1), P(0)], &[R(0)]);
    let mut inputs = Inputs {
        registers: [(R(1), Value::Word(0x7fc12345)), (P(0), Value::Predicate(false))].into(),
        ..Default::default()
    };
    assert_eq!(
        program
            .interpret_with_options(&inputs, OPTIONS)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(17)]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    let error = program.interpret_with_options(&inputs, OPTIONS).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(1))));
    for (policy, expected) in [(F2iNanBehavior::Zero, 0), (F2iNanBehavior::Indefinite, 0x80000000)]
    {
        inputs.f2i_nan_behavior = Some(policy);
        assert_eq!(
            program
                .interpret_with_options(&inputs, OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn float_predicates_distinguish_ordered_unordered_and_bit_patterns() {
    use Register::{Gpr as R, Predicate as P};
    for relation in 0..16u64 {
        // P0,P1 = (R1 comparison R2) AND PT, and its complement.
        let word = 0x5bb0038000270101 | (relation << 48);
        let program = lift(&[word, EXIT], &[R(1), R(2)], &[P(0), P(1)]);
        for (a, b, expect) in [
            (0x80000000, 0, matches!(relation, 2 | 3 | 6 | 7 | 10 | 11 | 14 | 15)),
            (1, 0, matches!(relation, 4 | 5 | 6 | 7 | 12 | 13 | 14 | 15)),
            (0x7fc12345, 0, relation >= 8),
            (0x7f800001, 0x7fc12345, relation >= 8),
        ] {
            let inputs = Inputs {
                registers: [(R(1), Value::Word(a)), (R(2), Value::Word(b))].into(),
                ..Default::default()
            };
            assert_eq!(
                program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
                [Value::Predicate(expect), Value::Predicate(!expect)],
                "relation={relation}, {a:08x}, {b:08x}"
            );
        }
    }
    // @P0 FSETP.LT.FTZ P0,P1,R1,R2,P0 reads the old guard and accumulator.
    let word = 0x5bb1000000200101 | (1 << 47);
    let program = lift(&[word, EXIT], &[R(1), R(2), P(0), P(1)], &[P(0), P(1)]);
    let inputs = Inputs {
        registers: [
            (R(1), Value::Word(0x80000001)),
            (R(2), Value::Word(0)),
            (P(0), Value::Predicate(true)),
            (P(1), Value::Predicate(false)),
        ]
        .into(),
        ..Default::default()
    };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Predicate(false), Value::Predicate(true)]
    );
}

#[test]
fn numeric_modes_without_established_semantics_remain_unsupported() {
    for word in [
        FMUL | (1 << 45),
        FMUL | (7 << 41),
        FFMA | (1 << 54),
        FADD | (1 << 47),
        f2i(true, 0) | (1 << 41),
        i2f(false, 2, 0) | (1 << 49),
        i2f(false, 3, 0),
    ] {
        let error = Program::lift(
            MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
            &Signature::default(),
        )
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

#[test]
fn verifier_rejects_invalid_numeric_widths_and_operand_types() {
    let function =
        lift(&[i2f(false, 2, 0), EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)]).function;
    let mut bad = function.clone();
    let Operation::Float(FloatOperation::FromInteger { bits, .. }) =
        &mut bad.blocks[1].instructions[0].operation
    else {
        panic!()
    };
    *bits = 24;
    assert!(bad.verify().unwrap_err().message.contains("source width"));
    let mut bad = function;
    let wrong_type = bad.value(Type::Predicate, Definition::Predicate(true));
    let Operation::Float(FloatOperation::FromInteger { value, .. }) =
        &mut bad.blocks[1].instructions[0].operation
    else {
        panic!()
    };
    *value = wrong_type;
    assert!(bad.verify().unwrap_err().message.contains("Bits32"));
}
