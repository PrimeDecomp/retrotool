use std::{io::Cursor, num::NonZeroUsize};

use anyhow::{anyhow, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use tegra_swizzle::surface::BlockDim;

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
    _1D = 0,
    _2D = 1,
    _3D = 2,
    Cube = 3,
    _1DArray = 4,
    _2DArray = 5,
    _2DMultisample = 6,
    _2DMultisampleArray = 7,
    CubeArray = 8,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ETextureFilter {
    Nearest = 0,
    Linear = 1,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ETextureMipFilter {
    Nearest = 0,
    Linear = 1,
}

#[binrw]
#[repr(u8)]
#[brw(repr(u8))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ETextureAnisotropicRatio {
    None = u8::MAX,
    _1 = 0,
    _2 = 1,
    _4 = 2,
    _8 = 3,
    _16 = 4,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureHeader {
    pub kind: ETextureType,
    pub format: ETextureFormat,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub tile_mode: u32,
    pub swizzle: u32,
    #[bw(try_calc = mip_sizes.len().try_into())]
    pub mip_count: u32,
    #[br(count = mip_count)]
    pub mip_sizes: Vec<u32>,
    pub sampler_data: STextureSamplerData,
}

#[binrw]
#[derive(Clone, Debug)]
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

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureCompressedBufferInfo {
    pub index: u32,
    pub offset: u32,
    pub size: u32,
    pub dest_offset: u32,
    pub dest_size: u32,
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
    #[bw(try_calc = buffers.len().try_into())]
    pub buffer_count: u32,
    #[br(count = buffer_count)]
    pub buffers: Vec<STextureCompressedBufferInfo>,
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
    let expected_size = tegra_swizzle::surface::swizzled_surface_size(
        header.width as usize,
        header.height as usize,
        1,
        block_dim,
        None,
        bpp,
        header.mip_sizes.len(),
        header.layers as usize,
    );
    ensure!(data.len() == expected_size);
    Ok(tegra_swizzle::surface::deswizzle_surface(
        header.width as usize,
        header.height as usize,
        1,
        data,
        block_dim,
        None,
        bpp,
        header.mip_sizes.len(),
        header.layers as usize,
    )?)
}

#[derive(Debug, Clone)]
pub struct TextureData {
    pub head: STextureHeader,
    pub data: Vec<u8>,
}

impl TextureData {
    pub fn slice(data: &[u8], meta: &[u8], e: Endian) -> Result<TextureData> {
        let (txtr_desc, txtr_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(txtr_desc.id == K_FORM_TXTR);
        ensure!(txtr_desc.reader_version == 47);
        ensure!(txtr_desc.writer_version == 51);

        let (head_desc, head_data, _) = ChunkDescriptor::slice(txtr_data, Endian::Little)?;
        ensure!(head_desc.id == K_CHUNK_HEAD);
        let head: STextureHeader = Cursor::new(head_data).read_type(Endian::Little)?;

        // log::debug!("META: {meta:#?}");
        // log::debug!("HEAD: {head:#?}");

        let meta: STextureMetaData = Cursor::new(meta).read_type(e)?;
        let mut buffer = vec![0u8; meta.decompressed_size as usize];
        for info in &meta.buffers {
            let read =
                meta.info.iter().find(|i| i.index as u32 == info.index).ok_or_else(|| {
                    anyhow!("Failed to locate read info for buffer {}", info.index)
                })?;
            let read_buf = &data[read.offset as usize..(read.offset + read.size) as usize];
            let comp_buf = &read_buf[info.offset as usize..(info.offset + info.size) as usize];
            decompress_into(
                comp_buf,
                &mut buffer
                    [info.dest_offset as usize..(info.dest_offset + info.dest_size) as usize],
            )?;
        }
        let deswizzled = deswizzle(&head, &buffer)?;
        Ok(TextureData { head, data: deswizzled })
    }
}
