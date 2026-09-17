use Register::{Gpr as R, Predicate as P};

use super::{add, branch, bytes, isetp, mov, offset, EXIT};
use crate::{
    ir::{Operation, Type},
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Value},
};

pub(super) mod promotion;
mod reference;

pub(super) fn access(store: bool, register: u8, address: u8, displacement: i32, ty: u8) -> u64 {
    let opcode = if store { 0xef50000000000000 } else { 0xef40000000000000 };
    opcode
        | u64::from(register)
        | (u64::from(address) << 8)
        | (7 << 16)
        | ((displacement as u64 & 0xffffff) << 20)
        | (u64::from(ty) << 48)
}

pub(super) fn lift(
    words: &[u64],
    frame: u32,
    inputs: &[Register],
    outputs: &[Register],
) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: frame,
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
        ..Default::default()
    })
    .unwrap()
}

fn input(registers: &[(Register, Value)]) -> Inputs {
    Inputs { registers: registers.iter().copied().collect(), ..Default::default() }
}

#[test]
fn local_spills_round_trip_and_wide_loads_capture_the_address_before_writes() {
    let program = lift(
        &[access(true, 8, 255, 16, 6), access(false, 4, 4, -16, 6), EXIT],
        32,
        &[R(4), R(8), R(9), R(10), R(11)],
        &[R(4), R(5), R(6), R(7)],
    );
    let expected = [0x7fc12345, 0x80000000, 0xffffffff, 0x12345678];
    let mut inputs = input(&[(R(4), Value::Word(32))]);
    inputs.registers.extend(
        expected.into_iter().enumerate().map(|(i, word)| (R(8 + i as u8), Value::Word(word))),
    );
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        expected.map(Value::Word)
    );
    let store = program
        .function
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find(|i| matches!(i.operation, Operation::LocalStore(_)))
        .unwrap();
    assert_eq!(store.result, None);
}

#[test]
fn local_subword_aliases_preserve_neighbors_and_sign_extension() {
    let program = lift(
        &[
            mov(4, 0x12345678, 7),
            access(true, 4, 255, 0, 4),
            mov(5, 0xaa, 7),
            access(true, 5, 255, 1, 0),
            mov(5, 0xff80, 7),
            access(true, 5, 255, 2, 2),
            access(false, 0, 255, 0, 4),
            access(false, 1, 255, 1, 1),
            access(false, 2, 255, 2, 3),
            EXIT,
        ],
        16,
        &[],
        &[R(0), R(1), R(2)],
    );
    assert_eq!(
        program
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(0xff80aa78), Value::Word(0xffffffaa), Value::Word(0xffffff80),]
    );
}

#[test]
fn local_availability_follows_bytes_and_unknown_stores_overwrite_known_bytes() {
    let partial = [mov(4, 0x1234, 7), access(true, 4, 255, 2, 2)];
    let mut words = partial.to_vec();
    words.extend([access(false, 0, 255, 2, 2), EXIT]);
    assert_eq!(
        lift(&words, 16, &[], &[R(0)])
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(0x1234)]
    );
    words[2] = access(false, 0, 255, 0, 4);
    let error = lift(&words, 16, &[], &[R(0)]).interpret(&Inputs::default(), 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::UnavailableValue, Some(offset(3))));

    let program = lift(
        &[
            mov(4, 7, 7),
            access(true, 4, 255, 0, 4),
            access(true, 9, 255, 1, 0),
            access(false, 0, 255, 0, 4),
            EXIT,
        ],
        16,
        &[],
        &[R(0)],
    );
    assert_eq!(
        program.interpret(&Inputs::default(), 100).unwrap_err().kind,
        DiagnosticKind::UnavailableValue
    );

    // An uninitialized load is harmless until its value is observed. Addresses
    // and memory bounds still have to be valid even for discarded load results.
    let program = lift(&[access(false, 255, 255, 0, 4), EXIT], 16, &[], &[]);
    assert!(program.interpret(&Inputs::default(), 100).is_ok());
    let program = lift(&[access(false, 255, 255, 16, 4), EXIT], 16, &[], &[]);
    assert_eq!(program.interpret(&Inputs::default(), 100).unwrap_err().pc, Some(offset(0)));
}

#[test]
fn local_wide_load_does_not_observe_unrequested_unknown_components() {
    let program = lift(
        &[access(true, 4, 255, 0, 6), access(false, 0, 255, 0, 6), EXIT],
        16,
        &[R(4), R(5)],
        &[R(0), R(1)],
    );
    let inputs = input(&[(R(4), Value::Word(0x12345678)), (R(5), Value::Word(0xabcdef01))]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(0x12345678), Value::Word(0xabcdef01),]
    );
}

#[test]
fn local_effects_respect_predication_and_frames_do_not_survive_calls() {
    let store = access(true, 4, 255, 0, 4) & !(7 << 16);
    let program = lift(&[store, access(false, 0, 255, 0, 4), EXIT], 16, &[R(4), P(0)], &[R(0)]);
    let mut inputs = input(&[(R(4), Value::Word(29)), (P(0), Value::Predicate(true))]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(29)]
    );
    inputs.registers.insert(P(0), Value::Predicate(false));
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
    // A skipped effect must not observe its absent address or data.
    let program = lift(&[access(true, 4, 8, 0, 4) & !(7 << 16), EXIT], 16, &[P(0), R(8)], &[]);
    assert!(program.interpret(&input(&[(P(0), Value::Predicate(false))]), 100).is_ok());
}

fn loop_program() -> Program {
    lift(
        &[
            mov(0, 0, 7),
            access(true, 0, 255, 0, 4),
            mov(1, 0, 7),
            access(false, 2, 255, 0, 4),
            add(2, 2, 3),
            access(true, 2, 255, 0, 4),
            add(1, 1, 1),
            isetp(0, 7, 1, 3, 1, 7, 7),
            branch(8, 3, 0),
            access(false, 0, 255, 0, 4),
            EXIT,
        ],
        16,
        &[R(3)],
        &[R(0)],
    )
}

#[test]
fn local_loop_reads_the_most_recent_executed_write() {
    for iterations in 1..8 {
        assert_eq!(
            loop_program()
                .interpret(&input(&[(R(3), Value::Word(iterations))]), 1000)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(iterations * 3)]
        );
    }
}

#[test]
fn verifier_rejects_store_results_and_nonword_memory_operands() {
    let program = lift(&[access(true, 4, 255, 0, 4), EXIT], 16, &[R(4)], &[]);
    let mut bad = program.function.clone();
    let instruction = bad
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::LocalStore(_)))
        .unwrap();
    instruction.result = Some(crate::ir::ValueId(0));
    assert!(bad.verify().unwrap_err().message.contains("local-store effect"));
    let mut bad = program.function;
    let value = bad
        .values
        .iter_mut()
        .find(|value| matches!(value.definition, crate::ir::Definition::Input(_)))
        .unwrap();
    value.ty = Type::Predicate;
    assert!(bad.verify().is_err());
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut programs = Vec::new();
    // Host expectations also cover each width independently of emission.
    for ty in 0..7 {
        let outputs = (0..if ty == 6 {
            4
        } else if ty == 5 {
            2
        } else {
            1
        })
            .map(R)
            .collect::<Vec<_>>();
        let program = lift(
            &[access(true, 4, 8, 0, 6), access(false, 0, 9, 0, ty), EXIT],
            32,
            &[R(4), R(5), R(6), R(7), R(8), R(9)],
            &outputs,
        );
        let cases = (0..40)
            .map(|address| {
                input(&[
                    (R(4), Value::Word(0xb4a39281)),
                    (R(5), Value::Word(0xf8e7d6c5)),
                    (R(6), Value::Word(0x01020304)),
                    (R(7), Value::Word(0xffeeddcc)),
                    (R(8), Value::Word(0)),
                    (R(9), Value::Word(address)),
                ])
            })
            .collect();
        programs.push((program, cases));
    }
    // A runtime byte/halfword overwrite must preserve neighboring bytes, while
    // an unavailable payload invalidates only the overwritten portion.
    for ty in [0, 2] {
        let program = lift(
            &[
                access(true, 4, 255, 0, 4),
                access(true, 5, 8, 0, ty),
                access(false, 0, 255, 0, 4),
                access(false, 1, 255, 0, 0),
                access(false, 2, 255, 3, 1),
                EXIT,
            ],
            4,
            &[R(4), R(5), R(8)],
            &[R(0), R(1), R(2)],
        );
        let cases = (0..16)
            .map(|case| {
                let mut inputs = input(&[
                    (R(4), Value::Word(0x12345678)),
                    (R(5), Value::Word(0xabcdff80)),
                    (R(8), Value::Word(case % 8)),
                ]);
                if case >= 8 {
                    inputs.registers.remove(&R(5));
                }
                inputs
            })
            .collect();
        programs.push((program, cases));
    }
    let program = lift(
        &[
            access(true, 4, 255, 0, 4),
            access(true, 5, 255, 1, 0),
            access(false, 0, 255, 0, 0),
            access(false, 1, 255, 2, 2),
            EXIT,
        ],
        4,
        &[R(4)],
        &[R(0), R(1)],
    );
    programs.push((program, vec![input(&[(R(4), Value::Word(0x12345678))])]));
    // Exercise effect-only kernels and wrapping relative addresses as well as
    // a discarded unavailable load. Invalid accesses still report a fault.
    for store in [false, true] {
        let program = lift(&[access(store, 255, 8, -4, 4), EXIT], 16, &[R(8)], &[]);
        let cases = [0, 1, 4, 8, 16, 20, u32::MAX]
            .into_iter()
            .map(|address| input(&[(R(8), Value::Word(address))]))
            .collect();
        programs.push((program, cases));
    }
    let program = lift(
        &[access(true, 4, 8, -4, 4) & !(7 << 16), access(false, 0, 9, 0, 4), EXIT],
        16,
        &[R(4), R(8), R(9), P(0)],
        &[R(0)],
    );
    let cases = (0..64)
        .map(|case| {
            let mut inputs = input(&[
                (R(4), Value::Word(0x12345678)),
                (R(8), Value::Word(case % 24)),
                (R(9), Value::Word(0)),
                (P(0), Value::Predicate(case % 2 == 0)),
            ]);
            if case % 5 == 0 {
                inputs.registers.remove(&R(4));
            }
            if case % 7 == 0 {
                inputs.registers.remove(&R(8));
            }
            inputs
        })
        .collect();
    programs.push((program, cases));
    programs
        .push((loop_program(), (1..8).map(|count| input(&[(R(3), Value::Word(count))])).collect()));
    programs
}
