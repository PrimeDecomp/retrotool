use Register::{Gpr as R, Predicate as P};

use super::{add, branch, bytes, isetp, lift, mov, offset, reduction, EXIT};
use crate::{
    ir::{float, Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, Inputs, InterpretOptions, Program, Register, Signature,
        SpecialFunctionModel, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 10000,
    special_functions: SpecialFunctionModel::HostF32,
    float_model: crate::program::FloatModel::Reject,
};

pub(super) mod original;

fn reduce(destination: u8, source: u8) -> u64 {
    0x5c90000000070000 | (u64::from(source) << 20) | u64::from(destination)
}

fn sin(destination: u8, source: u8) -> u64 {
    0x5080000000170000 | (u64::from(source) << 8) | u64::from(destination)
}

fn cos(destination: u8, source: u8) -> u64 {
    0x5080000000070000 | (u64::from(source) << 8) | u64::from(destination)
}

fn copy(destination: u8, source: u8) -> u64 {
    0x5c98078000070000 | (u64::from(source) << 20) | u64::from(destination)
}

fn inputs(argument: f32) -> Inputs {
    Inputs { registers: [(R(0), Value::Word(argument.to_bits()))].into(), ..Default::default() }
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    reduction::transport_cases(reduce, sin)
}

#[test]
fn one_reduction_feeds_both_functions_after_source_and_producer_overwrites() {
    let program = lift(
        &[
            reduce(1, 0) | (1 << 49) | (1 << 45), // -abs(argument), before reduction
            copy(2, 1),
            mov(0, 99f32.to_bits(), 7),
            mov(1, 0, 7),
            sin(3, 2),
            cos(4, 2),
            sin(5, 2) | (1 << 50),
            cos(6, 2) | (1 << 50),
            EXIT,
        ],
        &[R(0)],
        &[R(3), R(4), R(5), R(6)],
    );
    let error = program.interpret(&inputs(1.), 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(offset(4))));
    for argument in [-7f32, -3., -1., 0., 1., 3., 7., 1000.] {
        let sine = (-argument.abs()).sin();
        let cosine = (-argument.abs()).cos();
        let expected =
            [sine, cosine, sine.max(0.), cosine.max(0.)].map(|value| Value::Word(value.to_bits()));
        assert_eq!(
            program
                .interpret_with_options(&inputs(argument), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            expected
        );
    }
}

#[test]
fn host_model_preserves_radians_and_applies_result_flushing_and_saturation() {
    let program =
        lift(&[reduce(1, 0), sin(2, 1), cos(3, 1), sin(4, 1) | (1 << 50), EXIT], &[R(0)], &[
            R(2),
            R(3),
            R(4),
        ]);
    for (argument, expected) in [
        (0f32, [0f32, 1., 0.]),
        (-0., [-0., 1., 0.]),
        (f32::from_bits(1), [0., 1., 0.]),
        (-f32::from_bits(1), [-0., 1., 0.]),
        (1., [1f32.sin(), 1f32.cos(), 1f32.sin()]),
    ] {
        assert_eq!(
            program
                .interpret_with_options(&inputs(argument), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            expected.map(|value| Value::Word(value.to_bits())),
        );
    }
    for argument in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let result = program
            .interpret_with_options(&inputs(argument), OPTIONS)
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        for value in &result[..2] {
            let Value::Word(word) = value else { panic!() };
            assert!(f32::from_bits(*word).is_nan());
        }
        assert_eq!(result[2], Value::Word(0));
    }
    let dead = lift(&[reduce(1, 0), EXIT], &[R(0)], &[]);
    assert!(dead
        .interpret(&Inputs::default(), 100)
        .unwrap()
        .into_outputs()
        .expect("live invocation")
        .is_empty());
}

#[test]
fn sine_arguments_follow_selected_paths_and_parallel_loop_transfers() {
    for (program, cases) in comparison_cases() {
        for input in cases {
            let arguments = if program.function.outputs.len() == 1 {
                let selected =
                    if input.registers[&P(0)] == Value::Predicate(true) { R(1) } else { R(0) };
                let Value::Word(word) = input.registers[&selected] else { panic!() };
                vec![f32::from_bits(word)]
            } else {
                let Value::Word(count) = input.registers[&R(5)] else { panic!() };
                if count % 2 == 0 {
                    vec![2f32, -3.]
                } else {
                    vec![-3f32, 2.]
                }
            };
            let expected = arguments
                .into_iter()
                .map(|argument| Value::Word(argument.sin().to_bits()))
                .collect::<Vec<_>>();
            assert_eq!(
                program
                    .interpret_with_options(&input, OPTIONS)
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation"),
                expected
            );
        }
    }
}

#[test]
fn untaken_producers_and_consumers_do_not_observe_missing_inputs() {
    let program = lift(
        &[reduce(2, 0), reduce(2, 1) & !(7 << 16), sin(3, 2) & !(7 << 16), EXIT],
        &[R(0), R(1), R(3), P(0)],
        &[R(3)],
    );
    let mut input = Inputs {
        registers: [(P(0), Value::Predicate(false)), (R(3), Value::Word(42))].into(),
        ..Default::default()
    };
    assert_eq!(program.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(42)
    ]);
    input.registers.insert(P(0), Value::Predicate(true));
    let error = program.interpret_with_options(&input, OPTIONS).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(3))));
    input.registers.insert(R(1), Value::Word(1f32.to_bits()));
    assert_eq!(
        program
            .interpret_with_options(&input, OPTIONS)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(1f32.sin().to_bits())]
    );
}

#[test]
fn raw_observations_consumer_modifiers_and_cross_domain_uses_are_rejected() {
    let exponent_reduction = reduce(1, 0) | (1 << 39);
    let exponent_consumer = sin(2, 1) + (1 << 20);
    for (words, outputs, bad) in [
        (vec![sin(1, 0), EXIT], vec![], 0),
        (vec![cos(1, 0), EXIT], vec![], 0),
        (vec![reduce(1, 0), EXIT], vec![R(1)], 1),
        (vec![reduce(1, 0), add(2, 1, 1), EXIT], vec![], 1),
        (vec![reduce(1, 0), sin(2, 1) | (1 << 46), EXIT], vec![], 1),
        (vec![reduce(1, 0), cos(2, 1) | (1 << 48), EXIT], vec![], 1),
        (vec![exponent_reduction, sin(2, 1), EXIT], vec![], 1),
        (vec![reduce(1, 0), exponent_consumer, EXIT], vec![], 1),
        (vec![reduce(1, 0) & !(7 << 16), sin(2, 1), EXIT], vec![], 1),
        (vec![exponent_reduction, reduce(1, 0) & !(7 << 16), sin(2, 1), EXIT], vec![], 2),
        // The loop header first receives SinCos; the backedge sends Exp2.
        (
            vec![
                reduce(1, 0),
                copy(2, 1),
                exponent_reduction,
                add(4, 4, 1),
                isetp(0, 7, 4, 5, 1, 7, 7),
                branch(5, 1, 0),
                sin(3, 2),
                EXIT,
            ],
            vec![],
            1,
        ),
    ] {
        let error = Program::lift(MaxwellCode::parse(&bytes(&words)).unwrap(), &Signature {
            indexed_attributes: vec![],
            inputs: vec![R(0), R(4), R(5), P(0)],
            outputs,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(
            (error.kind, error.pc),
            (DiagnosticKind::Unsupported, Some(offset(bad))),
            "{words:x?}"
        );
    }
}

#[test]
fn verifier_rejects_mixed_domains_in_consumers_and_selects() {
    let program =
        lift(&[reduce(1, 0), reduce(4, 0) | (1 << 39), copy(2, 1), sin(3, 2), EXIT], &[R(0)], &[
            R(3),
        ]);
    let mut function = program.function.clone();
    for instruction in function.blocks.iter_mut().flat_map(|block| &mut block.instructions) {
        if let Operation::Float(float::Operation::Reduced { function, .. }) =
            &mut instruction.operation
        {
            *function = float::ReducedFunction::Exp2;
        }
    }
    assert!(function.verify().unwrap_err().message.contains("Reduced(Exp2)"));

    let mut function = program.function;
    let condition = function.value(Type::Predicate, Definition::Predicate(true));
    let other = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find(|instruction| {
            matches!(instruction.operation, Operation::Reduce { kind: float::Reduction::Exp2, .. })
        })
        .unwrap()
        .result
        .unwrap();
    let instruction = function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find(|instruction| matches!(instruction.operation, Operation::Copy(_)))
        .unwrap();
    let Operation::Copy(value) = instruction.operation else { panic!() };
    instruction.operation = Operation::Select { condition, yes: value, no: other };
    assert!(function.verify().is_err());
}
