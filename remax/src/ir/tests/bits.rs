//! Bit operations needed by the real vine light-mask loop. Expected extraction
//! is calculated one source bit at a time, separately from the implementation.
//! FLO zero behavior follows Mesa NAK's OpFlo fold/hardware-test contract;
//! the inspected Yuzu frontend differs for FLO.SH on zero.

use super::{lift, EXIT};
use crate::program::{Inputs, Program, Register, Value};

fn inputs(value: u32, range: u32) -> Inputs {
    Inputs {
        registers: [(Register::Gpr(1), Value::Word(value)), (Register::Gpr(2), Value::Word(range))]
            .into(),
        ..Default::default()
    }
}

fn extract_program(signed: bool, reverse: bool) -> Program {
    let instruction = 0x5c00000000270100 | (u64::from(signed) << 48) | (u64::from(reverse) << 40);
    lift(&[instruction, EXIT], &[Register::Gpr(1), Register::Gpr(2)], &[Register::Gpr(0)])
}

fn leading_program(signed: bool, invert: bool, shift: bool) -> Program {
    let instruction = 0x5c30000000170000
        | (u64::from(signed) << 48)
        | (u64::from(invert) << 40)
        | (u64::from(shift) << 41);
    lift(&[instruction, EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)])
}

fn test_values() -> Vec<u32> {
    let mut values = vec![0, u32::MAX, 0x12345678, 0x87654321, 0x7fffffff, 0x80000001];
    values.extend((0..32).map(|bit| 1 << bit));
    values
}

fn extract_cases() -> Vec<Inputs> {
    let mut cases = Vec::new();
    for value in test_values() {
        for offset in [0, 1, 15, 31, 32, 33, 255] {
            for count in [0, 1, 7, 31, 32, 33, 255] {
                cases.push(inputs(value, 0x12340000 | (count << 8) | offset));
            }
        }
    }
    cases
}

fn expected_extract(value: u32, range: u32, signed: bool, reverse: bool) -> u32 {
    let offset = range & 255;
    let count = (range >> 8) & 255;
    if count == 0 {
        return 0;
    }
    let source_bit = |index: u32| {
        let index = index.min(31);
        (value >> if reverse { 31 - index } else { index }) & 1
    };
    let padding = if signed { source_bit(offset + count - 1) } else { 0 };
    (0..32).fold(0, |result, bit| {
        let next =
            if bit < count && offset + bit < 32 { source_bit(offset + bit) } else { padding };
        result | (next << bit)
    })
}

fn expected_leading(value: u32, signed: bool, invert: bool, shift: bool) -> u32 {
    let value = if invert { !value } else { value };
    let ignored_bit = u32::from(signed && value & 0x80000000 != 0);
    let highest = (0..32).rev().find(|bit| ((value >> bit) & 1) != ignored_bit);
    if shift {
        highest.map_or(32, |bit| 31 - bit)
    } else {
        highest.unwrap_or(u32::MAX)
    }
}

#[test]
fn bit_fields_preserve_padding_reverse_and_unsigned_range_bytes() {
    for signed in [false, true] {
        for reverse in [false, true] {
            let program = extract_program(signed, reverse);
            for input in extract_cases() {
                let Value::Word(value) = input.registers[&Register::Gpr(1)] else { unreachable!() };
                let Value::Word(range) = input.registers[&Register::Gpr(2)] else { unreachable!() };
                assert_eq!(
                    program
                        .interpret(&input, 100)
                        .unwrap()
                        .into_outputs()
                        .expect("live invocation"),
                    [Value::Word(expected_extract(value, range, signed, reverse))]
                );
            }
        }
    }
}

#[test]
fn leading_bit_modes_include_zero_and_all_sign_bits() {
    for signed in [false, true] {
        for invert in [false, true] {
            for shift in [false, true] {
                let program = leading_program(signed, invert, shift);
                for value in test_values() {
                    assert_eq!(
                        program
                            .interpret(&inputs(value, 0), 100)
                            .unwrap()
                            .into_outputs()
                            .expect("live invocation"),
                        [Value::Word(expected_leading(value, signed, invert, shift))]
                    );
                }
            }
        }
    }
}

#[test]
fn original_vine_selects_the_lowest_light_mask_bit() {
    // Original fragment PCs 0x750/0x758: BFE.U32.BREV R39,R40,0x2000;
    // FLO.U32.SH R39,R39. The preceding branch guards the zero-mask case.
    let program = lift(&[0x3800010200072827, 0x5c30020002770027, EXIT], &[Register::Gpr(40)], &[
        Register::Gpr(39),
    ]);
    for mask in test_values() {
        let input = Inputs {
            registers: [(Register::Gpr(40), Value::Word(mask))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(mask.trailing_zeros())]
        );
    }
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut programs = Vec::new();
    for signed in [false, true] {
        for reverse in [false, true] {
            programs.push((extract_program(signed, reverse), extract_cases()));
        }
        for invert in [false, true] {
            for shift in [false, true] {
                programs.push((
                    leading_program(signed, invert, shift),
                    test_values().into_iter().map(|value| inputs(value, 0)).collect(),
                ));
            }
        }
    }
    programs
}
