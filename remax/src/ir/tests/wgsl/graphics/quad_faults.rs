//! Exercise helper-exclusive faults with real partial primitive coverage.

use std::collections::BTreeMap;

use super::{
    bytes, graphics, header, ipa, mov, options,
    render::{render, Resources},
    translate, validate, vertex, ConstantMemory, FragmentFrequency, Gpu, GraphicsProgram,
    MaxwellCode, StageSource, EXIT,
};
use crate::{
    maxwell::{graphics::QuadFaultMethod, texture::TextureDimension},
    program::wgsl::{
        texture::{Binding, TextureBinding, TextureValueType},
        FaultScope,
    },
};

const TRANSPORT: &str = include_str!("../../../../maxwell/graphics/emit/faults.wgsl");
const STATUSES: [u32; 4] = [0x80000001, 0x7fff8000, 0x01010001, 0xffffffff];
const PCS: [u32; 4] = [0x12345678, 0xfffe4321, 0xfedcba98, 0xabcdef07];

fn transport_probe(frequency: FragmentFrequency) -> GraphicsProgram {
    // A c0 reference establishes the normal packed constant-memory binding.
    let mut program = translate(
        &[0x4c98078000070000, mov(1, 0, 7), mov(2, 0, 7), mov(3, 0, 7), EXIT],
        &[],
        options(frequency),
    );
    let (sample_argument, sample_mask) = match frequency {
        FragmentFrequency::Fragment => ("", "4294967295u"),
        FragmentFrequency::Sample => (", @builtin(sample_index) sample: u32", "1u << sample"),
    };
    // Isolate the actual backend helper used by the linker. Control cases select
    // local reporting after the collective has run, showing why atomics alone
    // cannot observe a helper-only failure. Both modes return a coverage marker.
    program.quad_faults = Some(QuadFaultMethod::QuadBroadcast);
    program.fragment.source = format!(
        r#"{}
{TRANSPORT}
@group(0) @binding(3) var<storage, read_write> remax_draw_faults: array<atomic<u32>, 4>;
struct ProbeOutput {{
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) mask: u32,
}}
@fragment
fn remax_fragment_main(@builtin(position) position: vec4<f32>{sample_argument}) -> ProbeOutput {{
    let pixel = vec2<u32>(floor(position.xy));
    let lane = (pixel.x & 1u) | ((pixel.y & 1u) << 1u);
    var memory_status = 0u;
    let settings = remax_ldc(0u, 32u, 4u, &memory_status).x;
    let has_fault = (settings & (1u << lane)) != 0u;
    let status = select(0u, remax_ldc(0u, lane * 4u, 4u, &memory_status).x, has_fault);
    let pc = remax_ldc(0u, 16u + lane * 4u, 4u, &memory_status).x;
    let collective = remax_quad_fault(status, pc);
    let fault = select(vec2<u32>(status, pc), collective, (settings & 65536u) != 0u);
    if (fault.x != 0u) {{
        atomicOr(&remax_draw_faults[2], fault.x);
        atomicMin(&remax_draw_faults[3], fault.y);
    }}
    var output: ProbeOutput;
    output.color = vec4<f32>(1.0);
    output.mask = {sample_mask};
    return output;
}}
"#,
        program.fragment.kernel.source
    );
    program
}

// This triangle touches one pixel only. Other positions in its raster quad
// must be helpers, rather than covered fragments hidden by a late discard.
pub(super) fn single_pixel(pixel: [u32; 2]) -> Vec<BTreeMap<u16, u32>> {
    [[0.05f32, 0.05f32], [0.95, 0.05], [0.5, 0.95]]
        .map(|point| {
            let x = pixel[0] as f32 + point[0];
            let y = pixel[1] as f32 + point[1];
            let mut vertex =
                (0x80..0xb0).step_by(4).map(|offset| (offset, 0u32)).collect::<BTreeMap<_, _>>();
            for (offset, value) in
                [(0x80, x / 4. - 1.), (0x84, 1. - y / 4.), (0x88, 0.5), (0x8c, 1.)]
            {
                vertex.insert(offset, value.to_bits());
            }
            vertex
        })
        .into()
}

fn constants(mask: u32, collective: bool) -> ConstantMemory {
    let words = STATUSES.into_iter().chain(PCS).chain([mask | (u32::from(collective) << 16)]);
    let bytes = words.flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    ConstantMemory::new(&[(0, &bytes)]).unwrap()
}

#[test]
fn helper_transport_validates_with_current_shader_compiler() {
    for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
        validate(&transport_probe(frequency));
    }
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn helper_faults_transfer_on_native_gpu() {
    let gpu =
        Gpu::with_features(wgpu::Features::VERTEX_WRITABLE_STORAGE | wgpu::Features::SUBGROUP);
    let mut draws = 0;
    for (frequency, samples) in [(FragmentFrequency::Fragment, 1), (FragmentFrequency::Sample, 4)] {
        let program = transport_probe(frequency);
        for covered_lane in 0..4 {
            let pixel = [2 + covered_lane % 2, 2 + covered_lane / 2];
            let vertices = single_pixel(pixel);
            for mask in 0u32..16 {
                for collective in [false, true] {
                    let constants = constants(mask, collective);
                    let rendered = render(&gpu, &program, &vertices, samples, Resources {
                        fragment_constants: Some(&constants),
                        ..Default::default()
                    });
                    let observed_mask = if collective { mask } else { mask & (1 << covered_lane) };
                    let mut expected = [0u32, u32::MAX, 0, u32::MAX];
                    for lane in 0..4 {
                        if observed_mask & (1 << lane) != 0 {
                            expected[2] |= STATUSES[lane];
                            expected[3] = expected[3].min(PCS[lane]);
                        }
                    }
                    assert_eq!(
                        rendered.faults, expected,
                        "{frequency:?} lane{covered_lane} mask{mask:x} collective{collective}"
                    );
                    for (index, color) in rendered.colors[&0].iter().enumerate() {
                        if index == (pixel[0] + pixel[1] * 8) as usize {
                            assert!(color[3] > 0., "probe's pixel was not covered");
                        } else {
                            assert_eq!(*color, [0.; 4], "unexpected covered pixel {index}");
                        }
                    }
                    draws += 1;
                }
            }
        }
    }
    println!("Checked {draws} partial-coverage draws, including helper-only faults and local-reporting controls");
}

#[test]
fn implicit_graphics_requires_a_selected_helper_fault_method() {
    let vertex = bytes(&vertex());
    let fragment = bytes(&[
        ipa(6, 0x80, 0, 0),
        ipa(10, 0x84, 0, 0),
        0xd830123040a70600, // TEXS: descriptor0x123, UV R6/R10 -> R0/R1/R4/R5
        0x5c98078000470002,
        0x5c98078000570003,
        EXIT,
    ]);
    let vertex_header = header(true, &[]);
    let fragment_header = header(false, &[(0x80, 3), (0x84, 3)]);
    let binding = Binding {
        view: TextureBinding {
            texture_index: 0x123,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            lod_bias: 0.,
            sampling: None,
        },
        group: 1,
        texture_binding: 0,
        sampler_binding: Some(1),
        depth_components: None,
    };
    let run = |settings| {
        graphics::translate(
            StageSource { code: MaxwellCode::parse(&vertex).unwrap(), header: &vertex_header },
            StageSource { code: MaxwellCode::parse(&fragment).unwrap(), header: &fragment_header },
            settings,
            &[],
            &[binding],
        )
    };
    let mut settings = options(FragmentFrequency::Fragment);
    assert!(run(settings).unwrap_err().message.contains("helper-aware"));
    settings.quad_faults = Some(QuadFaultMethod::QuadBroadcast);
    let program = run(settings).unwrap();
    assert_eq!(program.quad_faults, settings.quad_faults);
    assert_eq!(program.fragment.kernel.fault_scope, FaultScope::Quad);
    validate(&program);
}
