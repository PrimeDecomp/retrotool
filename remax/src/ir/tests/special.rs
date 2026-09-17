use super::{bytes, lift, mov, EXIT};
use crate::{
    ir::{float::Operation as FloatOperation, Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, FloatModel, Inputs, InterpretOptions, Program, Register, Signature,
        SpecialFunctionModel, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 100,
    float_model: FloatModel::Reject,
    special_functions: SpecialFunctionModel::HostF32,
};

fn mufu(function: u8) -> u64 { 0x5080000000070100 | (u64::from(function) << 20) }

fn run(word: u64, argument: u32) -> u32 {
    let program = lift(&[word, EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)]);
    let inputs = Inputs {
        registers: [(Register::Gpr(1), Value::Word(argument))].into(),
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

#[test]
fn special_function_approximation_requires_independent_opt_in() {
    let program = lift(&[mufu(4), EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)]);
    let inputs = Inputs {
        registers: [(Register::Gpr(1), Value::Word(4.0f32.to_bits()))].into(),
        ..Default::default()
    };
    for float_model in [FloatModel::Reject, FloatModel::HostF32, FloatModel::SoftwareIeee] {
        let options = InterpretOptions {
            float_model,
            special_functions: SpecialFunctionModel::Reject,
            ..OPTIONS
        };
        let error = program.interpret_with_options(&inputs, options).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
    assert_eq!(
        program
            .interpret_with_options(&inputs, OPTIONS)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(0.25f32.to_bits())]
    );
}

#[test]
fn host_reference_matches_exact_values_in_each_supported_domain() {
    for (op, input, expected) in [
        (4, 4.0f32, 0.25f32),
        (4, -8.0, -0.125),
        (5, 16.0, 0.25),
        (5, 0.25, 2.0),
        (8, 16.0, 4.0),
        (8, 0.25, 0.5),
        (3, 16.0, 4.0),
        (3, 0.25, -2.0),
    ] {
        assert_eq!(run(mufu(op), input.to_bits()), expected.to_bits());
    }
}

#[test]
fn input_and_output_flushing_preserve_signs_around_zero() {
    for magnitude in [0, 1, 0x00400000, 0x007fffff] {
        for sign in [0, 0x80000000] {
            let input = magnitude | sign;
            assert_eq!(run(mufu(4), input), 0x7f800000 | sign);
            assert_eq!(run(mufu(5), input), 0x7f800000 | sign);
            assert_eq!(run(mufu(8), input), sign);
            assert_eq!(run(mufu(3), input), 0xff800000);
        }
    }
    // A normal input may produce a subnormal reciprocal, which is also flushed.
    for sign in [0, 0x80000000] {
        assert_eq!(run(mufu(4), 0x7f7fffff | sign), sign);
        assert_eq!(run(mufu(4), 0x7f800000 | sign), sign);
    }
    assert_eq!(run(mufu(8), 0x7f800000), 0x7f800000);
    assert_eq!(run(mufu(3), 0x7f800000), 0x7f800000);
}

#[test]
fn invalid_domains_produce_nan_without_assuming_its_payload() {
    for function in [3, 4, 5, 8] {
        for bits in [0x7f812345, 0x7fc12345, 0xff812345, 0xffc12345] {
            assert!(f32::from_bits(run(mufu(function), bits)).is_nan());
        }
    }
    for function in [3, 5, 8] {
        for bits in [0xbf800000, 0xff800000] {
            assert!(f32::from_bits(run(mufu(function), bits)).is_nan());
        }
    }
}

#[test]
fn modifiers_precede_function_and_saturation_follows_it() {
    assert_eq!(run(mufu(4) | (1 << 46), (-4.0f32).to_bits()), 0.25f32.to_bits());
    assert_eq!(run(mufu(4) | (1 << 46) | (1 << 48), (-4.0f32).to_bits()), (-0.25f32).to_bits());
    assert!(f32::from_bits(run(mufu(8) | (1 << 46) | (1 << 48), (-4.0f32).to_bits())).is_nan());
    for (function, input, expected) in [
        (4, 0.25f32.to_bits(), 1.0f32.to_bits()),
        (4, 4.0f32.to_bits(), 0.25f32.to_bits()),
        (4, (-4.0f32).to_bits(), 0),
        (4, 0x7fc12345, 0),
        (4, 0xff800000, 0),
        (8, (-4.0f32).to_bits(), 0),
        (3, 0, 0),
    ] {
        assert_eq!(run(mufu(function) | (1 << 50), input), expected);
    }
}

#[test]
fn predication_and_aliasing_preserve_the_incoming_source() {
    use Register::{Gpr as R, Predicate as P};
    let word = (mufu(4) | 1) & !(7 << 16); // @P0 MUFU.RCP R1,R1
    let program = lift(&[word, EXIT], &[R(1), P(0)], &[R(1)]);
    for enabled in [false, true] {
        let inputs = Inputs {
            registers: [(R(1), Value::Word(4.0f32.to_bits())), (P(0), Value::Predicate(enabled))]
                .into(),
            ..Default::default()
        };
        if !enabled {
            assert_eq!(
                program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
                [Value::Word(4.0f32.to_bits())]
            );
        }
        assert_eq!(
            program
                .interpret_with_options(&inputs, OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(if enabled { 0.25f32 } else { 4.0f32 }.to_bits())]
        );
    }
}

#[test]
fn basic_arithmetic_and_special_function_models_can_be_combined() {
    use Register::Gpr as R;
    // R0 = rcp(R1); R0 = R0 * R2. The two operations keep separate intent in SSA.
    let program = lift(&[mufu(4), 0x5c68000000270000, EXIT], &[R(1), R(2)], &[R(0)]);
    let inputs = Inputs {
        registers: [(R(1), Value::Word(4.0f32.to_bits())), (R(2), Value::Word(3.0f32.to_bits()))]
            .into(),
        ..Default::default()
    };
    assert_eq!(
        program.interpret_with_options(&inputs, OPTIONS).unwrap_err().kind,
        DiagnosticKind::Unsupported
    );
    let options = InterpretOptions { float_model: FloatModel::SoftwareIeee, ..OPTIONS };
    assert_eq!(
        program
            .interpret_with_options(&inputs, options)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(0.75f32.to_bits())]
    );
    assert!(program
        .function
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .any(|i| matches!(i.operation, Operation::Float(FloatOperation::Approximate { .. }))));
}

#[test]
fn bare_reduction_consumers_and_double_high_functions_do_not_become_host_math() {
    for word in
        [mufu(0), mufu(1), mufu(2), mufu(6), mufu(7), mufu(9), mufu(15), mufu(4) | (1 << 47)]
    {
        let error = Program::lift(
            MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
            &Signature::default(),
        )
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)), "{word:016x}");
    }
}

#[test]
fn verifier_checks_special_function_operand_types_independently() {
    let mut function = lift(&[mov(1, 0, 7), mufu(4), EXIT], &[], &[]).function;
    let predicate = function.value(Type::Predicate, Definition::Predicate(true));
    let operation = &mut function
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::Float(FloatOperation::Approximate { .. })))
        .unwrap()
        .operation;
    let Operation::Float(FloatOperation::Approximate { value, .. }) = operation else {
        unreachable!()
    };
    *value = predicate;
    assert!(function.verify().unwrap_err().message.contains("Bits32"));
}
