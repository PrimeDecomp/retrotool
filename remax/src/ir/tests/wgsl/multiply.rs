use super::{gpu::Gpu, options, validate};
use crate::{
    ir::tests::{lift, multiply, EXIT},
    program::{wgsl::Profile, DiagnosticKind, Inputs, Program, Register, Value},
};

// Each column keeps its native controls; all columns read the same two inputs.
fn program(exponent: i8, rounds: &[u8]) -> Program {
    let mut words = Vec::new();
    for &round in rounds {
        for ftz in [false, true] {
            for saturate in [false, true] {
                let word = multiply::instruction(exponent, round, ftz, saturate);
                let word = (word & !(255 << 8) & !(255 << 20)) | (253 << 8) | (254 << 20);
                words.push(word | words.len() as u64);
            }
        }
    }
    let outputs = (0..words.len()).map(|r| Register::Gpr(r as u8)).collect::<Vec<_>>();
    words.push(EXIT);
    lift(&words, &[Register::Gpr(253), Register::Gpr(254)], &outputs)
}

fn inputs(a: u32, b: u32) -> Inputs {
    Inputs {
        registers: [(Register::Gpr(253), Value::Word(a)), (Register::Gpr(254), Value::Word(b))]
            .into(),
        ..Default::default()
    }
}

fn assert_numeric_equal(actual: u32, expected: u32, context: impl std::fmt::Debug) {
    // NaN payload/sign are a separate, explicitly unverified hardware policy.
    if f32::from_bits(expected).is_nan() {
        assert!(f32::from_bits(actual).is_nan(), "{context:?}: {actual:08x} != NaN");
    } else {
        assert_eq!(actual, expected, "{context:?}");
    }
}

#[test]
fn scaled_multiplication_validates_with_the_explicit_numeric_policy() {
    let p = program(-1, &[0, 1, 2, 3]);
    assert_eq!(p.emit_wgsl(Default::default()).unwrap_err().kind, DiagnosticKind::Unsupported);
    for profile in [Profile::Checked, Profile::Rendering] {
        let kernel = p.emit_wgsl(crate::program::wgsl::Options { profile, ..options() }).unwrap();
        assert!(kernel.approximate_arithmetic);
        validate(&kernel);
    }
}

#[test]
#[ignore = "requires a native GPU"]
fn scaled_multiplication_gpu_matches_software_rounding() {
    let gpu = Gpu::new();
    let edge = [
        0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x807fffff, 0x00800000, 0x00800001, 0x00ffffff,
        0x80800000, 0x3f000000, 0x3f7fffff, 0x3f800000, 0x3f800001, 0x40000000, 0x7f000000,
        0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000, 0x7f800001, 0x7fc12345,
    ];
    let mut cases =
        edge.iter().flat_map(|&a| edge.iter().map(move |&b| inputs(a, b))).collect::<Vec<_>>();
    let mut state = 0x53u32;
    let mut next_word = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    cases.extend((0..4096).map(|_| inputs(next_word(), next_word())));
    let mut comparisons = 0;
    for exponent in -3..=3 {
        // Unscaled nearest-even arithmetic retains the existing host-f32 path
        // and its explicit approximation policy. Test the integer path here.
        let rounds: &[u8] = if exponent == 0 { &[1, 2, 3] } else { &[0, 1, 2, 3] };
        let p = program(exponent, rounds);
        let expected = cases
            .iter()
            .map(|c| {
                p.interpret_with_options(c, multiply::OPTIONS)
                    .unwrap()
                    .into_outputs()
                    .expect("live invocation")
            })
            .collect::<Vec<_>>();
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel =
                p.emit_wgsl(crate::program::wgsl::Options { profile, ..options() }).unwrap();
            for (case, (actual, expected)) in
                gpu.run(&kernel, &cases).iter().zip(&expected).enumerate()
            {
                assert_eq!(actual[expected.len()], 0, "GPU fault");
                for (column, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
                    let Value::Word(expected) = *expected else {
                        panic!("float output");
                    };
                    assert_numeric_equal(actual, expected, (exponent, profile, case, column));
                    comparisons += 1;
                }
            }
        }
    }
    println!("{comparisons} scaled/rounded multiply comparisons against APFloat");
}

#[test]
#[ignore = "requires MPR_SCALED_MULTIPLY hardware captures and a native GPU"]
fn scaled_multiplication_matches_sm75_capture() {
    let path = std::path::PathBuf::from(
        std::env::var_os("MPR_SCALED_MULTIPLY").expect("hardware capture directory"),
    );
    let pairs = std::fs::read(path.join("sm75-inputs.bin")).unwrap();
    let outputs = std::fs::read(path.join("sm75-results.bin")).unwrap();
    assert_eq!(pairs.len() % 8, 0);
    assert_eq!(outputs.len(), pairs.len() / 8 * 28 * 4);
    assert!(!pairs.is_empty());
    let read_word = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap());
    let cases = pairs
        .chunks_exact(8)
        .map(|pair| inputs(read_word(&pair[..4]), read_word(&pair[4..])))
        .collect::<Vec<_>>();
    let gpu = Gpu::new();
    let mut interpreter_count = 0;
    let mut gpu_count = 0;
    for (scale_index, exponent) in (-3..=3).enumerate() {
        let p = program(exponent, &[0]);
        let expected = outputs
            .chunks_exact(28 * 4)
            .map(|row| {
                row[scale_index * 16..][..16].chunks_exact(4).map(read_word).collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for (case, (input, expected)) in cases.iter().zip(&expected).enumerate() {
            for (column, (actual, &expected)) in p
                .interpret_with_options(input, multiply::OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation")
                .iter()
                .zip(expected)
                .enumerate()
            {
                let Value::Word(actual) = *actual else {
                    panic!("float output");
                };
                assert_numeric_equal(actual, expected, (exponent, case, column));
                interpreter_count += 1;
            }
        }
        if exponent == 0 {
            continue; // The unscaled RN backend uses approximate host arithmetic.
        }
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel =
                p.emit_wgsl(crate::program::wgsl::Options { profile, ..options() }).unwrap();
            for (case, (actual, expected)) in
                gpu.run(&kernel, &cases).iter().zip(&expected).enumerate()
            {
                assert_eq!(actual[4], 0, "GPU fault");
                for (column, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
                    assert_numeric_equal(actual, expected, (exponent, profile, case, column));
                    gpu_count += 1;
                }
            }
        }
    }
    println!("SM75 capture: {interpreter_count} interpreter and {gpu_count} GPU comparisons; NaNs compared by class");
}
