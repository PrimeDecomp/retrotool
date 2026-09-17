use Register::Gpr as R;

use super::{gpu::Gpu, options, validate};
use crate::{
    ir::tests::{lift, mov, EXIT},
    program::{FloatModel, Inputs, InterpretOptions, Register, SpecialFunctionModel, Value},
};

pub(super) fn compare_exponentiation(gpu: &Gpu) -> usize {
    // Capture, copy, overwrite the original argument, then consume twice.
    let pair = lift(
        &[
            0x5c90008000070001,
            0x5c98078000170002,
            mov(0, 99f32.to_bits(), 7),
            0x5080000000270203,
            0x5084000000270204,
            EXIT,
        ],
        &[R(0)],
        &[R(3), R(4)],
    );
    let cases = (-2016..=2016)
        .map(|i| Inputs {
            registers: [(R(0), Value::Word((i as f32 / 16.).to_bits()))].into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let count = cases.len();
    let kernel = pair.emit_wgsl(options()).unwrap();
    validate(&kernel);
    for (input, actual) in cases.iter().zip(gpu.run(&kernel, &cases)) {
        let Value::Word(x) = input.registers[&R(0)] else { panic!() };
        let expected = f32::from_bits(x).exp2();
        assert_eq!(&actual[2..], &[0, 0, 0]);
        for (&word, expected) in actual.iter().zip([expected, expected.min(1.)]) {
            let value = f32::from_bits(word);
            assert!(
                value.is_finite() && (value - expected).abs() <= expected.abs() * 3e-6,
                "exp2({}): {value} != {expected}",
                f32::from_bits(x)
            );
        }
    }
    // Original source328's LG2, FMUL, RRO.EX2 and MUFU.EX2 sequence for a lobe.
    // Supply t and exponent at the slice boundary; no raster or probe loads.
    let power = lift(
        &[0x5080000000370a01, 0x5c68100000170301, 0x5c90008000170001, 0x5080000000270101, EXIT],
        &[R(10), R(3)],
        &[R(1)],
    );
    let cases = (1..=1024)
        .flat_map(|i| {
            [1f32, 1.25, 2., 3.].map(move |exponent| Inputs {
                registers: [
                    (R(10), Value::Word((i as f32 / 1024.).to_bits())),
                    (R(3), Value::Word(exponent.to_bits())),
                ]
                .into(),
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    let kernel = power.emit_wgsl(options()).unwrap();
    validate(&kernel);
    for (input, actual) in cases.iter().zip(gpu.run(&kernel, &cases)) {
        let expected = power
            .interpret_with_options(input, InterpretOptions {
                step_budget: 100,
                float_model: FloatModel::SoftwareIeee,
                special_functions: SpecialFunctionModel::HostF32,
            })
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        let Value::Word(word) = expected[0] else { panic!() };
        let (expected, value) = (f32::from_bits(word), f32::from_bits(actual[0]));
        assert_eq!(&actual[1..], &[0, 0, 0]);
        assert!(
            value.is_finite() && (value - expected).abs() <= 8e-6 * expected.abs(),
            "{input:?}: {value} != {expected}"
        );
    }
    count + cases.len()
}
