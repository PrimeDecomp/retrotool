use anyhow::Context;

use super::*;

const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];

#[test]
fn captured_record_round_trips_unused_words_and_nonfinite_bits() {
    let words = [
        0, 0x80000000, 0x7f800000, 0x7fa12345, 0x7fc54321, 0xff800000, 0xffc12345, 0xffa54321, 1,
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    let probe = NonlinearProbe::parse(&bytes).unwrap();
    assert_eq!(probe.to_bytes().as_slice(), bytes);
    assert_eq!(probe.gradients[1].exponent.to_bits(), 8);
    assert!(NonlinearProbe::parse(&bytes[..79]).is_err());
    assert!(NonlinearProbe::parse(&[0; 81]).is_err());
    assert!(probe.irradiance([1., 0., 0.]).is_err());
}

#[test]
fn directional_lobes_preserve_length_normalize_surface_and_allow_negative_light() {
    let mut samples = ProbeSamples {
        midpoint_rgb: [2., 4., 6.],
        range_rgb: [3., 2., 1.],
        exponent_rgb: [0., 0.5, 1.], // Exponents 1, 2, 3.
        gradient_directions: [[1., 0.5, 0.5]; 3],
    };
    let probe = NonlinearProbe::from_samples(&samples, IDENTITY).unwrap();
    assert_eq!(probe.irradiance([2., 0., 0.]).unwrap(), [5., 8., 9.]);
    assert_eq!(probe.irradiance([-2., 0., 0.]).unwrap(), [-1., 2., 5.]);
    assert_eq!(probe.irradiance([0., 2., 0.]).unwrap(), [2., 3.5, 5.5]);
    samples.exponent_rgb = [0.; 3];
    samples.gradient_directions = [[0.75, 0.5, 0.5]; 3];
    let probe = NonlinearProbe::from_samples(&samples, IDENTITY).unwrap();
    assert_eq!(probe.irradiance([1., 0., 0.]).unwrap(), [3.5, 5., 6.5]);
    assert!(probe.irradiance([0.; 3]).is_err());
    assert!(probe.irradiance([f32::MAX; 3]).is_err());
    assert!(probe.irradiance([f32::NAN, 0., 0.]).is_err());
    samples.exponent_rgb[0] = -0.01;
    assert!(NonlinearProbe::from_samples(&samples, IDENTITY).is_err());
    samples.exponent_rgb[0] = f32::NAN;
    assert!(NonlinearProbe::from_samples(&samples, IDENTITY).is_err());
}

#[test]
fn sampled_coefficients_match_original_producer_arithmetic() -> Result<()> {
    use remax::{
        maxwell::MaxwellCode,
        program::{FloatModel, Inputs, InterpretOptions, Program, Register, Signature, Value},
        resources::ConstantMemory,
    };
    // Original BakedLighting_ProbeTexture SNVN 98 VS: the FMUL/FFMA instructions
    // from 0x218 through 0x488, in order. Inputs are the six texture results;
    // outputs R12..R23 are the three vectors stored at record offsets 32/48/64.
    // Texture sampling, address arithmetic and STG are outside this slice.
    // Comparisons exercise the recovered equation using the separate IEEE
    // interpreter; they do not establish hardware texture or SFU precision.
    let words: &[u64] = &[
        0x32a202c000070606,
        0x32a202c000070707,
        0x32a202c000070808,
        0x32a202c000070202,
        0x32a202c000071010,
        0x32a002c00007140f,
        0x4c68100c05470609,
        0x4c68100c0507060a,
        0x4c68100c04c70606,
        0x4c68100c05470212,
        0x4c68100c05070224,
        0x4c68100c04c70202,
        0x49a0048c05570709,
        0x49a0050c0517070d,
        0x49a0030c04d70706,
        0x32a002c000071513,
        0x49a0048c0567080e,
        0x49a0068c0527080d,
        0x49a0030c04e7080c,
        0x32a202c000070307,
        0x32a202c000071111,
        0x4c68100c05071015,
        0x4c68100c04c71014,
        0x49a0090c05570712,
        0x49a0120c05170703,
        0x49a0010c04d70702,
        0x4c68100c05471024,
        0x32a002c000071717,
        0x32a202c000072310,
        0x32a202c000071605,
        0x49a0120c05571116,
        0x49a00a8c05171115,
        0x49a00a0c04d71114,
        0x49a0090c05671012,
        0x49a0018c05271011,
        0x49a0010c04e71010,
        0x49a00b0c05670516,
        0x49a00a8c05270515,
        0x49a00a0c04e70514,
        0xe30000000007000f,
    ];
    let code: Vec<_> = words
        .chunks(3)
        .flat_map(|g| [0x001f8000fc0007e0, g[0], *g.get(1).unwrap_or(&0), *g.get(2).unwrap_or(&0)])
        .flat_map(u64::to_le_bytes)
        .collect();
    let input_registers = [2, 3, 5, 6, 7, 8, 16, 17, 20, 21, 22, 23, 35];
    let program = Program::lift(MaxwellCode::parse(&code)?, &Signature {
        indexed_attributes: vec![],
        inputs: input_registers.map(Register::Gpr).to_vec(),
        outputs: (12..24).map(Register::Gpr).collect(),
        ..Default::default()
    })?;
    let mut seed = 0x5198u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / 16777215.0
    };
    for case in 0..256 {
        let transform: [[f32; 3]; 3] =
            std::array::from_fn(|_| std::array::from_fn(|_| next() * 4.0 - 2.0));
        let samples = ProbeSamples {
            midpoint_rgb: [1., 2., 3.],
            range_rgb: [4., 5., 6.],
            exponent_rgb: std::array::from_fn(|_| next()),
            gradient_directions: std::array::from_fn(|_| std::array::from_fn(|_| next())),
        };
        let mut bank = vec![0; 0x160];
        for (i, row) in transform.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let offset = 0x130 + i * 16 + j * 4;
                bank[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        let mut inputs =
            Inputs { constants: ConstantMemory::new(&[(3, &bank)])?, ..Default::default() };
        inputs.registers.insert(Register::Gpr(5), Value::Word(1.0f32.to_bits()));
        for (registers, values) in [
            ([6, 7, 8], samples.gradient_directions[0]),
            ([2, 3, 35], samples.gradient_directions[1]),
            ([16, 17, 22], samples.gradient_directions[2]),
            ([20, 21, 23], samples.exponent_rgb),
        ] {
            for (r, v) in registers.into_iter().zip(values) {
                inputs.registers.insert(Register::Gpr(r), Value::Word(v.to_bits()));
            }
        }
        let result = program
            .interpret_with_options(&inputs, InterpretOptions {
                step_budget: 1000,
                float_model: FloatModel::SoftwareIeee,
                ..Default::default()
            })?
            .into_outputs()
            .context("reference invocation was discarded")?;
        let record = NonlinearProbe::from_samples(&samples, transform)?;
        let expected: Vec<_> = record
            .gradients
            .iter()
            .flat_map(|g| g.direction.into_iter().chain([g.exponent]))
            .map(|v| Value::Word(v.to_bits()))
            .collect();
        assert_eq!(result, expected, "sample case {case}");
        let bytes = record.to_bytes();
        assert_eq!(&bytes[12..16], &[0; 4]);
        assert_eq!(&bytes[28..32], &[0; 4]);
        assert_eq!(NonlinearProbe::parse(&bytes)?.midpoint_rgb, samples.midpoint_rgb);
        assert_eq!(NonlinearProbe::parse(&bytes)?.range_rgb, samples.range_rgb);
    }
    Ok(())
}

#[test]
#[ignore = "requires MPR_MATERIAL_ARCHIVE pointing to the extracted startup MaterialArchive.arc"]
fn full_nonlinear_shader_matches_recovered_lighting_equation() -> Result<()> {
    use anyhow::Context;
    use remax::{
        program::{
            FloatModel, Inputs, Interpolant, InterpolationLocation as Location,
            InterpolationMode as Mode, InterpretOptions, Program, Providers, Register, Signature,
            SpecialFunctionModel, Stage, Value,
        },
        resources::{
            memory::{Region, Snapshot},
            ConstantMemory,
        },
    };

    use crate::format::{material_archive::MaterialArchive, shader_nvn::NvnShader};

    let bytes = std::fs::read(std::env::var("MPR_MATERIAL_ARCHIVE")?)?;
    let (archive, _) = MaterialArchive::slice(&bytes)?;
    let backend = archive.backends.iter().find(|b| b.id == *b"SNVN").context("No SNVN backend")?;
    let shader = NvnShader::parse(backend.sources[328].binary)?;
    let programs = shader.programs()?;
    let source = programs.iter().find(|p| p.stage.kind == 1).context("No pixel shader")?;
    let program = Program::lift(source.code, &Signature {
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        outputs: (0..4).map(Register::Gpr).collect(),
        ..Default::default()
    })?;
    let base = 0xfffffff0u64;
    let mut driver = vec![0; 0x640];
    driver[0x510..0x518].copy_from_slice(&base.to_le_bytes());
    // Explicit single-sample fixture: the driver lookup supplies offset zero.
    // Interpolation values below are supplied at that exact requested offset;
    // neither the rasterizer nor the scene's binding construction is simulated.
    let diffuse = [2f32, 0.25, 4.];
    let modulation = [1f32, 3., 0.5];
    let uniforms = diffuse
        .into_iter()
        .chain([0.])
        .chain(modulation)
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let constants = ConstantMemory::new(&[(0, &driver), (3, &uniforms)])?;
    let interp = |byte_offset, mode, location| Interpolant { byte_offset, mode, location };
    let mut state = 0x328577u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 8) as f32 / 16777215.0
    };
    for case in 0..256 {
        let index = [0u32, 1, 2, 0x10001][case % 4];
        let normal = match case % 8 {
            0 => [2., 0., 0.],
            1 => [-2., 0., 0.],
            _ => std::array::from_fn(|_| next() * 2. - 1.),
        };
        let record = NonlinearProbe {
            midpoint_rgb: std::array::from_fn(|_| next() * 4.),
            range_rgb: std::array::from_fn(|_| next() * 3.),
            gradients: std::array::from_fn(|channel| ProbeGradient {
                direction: if case % 8 <= 1 {
                    [1., 0., 0.]
                } else {
                    std::array::from_fn(|_| next() * 4. - 2.)
                },
                exponent: if case % 8 <= 1 { (channel + 1) as f32 } else { next() * 2. + 1. },
            }),
            unused: [99., -99.],
        };
        let data = record.to_bytes();
        let mut memory = Snapshot::new(&[Region {
            address: base + u64::from(index) * 80,
            bytes: &data,
            known: None,
        }])?;
        let mut input = Inputs {
            constants: constants.clone(),
            sample_index: Some(0),
            interpolants: [
                (interp(0x90, Mode::Constant, Location::Default), index),
                (interp(0x70, Mode::Pass, Location::Default), 1.25f32.to_bits()),
                (interp(0x74, Mode::Pass, Location::Default), 2.25f32.to_bits()),
                (interp(0x7c, Mode::Pass, Location::Offset(0)), 2f32.to_bits()),
            ]
            .into(),
            ..Default::default()
        };
        for (i, component) in normal.into_iter().enumerate() {
            input.interpolants.insert(
                interp(0x80 + i as u16 * 4, Mode::Perspective, Location::Offset(0)),
                (component * 2.).to_bits(),
            );
        }
        let output = program
            .interpret_with_resources(
                &input,
                InterpretOptions {
                    step_budget: 10000,
                    float_model: FloatModel::SoftwareIeee,
                    special_functions: SpecialFunctionModel::HostF32,
                },
                Providers { memory: Some(&mut memory), ..Default::default() },
            )?
            .into_outputs()
            .context("reference invocation was discarded")?;
        let irradiance = record.irradiance(normal)?;
        for channel in 0..3 {
            let expected = irradiance[channel]
                * (diffuse[channel] * modulation[channel])
                * f32::from_bits(0x3ea2f983);
            let Value::Word(word) = output[channel] else { unreachable!() };
            let actual = f32::from_bits(word);
            assert!(
                (actual - expected).abs() <= 1e-5 * expected.abs().max(1.),
                "case {case}, channel {channel}: {actual} != {expected}"
            );
        }
        assert_eq!(output[3], Value::Word(1f32.to_bits()));
    }
    Ok(())
}

#[test]
#[ignore = "requires MPR_MATERIAL_ARCHIVE pointing to the extracted startup MaterialArchive.arc"]
fn record_layout_matches_paired_archive_reflection() -> Result<()> {
    use anyhow::Context;

    use crate::format::{
        material_archive::MaterialArchive, shader_dxbc::Dxbc, shader_nvn::NvnReflection,
        shader_sdx::SdxShader,
    };
    let bytes = std::fs::read(std::env::var("MPR_MATERIAL_ARCHIVE")?)?;
    let (archive, _) = MaterialArchive::slice(&bytes)?;
    let dx = archive.backends.iter().find(|b| b.id == *b"SDX ").context("No SDX backend")?;
    let nvn = archive.backends.iter().find(|b| b.id == *b"SNVN").context("No SNVN backend")?;
    for (id, hash, permutation, dx_index, nvn_index, buffer_name) in [
        ("761388bc-974e-4b92-b00d-c312e89cd868", 0xe3eb59fb, 0, 271, 98, "rw_outputProbeBuffer"),
        (
            "33e72f05-2050-48f7-9df7-10ff8565c799",
            0x46f8fbaa,
            0,
            577,
            328,
            "uc_bakedLightProbeCoefficients",
        ),
        (
            "33e72f05-2050-48f7-9df7-10ff8565c799",
            0x46f8fbaa,
            1,
            579,
            329,
            "uc_bakedLightProbeCoefficients",
        ),
    ] {
        let id = uuid::Uuid::parse_str(id)?;
        for (backend, expected_source) in [(dx, dx_index), (nvn, nvn_index)] {
            let material = backend
                .materials
                .iter()
                .find(|m| m.id == id && m.hash == hash)
                .context("No matching probe material")?;
            let shader = material
                .shaders
                .iter()
                .find(|s| {
                    s.identifier.technique == b'x' && s.identifier.permutation == [permutation, 0]
                })
                .context("No matching probe permutation")?;
            let source = if buffer_name == "rw_outputProbeBuffer" {
                if backend.id == *b"SDX " {
                    shader.compute_program
                } else {
                    shader.vertex_program
                }
            } else {
                shader.pixel_program
            };
            assert_eq!(source, expected_source);
            if backend.id == *b"SNVN" {
                let reflection = NvnReflection::parse(shader.metadata)?;
                let stage = if buffer_name == "rw_outputProbeBuffer" { 0 } else { 1 };
                assert!(reflection.resources.iter().any(|r| r.kind == 1 && r.bindings[stage] == 0));
            }
        }
        let shader = SdxShader::parse(dx.sources[dx_index as usize].binary)?;
        let reflection = Dxbc::parse(&shader.dxbc)?.reflection.context("Missing probe RDEF")?;
        let buffer = reflection
            .buffers
            .iter()
            .find(|b| b.name == buffer_name)
            .context("Missing probe buffer")?;
        let element = buffer
            .variables
            .iter()
            .find(|v| v.name == "$Element")
            .context("Missing buffer element")?;
        assert_eq!(element.size as usize, NonlinearProbe::BYTE_LEN);
        let record = &reflection.types[&element.type_offset];
        assert_eq!(record.name, Some("SLightProbeNonlinear"));
        assert_eq!(record.members.len(), 7);
        for (member, (name, offset, columns)) in record.members.iter().zip([
            ("mMidpointRGB", 0x00, 3),
            ("mUnused0", 0x0c, 1),
            ("mRangeRGB", 0x10, 3),
            ("mUnused1", 0x1c, 1),
            ("mGradientDirExponentR", 0x20, 4),
            ("mGradientDirExponentG", 0x30, 4),
            ("mGradientDirExponentB", 0x40, 4),
        ]) {
            assert_eq!((member.name, member.offset), (name, offset));
            let ty = &reflection.types[&member.type_offset];
            assert_eq!((ty.base_type, ty.rows, ty.columns, ty.elements), (3, 1, columns, 0));
        }
        let binding = reflection
            .bindings
            .iter()
            .find(|b| b.name == buffer_name)
            .context("Missing probe binding")?;
        assert_eq!(binding.sample_count as usize, NonlinearProbe::BYTE_LEN);
    }
    Ok(())
}
