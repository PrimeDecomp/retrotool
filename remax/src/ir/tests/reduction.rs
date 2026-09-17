use Register::{Gpr as R, Predicate as P};

use super::{add, branch, bytes, isetp, lift, mov, offset, EXIT};
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

fn rro(dst: u8, src: u8) -> u64 { 0x5c90008000070000 | (u64::from(src) << 20) | u64::from(dst) }
fn exp2(dst: u8, src: u8) -> u64 { 0x5080000000270000 | (u64::from(src) << 8) | u64::from(dst) }
fn copy(dst: u8, src: u8) -> u64 { 0x5c98078000070000 | (u64::from(src) << 20) | u64::from(dst) }
fn inputs(x: f32) -> Inputs {
    Inputs { registers: [(R(0), Value::Word(x.to_bits()))].into(), ..Default::default() }
}

#[test]
fn paired_exponentiation_requires_its_own_numeric_policy() {
    let p = lift(&[rro(1, 0), exp2(2, 1), EXIT], &[R(0)], &[R(2)]);
    let e = p.interpret(&inputs(3.), 100).unwrap_err();
    assert_eq!((e.kind, e.pc), (DiagnosticKind::Unsupported, Some(offset(1))));
    let dead = lift(&[rro(1, 0), EXIT], &[R(0)], &[]);
    assert!(dead
        .interpret(&Inputs::default(), 100)
        .unwrap()
        .into_outputs()
        .expect("live invocation")
        .is_empty());
    for (x, expected) in [
        (-126., f32::MIN_POSITIVE),
        (-127., 0.),
        (-150., 0.),
        (-3., 0.125),
        (-0., 1.),
        (0., 1.),
        (1., 2.),
        (3., 8.),
        (127., 2f32.powi(127)),
        (128., f32::INFINITY),
        (f32::NEG_INFINITY, 0.),
        (f32::INFINITY, f32::INFINITY),
    ] {
        assert_eq!(
            p.interpret_with_options(&inputs(x), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected.to_bits())]
        );
    }
    let [Value::Word(nan)] = p
        .interpret_with_options(&inputs(f32::NAN), OPTIONS)
        .unwrap()
        .into_outputs()
        .expect("live invocation")[..]
    else {
        panic!()
    };
    assert!(f32::from_bits(nan).is_nan());
}

#[test]
fn opaque_copies_preserve_producer_modifiers_and_capture_before_overwrites() {
    let p = lift(
        &[
            rro(1, 0) | (1 << 49) | (1 << 45), // RRO.EX2 R1,-abs(R0)
            copy(2, 1),
            mov(0, 99f32.to_bits(), 7),
            mov(1, 0, 7),
            exp2(3, 2),
            exp2(4, 2) | (1 << 50),
            EXIT,
        ],
        &[R(0)],
        &[R(3), R(4)],
    );
    for x in [-3., 3.] {
        assert_eq!(
            p.interpret_with_options(&inputs(x), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(0.125f32.to_bits()); 2]
        );
    }
    let p = lift(&[rro(1, 0), exp2(2, 1) | (1 << 50), EXIT], &[R(0)], &[R(2)]);
    for (x, expected) in [(2., 1f32), (-2., 0.25), (f32::NAN, 0.)] {
        assert_eq!(
            p.interpret_with_options(&inputs(x), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected.to_bits())]
        );
    }
}

#[test]
fn complementary_consumers_can_overwrite_their_shared_reduction_register() {
    let words = [rro(1, 0), exp2(1, 1) & !(7 << 16), exp2(1, 1) & !(7 << 16) | (8 << 16), EXIT];
    let program = lift(&words, &[R(0), P(0)], &[R(1)]);
    for enabled in [false, true] {
        for (argument, expected) in [(-3., 0.125f32), (0., 1.), (3., 8.)] {
            let mut inputs = inputs(argument);
            inputs.registers.insert(P(0), Value::Predicate(enabled));
            assert_eq!(
                program.interpret_with_options(&inputs, OPTIONS).unwrap().into_outputs().unwrap(),
                [Value::Word(expected.to_bits())]
            );
            inputs.registers.remove(&R(0));
            let error = program.interpret_with_options(&inputs, OPTIONS).unwrap_err();
            assert_eq!(error.kind, DiagnosticKind::MissingInput);
            // Pure arithmetic propagates unavailable input until the export.
            assert_eq!(error.pc, Some(offset(3)));
        }
    }
    let mut different = words;
    different[2] |= 1 << 16;
    assert!(Program::lift(MaxwellCode::parse(&bytes(&different)).unwrap(), &Signature {
        inputs: vec![R(0), P(0), P(1)],
        outputs: vec![R(1)],
        ..Default::default()
    })
    .is_err());
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> { transport_cases(rro, exp2) }

pub(super) fn transport_cases(
    reduce: fn(u8, u8) -> u64,
    consume: fn(u8, u8) -> u64,
) -> Vec<(Program, Vec<Inputs>)> {
    let mut result = Vec::new();
    // Both paths produce a reduced value, so the join transports opaque payloads.
    let p = lift(
        &[reduce(2, 0), reduce(2, 1) & !(7 << 16), copy(3, 2), consume(4, 3), EXIT],
        &[R(0), R(1), P(0)],
        &[R(4)],
    );
    let cases = (-20..=20)
        .flat_map(|x| {
            [false, true].map(move |enabled| Inputs {
                registers: [
                    (R(0), Value::Word((x as f32).to_bits())),
                    (R(1), Value::Word((-x as f32).to_bits())),
                    (P(0), Value::Predicate(enabled)),
                ]
                .into(),
                ..Default::default()
            })
        })
        .collect();
    result.push((p, cases));
    // Two distinct reduced values travel around a backedge and exchange places.
    let p = lift(
        &[
            reduce(2, 0),
            reduce(3, 1),
            copy(6, 2),
            copy(2, 3),
            copy(3, 6),
            add(4, 4, 1),
            isetp(0, 7, 4, 5, 1, 7, 7),
            branch(7, 2, 0),
            consume(6, 2),
            consume(7, 3),
            EXIT,
        ],
        &[R(0), R(1), R(4), R(5)],
        &[R(6), R(7)],
    );
    let cases = (1..=16)
        .map(|count| Inputs {
            registers: [
                (R(0), Value::Word(2f32.to_bits())),
                (R(1), Value::Word((-3f32).to_bits())),
                (R(4), Value::Word(0)),
                (R(5), Value::Word(count)),
            ]
            .into(),
            ..Default::default()
        })
        .collect();
    result.push((p, cases));
    // A later if-conversion pass may use a typed select instead of a branch.
    // Exercise that representation directly, including the backend's struct
    // payload handling, without claiming Maxwell SEL decoding support.
    let mut p =
        lift(&[reduce(2, 0), reduce(3, 1), copy(4, 2), consume(5, 4), EXIT], &[R(0), R(1)], &[R(
            5,
        )]);
    let condition = p.function.value(Type::Predicate, Definition::Input(P(0)));
    let values = p
        .function
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter(|i| matches!(i.operation, Operation::Reduce { .. }))
        .map(|i| i.result.unwrap())
        .collect::<Vec<_>>();
    let copy = p
        .function
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::Copy(_)))
        .unwrap();
    copy.operation = Operation::Select { condition, yes: values[1], no: values[0] };
    p.function.verify().unwrap();
    let cases = [false, true]
        .map(|enabled| Inputs {
            registers: [
                (R(0), Value::Word(2f32.to_bits())),
                (R(1), Value::Word((-3f32).to_bits())),
                (P(0), Value::Predicate(enabled)),
            ]
            .into(),
            ..Default::default()
        })
        .to_vec();
    result.push((p, cases));
    result
}

#[test]
fn predicated_consumers_and_missing_arguments_keep_path_sensitive_validity() {
    let p = lift(&[rro(1, 0), exp2(2, 1) & !(7 << 16), EXIT], &[R(0), R(2), P(0)], &[R(2)]);
    let mut input = Inputs {
        registers: [(P(0), Value::Predicate(false)), (R(2), Value::Word(0x12345678))].into(),
        ..Default::default()
    };
    // No source and no approximation policy are needed on the untaken consumer.
    assert_eq!(p.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(0x12345678)
    ]);
    input.registers.insert(P(0), Value::Predicate(true));
    let error = p.interpret_with_options(&input, OPTIONS).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(2))));
    input.registers.insert(R(0), Value::Word(3f32.to_bits()));
    assert_eq!(
        p.interpret_with_options(&input, OPTIONS).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(8f32.to_bits())]
    );
}

#[test]
fn reduced_values_survive_joins_and_loop_carried_parallel_transfers() {
    for (p, cases) in comparison_cases() {
        assert!(p.function.values.iter().any(|v| v.ty == Type::Reduced(float::Reduction::Exp2)));
        for input in cases {
            let expected = if p.function.outputs.len() == 1 {
                let selected =
                    if input.registers[&P(0)] == Value::Predicate(true) { R(1) } else { R(0) };
                let Value::Word(x) = input.registers[&selected] else { panic!() };
                vec![f32::from_bits(x).exp2()]
            } else {
                let Value::Word(count) = input.registers[&R(5)] else { panic!() };
                if count % 2 == 0 {
                    vec![4., 0.125]
                } else {
                    vec![0.125, 4.]
                }
            };
            assert_eq!(
                p.interpret_with_options(&input, OPTIONS)
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation"),
                expected.into_iter().map(|x| Value::Word(x.to_bits())).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn raw_observations_modifiers_and_mixed_paths_fail_closed_at_the_original_pc() {
    for (words, outputs, bad) in [
        (vec![rro(1, 0), EXIT], vec![R(1)], 1),
        (vec![rro(1, 0), copy(2, 1), EXIT], vec![R(2)], 2),
        (vec![rro(1, 0), add(2, 1, 1), EXIT], vec![], 1),
        (vec![rro(1, 0), exp2(2, 1) | (1 << 46), EXIT], vec![], 1),
        (vec![rro(1, 0), exp2(2, 1) | (1 << 48), EXIT], vec![], 1),
        (vec![rro(1, 0), exp2(2, 1) + (2 << 20), EXIT], vec![], 1), // RCP
        (vec![rro(1, 0), rro(2, 1), EXIT], vec![], 1),
        (vec![rro(1, 0), mov(1, 2f32.to_bits(), 7), exp2(2, 1), EXIT], vec![], 2),
        (vec![rro(1, 0) & !(7 << 16), exp2(2, 1), EXIT], vec![], 1),
    ] {
        let e = Program::lift(MaxwellCode::parse(&bytes(&words)).unwrap(), &Signature {
            indexed_attributes: vec![],
            inputs: vec![R(0), P(0)],
            outputs,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!((e.kind, e.pc), (DiagnosticKind::Unsupported, Some(offset(bad))), "{words:x?}");
    }
}

#[test]
fn verifier_independently_checks_opaque_consumer_and_transport_types() {
    let p = lift(&[rro(1, 0), copy(3, 1), exp2(2, 3), EXIT], &[R(0)], &[R(2)]);
    let mut f = p.function.clone();
    let ordinary = f.value(Type::Bits32, Definition::Word(0));
    for i in f.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
        if let Operation::Float(float::Operation::Reduced { value, .. }) = &mut i.operation {
            *value = ordinary;
        }
    }
    assert!(f.verify().unwrap_err().message.contains("Reduced(Exp2)"));
    let mut f = p.function;
    let result = f
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find(|i| matches!(i.operation, Operation::Copy(_)))
        .unwrap()
        .result
        .unwrap();
    f.values[result.0].ty = Type::Bits32;
    assert!(f.verify().is_err());
}
