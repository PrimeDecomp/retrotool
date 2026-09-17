use super::{bytes, lift, mov, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        DiagnosticKind, FloatModel, Inputs, InterpretOptions, Program, Register, Signature, Value,
    },
};

// FMNMX R0, R1, R2, PT: true chooses minimum, independently of execution predication.
const FMNMX: u64 = 0x5c60038000270100;
const INVERT_SELECTOR: u64 = 1 << 42;
const FTZ: u64 = 1 << 44;
const ABS_A: u64 = 1 << 46;
const NEGATE_A: u64 = 1 << 48;
const ABS_B: u64 = 1 << 49;
const NEGATE_B: u64 = 1 << 45;
const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: crate::program::SpecialFunctionModel::Reject,
};

fn program(word: u64) -> Program {
    lift(&[word, EXIT], &[Register::Gpr(1), Register::Gpr(2), Register::Predicate(0)], &[
        Register::Gpr(0),
    ])
}

fn inputs(a: u32, b: u32, minimum: bool) -> Inputs {
    Inputs {
        registers: [
            (Register::Gpr(1), Value::Word(a)),
            (Register::Gpr(2), Value::Word(b)),
            (Register::Predicate(0), Value::Predicate(minimum)),
        ]
        .into(),
        ..Default::default()
    }
}

fn result(program: &Program, input: &Inputs) -> u32 {
    let values = program
        .interpret_with_options(input, OPTIONS)
        .unwrap()
        .into_outputs()
        .expect("live invocation");
    let [Value::Word(word)] = values[..] else { panic!("expected one word") };
    word
}

#[test]
fn minmax_prefers_numbers_and_orders_signed_zeros() {
    let minimum = program(FMNMX);
    let maximum = program(FMNMX | INVERT_SELECTOR);
    for (a, b, low, high) in [
        (0x3f800000, 0x40000000, 0x3f800000, 0x40000000),
        (0xbf800000, 0xc0000000, 0xc0000000, 0xbf800000),
        (0xbf800000, 0x3f800000, 0xbf800000, 0x3f800000),
        (0x00000000, 0x80000000, 0x80000000, 0x00000000),
        (0x80000000, 0x80000000, 0x80000000, 0x80000000),
        (0x00000000, 0x00000000, 0x00000000, 0x00000000),
        (0x00000001, 0x00000002, 0x00000001, 0x00000002),
        (0x80000001, 0x80000002, 0x80000002, 0x80000001),
        (0x007fffff, 0x00800000, 0x007fffff, 0x00800000),
        (0x7f800000, 0xff800000, 0xff800000, 0x7f800000),
        (0x7fc12345, 0x3f800000, 0x3f800000, 0x3f800000),
        (0x7f800001, 0x80000000, 0x80000000, 0x80000000),
        (0xff800001, 0x00000001, 0x00000001, 0x00000001),
        (0xffcabcde, 0xff800000, 0xff800000, 0xff800000),
        // Both-NaN payload is deliberately the model's canonical quiet NaN.
        (0x7fc12345, 0xff800001, 0x7fffffff, 0x7fffffff),
    ] {
        for (left, right) in [(a, b), (b, a)] {
            let input = inputs(left, right, false);
            assert_eq!(result(&minimum, &input), low, "min({left:#x}, {right:#x})");
            assert_eq!(result(&maximum, &input), high, "max({left:#x}, {right:#x})");
        }
    }
}

#[test]
fn minmax_applies_source_modifiers_before_selection_and_ftz() {
    for (modifiers, a, b, low, high) in [
        (ABS_A, 0xc0000000, 0x3f800000, 0x3f800000, 0x40000000),
        (ABS_A | NEGATE_A, 0xc0000000, 0xbf800000, 0xc0000000, 0xbf800000),
        (ABS_B, 0x3f800000, 0xc0000000, 0x3f800000, 0x40000000),
        (ABS_B | NEGATE_B, 0xbf800000, 0xc0000000, 0xc0000000, 0xbf800000),
        (NEGATE_A | NEGATE_B, 0, 0x80000000, 0x80000000, 0),
        (FTZ, 0x80000001, 1, 0x80000000, 0),
        (FTZ, 0x80000001, 0x80000002, 0x80000000, 0x80000000),
        (FTZ | ABS_A | NEGATE_A, 1, 0, 0x80000000, 0),
        (FTZ | ABS_B | NEGATE_B, 0, 0x80000001, 0x80000000, 0),
        (FTZ, 0x7fc12345, 0x80000001, 0x80000000, 0x80000000),
    ] {
        let input = inputs(a, b, false);
        assert_eq!(result(&program(FMNMX | modifiers), &input), low);
        assert_eq!(result(&program(FMNMX | modifiers | INVERT_SELECTOR), &input), high);
    }
}

#[test]
fn minmax_selector_is_independent_of_execution_predication() {
    use Register::{Gpr, Predicate};
    // P0 selects min/max; P1 decides whether the instruction executes at all.
    let word = (FMNMX & !(7 << 39) & !(7 << 16)) | (1 << 16);
    for inverted in [false, true] {
        let word = word | if inverted { INVERT_SELECTOR } else { 0 };
        let program = lift(
            &[mov(0, 0x12345678, 7), word, EXIT],
            &[Gpr(1), Gpr(2), Predicate(0), Predicate(1)],
            &[Gpr(0)],
        );
        for selector in [false, true] {
            let mut input = inputs(1.0f32.to_bits(), 2.0f32.to_bits(), selector);
            input.registers.insert(Predicate(1), Value::Predicate(true));
            let expected: f32 = if selector != inverted { 1.0 } else { 2.0 };
            assert_eq!(result(&program, &input), expected.to_bits());
            input.registers.remove(&Predicate(0));
            assert_eq!(
                program.interpret_with_options(&input, OPTIONS).unwrap_err().kind,
                DiagnosticKind::MissingInput
            );
        }
        // Skipping does not require either source or the min/max selector.
        let input = Inputs {
            registers: [(Predicate(1), Value::Predicate(false))].into(),
            ..Default::default()
        };
        assert_eq!(result(&program, &input), 0x12345678);
    }
}

#[test]
fn minmax_keeps_numeric_policy_and_rejects_condition_code_writes() {
    let program = program(FMNMX);
    for model in [FloatModel::Reject, FloatModel::HostF32] {
        let error = program
            .interpret_with_options(&inputs(0, 0, true), InterpretOptions {
                float_model: model,
                ..OPTIONS
            })
            .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
    let code = bytes(&[FMNMX | (1 << 47), EXIT]);
    let error =
        Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut values = vec![
        0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x807fffff, 0x00800000, 0x80800000, 0x3f800000,
        0xbf800000, 0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000, 0x7f800001, 0xff800001,
        0x7fc12345, 0xffcabcde,
    ];
    // Exercise varied exponents and mantissas in addition to the boundary table.
    let mut state = 0x28d91c37u32;
    for _ in 0..16 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        values.push(state);
    }
    let mut programs = Vec::new();
    for ftz in [0, FTZ] {
        for word in [
            FMNMX,
            FMNMX | INVERT_SELECTOR,
            FMNMX & !(7 << 39),
            (FMNMX & !(7 << 39)) | INVERT_SELECTOR,
            (FMNMX & !(7 << 39)) | ABS_A | NEGATE_B,
            (FMNMX & !(7 << 39)) | ABS_B | NEGATE_A,
        ] {
            let mut cases = Vec::new();
            for &a in &values {
                for &b in &values {
                    for selector in [false, true] {
                        cases.push(inputs(a, b, selector));
                    }
                }
            }
            cases.push(Inputs::default());
            programs.push((program(word | ftz), cases));
        }
    }
    programs
}
