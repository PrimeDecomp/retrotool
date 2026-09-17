use Register::Gpr as R;

use super::{gpu::Gpu, options};
use crate::{
    ir::tests::{
        interpolation::{ipa, program},
        mov, EXIT,
    },
    program::{
        interpolation::{Component, OffsetModel},
        wgsl::{Input, Options},
        Register,
    },
};

#[test]
fn plane_faults_preserve_quad_participation_before_implicit_sampling() {
    use crate::{
        maxwell::texture::TextureDimension,
        program::wgsl::{texture::TextureValueType, FaultScope},
    };
    let p = program(&[ipa(0, 2) | 6, 0xd830012ff0770606, EXIT], &[R(6), R(7)], &[R(6)]);
    let mut binding = super::texture::binding(TextureDimension::D2, TextureValueType::Float);
    binding.view.texture_index = 0x12;
    let k = p
        .emit_wgsl_with_textures(
            Options { offset_interpolation: Some(OffsetModel::SignedSixteenths), ..options() },
            &[binding],
        )
        .unwrap();
    super::texture::validate(&k).unwrap();
    assert_eq!(k.fault_scope, FaultScope::Quad);
    let main = k.source.split("fn remax_main").nth(1).unwrap();
    let sample = main.rfind("textureSampleBias(").unwrap();
    assert!(!main[..sample].contains("return remax_fault"));
}

pub(super) fn raster_planes(gpu: &Gpu) -> usize {
    let p = program(
        &[ipa(0, 2), mov(1, 0, 7), mov(2, 0, 7), mov(3, 1f32.to_bits(), 7), EXIT],
        &[R(6)],
        &[R(0), R(1), R(2), R(3)],
    );
    let k = p
        .emit_wgsl(Options {
            offset_interpolation: Some(OffsetModel::SignedSixteenths),
            ..options()
        })
        .unwrap();
    let mut total = 0;
    for (byte, delta) in [(0x00u32, 0f32), (0x4c, 0.25), (0x88, -1.5), (0x77, 1.3125)] {
        let packed = (byte << 20) | (byte << 8);
        let words = k
            .inputs
            .iter()
            .map(|input| match input {
                Input::Register(R(6)) => format!("{packed}u"),
                Input::InterpolationPlane { component, .. } => {
                    let plane = "(position.x + 2.0*position.y)";
                    let value = match component {
                        Component::Center => plane.to_string(),
                        Component::Dx => format!("dpdx({plane})"),
                        Component::Dy => format!("dpdy({plane})"),
                    };
                    format!("bitcast<u32>({value})")
                }
                _ => panic!("unexpected input {input:?}"),
            })
            .collect::<Vec<_>>();
        let valid = vec!["1u".to_string(); words.len()];
        // Derivatives are formed by the fragment wrapper before calling the
        // kernel. This test uses 1x rasterization and native host window axes.
        let actual = gpu.render_with_resources(&k, &words, &valid, [8, 8], |_, _, _| vec![]);
        for (pixel, result) in actual.iter().enumerate() {
            let expected = (pixel % 8) as f32 + 0.5 + 2. * ((pixel / 8) as f32 + 0.5) + delta;
            assert_eq!(result, &[expected.to_bits(), 0, 0, 1f32.to_bits(), 0, 0, 0]);
        }
        total += actual.len();
    }
    total
}
