use super::{lift, mov, EXIT};
use crate::program::{FloatModel, Inputs, InterpretOptions, Program, Register, Value};

pub(super) const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: crate::program::SpecialFunctionModel::Reject,
};

pub(super) fn instruction(exponent: i8, rounding: u8, ftz: bool, saturate: bool) -> u64 {
    let scale = match exponent {
        0 => 0,
        -1 => 1,
        -2 => 2,
        -3 => 3,
        3 => 4,
        2 => 5,
        1 => 6,
        _ => panic!("invalid FMUL scale"),
    };
    0x5c68000000270100
        | (scale << 41)
        | (u64::from(rounding) << 39)
        | (u64::from(ftz) << 44)
        | (u64::from(saturate) << 50)
}

pub(super) fn program(exponent: i8, rounding: u8, ftz: bool, saturate: bool) -> Program {
    lift(
        &[instruction(exponent, rounding, ftz, saturate), EXIT],
        &[Register::Gpr(1), Register::Gpr(2)],
        &[Register::Gpr(0)],
    )
}

pub(super) fn inputs(a: u32, b: u32) -> Inputs {
    Inputs {
        registers: [(Register::Gpr(1), Value::Word(a)), (Register::Gpr(2), Value::Word(b))].into(),
        ..Default::default()
    }
}

pub(super) fn result(program: &Program, input: &Inputs) -> u32 {
    let [Value::Word(word)] = program
        .interpret_with_options(input, OPTIONS)
        .unwrap()
        .into_outputs()
        .expect("live invocation")[..]
    else {
        panic!("expected a single product");
    };
    word
}

#[test]
fn multiplication_scales_before_rounding_flushing_and_saturation() {
    // These distinguish a scaled product from two separately rounded multiplies.
    // The FTZ boundary behavior is measured on SM75, not yet SM53.
    for (exponent, ftz, saturate, a, b, expected) in [
        (-1, true, false, 0x40000000, 0x7f7fffff, 0x7f7fffff),
        (-1, true, false, 0x00800000, 0x40000000, 0x00800000),
        (-1, true, false, 0x00ffffff, 0x3f800000, 0),
        (-1, false, false, 0x00ffffff, 0x3f800000, 0x00800000),
        (-1, true, false, 0x80ffffff, 0x3f800000, 0x80000000),
        (0, true, false, 0x00800000, 0x3f7fffff, 0),
        (0, false, false, 0x00800000, 0x3f7fffff, 0x00800000),
        (1, true, false, 0x7f7fffff, 0x3f000000, 0x7f7fffff),
        (1, true, false, 0x00800000, 0x3f000000, 0x00800000),
        (-3, true, true, 0x40000000, 0x40000000, 0x3f000000),
        (3, true, true, 0x3f000000, 0x3f000000, 0x3f800000),
        (3, true, true, 0x7fc12345, 0x40000000, 0),
    ] {
        let p = program(exponent, 0, ftz, saturate);
        assert_eq!(result(&p, &inputs(a, b)), expected, "{exponent} {ftz} {saturate} {a:x} {b:x}");
    }
}

#[test]
fn scaled_multiplication_retains_rounding_and_signed_underflow() {
    for exponent in -3..=3 {
        for (round, positive, negative) in [
            (0, 0x3f800002, 0xbf800002),
            (1, 0x3f800002, 0xbf800003),
            (2, 0x3f800003, 0xbf800002),
            (3, 0x3f800002, 0xbf800002),
        ] {
            let p = program(exponent, round, false, false);
            let scale_bits = |bits: u32| bits.wrapping_add_signed(i32::from(exponent) << 23);
            assert_eq!(result(&p, &inputs(0x3f800001, 0x3f800001)), scale_bits(positive));
            assert_eq!(result(&p, &inputs(0xbf800001, 0x3f800001)), scale_bits(negative));
        }
    }
    for (round, positive, negative) in
        [(0, 0, 0x80000000), (1, 0, 0x80000001), (2, 1, 0x80000000), (3, 0, 0x80000000)]
    {
        let p = program(-3, round, false, false);
        assert_eq!(result(&p, &inputs(1, 1)), positive);
        assert_eq!(result(&p, &inputs(0x80000001, 1)), negative);
    }
}

#[test]
fn scaled_multiply_predication_and_aliasing_preserve_old_inputs() {
    use Register::{Gpr as R, Predicate as P};
    // @P0 FMUL.FTZ.D2 R1, -R1, R2; destination aliases the first source.
    let word = (instruction(-1, 0, true, false) & !(7 << 16)) | (1 << 48) | 1;
    let p = lift(&[mov(1, 0x00800000, 7), word, EXIT], &[R(2), P(0)], &[R(1)]);
    for (predicate, expected) in [(false, 0x00800000), (true, 0x80800000)] {
        let mut input = inputs(0, 0x40000000);
        input.registers.insert(P(0), Value::Predicate(predicate));
        if !predicate {
            input.registers.remove(&R(2));
        }
        assert_eq!(result(&p, &input), expected);
    }
}
