use std::{
    cmp::max,
    fmt::{Display, Formatter},
    io::Cursor,
    marker::PhantomData,
    num::NonZeroUsize,
    ops::Range,
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use binrw::{binrw, BinReaderExt, Endian};
use image::{
    DynamicImage, GrayImage, ImageBuffer, Luma, LumaA, Pixel, Rgb, RgbImage, Rgba, Rgba32FImage,
    RgbaImage,
};
use tegra_swizzle::surface::BlockDim;
use zerocopy::ByteOrder;

use crate::{
    format::{chunk::ChunkDescriptor, rfrm::FormDescriptor, FourCC},
    util::compression::decompress_into,
};

// Texture
pub const K_FORM_TXTR: FourCC = FourCC(*b"TXTR");
// Texture header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// GPU data
pub const K_CHUNK_GPU: FourCC = FourCC(*b"GPU ");

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ETextureType {
    D1 = 0,
    D2 = 1,
    D3 = 2,
    Cube = 3,
    D1Array = 4,
    D2Array = 5,
    D2Multisample = 6,
    D2MultisampleArray = 7,
    CubeArray = 8,
}

impl Display for ETextureType {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.write_str(match self {
            ETextureType::D1 => "1D",
            ETextureType::D2 => "2D",
            ETextureType::D3 => "3D",
            ETextureType::Cube => "Cube",
            ETextureType::D1Array => "1D Array",
            ETextureType::D2Array => "2D Array",
            ETextureType::D2Multisample => "2D Multisample",
            ETextureType::D2MultisampleArray => "2D Multisample Array",
            ETextureType::CubeArray => "Cube Array",
        })
    }
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ETextureWrap {
    ClampToEdge = 0,
    Repeat = 1,
    MirroredRepeat = 2,
    MirrorClamp = 3,
    ClampToBorder = 4,
    Clamp = 5,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ETextureFilter {
    Nearest = 0,
    Linear = 1,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ETextureMipFilter {
    Nearest = 0,
    Linear = 1,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ETextureAnisotropicRatio {
    None = u8::MAX,
    Ratio1 = 0,
    Ratio2 = 1,
    Ratio4 = 2,
    Ratio8 = 3,
    Ratio16 = 4,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureHeader {
    pub kind: ETextureType,
    pub format: ETextureFormat,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub components: [u8; 4],
    // pub tile_mode: u32,
    // pub swizzle: u32,
    #[bw(try_calc = mip_sizes.len().try_into())]
    pub mip_count: u32,
    #[br(count = mip_count)]
    pub mip_sizes: Vec<u32>,
    pub sampler_data: STextureSamplerData,
}

#[binrw]
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct STextureSamplerData {
    pub unk: u32,
    pub filter: ETextureFilter,
    pub mip_filter: ETextureMipFilter,
    pub wrap_x: ETextureWrap,
    pub wrap_y: ETextureWrap,
    pub wrap_z: ETextureWrap,
    pub aniso: ETextureAnisotropicRatio,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureReadInfo {
    pub index: u8,
    pub offset: u32,
    pub size: u32,
}

// #[binrw]
// #[derive(Clone, Debug)]
// pub struct STextureCompressedBufferInfo {
//     pub index: u32,
//     pub offset: u32,
//     pub size: u32,
//     pub dest_offset: u32,
//     pub dest_size: u32,
// }

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureCompressedBufferInfo2 {
    pub first_index: u32,
    pub first_size: u32,
    pub first_dest_offset: u32,
    pub first_dest_size: u32,
    pub unk: u32,
    pub second_index: u32,
    pub second_size: u32,
    pub second_dest_offset: u32,
    pub second_dest_size: u32,
    // Within the second buffer, part of the data may be uncompressed.
    pub second_decompressed_len: u32,
    pub second_compressed_len: u32,
    pub second_compressed_offset: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureMetaData {
    pub unk1: u32,
    pub unk2: u32,
    pub alloc_category: u32,
    pub gpu_offset: u32,
    pub align: u32,
    pub decompressed_size: u32,
    #[bw(try_calc = info.len().try_into())]
    pub info_count: u32,
    #[br(count = info_count)]
    pub info: Vec<STextureReadInfo>,
    pub buffers: STextureCompressedBufferInfo2,
    // #[bw(try_calc = buffers.len().try_into())]
    // pub buffer_count: u32,
    // #[br(count = buffer_count)]
    // pub buffers: Vec<STextureCompressedBufferInfo>,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ETextureFormat {
    R8Unorm = 0,
    R8Snorm = 1,
    R8Uint = 2,
    R8Sint = 3,
    R16Unorm = 4,
    R16Snorm = 5,
    R16Uint = 6,
    R16Sint = 7,
    R16Float = 8,
    R32Uint = 9,
    R32Sint = 10,
    Rgb8Unorm = 11,
    Rgba8Unorm = 12,
    Rgba8Srgb = 13,
    Rgba16Float = 14,
    Rgba32Float = 15,
    Depth16Unorm = 16,
    Depth16Unorm2 = 17, // ?
    Depth24S8Unorm = 18,
    Depth32Float = 19,
    RgbaBc1Unorm = 20, // DXT1
    RgbaBc1Srgb = 21,  // DXT1
    RgbaBc2Unorm = 22, // DXT3
    RgbaBc2Srgb = 23,  // DXT3
    RgbaBc3Unorm = 24, // DXT5
    RgbaBc3Srgb = 25,  // DXT5
    RgbaBc4Unorm = 26, // RGTC1
    RgbaBc4Snorm = 27, // RGTC1
    RgbaBc5Unorm = 28, // RGTC2
    RgbaBc5Snorm = 29, // RGTC2
    Rg11B10Float = 30,
    R32Float = 31,
    Rg8Unorm = 32,
    Rg8Snorm = 33,
    Rg8Uint = 34,
    Rg8Sint = 35,
    Rg16Float = 36,
    Rg16Unorm = 37,
    Rg16Snorm = 38,
    Rg16Uint = 39,
    Rg16Sint = 40,
    Rgb10A2Unorm = 41,
    Rgb10A2Uint = 42,
    Rg32Uint = 43,
    Rg32Sint = 44,
    Rg32Float = 45,
    Rgba16Unorm = 46,
    Rgba16Snorm = 47,
    Rgba16Uint = 48,
    Rgba16Sint = 49,
    Rgba32Uint = 50,
    Rgba32Sint = 51,
    None = 52,
    RgbaAstc4x4 = 53,
    RgbaAstc5x4 = 54,
    RgbaAstc5x5 = 55,
    RgbaAstc6x5 = 56,
    RgbaAstc6x6 = 57,
    RgbaAstc8x5 = 58,
    RgbaAstc8x6 = 59,
    RgbaAstc8x8 = 60,
    RgbaAstc10x5 = 61,
    RgbaAstc10x6 = 62,
    RgbaAstc10x8 = 63,
    RgbaAstc10x10 = 64,
    RgbaAstc12x10 = 65,
    RgbaAstc12x12 = 66,
    RgbaAstc4x4Srgb = 67,
    RgbaAstc5x4Srgb = 68,
    RgbaAstc5x5Srgb = 69,
    RgbaAstc6x5Srgb = 70,
    RgbaAstc6x6Srgb = 71,
    RgbaAstc8x5Srgb = 72,
    RgbaAstc8x6Srgb = 73,
    RgbaAstc8x8Srgb = 74,
    RgbaAstc10x5Srgb = 75,
    RgbaAstc10x6Srgb = 76,
    RgbaAstc10x8Srgb = 77,
    RgbaAstc10x10Srgb = 78,
    RgbaAstc12x10Srgb = 79,
    RgbaAstc12x12Srgb = 80,
    BptcUfloat = 81,
    BptcSfloat = 82,
    BptcUnorm = 83,
    BptcUnormSrgb = 84,
}

impl Display for ETextureFormat {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        f.write_str(match self {
            ETextureFormat::R8Unorm => "R8 UNORM",
            ETextureFormat::R8Snorm => "R8 SNORM",
            ETextureFormat::R8Uint => "R8 UINT",
            ETextureFormat::R8Sint => "R8 SINT",
            ETextureFormat::R16Unorm => "R16 UNORM",
            ETextureFormat::R16Snorm => "R16 SNORM",
            ETextureFormat::R16Uint => "R16 UINT",
            ETextureFormat::R16Sint => "R16 SINT",
            ETextureFormat::R16Float => "R16 FLOAT",
            ETextureFormat::R32Uint => "R32 UINT",
            ETextureFormat::R32Sint => "R32 SINT",
            ETextureFormat::Rgb8Unorm => "RGB8 UNORM",
            ETextureFormat::Rgba8Unorm => "RGBA8 UNORM",
            ETextureFormat::Rgba8Srgb => "RGBA8 UNORM (sRGB)",
            ETextureFormat::Rgba16Float => "RGBA16 FLOAT",
            ETextureFormat::Rgba32Float => "RGBA32 FLOAT",
            ETextureFormat::Depth16Unorm | ETextureFormat::Depth16Unorm2 => "D16 UNORM",
            ETextureFormat::Depth24S8Unorm => "D24S8 UNORM",
            ETextureFormat::Depth32Float => "D32 FLOAT",
            ETextureFormat::RgbaBc1Unorm => "BC1 UNORM",
            ETextureFormat::RgbaBc1Srgb => "BC1 UNORM (sRGB)",
            ETextureFormat::RgbaBc2Unorm => "BC2 UNORM",
            ETextureFormat::RgbaBc2Srgb => "BC2 UNORM (sRGB)",
            ETextureFormat::RgbaBc3Unorm => "BC3 UNORM",
            ETextureFormat::RgbaBc3Srgb => "BC3 UNORM (sRGB)",
            ETextureFormat::RgbaBc4Unorm => "BC4 UNORM",
            ETextureFormat::RgbaBc4Snorm => "BC4 SNORM",
            ETextureFormat::RgbaBc5Unorm => "BC5 UNORM",
            ETextureFormat::RgbaBc5Snorm => "BC5 SNORM",
            ETextureFormat::Rg11B10Float => "RG11B10 FLOAT",
            ETextureFormat::R32Float => "R32 FLOAT",
            ETextureFormat::Rg8Unorm => "RG8 UNORM",
            ETextureFormat::Rg8Snorm => "RG8 SNORM",
            ETextureFormat::Rg8Uint => "RG8 UINT",
            ETextureFormat::Rg8Sint => "RG16 SINT",
            ETextureFormat::Rg16Float => "RG16 FLOAT",
            ETextureFormat::Rg16Unorm => "RG16 UNORM",
            ETextureFormat::Rg16Snorm => "RG16 SNORM",
            ETextureFormat::Rg16Uint => "RG16 UINT",
            ETextureFormat::Rg16Sint => "RG16 SINt",
            ETextureFormat::Rgb10A2Unorm => "RGB10A2 UNORM",
            ETextureFormat::Rgb10A2Uint => "RGB10A2 UINT",
            ETextureFormat::Rg32Uint => "RG32 UINT",
            ETextureFormat::Rg32Sint => "RG32 SINT",
            ETextureFormat::Rg32Float => "RG32 FLOAT",
            ETextureFormat::Rgba16Unorm => "RGBA16 UNORM",
            ETextureFormat::Rgba16Snorm => "RGBA16 SNORM",
            ETextureFormat::Rgba16Uint => "RGBA16 UINT",
            ETextureFormat::Rgba16Sint => "RGBA16 SINT",
            ETextureFormat::Rgba32Uint => "RGBA32 UINT",
            ETextureFormat::Rgba32Sint => "RGBA32 SINT",
            ETextureFormat::None => "[unknown]",
            ETextureFormat::RgbaAstc4x4 => "ASTC 4x4",
            ETextureFormat::RgbaAstc5x4 => "ASTC 5x4",
            ETextureFormat::RgbaAstc5x5 => "ASTC 5x5",
            ETextureFormat::RgbaAstc6x5 => "ASTC 6x5",
            ETextureFormat::RgbaAstc6x6 => "ASTC 6x6",
            ETextureFormat::RgbaAstc8x5 => "ASTC 8x5",
            ETextureFormat::RgbaAstc8x6 => "ASTC 8x6",
            ETextureFormat::RgbaAstc8x8 => "ASTC 8x8",
            ETextureFormat::RgbaAstc10x5 => "ASTC 10x5",
            ETextureFormat::RgbaAstc10x6 => "ASTC 10x6",
            ETextureFormat::RgbaAstc10x8 => "ASTC 10x8",
            ETextureFormat::RgbaAstc10x10 => "ASTC 10x10",
            ETextureFormat::RgbaAstc12x10 => "ASTC 12x10",
            ETextureFormat::RgbaAstc12x12 => "ASTC 12x12",
            ETextureFormat::RgbaAstc4x4Srgb => "ASTC 4x4 (sRGB)",
            ETextureFormat::RgbaAstc5x4Srgb => "ASTC 5x4 (sRGB)",
            ETextureFormat::RgbaAstc5x5Srgb => "ASTC 5x5 (sRGB)",
            ETextureFormat::RgbaAstc6x5Srgb => "ASTC 6x5 (sRGB)",
            ETextureFormat::RgbaAstc6x6Srgb => "ASTC 6x6 (sRGB)",
            ETextureFormat::RgbaAstc8x5Srgb => "ASTC 8x5 (sRGB)",
            ETextureFormat::RgbaAstc8x6Srgb => "ASTC 8x6 (sRGB)",
            ETextureFormat::RgbaAstc8x8Srgb => "ASTC 8x8 (sRGB)",
            ETextureFormat::RgbaAstc10x5Srgb => "ASTC 10x5 (sRGB)",
            ETextureFormat::RgbaAstc10x6Srgb => "ASTC 10x6 (sRGB)",
            ETextureFormat::RgbaAstc10x8Srgb => "ASTC 10x8 (sRGB)",
            ETextureFormat::RgbaAstc10x10Srgb => "ASTC 10x10 (sRGB)",
            ETextureFormat::RgbaAstc12x10Srgb => "ASTC 12x10 (sRGB)",
            ETextureFormat::RgbaAstc12x12Srgb => "ASTC 12x12 (sRGB)",
            ETextureFormat::BptcUfloat => "BC6H UFLOAT",
            ETextureFormat::BptcSfloat => "BC6H SFLOAT",
            ETextureFormat::BptcUnorm => "BC7 UNORM",
            ETextureFormat::BptcUnormSrgb => "BC7 UNORM (sRGB)",
        })
    }
}

impl ETextureFormat {
    pub fn block_size(self) -> (u8, u8, u8) {
        match self {
            ETextureFormat::RgbaBc1Unorm
            | ETextureFormat::RgbaBc1Srgb
            | ETextureFormat::RgbaBc2Unorm
            | ETextureFormat::RgbaBc2Srgb
            | ETextureFormat::RgbaBc3Unorm
            | ETextureFormat::RgbaBc3Srgb
            | ETextureFormat::RgbaBc4Unorm
            | ETextureFormat::RgbaBc4Snorm
            | ETextureFormat::RgbaBc5Unorm
            | ETextureFormat::RgbaBc5Snorm
            | ETextureFormat::BptcUfloat
            | ETextureFormat::BptcSfloat
            | ETextureFormat::BptcUnorm
            | ETextureFormat::BptcUnormSrgb => (4, 4, 1),
            ETextureFormat::RgbaAstc4x4 | ETextureFormat::RgbaAstc4x4Srgb => (4, 4, 1),
            ETextureFormat::RgbaAstc5x4 | ETextureFormat::RgbaAstc5x4Srgb => (5, 4, 1),
            ETextureFormat::RgbaAstc5x5 | ETextureFormat::RgbaAstc5x5Srgb => (5, 5, 1),
            ETextureFormat::RgbaAstc6x5 | ETextureFormat::RgbaAstc6x5Srgb => (6, 5, 1),
            ETextureFormat::RgbaAstc6x6 | ETextureFormat::RgbaAstc6x6Srgb => (6, 6, 1),
            ETextureFormat::RgbaAstc8x5 | ETextureFormat::RgbaAstc8x5Srgb => (8, 5, 1),
            ETextureFormat::RgbaAstc8x6 | ETextureFormat::RgbaAstc8x6Srgb => (8, 6, 1),
            ETextureFormat::RgbaAstc8x8 | ETextureFormat::RgbaAstc8x8Srgb => (8, 8, 1),
            ETextureFormat::RgbaAstc10x5 | ETextureFormat::RgbaAstc10x5Srgb => (10, 5, 1),
            ETextureFormat::RgbaAstc10x6 | ETextureFormat::RgbaAstc10x6Srgb => (10, 6, 1),
            ETextureFormat::RgbaAstc10x8 | ETextureFormat::RgbaAstc10x8Srgb => (10, 8, 1),
            ETextureFormat::RgbaAstc10x10 | ETextureFormat::RgbaAstc10x10Srgb => (10, 10, 1),
            ETextureFormat::RgbaAstc12x10 | ETextureFormat::RgbaAstc12x10Srgb => (12, 10, 1),
            ETextureFormat::RgbaAstc12x12 | ETextureFormat::RgbaAstc12x12Srgb => (12, 12, 1),
            _ => (1, 1, 1),
        }
    }

    pub fn is_astc(self) -> bool {
        matches!(
            self,
            ETextureFormat::RgbaAstc4x4
                | ETextureFormat::RgbaAstc5x4
                | ETextureFormat::RgbaAstc5x5
                | ETextureFormat::RgbaAstc6x5
                | ETextureFormat::RgbaAstc6x6
                | ETextureFormat::RgbaAstc8x5
                | ETextureFormat::RgbaAstc8x6
                | ETextureFormat::RgbaAstc8x8
                | ETextureFormat::RgbaAstc10x5
                | ETextureFormat::RgbaAstc10x6
                | ETextureFormat::RgbaAstc10x8
                | ETextureFormat::RgbaAstc10x10
                | ETextureFormat::RgbaAstc12x10
                | ETextureFormat::RgbaAstc12x12
                | ETextureFormat::RgbaAstc4x4Srgb
                | ETextureFormat::RgbaAstc5x4Srgb
                | ETextureFormat::RgbaAstc5x5Srgb
                | ETextureFormat::RgbaAstc6x5Srgb
                | ETextureFormat::RgbaAstc6x6Srgb
                | ETextureFormat::RgbaAstc8x5Srgb
                | ETextureFormat::RgbaAstc8x6Srgb
                | ETextureFormat::RgbaAstc8x8Srgb
                | ETextureFormat::RgbaAstc10x5Srgb
                | ETextureFormat::RgbaAstc10x6Srgb
                | ETextureFormat::RgbaAstc10x8Srgb
                | ETextureFormat::RgbaAstc10x10Srgb
                | ETextureFormat::RgbaAstc12x10Srgb
                | ETextureFormat::RgbaAstc12x12Srgb
        )
    }

    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            ETextureFormat::Rgba8Srgb
                | ETextureFormat::RgbaBc1Srgb
                | ETextureFormat::RgbaBc2Srgb
                | ETextureFormat::RgbaBc3Srgb
                | ETextureFormat::RgbaAstc4x4Srgb
                | ETextureFormat::RgbaAstc5x4Srgb
                | ETextureFormat::RgbaAstc5x5Srgb
                | ETextureFormat::RgbaAstc6x5Srgb
                | ETextureFormat::RgbaAstc6x6Srgb
                | ETextureFormat::RgbaAstc8x5Srgb
                | ETextureFormat::RgbaAstc8x6Srgb
                | ETextureFormat::RgbaAstc8x8Srgb
                | ETextureFormat::RgbaAstc10x5Srgb
                | ETextureFormat::RgbaAstc10x6Srgb
                | ETextureFormat::RgbaAstc10x8Srgb
                | ETextureFormat::RgbaAstc10x10Srgb
                | ETextureFormat::RgbaAstc12x10Srgb
                | ETextureFormat::RgbaAstc12x12Srgb
                | ETextureFormat::BptcUnormSrgb
        )
    }

    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            ETextureFormat::R8Unorm
            | ETextureFormat::R8Snorm
            | ETextureFormat::R8Uint
            | ETextureFormat::R8Sint => 1,
            ETextureFormat::R16Unorm
            | ETextureFormat::R16Snorm
            | ETextureFormat::R16Uint
            | ETextureFormat::R16Sint
            | ETextureFormat::R16Float => 2,
            ETextureFormat::R32Uint | ETextureFormat::R32Sint => 4,
            ETextureFormat::Rgb8Unorm => 3,
            ETextureFormat::Rgba8Unorm | ETextureFormat::Rgba8Srgb => 4,
            ETextureFormat::Rgba16Float => 8,
            ETextureFormat::Rgba32Float => 16,
            ETextureFormat::Depth16Unorm | ETextureFormat::Depth16Unorm2 => 2,
            ETextureFormat::Depth24S8Unorm | ETextureFormat::Depth32Float => 4,
            ETextureFormat::RgbaBc1Unorm | ETextureFormat::RgbaBc1Srgb => 8,
            ETextureFormat::RgbaBc2Unorm
            | ETextureFormat::RgbaBc2Srgb
            | ETextureFormat::RgbaBc3Unorm
            | ETextureFormat::RgbaBc3Srgb => 16,
            ETextureFormat::RgbaBc4Unorm | ETextureFormat::RgbaBc4Snorm => 8,
            ETextureFormat::RgbaBc5Unorm | ETextureFormat::RgbaBc5Snorm => 16,
            ETextureFormat::Rg11B10Float | ETextureFormat::R32Float => 4,
            ETextureFormat::Rg8Unorm
            | ETextureFormat::Rg8Snorm
            | ETextureFormat::Rg8Uint
            | ETextureFormat::Rg8Sint => 2,
            ETextureFormat::Rg16Float
            | ETextureFormat::Rg16Unorm
            | ETextureFormat::Rg16Snorm
            | ETextureFormat::Rg16Uint
            | ETextureFormat::Rg16Sint => 4,
            ETextureFormat::Rgb10A2Unorm | ETextureFormat::Rgb10A2Uint => 4,
            ETextureFormat::Rg32Uint | ETextureFormat::Rg32Sint | ETextureFormat::Rg32Float => 8,
            ETextureFormat::Rgba16Unorm
            | ETextureFormat::Rgba16Snorm
            | ETextureFormat::Rgba16Uint
            | ETextureFormat::Rgba16Sint => 64,
            ETextureFormat::Rgba32Uint | ETextureFormat::Rgba32Sint => 128,
            ETextureFormat::None => 0,
            ETextureFormat::RgbaAstc4x4
            | ETextureFormat::RgbaAstc5x4
            | ETextureFormat::RgbaAstc5x5
            | ETextureFormat::RgbaAstc6x5
            | ETextureFormat::RgbaAstc6x6
            | ETextureFormat::RgbaAstc8x5
            | ETextureFormat::RgbaAstc8x6
            | ETextureFormat::RgbaAstc8x8
            | ETextureFormat::RgbaAstc10x5
            | ETextureFormat::RgbaAstc10x6
            | ETextureFormat::RgbaAstc10x8
            | ETextureFormat::RgbaAstc10x10
            | ETextureFormat::RgbaAstc12x10
            | ETextureFormat::RgbaAstc12x12
            | ETextureFormat::RgbaAstc4x4Srgb
            | ETextureFormat::RgbaAstc5x4Srgb
            | ETextureFormat::RgbaAstc5x5Srgb
            | ETextureFormat::RgbaAstc6x5Srgb
            | ETextureFormat::RgbaAstc6x6Srgb
            | ETextureFormat::RgbaAstc8x5Srgb
            | ETextureFormat::RgbaAstc8x6Srgb
            | ETextureFormat::RgbaAstc8x8Srgb
            | ETextureFormat::RgbaAstc10x5Srgb
            | ETextureFormat::RgbaAstc10x6Srgb
            | ETextureFormat::RgbaAstc10x8Srgb
            | ETextureFormat::RgbaAstc10x10Srgb
            | ETextureFormat::RgbaAstc12x10Srgb
            | ETextureFormat::RgbaAstc12x12Srgb
            | ETextureFormat::BptcUfloat
            | ETextureFormat::BptcSfloat
            | ETextureFormat::BptcUnorm
            | ETextureFormat::BptcUnormSrgb => 16,
        }
    }
}

fn deswizzle(header: &STextureHeader, data: &[u8]) -> Result<Vec<u8>> {
    let (bw, bh, bd) = header.format.block_size();
    let block_dim = BlockDim {
        width: NonZeroUsize::new(bw as usize).unwrap(),
        height: NonZeroUsize::new(bh as usize).unwrap(),
        depth: NonZeroUsize::new(bd as usize).unwrap(),
    };
    let bpp = header.format.bytes_per_pixel() as usize;
    let (depth, layers) = if header.kind == ETextureType::D3 {
        (header.layers as usize, 1)
    } else {
        (1, header.layers as usize)
    };
    let expected_size = tegra_swizzle::surface::swizzled_surface_size(
        header.width as usize,
        header.height as usize,
        depth,
        block_dim,
        None,
        bpp,
        header.mip_sizes.len(),
        layers,
    );
    ensure!(data.len() == expected_size);
    Ok(tegra_swizzle::surface::deswizzle_surface(
        header.width as usize,
        header.height as usize,
        depth,
        data,
        block_dim,
        None,
        bpp,
        header.mip_sizes.len(),
        layers,
    )?)
}

#[derive(Debug, Clone)]
pub struct TextureData<O: ByteOrder> {
    pub head: STextureHeader,
    pub data: Vec<u8>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> TextureData<O> {
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (txtr_desc, txtr_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(txtr_desc.id == K_FORM_TXTR);
        ensure!(txtr_desc.reader_version.get() == 65);
        ensure!(txtr_desc.writer_version.get() == 66);

        let (head_desc, head_data, _) = ChunkDescriptor::<O>::slice(txtr_data)?;
        ensure!(head_desc.id == K_CHUNK_HEAD);
        let head: STextureHeader = Cursor::new(head_data).read_type(Endian::Little)?;

        // log::debug!("META: {meta:#?}");
        // log::debug!("HEAD: {head:#?}");

        let meta: STextureMetaData = Cursor::new(meta).read_type(Endian::Little)?;
        let mut buffer = vec![0u8; meta.decompressed_size as usize];
        // for info in &meta.buffers {
        //     let (read_idx, read) = meta
        //         .info
        //         .iter()
        //         .enumerate()
        //         .find(|(_, i)| i.index as u32 == info.index)
        //         .ok_or_else(|| anyhow!("Failed to locate read info for buffer {}", info.index))?;
        //     ensure!(read.index as usize == read_idx); // do these ever differ?
        //     let read_buf = &data[read.offset as usize..(read.offset + read.size) as usize];
        //     let comp_buf = &read_buf[info.offset as usize..(info.offset + info.size) as usize];
        //     decompress_into(
        //         comp_buf,
        //         &mut buffer
        //             [info.dest_offset as usize..(info.dest_offset + info.dest_size) as usize],
        //     )?;
        // }

        // First buffer
        if meta.buffers.first_size > 0 {
            let read = meta.info.get(meta.buffers.first_index as usize).ok_or_else(|| {
                anyhow!("Failed to locate read info for buffer {}", meta.buffers.first_index)
            })?;
            let read_buf = &data[read.offset as usize..(read.offset + read.size) as usize];
            let comp_buf = &read_buf[..meta.buffers.first_size as usize];

            let dst_start = meta.buffers.first_dest_offset;
            let dst_end = dst_start + meta.buffers.first_dest_size;

            log::debug!(
                "Decompressing first buffer: {:#x}-{:#x} ({:#x}) -> {:#x}-{:#x} ({:#x})",
                read.offset,
                read.offset + meta.buffers.first_size,
                meta.buffers.first_size,
                dst_start,
                dst_end,
                dst_end - dst_start
            );
            decompress_into(comp_buf, &mut buffer[dst_start as usize..dst_end as usize])
                .context("Decompressing first buffer")?;
        }

        if meta.buffers.second_size > 0 {
            let read = meta.info.get(meta.buffers.second_index as usize).ok_or_else(|| {
                anyhow!("Failed to locate read info for buffer {}", meta.buffers.second_index)
            })?;
            let read_buf = &data[read.offset as usize..(read.offset + read.size) as usize];

            // Compressed part of second buffer
            if meta.buffers.second_compressed_offset < meta.buffers.second_size {
                let comp_buf = &read_buf[meta.buffers.second_compressed_offset as usize
                    ..(meta.buffers.second_compressed_offset + meta.buffers.second_compressed_len)
                        as usize];

                let dst_start = meta.buffers.second_dest_offset;
                let dst_end = dst_start + meta.buffers.second_decompressed_len;

                log::debug!(
                    "Decompressing second buffer: {:#x}-{:#x} ({:#x}) -> {:#x}-{:#x} ({:#x})",
                    read.offset + meta.buffers.second_compressed_offset,
                    read.offset
                        + meta.buffers.second_compressed_offset
                        + meta.buffers.second_compressed_len,
                    meta.buffers.second_compressed_len,
                    dst_start,
                    dst_end,
                    dst_end - dst_start
                );
                decompress_into(comp_buf, &mut buffer[dst_start as usize..dst_end as usize])
                    .context("Decompressing second buffer")?;
            }

            // Uncompressed start of second buffer
            if meta.buffers.second_compressed_offset > 0 {
                let src_start = 0;
                let src_end = meta.buffers.second_compressed_offset;
                let comp_buf = &read_buf[src_start as usize..src_end as usize];

                let dst_start =
                    meta.buffers.second_dest_offset + meta.buffers.second_decompressed_len;
                let dst_end = dst_start + meta.buffers.second_compressed_offset;

                log::debug!(
                    "Copying uncompressed data from {:#x}-{:#x} ({:#x}) to {:#x}-{:#x} ({:#x})",
                    read.offset + src_start,
                    read.offset + src_end,
                    src_end - src_start,
                    dst_start,
                    dst_end,
                    dst_end - dst_start
                );
                buffer[dst_start as usize..dst_end as usize].copy_from_slice(comp_buf);
            }

            // Uncompressed end of second buffer
            if meta.buffers.second_compressed_offset + meta.buffers.second_compressed_len
                < meta.buffers.second_size
            {
                let src_start =
                    meta.buffers.second_compressed_offset + meta.buffers.second_compressed_len;
                let src_end = meta.buffers.second_size;
                let comp_buf = &read_buf[src_start as usize..src_end as usize];

                let dst_start = meta.buffers.second_dest_offset
                    + meta.buffers.second_compressed_offset
                    + meta.buffers.second_decompressed_len;
                let dst_end = meta.buffers.second_dest_offset + meta.buffers.second_dest_size;

                log::debug!(
                    "Copying uncompressed data from {:#x}-{:#x} ({:#x}) to {:#x}-{:#x} ({:#x})",
                    read.offset + src_start,
                    read.offset + src_end,
                    src_end - src_start,
                    dst_start,
                    dst_end,
                    dst_end - dst_start
                );
                buffer[dst_start as usize..dst_end as usize].copy_from_slice(comp_buf);
            }
        }

        let deswizzled = deswizzle(&head, &buffer)?;
        Ok(Self { head, data: deswizzled, _marker: PhantomData })
    }
}

#[derive(Debug, Clone)]
pub struct TextureSlice {
    pub width: u32,
    pub height: u32,
    pub data_range: Range<usize>,
}

pub fn slice_texture<O: ByteOrder>(texture: &TextureData<O>) -> Result<Vec<Vec<TextureSlice>>> {
    let (bw, bh, bd) = texture.head.format.block_size();
    let mut out = Vec::with_capacity(texture.head.mip_sizes.len());
    let mut w = texture.head.width;
    let mut h = texture.head.height;
    let mut d = texture.head.layers;
    let mut start = 0usize;
    if texture.head.kind == ETextureType::D3 {
        for &size in &texture.head.mip_sizes {
            let layer_size = size as usize / d as usize;
            ensure!(layer_size * d as usize == size as usize);
            out.push(
                (start..start + size as usize)
                    .step_by(layer_size)
                    .map(|layer_start| TextureSlice {
                        width: w,
                        height: h,
                        data_range: layer_start..layer_start + layer_size,
                    })
                    .collect(),
            );
            start += size as usize;
            w = max(w / 2, bw as u32);
            h = max(h / 2, bh as u32);
            d = max(d / 2, bd as u32);
        }
    } else {
        out.resize(texture.head.mip_sizes.len(), Vec::<TextureSlice>::with_capacity(d as usize));
        for _ in 0..d {
            w = texture.head.width;
            h = texture.head.height;
            for (mip_idx, &size) in texture.head.mip_sizes.iter().enumerate() {
                let layer_size = size as usize / d as usize;
                out[mip_idx].push(TextureSlice {
                    width: w,
                    height: h,
                    data_range: start..start + layer_size,
                });
                start += layer_size;
                w = max(w / 2, bw as u32);
                h = max(h / 2, bh as u32);
            }
        }
    }
    Ok(out)
}

const BC1_BLOCK_SIZE: usize = 8;
const BC2_BLOCK_SIZE: usize = 16;
const BC3_BLOCK_SIZE: usize = 16;
const BC4_BLOCK_SIZE: usize = 8;
const BC5_BLOCK_SIZE: usize = 16;
const BC6H_BLOCK_SIZE: usize = 16;
const BC7_BLOCK_SIZE: usize = 16;

pub fn decompress_image(
    format: ETextureFormat,
    w: u32,
    h: u32,
    data: &[u8],
) -> Result<DynamicImage> {
    Ok(match format {
        ETextureFormat::R8Unorm => {
            DynamicImage::ImageLuma8(GrayImage::from_raw(w, h, data.to_vec()).ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?)
        }
        ETextureFormat::R16Unorm => DynamicImage::ImageLuma16(
            ImageBuffer::<Luma<u16>, Vec<u16>>::from_raw(w, h, bytemuck::cast_vec(data.to_vec()))
                .ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?,
        ),
        ETextureFormat::Rgb8Unorm => {
            DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, data.to_vec()).ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?)
        }
        ETextureFormat::Rgba8Unorm | ETextureFormat::Rgba8Srgb => {
            DynamicImage::ImageRgba8(RgbaImage::from_raw(w, h, data.to_vec()).ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?)
        }
        ETextureFormat::Rgba32Float => DynamicImage::ImageRgba32F(
            Rgba32FImage::from_raw(w, h, bytemuck::cast_vec(data.to_vec())).ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?,
        ),
        ETextureFormat::RgbaBc1Unorm | ETextureFormat::RgbaBc1Srgb => DynamicImage::ImageRgba8(
            decompress_bcn::<Rgba<u8>, _, BC1_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc1(src, dst, pitch)
            })?,
        ),
        ETextureFormat::RgbaBc2Unorm | ETextureFormat::RgbaBc2Srgb => DynamicImage::ImageRgba8(
            decompress_bcn::<Rgba<u8>, _, BC2_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc2(src, dst, pitch)
            })?,
        ),
        ETextureFormat::RgbaBc3Unorm | ETextureFormat::RgbaBc3Srgb => DynamicImage::ImageRgba8(
            decompress_bcn::<Rgba<u8>, _, BC3_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc3(src, dst, pitch)
            })?,
        ),
        // TODO snorm?
        ETextureFormat::RgbaBc4Unorm | ETextureFormat::RgbaBc4Snorm => DynamicImage::ImageLuma8(
            decompress_bcn::<Luma<u8>, _, BC4_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc4(src, dst, pitch)
            })?,
        ),
        // TODO snorm?
        ETextureFormat::RgbaBc5Unorm | ETextureFormat::RgbaBc5Snorm => DynamicImage::ImageLumaA8(
            decompress_bcn::<LumaA<u8>, _, BC5_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc5(src, dst, pitch)
            })?,
        ),
        ETextureFormat::Rgba16Unorm => DynamicImage::ImageRgba16(
            ImageBuffer::<Rgba<u16>, Vec<u16>>::from_raw(w, h, bytemuck::cast_vec(data.to_vec()))
                .ok_or_else(|| {
                anyhow!("Conversion failed: {:?} {}x{} from size {}", format, w, h, data.len())
            })?,
        ),
        ETextureFormat::RgbaAstc4x4
        | ETextureFormat::RgbaAstc5x4
        | ETextureFormat::RgbaAstc5x5
        | ETextureFormat::RgbaAstc6x5
        | ETextureFormat::RgbaAstc6x6
        | ETextureFormat::RgbaAstc8x5
        | ETextureFormat::RgbaAstc8x6
        | ETextureFormat::RgbaAstc8x8
        | ETextureFormat::RgbaAstc10x5
        | ETextureFormat::RgbaAstc10x6
        | ETextureFormat::RgbaAstc10x8
        | ETextureFormat::RgbaAstc10x10
        | ETextureFormat::RgbaAstc12x10
        | ETextureFormat::RgbaAstc12x12
        | ETextureFormat::RgbaAstc4x4Srgb
        | ETextureFormat::RgbaAstc5x4Srgb
        | ETextureFormat::RgbaAstc5x5Srgb
        | ETextureFormat::RgbaAstc6x5Srgb
        | ETextureFormat::RgbaAstc6x6Srgb
        | ETextureFormat::RgbaAstc8x5Srgb
        | ETextureFormat::RgbaAstc8x6Srgb
        | ETextureFormat::RgbaAstc8x8Srgb
        | ETextureFormat::RgbaAstc10x5Srgb
        | ETextureFormat::RgbaAstc10x6Srgb
        | ETextureFormat::RgbaAstc10x8Srgb
        | ETextureFormat::RgbaAstc10x10Srgb
        | ETextureFormat::RgbaAstc12x10Srgb
        | ETextureFormat::RgbaAstc12x12Srgb => {
            let (bw, bh, _) = format.block_size();
            let rw = w.div_ceil(bw as u32);
            let rh = h.div_ceil(bh as u32);
            ensure!(data.len() == rw as usize * rh as usize * 16);
            let mut image = RgbaImage::new(w, h);
            astc_decode::astc_decode(
                data,
                w,
                h,
                astc_decode::Footprint::new(bw as u32, bh as u32),
                |x, y, z| image.put_pixel(x, y, z.into()),
            )
            .with_context(|| {
                format!(
                    "Failed to decode ASTC {}x{} (block {}x{}) data size {}",
                    w,
                    h,
                    bw,
                    bh,
                    data.len()
                )
            })?;
            DynamicImage::ImageRgba8(image)
        }
        ETextureFormat::BptcUfloat | ETextureFormat::BptcSfloat => {
            let is_signed = format == ETextureFormat::BptcSfloat;
            DynamicImage::ImageRgb32F(decompress_bcn::<Rgb<f32>, _, BC6H_BLOCK_SIZE>(
                data,
                w,
                h,
                |src, dst, pitch| bcdec_rs::bc6h_float(src, dst, pitch, is_signed),
            )?)
        }
        ETextureFormat::BptcUnorm | ETextureFormat::BptcUnormSrgb => DynamicImage::ImageRgba8(
            decompress_bcn::<Rgba<u8>, _, BC7_BLOCK_SIZE>(data, w, h, |src, dst, pitch| {
                bcdec_rs::bc7(src, dst, pitch)
            })?,
        ),
        format => bail!("Unsupported conversion from {format:?}"),
    })
}

fn decompress_bcn<P, F, const BLOCK_SIZE: usize>(
    data: &[u8],
    w: u32,
    h: u32,
    func: F,
) -> Result<ImageBuffer<P, Vec<P::Subpixel>>>
where
    P: Pixel,
    F: Fn(&[u8], &mut [P::Subpixel], usize),
{
    let w = max(w, 4);
    let h = max(h, 4);
    ensure!(data.len() == ((w / 4) * (h / 4)) as usize * BLOCK_SIZE);
    let mut image = ImageBuffer::<P, Vec<P::Subpixel>>::new(w, h);
    let buffer = image.as_flat_samples_mut();
    let mut src = data;
    for i in (0..h as usize).step_by(4) {
        for j in (0..w as usize).step_by(4) {
            let start = i * buffer.layout.height_stride + j * buffer.layout.width_stride;
            let dst = &mut buffer.samples[start..];
            func(&src[..BLOCK_SIZE], dst, buffer.layout.height_stride);
            src = &src[BLOCK_SIZE..];
        }
    }
    Ok(image)
}
