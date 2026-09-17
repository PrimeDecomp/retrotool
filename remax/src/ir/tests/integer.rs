use Register::{Carry, Gpr as R, Predicate as P};

use super::{add, bytes, lift, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Value},
    resources::ConstantMemory,
};

fn words(values: &[(Register, u32)]) -> Inputs {
    Inputs {
        registers: values.iter().map(|&(r, v)| (r, Value::Word(v))).collect(),
        ..Default::default()
    }
}

#[test]
fn scaled_address_carry_uses_the_truncated_shift_and_original_operands() {
    // ISCADD R0.CC,R2,R0,shift; IADD.X R1,R1,RZ. Both destinations alias inputs.
    for shift in [0, 4, 16, 31] {
        let p = lift(
            &[0x5c18800000070200 | (shift << 39), 0x5c1008000ff70101, EXIT],
            &[R(0), R(1), R(2)],
            &[R(0), R(1), Carry],
        );
        for lo in [0, 1, 0x7fffffff, 0xfffffff0, u32::MAX] {
            for index in [0u32, 1, 2, 0x10000000, 0x80000000, u32::MAX] {
                for hi in [0, 17, u32::MAX] {
                    let delta = index.wrapping_shl(shift as u32);
                    let expected =
                        ((u64::from(hi) << 32) | u64::from(lo)).wrapping_add(u64::from(delta));
                    assert_eq!(
                        p.interpret(&words(&[(R(0), lo), (R(1), hi), (R(2), index)]), 100)
                            .unwrap()
                            .into_outputs()
                            .expect("live invocation"),
                        [
                            Value::Word(expected as u32),
                            Value::Word((expected >> 32) as u32),
                            Value::Predicate(
                                u64::from(lo) + u64::from(delta) > u64::from(u32::MAX)
                            ),
                        ]
                    );
                }
            }
        }
    }
}

#[test]
fn extended_add_carries_both_additions_and_preserves_carry_without_cc() {
    for cc in [false, true] {
        let p = lift(&[0x5c10080000270100 | (u64::from(cc) << 47), EXIT], &[R(1), R(2), Carry], &[
            R(0),
            Carry,
        ]);
        for a in [0, 1, 0x7fffffff, 0xfffffffe, u32::MAX] {
            for b in [0, 1, 0x80000000, u32::MAX] {
                for carry in [false, true] {
                    let mut input = words(&[(R(1), a), (R(2), b)]);
                    input.registers.insert(Carry, Value::Predicate(carry));
                    let expected = u64::from(a) + u64::from(b) + u64::from(carry);
                    assert_eq!(
                        p.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"),
                        [
                            Value::Word(expected as u32),
                            Value::Predicate(if cc { expected >> 32 != 0 } else { carry }),
                        ]
                    );
                }
            }
        }
    }
}

#[test]
fn predicated_flag_write_merges_carry_without_inventing_incoming_state() {
    // @P0 IADD32I RZ.CC,R1,1; IADD.X R0,RZ,RZ.
    let set = (add(255, 1, 1) | (1 << 52)) & !(7 << 16);
    let p = lift(&[set, 0x5c1008000ff7ff00, EXIT], &[P(0), R(1)], &[R(0)]);
    let mut input = words(&[(R(1), u32::MAX)]);
    input.registers.insert(P(0), Value::Predicate(true));
    assert_eq!(p.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(1)
    ]);
    input.registers.insert(P(0), Value::Predicate(false));
    assert_eq!(p.interpret(&input, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
    let p = lift(&[set, 0x5c1008000ff7ff00, EXIT], &[P(0), R(1), Carry], &[R(0)]);
    assert_eq!(p.interpret(&input, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
    input.registers.insert(Carry, Value::Predicate(true));
    input.registers.remove(&R(1)); // The skipped producer must not consume this input.
    assert_eq!(p.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(1)
    ]);
    input.registers.insert(Carry, Value::Word(1));
    assert_eq!(p.interpret(&input, 100).unwrap_err().kind, DiagnosticKind::InvalidInput);
}

#[test]
fn modified_carry_remains_explicitly_unsupported() {
    for word in [
        0x5c11080000270100, // IADD.X with a complemented input.
        0x5c11800000270100, // IADD.CC with subtraction.
        0x5c13000000270100, // IADD.PO.
        0x5c1b820000270100, // ISCADD.PO.
    ] {
        let code = bytes(&[word, EXIT]);
        let e =
            Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).unwrap_err();
        assert_eq!((e.kind, e.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let values = [0, 1, 2, 0x7fffffff, 0x80000000, 0xfffffffe, u32::MAX];
    let mut cases = vec![];
    for a in values {
        for b in values {
            for carry in [false, true] {
                for enabled in [false, true] {
                    let mut input = words(&[(R(1), a), (R(2), b)]);
                    input.registers.insert(Carry, Value::Predicate(carry));
                    input.registers.insert(P(0), Value::Predicate(enabled));
                    cases.push(input);
                }
            }
        }
    }
    // Missing carry or operand faults are exercised on taken/skipped producers.
    for enabled in [false, true] {
        let mut input = words(&[(R(1), 0), (R(2), u32::MAX)]);
        input.registers.insert(P(0), Value::Predicate(enabled));
        cases.push(input);
    }
    let mut programs = vec![];
    for word in [
        0x5c10880000270100, // IADD.X R0.CC,R1,R2.
        0x5c10080000270100, // IADD.X R0,R1,R2.
        0x5c18820000270100, // ISCADD R0.CC,R1,R2,4.
        0x17f8000000170100, // ISCADD32I R0.CC,R1,-0x7fffffff,31.
    ] {
        programs.push((lift(&[word, EXIT], &[R(1), R(2), Carry], &[R(0), Carry]), cases.clone()));
    }
    let set = (add(255, 1, 1) | (1 << 52)) & !(7 << 16);
    programs.push((
        lift(&[set, 0x5c10080000270100, EXIT], &[P(0), R(1), R(2), Carry], &[R(0), Carry]),
        cases,
    ));

    // Original LumaUpdate address pair, with a fixed synthetic storage address.
    let p = lift(&[0x4c18820014470606, 0x4c1008001457ff07, EXIT], &[R(6)], &[R(6), R(7)]);
    let mut constants = vec![0u8; 0x518];
    constants[0x510..].copy_from_slice(&0x18fffffff0u64.to_le_bytes());
    let memory = ConstantMemory::new(&[(0, &constants)]).unwrap();
    let cases = values
        .into_iter()
        .map(|index| Inputs {
            registers: [(R(6), Value::Word(index))].into(),
            constants: memory.clone(),
            ..Default::default()
        })
        .collect();
    programs.push((p, cases));
    programs
}

#[test]
fn original_storage_address_pair_uses_driver_constants() {
    let (p, cases) = comparison_cases().pop().unwrap();
    for c in cases {
        let Value::Word(index) = c.registers[&R(6)] else { unreachable!() };
        let expected = 0x18fffffff0u64 + u64::from(index.wrapping_shl(4));
        assert_eq!(p.interpret(&c, 100).unwrap().into_outputs().expect("live invocation"), [
            Value::Word(expected as u32),
            Value::Word((expected >> 32) as u32)
        ]);
    }
}
