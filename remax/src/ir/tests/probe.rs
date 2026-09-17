mod code;
pub(super) mod vertex;

pub(super) use code::{FRAGMENT_HEADER, NONLINEAR, VERTEX, VERTEX_HEADER};

use super::bytes;
use crate::{
    maxwell::MaxwellCode,
    program::{
        interpolation::{Attribute, OffsetModel, Plane, Planes},
        Inputs, Interpolant, InterpolationLocation as Location, InterpolationMode as Mode, Program,
        Register, Signature, Stage,
    },
    resources::{
        memory::{Region, Snapshot},
        ConstantMemory,
    },
};

pub(super) fn nonlinear() -> (Program, Snapshot, Vec<Inputs>) {
    let p = Program::lift(MaxwellCode::parse(&bytes(NONLINEAR)).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        outputs: (0..4).map(Register::Gpr).collect(),
        ..Default::default()
    })
    .unwrap();
    let base = 0xfffffff0u64;
    let indices = [0u32, 1, 2, 0x10001];
    let records = (0..4)
        .map(|i| {
            let f = (i + 1) as f32;
            [
                2. * f,
                4. * f,
                6. * f,
                99.,
                1. * f,
                2. * f,
                3. * f,
                -99.,
                1.,
                0.,
                0.,
                1.,
                0.,
                1.,
                0.,
                2.,
                0.,
                0.,
                1.,
                3.,
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let snapshot = Snapshot::new(
        &indices
            .iter()
            .zip(&records)
            .map(|(&index, bytes)| Region {
                address: base + u64::from(index) * 80,
                bytes,
                known: None,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let uniforms = [2f32, 0.25, 4., 0., 1., 3., 0.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let planes = Planes::new(OffsetModel::SignedSixteenths, &[
        (Attribute { byte_offset: 0x7c, mode: Mode::Pass }, Plane {
            center: 2.,
            dx: 0.25,
            dy: -0.25,
        }),
        (Attribute { byte_offset: 0x80, mode: Mode::Perspective }, Plane {
            center: 1.,
            dx: 0.5,
            dy: -0.25,
        }),
        (Attribute { byte_offset: 0x84, mode: Mode::Perspective }, Plane {
            center: 2.,
            dx: -0.25,
            dy: 0.5,
        }),
        (Attribute { byte_offset: 0x88, mode: Mode::Perspective }, Plane {
            center: 3.,
            dx: 1.,
            dy: 0.25,
        }),
    ])
    .unwrap();
    // Every byte from the native sample-location packing, four probe indices.
    // This is per-invocation plane data, not a claim about coverage/rasterization.
    let mut cases = Vec::new();
    for high in 0..16u8 {
        let mut driver = vec![0; 0x640];
        driver[0x510..0x518].copy_from_slice(&base.to_le_bytes());
        for low in 0..16u8 {
            driver[0x620 + low as usize] = (high << 4) | low;
        }
        driver[0x630..0x634].copy_from_slice(&15u32.to_le_bytes());
        driver[0x634..0x638].copy_from_slice(&4u32.to_le_bytes());
        let constants = ConstantMemory::new(&[(0, &driver), (3, &uniforms)]).unwrap();
        for index in indices {
            for sample in 0..16 {
                cases.push(Inputs {
                    constants: constants.clone(),
                    sample_index: Some(sample),
                    offset_interpolation: Some(planes.clone()),
                    interpolants: [
                        (
                            Interpolant {
                                byte_offset: 0x90,
                                mode: Mode::Constant,
                                location: Location::Default,
                            },
                            index,
                        ),
                        (
                            Interpolant {
                                byte_offset: 0x70,
                                mode: Mode::Pass,
                                location: Location::Default,
                            },
                            1.25f32.to_bits(),
                        ),
                        (
                            Interpolant {
                                byte_offset: 0x74,
                                mode: Mode::Pass,
                                location: Location::Default,
                            },
                            2.25f32.to_bits(),
                        ),
                    ]
                    .into(),
                    ..Default::default()
                });
            }
        }
    }
    (p, snapshot, cases)
}

/// Independent f64 evaluation for the synthetic records and material uniforms
/// supplied by `nonlinear`. Positive reciprocal-W cancels during normalization.
pub(super) fn reference_color(normal: [f64; 3], record: usize) -> [f64; 4] {
    let length = normal.iter().map(|value| value * value).sum::<f64>().sqrt();
    let mut color = [0., 0., 0., 1.];
    for channel in 0..3 {
        let exponent = (channel + 1) as f64;
        let range = exponent * (record + 1) as f64;
        let weight = (0.5 * normal[channel] / length + 0.5).clamp(0., 1.);
        let irradiance = range + range * (exponent + 1.) * weight.powf(exponent);
        color[channel] = irradiance * [2., 0.75, 2.][channel] / std::f64::consts::PI;
    }
    color
}

#[test]
fn original_nonlinear_probe_shader_evaluates_all_sample_offsets() {
    use crate::program::{FloatModel, InterpretOptions, Providers, SpecialFunctionModel, Value};
    let (p, mut memory, cases) = nonlinear();
    for (case, input) in cases.iter().enumerate() {
        let byte = (((case / 64) << 4) | (case % 16)) as i8;
        let x = (byte.wrapping_shl(4) >> 4) as f64 / 16.;
        let y = (byte >> 4) as f64 / 16.;
        // Normalizing cancels the positive reciprocal-W factor. Evaluate the
        // independent reference in f64 using powf, not the shader's LG2/EX2 pair.
        let normal = [1. + 0.5 * x - 0.25 * y, 2. - 0.25 * x + 0.5 * y, 3. + x + 0.25 * y];
        let expected = reference_color(normal, (case / 16) % 4);
        let output = p
            .interpret_with_resources(
                input,
                InterpretOptions {
                    step_budget: 10000,
                    float_model: FloatModel::SoftwareIeee,
                    special_functions: SpecialFunctionModel::HostF32,
                },
                Providers { memory: Some(&mut memory), ..Default::default() },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        for channel in 0..3 {
            let expected = expected[channel];
            let Value::Word(bits) = output[channel] else { panic!() };
            let actual = f32::from_bits(bits) as f64;
            assert!(
                (actual - expected).abs() < expected.abs() * 2e-5,
                "case {case}, channel {channel}: {actual} != {expected}"
            );
        }
        assert_eq!(output[3], Value::Word(1f32.to_bits()));
    }
}
