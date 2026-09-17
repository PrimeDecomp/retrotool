//! Linked implicit sampling, including constant reads that fail only in helpers.
use std::collections::BTreeMap;

mod demote;
mod flow;
mod predication;

use super::{
    bytes, graphics, header, ipa, options,
    quad_faults::single_pixel,
    render::{render_with_bind_groups, Resources},
    validate, vertex, vertices, ConstantMemory, FragmentFrequency, Gpu, GraphicsProgram,
    MaxwellCode, StageSource, EXIT,
};
use crate::{
    maxwell::{graphics::QuadFaultMethod, texture::TextureDimension},
    program::wgsl::{
        texture::{Binding, TextureBinding, TextureValueType},
        FaultScope,
    },
    resources::ConstantBank,
};

fn prefix() -> Vec<u64> {
    vec![
        ipa(12, 0x88, 0, 0), // linear constant-memory byte address
        0x5cb0000000c70a0c,  // F2I.U32.F32 R12,R12, nearest
        0xef94003000070c06,  // LDC R6,c3[R12], U coordinate
        ipa(10, 0x84, 0, 0), // linear V coordinate
        0xd830123040a70600,  // implicit TEXS R0/R1/R4/R5, R6/R10
        0x5c98078000470002,  // MOV R2,R4
        0x5c98078000570003,  // MOV R3,R5
    ]
}

fn program(frequency: FragmentFrequency, bias: f32) -> GraphicsProgram {
    let mut words = prefix();
    words.push(EXIT);
    link(&words, frequency, bias, 65_536)
}

fn link(
    words: &[u64],
    frequency: FragmentFrequency,
    bias: f32,
    step_budget: u32,
) -> GraphicsProgram {
    link_profile(words, frequency, bias, step_budget, Default::default())
}

fn link_profile(
    words: &[u64],
    frequency: FragmentFrequency,
    bias: f32,
    step_budget: u32,
    profile: crate::program::wgsl::Profile,
) -> GraphicsProgram {
    let vertex_code = bytes(&vertex());
    let fragment_code = bytes(words);
    let mut settings = options(frequency);
    settings.quad_faults = Some(QuadFaultMethod::QuadBroadcast);
    settings.fragment.step_budget = step_budget;
    settings.fragment.profile = profile;
    let program = graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex_code).unwrap(), header: &header(true, &[]) },
        StageSource {
            code: MaxwellCode::parse(&fragment_code).unwrap(),
            header: &header(false, &[(0x84, 3), (0x88, 3)]),
        },
        settings,
        &[],
        &[Binding {
            view: TextureBinding {
                texture_index: 0x123,
                dimension: TextureDimension::D2,
                value_type: TextureValueType::Float,
                multisample: false,
                lod_bias: bias,
                sampling: None,
            },
            group: 1,
            texture_binding: 0,
            sampler_binding: Some(1),
            depth_components: None,
        }],
    )
    .unwrap();
    assert_eq!(program.fragment.kernel.fault_scope, FaultScope::Quad);
    program
}

fn set_varyings(vertices: &mut [BTreeMap<u16, u32>]) {
    for vertex in vertices {
        let w = f32::from_bits(vertex[&0x8c]);
        let x = (f32::from_bits(vertex[&0x80]) / w + 1.) * 4.;
        let y = (1. - f32::from_bits(vertex[&0x84]) / w) * 4.;
        vertex.insert(0x94, (y / 8.).to_bits());
        // At pixel centers this is exactly 4 * (column + 8 * row).
        vertex.insert(0x98, (4. * (x + 8. * y) - 18.).to_bits());
    }
}

fn constants(unknown: Option<[u32; 2]>) -> ConstantMemory {
    let data = (0..64)
        .flat_map(|index| ((index % 8) as f32 / 8. + 0.0625).to_le_bytes())
        .collect::<Vec<_>>();
    let mut known = vec![true; data.len()];
    if let Some([x, y]) = unknown {
        let offset = (4 * (x + 8 * y)) as usize;
        known[offset..offset + 4].fill(false);
    }
    ConstantMemory::from_banks(&[ConstantBank { bank: 3, bytes: &data, known: Some(&known) }])
        .unwrap()
}

fn mip_texture(gpu: &Gpu) -> wgpu::TextureView {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("implicit LOD fixture"),
        size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
        mip_level_count: 5,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, color) in
        [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255], [
            255, 0, 255, 255,
        ]]
        .into_iter()
        .enumerate()
    {
        let size = 16 >> level;
        let data = (0..size * size).flat_map(|_| color).collect::<Vec<_>>();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { mip_level: level as u32, ..texture.as_image_copy() },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&Default::default())
}

#[test]
fn implicit_graphics_with_indirect_constants_validates() {
    for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
        validate(&program(frequency, 0.));
    }
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn implicit_graphics_reports_helper_faults_on_native_gpu() {
    let gpu =
        Gpu::with_features(wgpu::Features::VERTEX_WRITABLE_STORAGE | wgpu::Features::SUBGROUP);
    let view = mip_texture(&gpu);
    let sampler = gpu.device.create_sampler(&Default::default());
    let mut draws = 0;
    for (frequency, samples) in [(FragmentFrequency::Fragment, 1), (FragmentFrequency::Sample, 4)] {
        for bias in [0., 1.] {
            let program = program(frequency, bias);
            let draw = |vertices: &[BTreeMap<u16, u32>], constants: &ConstantMemory| {
                render_with_bind_groups(
                    &gpu,
                    &program,
                    vertices,
                    samples,
                    Resources { fragment_constants: Some(constants), ..Default::default() },
                    |pipeline| {
                        vec![(
                            1,
                            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: None,
                                layout: &pipeline.get_bind_group_layout(1),
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(&view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(&sampler),
                                    },
                                ],
                            }),
                        )]
                    },
                )
            };
            let mut full = vertices();
            set_varyings(&mut full);
            let rendered = draw(&full, &constants(None));
            assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
            let color = if bias == 0. { [0., 1., 0., 1.] } else { [0., 0., 1., 1.] };
            assert!(rendered.colors[&0].iter().all(|pixel| *pixel == color));
            draws += 1;
            for covered_lane in 0..4 {
                let pixel = [2 + covered_lane % 2, 2 + covered_lane / 2];
                let mut small = single_pixel(pixel);
                set_varyings(&mut small);
                let rendered = draw(&small, &constants(None));
                assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
                assert!(rendered.colors[&0][(pixel[0] + pixel[1] * 8) as usize][3] > 0.);
                draws += 1;
                for fault_lane in 0..4 {
                    let unknown = [2 + fault_lane % 2, 2 + fault_lane / 2];
                    let rendered = draw(&small, &constants(Some(unknown)));
                    assert_eq!(
                        rendered.faults,
                        [0, u32::MAX, 8, 24],
                        "{frequency:?} covered {covered_lane}, faulty {fault_lane}, bias {bias}"
                    );
                    assert!(rendered.colors[&0].iter().all(|pixel| *pixel == [0.; 4]));
                    draws += 1;
                }
            }
        }
    }
    draws += flow::compare(&gpu, &view, &sampler);
    println!("Checked {draws} linked implicit-LOD draws, including divergent tails and helper-only faults");
}
