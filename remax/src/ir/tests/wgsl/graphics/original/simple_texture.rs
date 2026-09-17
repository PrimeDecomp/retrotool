use std::collections::BTreeMap;

use super::{
    bytes, graphics, options, parse_header, validate, ConstantMemory, FragmentFrequency, Gpu,
    GraphicsProgram, MaxwellCode, Resources, StageSource,
};
use crate::{
    ir::tests::wgsl::graphics::render::{render_with_bind_groups, Rendered},
    maxwell::texture::TextureDimension,
    program::wgsl::{
        texture::{Binding, TextureBinding, TextureFilter, TextureSampling, TextureValueType},
        FaultScope,
    },
};

mod code;
mod image;

const UV_TRANSFORM: [f32; 4] = [0.125, -0.0625, 1.5, 0.75];
const TINT: [f32; 4] = [0.75, 0.5, 0.875, 0.625];

// This is a concrete RGBA8-UNORM view used by the fixture. Reflection only
// establishes PS texture slot0 -> descriptor word8; it cannot supply the format.
fn binding(base_mip: u32) -> Binding {
    Binding {
        view: TextureBinding {
            texture_index: 8,
            dimension: TextureDimension::D2,
            value_type: TextureValueType::Float,
            multisample: false,
            // An explicit LOD must ignore the sampler's implicit-LOD bias.
            lod_bias: 1.25,
            sampling: Some(TextureSampling {
                mip_level_count: image::LEVELS - base_mip,
                min_filter: TextureFilter::Linear,
                mag_filter: TextureFilter::Linear,
                max_anisotropy: 1,
            }),
        },
        group: 1,
        texture_binding: 0,
        sampler_binding: Some(1),
        depth_components: None,
    }
}

fn program(binding: Binding) -> Result<GraphicsProgram, crate::program::Diagnostic> {
    let vertex = bytes(code::VERTEX);
    let fragment = bytes(code::FRAGMENT);
    graphics::translate(
        StageSource {
            code: MaxwellCode::parse(&vertex).unwrap(),
            header: &parse_header(&code::VERTEX_HEADER),
        },
        StageSource {
            code: MaxwellCode::parse(&fragment).unwrap(),
            header: &parse_header(&code::FRAGMENT_HEADER),
        },
        options(FragmentFrequency::Sample),
        &[],
        &[binding],
    )
}

#[test]
fn original_simple_texture_links_a_declared_view_and_preserves_explicit_lod() {
    let program = program(binding(0)).unwrap();
    assert_eq!(program.fragment.kernel.fault_scope, FaultScope::Invocation);
    let resource = &program.fragment.kernel.textures[0];
    assert_eq!(resource.binding.view.texture_index, 8);
    assert!(resource.uses_sampler);
    assert!(!resource.uses_implicit_lod);
    assert!(resource.helper_results_optional);
    assert!(program.vertex.kernel.textures.is_empty());
    assert_eq!(program.varying_locations, 1);
    validate(&program);
}

#[test]
fn original_simple_texture_rejects_incompatible_resource_declarations() {
    let mut declared = binding(0);
    declared.view.dimension = TextureDimension::D3;
    assert!(program(declared).unwrap_err().message.contains("dimension"));
    declared = binding(0);
    declared.view.texture_index = 0; // API slot0 is not descriptor word0.
    assert!(program(declared).unwrap_err().message.contains("declared host binding"));
    declared = binding(0);
    declared.view.value_type = TextureValueType::Uint;
    assert!(program(declared).unwrap_err().message.contains("integer filtered sampling"));
    declared = binding(0);
    declared.sampler_binding = None;
    assert!(program(declared).unwrap_err().message.contains("sampler binding"));
}

fn vertex_constants() -> ConstantMemory {
    // Projective clip transform produces W=1,2,4 and screen coordinates
    // (0,0), (16,0), (0,16). The original vertex shader also applies UV affine.
    let columns = [[1f32, 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 0.5, 1.], [0., 0., 0., 0.]];
    let transform = columns.into_iter().flatten().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
    let uv = UV_TRANSFORM.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
    ConstantMemory::new(&[(3, &transform), (4, &uv)]).unwrap()
}

fn vertices() -> [BTreeMap<u16, u32>; 3] {
    [([-1f32, 1., 1.], [0f32, 0.]), ([6., 2., 2.], [2., 0.]), ([-4., -12., 4.], [0., 2.])].map(
        |(position, uv)| {
            let mut attributes = BTreeMap::new();
            for (component, value) in position.into_iter().enumerate() {
                attributes.insert(0x80 + component as u16 * 4, value.to_bits());
            }
            for (component, value) in uv.into_iter().enumerate() {
                attributes.insert(0x90 + component as u16 * 4, value.to_bits());
            }
            attributes
        },
    )
}

fn fragment_constants(lod: f32, offset: u8) -> ConstantMemory {
    // A single offset is repeated for every sample. Grid masks remain zero,
    // so the original address computation always reads the first table byte.
    let mut driver = vec![0; 0x640];
    driver[0x620..0x630].fill(offset);
    let values = TINT.into_iter().chain([lod]).flat_map(f32::to_le_bytes).collect::<Vec<_>>();
    ConstantMemory::new(&[(0, &driver), (3, &values)]).unwrap()
}

fn expected_uv(x: f64, y: f64) -> [f64; 2] {
    let weights = [1. - x / 16. - y / 16., x / 16., y / 16.];
    let reciprocal_w = weights[0] + weights[1] / 2. + weights[2] / 4.;
    let uv = [weights[1] / reciprocal_w, weights[2] / (2. * reciprocal_w)];
    std::array::from_fn(|component| {
        uv[component] * f64::from(UV_TRANSFORM[component + 2]) + f64::from(UV_TRANSFORM[component])
    })
}

fn draw(
    gpu: &Gpu,
    program: &GraphicsProgram,
    vertices: &[BTreeMap<u16, u32>],
    samples: u32,
    resources: Resources<'_>,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> Rendered {
    let binding = program.fragment.kernel.textures[0].binding;
    render_with_bind_groups(gpu, program, vertices, samples, resources, |pipeline| {
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("original SimpleTexture view"),
            layout: &pipeline.get_bind_group_layout(binding.group),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: binding.texture_binding,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: binding.sampler_binding.unwrap(),
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        vec![(binding.group, group)]
    })
}

pub(in crate::ir::tests::wgsl::graphics) fn render_simple_texture(gpu: &Gpu) -> usize {
    let texture = image::upload(gpu);
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let vertex_constants = vertex_constants();
    let vertices = vertices();
    let mut checks = 0;
    for base_mip in [0, 1] {
        let declared = binding(base_mip);
        let program = program(declared).unwrap();
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: base_mip,
            mip_level_count: Some(image::LEVELS - base_mip),
            ..Default::default()
        });
        for lod in [0f32, 1., 1.5, 3.75] {
            for (offset, delta) in
                [(0u8, [0., 0.]), (0x4c, [-0.25, 0.25]), (0x77, [0.4375, 0.4375])]
            {
                let fragment_constants = fragment_constants(lod, offset);
                for samples in [1, 4] {
                    let rendered = draw(
                        gpu,
                        &program,
                        &vertices,
                        samples,
                        Resources {
                            vertex_constants: Some(&vertex_constants),
                            fragment_constants: Some(&fragment_constants),
                            ..Default::default()
                        },
                        &view,
                        &sampler,
                    );
                    assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
                    for (pixel, actual) in rendered.colors[&0].iter().enumerate() {
                        let uv = expected_uv(
                            (pixel % 8) as f64 + 0.5 + delta[0],
                            (pixel / 8) as f64 + 0.5 + delta[1],
                        );
                        let texel = image::sample(uv, f64::from(lod), base_mip);
                        for channel in 0..4 {
                            let expected = texel[channel] * f64::from(TINT[channel]);
                            // Allow host bilinear precision and the MSAA UNORM
                            // resolve. Wrong mip/view/UV choices exceed this.
                            assert!(
                                (f64::from(actual[channel]) - expected).abs() < 1.5 / 255.,
                                "base {base_mip}, LOD {lod}, offset {offset:#x}, samples {samples}, pixel {pixel}, channel {channel}: {actual:?} != {texel:?} * {TINT:?}"
                            );
                        }
                    }
                    checks += 64;
                }
            }
        }
    }
    // Validate the original program's resource guards at draw time as well as
    // its valid colors. A mismatched view must not silently sample another mip.
    let program = program(binding(0)).unwrap();
    let view = texture
        .create_view(&wgpu::TextureViewDescriptor { base_mip_level: 1, ..Default::default() });
    let constants = fragment_constants(0., 0);
    let rendered = draw(
        gpu,
        &program,
        &vertices,
        1,
        Resources {
            vertex_constants: Some(&vertex_constants),
            fragment_constants: Some(&constants),
            ..Default::default()
        },
        &view,
        &sampler,
    );
    assert_eq!(rendered.faults, [0, u32::MAX, 128, 0xe8]);
    let view = texture.create_view(&Default::default());
    let constants = fragment_constants(f32::NAN, 0);
    let rendered = draw(
        gpu,
        &program,
        &vertices,
        1,
        Resources {
            vertex_constants: Some(&vertex_constants),
            fragment_constants: Some(&constants),
            ..Default::default()
        },
        &view,
        &sampler,
    );
    assert_eq!(rendered.faults, [0, u32::MAX, 256, 0xe8]);
    checks
}
