use std::io::Write;

use anyhow::{ensure, Result};
use ddsfile::{AlphaMode, D3D10ResourceDimension, DxgiFormat, NewDxgiParams};

use crate::format::txtr::{ETextureFormat, ETextureType, STextureHeader};

pub fn write_dds<W: Write>(w: &mut W, head: &STextureHeader, data: Vec<u8>) -> Result<()> {
    let mut dds = ddsfile::Dds::new_dxgi(NewDxgiParams {
        height: head.height,
        width: head.width,
        depth: None,
        format: to_dxgi_format(head.format),
        mipmap_levels: Some(head.mip_sizes.len() as u32),
        array_layers: Some(head.layers),
        caps2: None,
        is_cubemap: matches!(head.kind, ETextureType::Cube | ETextureType::CubeArray),
        resource_dimension: match head.kind {
            ETextureType::_1D | ETextureType::_1DArray => D3D10ResourceDimension::Texture1D,
            ETextureType::_2D
            | ETextureType::_2DArray
            | ETextureType::_2DMultisample
            | ETextureType::_2DMultisampleArray
            | ETextureType::Cube
            | ETextureType::CubeArray => D3D10ResourceDimension::Texture2D,
            ETextureType::_3D => D3D10ResourceDimension::Texture3D,
        },
        alpha_mode: AlphaMode::Unknown,
    })?;
    // FIXME: ddsfile ASTC size calc is broken
    if !head.format.is_astc() {
        //ensure!(dds.data.len() == data.len());
    }
    dds.data = data;
    dds.write(w)?;
    Ok(())
}

fn to_dxgi_format(format: ETextureFormat) -> DxgiFormat {
    match format {
        ETextureFormat::R8Unorm => DxgiFormat::R8_UNorm,
        ETextureFormat::R8Snorm => DxgiFormat::R8_SNorm,
        ETextureFormat::R8Uint => DxgiFormat::R8_UInt,
        ETextureFormat::R8Sint => DxgiFormat::R8_SInt,
        ETextureFormat::R16Unorm => DxgiFormat::R16_UNorm,
        ETextureFormat::R16Snorm => DxgiFormat::R16_SNorm,
        ETextureFormat::R16Uint => DxgiFormat::R16_UInt,
        ETextureFormat::R16Sint => DxgiFormat::R16_SInt,
        ETextureFormat::R16Float => DxgiFormat::R16_Float,
        ETextureFormat::R32Uint => DxgiFormat::R32_UInt,
        ETextureFormat::R32Sint => DxgiFormat::R32_SInt,
        ETextureFormat::Rgb8Unorm => DxgiFormat::Unknown,
        ETextureFormat::Rgba8Unorm => DxgiFormat::R8G8B8A8_UNorm,
        ETextureFormat::Rgba8Srgb => DxgiFormat::R8G8B8A8_UNorm_sRGB,
        ETextureFormat::Rgba16Float => DxgiFormat::R16G16B16A16_Float,
        ETextureFormat::Rgba32Float => DxgiFormat::R32G32B32A32_Float,
        ETextureFormat::Depth16Unorm => DxgiFormat::D16_UNorm,
        ETextureFormat::Depth16Unorm2 => DxgiFormat::D16_UNorm,
        ETextureFormat::Depth24S8Unorm => DxgiFormat::D24_UNorm_S8_UInt,
        ETextureFormat::Depth32Float => DxgiFormat::D32_Float,
        ETextureFormat::RgbaBc1Unorm => DxgiFormat::BC1_UNorm,
        ETextureFormat::RgbaBc1Srgb => DxgiFormat::BC1_UNorm_sRGB,
        ETextureFormat::RgbaBc2Unorm => DxgiFormat::BC2_UNorm,
        ETextureFormat::RgbaBc2Srgb => DxgiFormat::BC2_UNorm_sRGB,
        ETextureFormat::RgbaBc3Unorm => DxgiFormat::BC3_UNorm,
        ETextureFormat::RgbaBc3Srgb => DxgiFormat::BC3_UNorm_sRGB,
        ETextureFormat::RgbaBc4Unorm => DxgiFormat::BC4_UNorm,
        ETextureFormat::RgbaBc4Snorm => DxgiFormat::BC4_SNorm,
        ETextureFormat::RgbaBc5Unorm => DxgiFormat::BC5_UNorm,
        ETextureFormat::RgbaBc5Snorm => DxgiFormat::BC5_SNorm,
        ETextureFormat::Rg11B10Float => DxgiFormat::R11G11B10_Float,
        ETextureFormat::R32Float => DxgiFormat::R32_Float,
        ETextureFormat::Rg8Unorm => DxgiFormat::R8G8_UNorm,
        ETextureFormat::Rg8Snorm => DxgiFormat::R8G8_SNorm,
        ETextureFormat::Rg8Uint => DxgiFormat::R8G8_UInt,
        ETextureFormat::Rg8Sint => DxgiFormat::R8G8_SInt,
        ETextureFormat::Rg16Float => DxgiFormat::R16G16_Float,
        ETextureFormat::Rg16Unorm => DxgiFormat::R16G16_UNorm,
        ETextureFormat::Rg16Snorm => DxgiFormat::R16G16_SNorm,
        ETextureFormat::Rg16Uint => DxgiFormat::R16G16_UInt,
        ETextureFormat::Rg16Sint => DxgiFormat::R16G16_SInt,
        ETextureFormat::Rgb10A2Unorm => DxgiFormat::R10G10B10A2_UNorm,
        ETextureFormat::Rgb10A2Uint => DxgiFormat::R10G10B10A2_UInt,
        ETextureFormat::Rg32Uint => DxgiFormat::R32G32_UInt,
        ETextureFormat::Rg32Sint => DxgiFormat::R32G32_SInt,
        ETextureFormat::Rg32Float => DxgiFormat::R32G32_Float,
        ETextureFormat::Rgba16Unorm => DxgiFormat::R16G16B16A16_UNorm,
        ETextureFormat::Rgba16Snorm => DxgiFormat::R16G16B16A16_SNorm,
        ETextureFormat::Rgba16Uint => DxgiFormat::R16G16B16A16_UInt,
        ETextureFormat::Rgba16Sint => DxgiFormat::R16G16B16A16_SInt,
        ETextureFormat::Rgba32Uint => DxgiFormat::R32G32B32A32_UInt,
        ETextureFormat::Rgba32Sint => DxgiFormat::R32G32B32A32_SInt,
        ETextureFormat::None => DxgiFormat::Unknown,
        ETextureFormat::RgbaAstc4x4 => DxgiFormat::ASTC_4x4_UNorm,
        ETextureFormat::RgbaAstc5x4 => DxgiFormat::ASTC_5x4_UNorm,
        ETextureFormat::RgbaAstc5x5 => DxgiFormat::ASTC_5x5_UNorm,
        ETextureFormat::RgbaAstc6x5 => DxgiFormat::ASTC_6x5_UNorm,
        ETextureFormat::RgbaAstc6x6 => DxgiFormat::ASTC_6x6_UNorm,
        ETextureFormat::RgbaAstc8x5 => DxgiFormat::ASTC_8x5_UNorm,
        ETextureFormat::RgbaAstc8x6 => DxgiFormat::ASTC_8x6_UNorm,
        ETextureFormat::RgbaAstc8x8 => DxgiFormat::ASTC_8x8_UNorm,
        ETextureFormat::RgbaAstc10x5 => DxgiFormat::ASTC_10x5_UNorm,
        ETextureFormat::RgbaAstc10x6 => DxgiFormat::ASTC_10x6_UNorm,
        ETextureFormat::RgbaAstc10x8 => DxgiFormat::ASTC_10x8_UNorm,
        ETextureFormat::RgbaAstc10x10 => DxgiFormat::ASTC_10x10_UNorm,
        ETextureFormat::RgbaAstc12x10 => DxgiFormat::ASTC_12x10_UNorm,
        ETextureFormat::RgbaAstc12x12 => DxgiFormat::ASTC_12x12_UNorm,
        ETextureFormat::RgbaAstc4x4Srgb => DxgiFormat::ASTC_4x4_UNorm_sRGB,
        ETextureFormat::RgbaAstc5x4Srgb => DxgiFormat::ASTC_5x4_UNorm_sRGB,
        ETextureFormat::RgbaAstc5x5Srgb => DxgiFormat::ASTC_5x5_UNorm_sRGB,
        ETextureFormat::RgbaAstc6x5Srgb => DxgiFormat::ASTC_6x5_UNorm_sRGB,
        ETextureFormat::RgbaAstc6x6Srgb => DxgiFormat::ASTC_6x6_UNorm_sRGB,
        ETextureFormat::RgbaAstc8x5Srgb => DxgiFormat::ASTC_8x5_UNorm_sRGB,
        ETextureFormat::RgbaAstc8x6Srgb => DxgiFormat::ASTC_8x6_UNorm_sRGB,
        ETextureFormat::RgbaAstc8x8Srgb => DxgiFormat::ASTC_8x8_UNorm_sRGB,
        ETextureFormat::RgbaAstc10x5Srgb => DxgiFormat::ASTC_10x5_UNorm_sRGB,
        ETextureFormat::RgbaAstc10x6Srgb => DxgiFormat::ASTC_10x6_UNorm_sRGB,
        ETextureFormat::RgbaAstc10x8Srgb => DxgiFormat::ASTC_10x8_UNorm_sRGB,
        ETextureFormat::RgbaAstc10x10Srgb => DxgiFormat::ASTC_10x10_UNorm_sRGB,
        ETextureFormat::RgbaAstc12x10Srgb => DxgiFormat::ASTC_12x10_UNorm_sRGB,
        ETextureFormat::RgbaAstc12x12Srgb => DxgiFormat::ASTC_12x12_UNorm_sRGB,
        ETextureFormat::BptcUfloat => DxgiFormat::BC6H_UF16,
        ETextureFormat::BptcSfloat => DxgiFormat::BC6H_SF16,
        ETextureFormat::BptcUnorm => DxgiFormat::BC7_UNorm,
        ETextureFormat::BptcUnormSrgb => DxgiFormat::BC7_UNorm_sRGB,
    }
}
