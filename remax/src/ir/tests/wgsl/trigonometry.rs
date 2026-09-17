use Register::Gpr as R;

use super::{gpu::Gpu, options, validate};
use crate::{
    ir::tests::{lift, mov, trigonometry, EXIT},
    program::{
        FloatModel, Inputs, InterpretOptions, Program, Register, SpecialFunctionModel, Value,
    },
};

fn host_result(program: &Program, input: &Inputs) -> Vec<Value> {
    program
        .interpret_with_options(input, InterpretOptions {
            step_budget: 10000,
            float_model: FloatModel::SoftwareIeee,
            special_functions: SpecialFunctionModel::HostF32,
        })
        .unwrap()
        .into_outputs()
        .expect("live invocation")
}

fn compare_functions(gpu: &Gpu, program: &Program, cases: &[Inputs]) {
    let kernel = program.emit_wgsl(options()).unwrap();
    validate(&kernel);
    for (input, actual) in cases.iter().zip(gpu.run(&kernel, cases)) {
        let expected = host_result(program, input);
        assert_eq!(&actual[expected.len()..], &[0, 0, 0]);
        for (expected, &word) in expected.iter().zip(&actual) {
            let Value::Word(expected) = expected else { panic!() };
            let (value, expected) = (f32::from_bits(word), f32::from_bits(*expected));
            assert!(
                value.is_finite() && (value - expected).abs() <= 1e-6,
                "{input:?}: {value} != {expected}"
            );
        }
    }
}

pub(super) fn compare_trigonometry(gpu: &Gpu) -> usize {
    let program = lift(
        &[
            0x5c92200000070001, // RRO.SINCOS R1,-abs(R0)
            0x5c98078000170002, // Copy opaque payload
            mov(0, 99f32.to_bits(), 7),
            mov(1, 0, 7),
            0x5080000000170203, // SIN R3,R2
            0x5080000000070204, // COS R4,R2
            0x5084000000170205, // SIN.SAT R5,R2
            0x5084000000070206, // COS.SAT R6,R2
            EXIT,
        ],
        &[R(0)],
        &[R(3), R(4), R(5), R(6)],
    );
    let cases = (-4096..=4096)
        .map(|index| Inputs {
            registers: [(R(0), Value::Word((index as f32 / 16.).to_bits()))].into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    compare_functions(gpu, &program, &cases);
    let mut count = cases.len();
    for (program, cases) in trigonometry::comparison_cases() {
        compare_functions(gpu, &program, &cases);
        count += cases.len();
    }
    count + compare_original_noise(gpu)
}

fn compare_original_noise(gpu: &Gpu) -> usize {
    let program = trigonometry::original::program();
    let cases = trigonometry::original::cases();
    let kernel = program.emit_wgsl(options()).unwrap();
    validate(&kernel);
    let mut maximum_phase_error = 0f32;
    let mut maximum_value_error = 0f32;
    for (input, actual) in cases.iter().zip(gpu.run(&kernel, &cases)) {
        let expected = host_result(&program, input);
        assert_eq!(&actual[3..], &[0, 0, 0]);
        for (expected, &word) in expected.iter().zip(&actual) {
            let Value::Word(expected) = expected else { panic!() };
            let (value, expected) = (f32::from_bits(word), f32::from_bits(*expected));
            assert!((-0.5..0.5).contains(&value), "invalid noise result: {value}");
            let difference = (value - expected).abs();
            let phase_error = difference.min(1. - difference);
            // Fraction wraps at an integer. Test amplification separately from
            // that discontinuity: the sine allowance times the original gain,
            // plus rounding of the scaled binary32 result (at most 2^-8 here).
            let allowed = 1e-6 * trigonometry::original::NOISE_GAIN + 1. / 256.;
            assert!(phase_error <= allowed, "{input:?}: {value} != {expected}");
            maximum_phase_error = maximum_phase_error.max(phase_error);
            maximum_value_error = maximum_value_error.max(difference);
        }
    }
    println!("Original sine noise: max phase error {maximum_phase_error}, max value error {maximum_value_error}");
    cases.len()
}
