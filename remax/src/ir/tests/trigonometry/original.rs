//! Original SimpleTexture noise arithmetic, isolated from raster and textures.
//! Startup SNVN source147, x/permutation0x10, material
//! 7e0d73f9-1d8b-4258-82eb-aafc5d6fbddc, hash93139a63.
//! Full fragment code SHA256:
//! 86714ce89964d447623c59c8e99ba1d1ba9400c1cfde185f533c6ed724e8ec46.

use super::{lift, Inputs, Program, Value, OPTIONS, R};
use crate::{program::FloatModel, resources::ConstantMemory};

// Consecutive original instructions from PC0x170 through0x238. Scheduling
// words are omitted; an EXIT ends the slice. No shader algebra is substituted.
const CODE: &[u64] = &[
    0x5c9000000097000a, // RRO.SINCOS R10,R9
    0x5080000000170a02, // SIN R2,R10
    0x5c90000000870008, // RRO.SINCOS R8,R8
    0x5c90000000570009, // RRO.SINCOS R9,R5
    0x5080000000170800, // SIN R0,R8
    0x1e2472aee8c70203, // Scale by 43758.546875
    0x5080000000170901, // SIN R1,R9
    0x1e2472aee8c70002,
    0x5ca8148000370a06, // floor(R3)
    0x1e2472aee8c70101,
    0x5ca8148000270a05, // floor(R2)
    0x5c58300000670306, // First fractional part
    0x5ca8148000170a00, // floor(R1)
    0x3958103f00070606, // Center at zero
    0x5c58300000570205,
    0x4c68100c00470606, // First channel's material amplitude
    0x5c58300000070100,
    0x3958103f00070505,
    0x49a0038400170606, // First channel's scale/base
    0x3958103f00070000,
    super::EXIT,
];

pub(crate) const NOISE_GAIN: f32 = f32::from_bits(0x472aee8c);

pub(crate) fn program() -> Program { lift(CODE, &[R(9), R(8), R(5), R(7)], &[R(6), R(5), R(0)]) }

pub(crate) fn cases() -> Vec<Inputs> {
    let mut bank1 = [0u8; 8];
    let mut bank3 = [0u8; 20];
    bank1[4..8].copy_from_slice(&1f32.to_le_bytes());
    bank3[16..20].copy_from_slice(&1f32.to_le_bytes());
    let constants = ConstantMemory::new(&[(1, &bank1), (3, &bank3)]).unwrap();
    (-2048..=2048)
        .map(|index| {
            let argument = index as f32 / 8.;
            Inputs {
                registers: [
                    (R(9), Value::Word(argument.to_bits())),
                    (R(8), Value::Word((-argument + 0.25).to_bits())),
                    (R(5), Value::Word((argument * 0.75 - 0.125).to_bits())),
                    (R(7), Value::Word(0)),
                ]
                .into(),
                constants: constants.clone(),
                ..Default::default()
            }
        })
        .collect()
}

#[test]
fn original_noise_slice_matches_its_host_model_without_reassociating_arithmetic() {
    let program = program();
    for input in cases() {
        let expected = [R(9), R(8), R(5)].map(|register| {
            let Value::Word(word) = input.registers[&register] else { panic!() };
            let scaled = f32::from_bits(word).sin() * NOISE_GAIN;
            Value::Word(((scaled - scaled.floor()) - 0.5).to_bits())
        });
        let actual = program
            .interpret_with_options(&input, crate::program::InterpretOptions {
                float_model: FloatModel::SoftwareIeee,
                ..OPTIONS
            })
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        assert_eq!(actual, expected, "{input:?}");
    }
}
