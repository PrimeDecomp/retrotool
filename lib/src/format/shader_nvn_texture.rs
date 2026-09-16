//! Convert MPR texture views and resolved NVN samplers into translator inputs.

use anyhow::{bail, ensure, Result};
use remax::{
    maxwell::texture::TextureDimension,
    wgsl::texture::{TextureBinding, TextureFilter, TextureSampling, TextureValueType},
};

use super::{
    texture_sampler::TextureSamplerParams,
    txtr::{ETextureFormat, ETextureType, STextureHeader},
};

pub fn texture_sampling_from_sampler(
    mip_level_count: u32,
    sampler: &TextureSamplerParams,
) -> Result<TextureSampling> {
    let [min, mag] = sampler.nvn_filters();
    let sampling = TextureSampling {
        mip_level_count,
        min_filter: if min == 3 || min == 5 {
            TextureFilter::Linear
        } else {
            TextureFilter::Nearest
        },
        mag_filter: if mag == 1 { TextureFilter::Linear } else { TextureFilter::Nearest },
        max_anisotropy: sampler.max_anisotropy(),
    };
    sampling.validate()?;
    Ok(sampling)
}

/// Declare the TXTR's default full view. Pass the resolved per-use sampler
/// (including sample-shading/streaming overrides), not necessarily its header
/// defaults. Explicit runtime views must use texture_binding_from_view instead.
pub fn texture_binding_from_header(
    texture_index: u16,
    header: &STextureHeader,
    sampler: &TextureSamplerParams,
) -> Result<TextureBinding> {
    let mut binding =
        texture_binding_from_view(texture_index, header.kind, header.format, sampler)?;
    binding.sampling =
        Some(texture_sampling_from_sampler(header.mip_sizes.len().try_into()?, sampler)?);
    Ok(binding)
}

/// Derive a declaration from an actual engine view's type and format.
/// This describes the binding, not support for every instruction that might
/// access it. It neither creates a GPU view nor infers a dimension override
/// from the shader. Ordinary 2D and one-layer 2D-array views remain distinct.
/// Depth formats select their default depth aspect; stencil views need a
/// separate explicit declaration/provider.
pub fn texture_binding_from_view(
    texture_index: u16,
    kind: ETextureType,
    format: ETextureFormat,
    sampler: &TextureSamplerParams,
) -> Result<TextureBinding> {
    ensure!(texture_index < 1 << 13, "texture binding index exceeds the shader's 13-bit field");
    ensure!(
        sampler.lod_bias.is_finite() && (-16.0..16.0).contains(&sampler.lod_bias),
        "texture LOD bias is outside the supported WGSL range"
    );
    let (dimension, multisample) = match kind {
        ETextureType::D1 => (TextureDimension::D1, false),
        ETextureType::D2 => (TextureDimension::D2, false),
        ETextureType::D3 => (TextureDimension::D3, false),
        ETextureType::Cube => (TextureDimension::Cube, false),
        ETextureType::D2Array => (TextureDimension::D2Array, false),
        ETextureType::D2Multisample => (TextureDimension::D2, true),
        _ => bail!("unsupported host texture view type {kind}"),
    };
    use ETextureFormat::*;
    let value_type = match format {
        None => bail!("texture view has no format"),
        R8Uint | R16Uint | R32Uint | Rg8Uint | Rg16Uint | Rgb10A2Uint | Rg32Uint | Rgba16Uint
        | Rgba32Uint => TextureValueType::Uint,
        R8Sint | R16Sint | R32Sint | Rg8Sint | Rg16Sint | Rg32Sint | Rgba16Sint | Rgba32Sint => {
            TextureValueType::Sint
        }
        Depth16Unorm | Depth16Unorm2 | Depth24S8Unorm | Depth32Float => TextureValueType::Depth,
        _ => TextureValueType::Float,
    };
    ensure!(
        value_type != TextureValueType::Depth
            || matches!(dimension, TextureDimension::D2 | TextureDimension::D2Array),
        "unsupported depth texture dimension"
    );
    Ok(TextureBinding {
        texture_index,
        dimension,
        value_type,
        multisample,
        lod_bias: sampler.lod_bias,
        sampling: Option::None,
    })
}

#[cfg(test)]
mod binding_tests {
    use binrw::BinReaderExt;

    use super::*;

    #[test]
    fn header_provider_preserves_array_dimension_and_resolved_sampler_bias() {
        // A one-layer 2D-array header: kind 5, RGBA8-sRGB, 64x32, one mip.
        let mut bytes = [5u32, 13, 64, 32, 1, 0, 0, 1, 8192, 2]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        bytes.extend([1, 1, 0, 0, 0, 255]);
        let mut header: STextureHeader = std::io::Cursor::new(bytes).read_le().unwrap();
        let sampler =
            TextureSamplerParams::from_header(&header.sampler_data).sample_shading_variant();
        let binding = texture_binding_from_header(8, &header, &sampler).unwrap();
        assert_eq!(binding.dimension, TextureDimension::D2Array);
        assert_eq!(binding.value_type, TextureValueType::Float);
        assert_eq!(binding.lod_bias, -1.0);
        header.kind = ETextureType::D2;
        assert_eq!(
            texture_binding_from_header(8, &header, &sampler).unwrap().dimension,
            TextureDimension::D2
        );
    }

    #[test]
    fn resource_type_comes_from_format_and_unsupported_views_reject() {
        let mut sampler = TextureSamplerParams::simple(1, [0; 3], -1);
        for (format, expected) in [
            (ETextureFormat::RgbaBc4Snorm, TextureValueType::Float),
            (ETextureFormat::BptcUfloat, TextureValueType::Float),
            (ETextureFormat::Rgba16Uint, TextureValueType::Uint),
            (ETextureFormat::Rg32Sint, TextureValueType::Sint),
            (ETextureFormat::Depth24S8Unorm, TextureValueType::Depth),
        ] {
            let binding = texture_binding_from_view(8, ETextureType::D2, format, &sampler).unwrap();
            assert_eq!(binding.value_type, expected);
        }
        let binding = texture_binding_from_view(
            8,
            ETextureType::D2Multisample,
            ETextureFormat::R8Uint,
            &sampler,
        )
        .unwrap();
        assert!(binding.multisample);
        assert_eq!(binding.dimension, TextureDimension::D2);
        for kind in
            [ETextureType::D1Array, ETextureType::D2MultisampleArray, ETextureType::CubeArray]
        {
            assert!(texture_binding_from_view(8, kind, ETextureFormat::R8Uint, &sampler).is_err());
        }
        assert!(
            texture_binding_from_view(8, ETextureType::D2, ETextureFormat::None, &sampler).is_err()
        );
        assert!(texture_binding_from_view(
            8192,
            ETextureType::D2,
            ETextureFormat::R8Uint,
            &sampler
        )
        .is_err());
        sampler.lod_bias = f32::NAN;
        assert!(texture_binding_from_view(8, ETextureType::D2, ETextureFormat::R8Uint, &sampler)
            .is_err());
    }
}
