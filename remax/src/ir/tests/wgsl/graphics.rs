use std::collections::BTreeMap;

mod clipping;
mod defaults;
mod dual_source;
mod implicit;
mod original;
mod outputs;
mod quad_faults;
mod raster;
mod render;

use super::gpu::Gpu;
use crate::{
    ir::tests::{attributes::access, bytes, mov, EXIT},
    maxwell::{
        graphics::{self, FragmentFrequency, GraphicsProgram, Options, StageSource},
        MaxwellCode, ShaderProgramHeader,
    },
    program::interpolation::OffsetModel,
    resources::ConstantMemory,
};

fn header(vertex: bool, modes: &[(u16, u32)]) -> ShaderProgramHeader {
    let mut words = [0u32; 20];
    words[0] = if vertex { 1 | (3 << 5) | (1 << 10) } else { 2 | (3 << 5) | (5 << 10) };
    if vertex {
        // Position outputs and generic inputs/outputs used by the fixture.
        for offset in [0x70u16, 0x74, 0x78, 0x7c] {
            let bit = 400 + usize::from(offset / 4);
            words[bit / 32] |= 1 << (bit % 32);
        }
        for base in [192, 432] {
            for i in 0..32 {
                words[(base + i) / 32] |= 1 << ((base + i) % 32);
            }
        }
    } else {
        words[5] = 0xf0000000;
        words[18] = 15;
        for &(offset, mode) in modes {
            let bit = 192 + usize::from((offset - 0x80) / 4) * 2;
            words[bit / 32] |= mode << (bit % 32);
        }
    }
    ShaderProgramHeader::parse(&words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>())
        .unwrap()
}

fn vertex() -> Vec<u64> {
    vec![
        access(true, 0, 0x80, 4),
        access(false, 0, 0x70, 4),
        access(true, 4, 0x90, 4),
        access(false, 4, 0x80, 4),
        access(true, 8, 0xa0, 4),
        access(false, 8, 0x90, 4),
        EXIT,
    ]
}

fn ipa(destination: u8, attribute: u16, mode: u8, location: u8) -> u64 {
    (crate::ir::tests::interpolation::ipa(mode, location) & !(0x3ff << 28))
        | (u64::from(attribute) << 28)
        | u64::from(destination)
}

fn options(frequency: FragmentFrequency) -> Options {
    let mut settings = Options::new(frequency);
    settings.vertex.approximate_arithmetic = true;
    settings.fragment.approximate_arithmetic = true;
    settings.fragment.approximate_special_functions = true;
    settings.fragment.offset_interpolation = Some(OffsetModel::SignedSixteenths);
    settings
}

fn translate(fragment: &[u64], modes: &[(u16, u32)], options: Options) -> GraphicsProgram {
    let vertex_code = bytes(&vertex());
    let fragment_code = bytes(fragment);
    graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex_code).unwrap(), header: &header(true, &[]) },
        StageSource {
            code: MaxwellCode::parse(&fragment_code).unwrap(),
            header: &header(false, modes),
        },
        options,
        &[],
        &[],
    )
    .unwrap()
}

fn validate(program: &GraphicsProgram) {
    let modern_fragment = program.quad_faults.is_some()
        || program.color_routing == graphics::ColorOutputRouting::DualSource;
    for (module, requires_modern_wgsl) in
        [(&program.vertex, program.clip_distance_mask != 0), (&program.fragment, modern_fragment)]
    {
        // Naga 0.11 predates quad, clip-distance and dual-source builtins.
        // Retain its coverage only for modules that do not need those features.
        if !requires_modern_wgsl {
            let old = naga::front::wgsl::parse_str(&module.source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&module.source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&old)
            .unwrap();
        }
        let new = naga_next::front::wgsl::parse_str(&module.source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&module.source)));
        naga_next::valid::Validator::new(
            naga_next::valid::ValidationFlags::all(),
            naga_next::valid::Capabilities::all(),
        )
        .validate(&new)
        .unwrap();
    }
}

fn perspective_fragment(byte: u32) -> Vec<u64> {
    vec![
        mov(6, (byte << 8) | (byte << 20), 7),
        ipa(5, 0x7c, 0, 2),
        0x5080000000470505, // MUFU.RCP R5,R5
        ipa(0, 0x80, 1, 2),
        ipa(1, 0x84, 0, 0),
        ipa(2, 0x88, 2, 0),
        mov(3, 1f32.to_bits(), 7),
        EXIT,
    ]
}

#[test]
fn graphics_linker_packs_varyings_and_validates_both_stage_modules() {
    let program = translate(
        &perspective_fragment(0x4c),
        &[(0x80, 2), (0x84, 3), (0x88, 1)],
        options(FragmentFrequency::Fragment),
    );
    assert_eq!(program.varying_locations, 2);
    assert!(program.vertex.source.contains(" / output.position.w"));
    assert!(
        program.fragment.source.find("let gradient").unwrap()
            < program.fragment.source.rfind("let result = remax_main").unwrap()
    );
    assert!(program
        .vertex
        .kernel
        .outputs
        .contains(&crate::program::Register::OutputAttribute(0x88)));
    assert!(!program
        .vertex
        .kernel
        .outputs
        .contains(&crate::program::Register::OutputAttribute(0x8c)));
    validate(&program);
    let mut fragment_code = perspective_fragment(0);
    fragment_code[4] = ipa(1, 0x84, 0, 1); // centroid becomes sample only at sample frequency
    for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
        let program =
            translate(&fragment_code, &[(0x80, 2), (0x84, 3), (0x88, 1)], options(frequency));
        let qualifier = if frequency == FragmentFrequency::Sample {
            "linear, sample"
        } else {
            "linear, centroid"
        };
        assert!(program.fragment.source.contains(qualifier));
        validate(&program);
    }
    let program = translate(
        &[ipa(0, 0x3fc, 2, 0), mov(1, 0, 7), mov(2, 0, 7), mov(3, 1f32.to_bits(), 7), EXIT],
        &[],
        options(FragmentFrequency::Fragment),
    );
    assert!(program.fragment.source.contains("@builtin(front_facing)"));
    validate(&program);
}

#[test]
fn graphics_linker_rejects_missing_maps_bindings_and_runtime_contracts() {
    let vertex_code = bytes(&vertex());
    let fragment_code = bytes(&perspective_fragment(0));
    let vertex_header = header(true, &[]);
    let fragment_header = header(false, &[(0x80, 2), (0x84, 3), (0x88, 1)]);
    let run = |vertex_header_override: &ShaderProgramHeader,
               fragment_header_override: &ShaderProgramHeader,
               settings| {
        graphics::translate(
            StageSource {
                code: MaxwellCode::parse(&vertex_code).unwrap(),
                header: vertex_header_override,
            },
            StageSource {
                code: MaxwellCode::parse(&fragment_code).unwrap(),
                header: fragment_header_override,
            },
            settings,
            &[],
            &[],
        )
    };
    let mut settings = options(FragmentFrequency::Fragment);
    settings.vertex.approximate_arithmetic = false;
    assert!(run(&vertex_header, &fragment_header, settings)
        .unwrap_err()
        .message
        .contains("vertex arithmetic"));
    let mut settings = options(FragmentFrequency::Fragment);
    settings.vertex_inputs = settings.faults;
    assert!(run(&vertex_header, &fragment_header, settings)
        .unwrap_err()
        .message
        .contains("overlapping"));
    let mut vertex_header_override = vertex_header.clone();
    vertex_header_override.words[13] &= !(1 << 12);
    assert!(run(&vertex_header_override, &fragment_header, options(FragmentFrequency::Fragment))
        .unwrap_err()
        .message
        .contains("vertex output"));
    let mut fragment_header_override = fragment_header.clone();
    fragment_header_override.words[6] = 0;
    assert!(run(&vertex_header, &fragment_header_override, options(FragmentFrequency::Fragment))
        .unwrap_err()
        .message
        .contains("absent from SPH"));
    let mut fragment_header_override = fragment_header.clone();
    fragment_header_override.words[18] = 3;
    assert!(run(&vertex_header, &fragment_header_override, options(FragmentFrequency::Fragment))
        .unwrap_err()
        .message
        .contains("RGBA"));
    fragment_header_override.words[18] = 0xff;
    assert!(run(&vertex_header, &fragment_header_override, options(FragmentFrequency::Fragment))
        .unwrap_err()
        .message
        .contains("broadcast"));
    fragment_header_override.words[18] = 15;
    fragment_header_override.words[19] = 4;
    assert!(run(&vertex_header, &fragment_header_override, options(FragmentFrequency::Fragment))
        .unwrap_err()
        .message
        .contains("known pixel IO"));
    let fragment_code =
        bytes(&[0xefe8e0028007ff00, mov(1, 0, 7), mov(2, 0, 7), mov(3, 1f32.to_bits(), 7), EXIT]);
    let result = graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex_code).unwrap(), header: &vertex_header },
        StageSource { code: MaxwellCode::parse(&fragment_code).unwrap(), header: &fragment_header },
        options(FragmentFrequency::Fragment),
        &[],
        &[],
    );
    assert!(result.unwrap_err().message.contains("representative-sample"));
}

// An oversized triangle with unequal clip W. Authored values reproduce a chosen
// affine numerator in window coordinates; hardware performs the rasterization.
fn vertices() -> Vec<BTreeMap<u16, u32>> {
    [(0f32, 0f32, 1f32), (16., 0., 2.), (0., 16., 4.)]
        .into_iter()
        .map(|(x, y, w)| {
            [
                (0x80, (x / 4. - 1.) * w),
                (0x84, (1. - y / 4.) * w),
                (0x88, 0.5 * w),
                (0x8c, w),
                (0x90, (0.25 + x / 64. + y / 32.) * w),
                (0x94, 0.125 + x / 32. - y / 64.),
                (0x98, 0.375),
                (0x9c, 0.),
                (0xa0, 0.),
                (0xa4, 0.),
                (0xa8, 0.),
                (0xac, 0.),
            ]
            .into_iter()
            .map(|(a, v)| (a, v.to_bits()))
            .collect()
        })
        .collect()
}
