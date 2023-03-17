use anyhow::{anyhow, Error, Result};
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    render::{
        render_resource::{
            Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        },
        renderer::RenderDevice,
        texture::CompressedImageFormats,
    },
};
use binrw::Endian;
use retrolib::format::{
    foot::{locate_asset_id, locate_meta},
    txtr::{
        decompress_image, slice_texture, ETextureFormat, ETextureType, TextureData, K_FORM_TXTR,
    },
};

use crate::AssetRef;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e828"]
pub struct TextureAsset {
    pub asset_ref: AssetRef,
    pub inner: TextureData,
    pub texture: Image,
    pub slices: Vec<Vec<Image>>, // [mip][layer]
}

pub struct TextureAssetLoader {
    supported_formats: CompressedImageFormats,
}

impl FromWorld for TextureAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let supported_formats = match world.get_resource::<RenderDevice>() {
            Some(render_device) => CompressedImageFormats::from_features(render_device.features()),
            None => CompressedImageFormats::all(),
        };
        Self { supported_formats }
    }
}

impl AssetLoader for TextureAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let id = locate_asset_id(bytes, Endian::Little)?;
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = TextureData::slice(bytes, meta, Endian::Little)?;
            info!("Loading texture {} {:?}", id, data.head);

            let is_srgb = data.head.format.is_srgb();
            let slices = slice_texture(&data)?;
            let (bw, bh, _) = data.head.format.block_size();
            let format = wgpu_format(data.head.format)
                .ok_or_else(|| anyhow!("Texture format unsupported: {:?}", data.head.format))?;
            let supported =
                texture_format_supported(data.head.kind, format, &self.supported_formats);

            let mut images = Vec::with_capacity(slices.len());
            for mip in &slices {
                let mut slice_images = Vec::with_capacity(mip.len());
                for slice in mip {
                    let slice_data = &data.data[slice.data_range.clone()];
                    slice_images.push(if supported {
                        texture_slice_to_image(
                            format,
                            slice_data.to_vec(),
                            slice.width,
                            slice.height,
                            bw,
                            bh,
                        )
                    } else {
                        Image::from_dynamic(
                            decompress_image(
                                data.head.format,
                                slice.width,
                                slice.height,
                                slice_data,
                            )?,
                            is_srgb,
                        )
                    });
                }
                images.push(slice_images);
            }

            let (image_data, format) = if supported {
                (data.data.clone(), format)
            } else {
                (
                    images.iter().flatten().flat_map(|i| &i.data).cloned().collect(),
                    TextureFormat::Rgba8Unorm,
                )
            };
            let texture = texture_to_image(&data, format, image_data)?;
            load_context.set_default_asset(LoadedAsset::new(TextureAsset {
                asset_ref: AssetRef { id, kind: K_FORM_TXTR },
                inner: data,
                texture,
                slices: images,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["txtr"] }
}

/// Create an [Image] from a 2D texture slice.
fn texture_slice_to_image(
    format: TextureFormat,
    data: Vec<u8>,
    mut width: u32,
    mut height: u32,
    bw: u8,
    bh: u8,
) -> Image {
    if let TextureFormat::Astc { .. } = format {
        // Round up width / height to ASTC block size
        // wgpu requires it, but should it?
        width = (width + (bw as u32 - 1)) / bw as u32 * bw as u32;
        height = (height + (bh as u32 - 1)) / bh as u32 * bh as u32;
    }
    Image {
        data,
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        ..default()
    }
}

fn texture_format_supported(
    kind: ETextureType,
    format: TextureFormat,
    supported_formats: &CompressedImageFormats,
) -> bool {
    supported_formats.supports(format)
        // ASTC 3D textures are unsupported
        && !(kind == ETextureType::D3 && matches!(format, TextureFormat::Astc { .. }))
}

/// Creates an [Image] from a full texture.
fn texture_to_image(
    data: &TextureData,
    format: TextureFormat,
    image_data: Vec<u8>,
) -> Result<Image> {
    let mut width = data.head.width;
    let mut height = data.head.height;
    if let TextureFormat::Astc { .. } = format {
        // Round up width / height to ASTC block size
        // wgpu requires it, but should it?
        let (bx, by, _) = data.head.format.block_size();
        width = (width + (bx as u32 - 1)) / bx as u32 * bx as u32;
        height = (height + (by as u32 - 1)) / by as u32 * by as u32;
    }
    Ok(Image {
        data: image_data,
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d { width, height, depth_or_array_layers: data.head.layers },
            mip_level_count: data.head.mip_sizes.len() as u32,
            sample_count: 1,
            dimension: wgpu_dimension(data.head.kind),
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        ..default()
    })
}

fn wgpu_format(format: ETextureFormat) -> Option<TextureFormat> {
    use wgpu_types::{AstcBlock::*, AstcChannel::*, TextureFormat::*};
    Some(match format {
        ETextureFormat::R8Unorm => Rgba8Unorm,
        ETextureFormat::R8Snorm => R8Snorm,
        ETextureFormat::R8Uint => R8Uint,
        ETextureFormat::R8Sint => R8Sint,
        ETextureFormat::R16Unorm => R16Unorm,
        ETextureFormat::R16Snorm => R16Snorm,
        ETextureFormat::R16Uint => R16Uint,
        ETextureFormat::R16Sint => R16Sint,
        ETextureFormat::R16Float => R16Float,
        ETextureFormat::R32Uint => R32Uint,
        ETextureFormat::R32Sint => R32Sint,
        ETextureFormat::Rgba8Unorm => Rgba8Unorm,
        ETextureFormat::Rgba8Srgb => Rgba8UnormSrgb,
        ETextureFormat::Rgba16Float => Rgba16Float,
        ETextureFormat::Rgba32Float => Rgba32Float,
        ETextureFormat::Depth16Unorm => Depth16Unorm,
        ETextureFormat::Depth16Unorm2 => Depth16Unorm,
        ETextureFormat::Depth24S8Unorm => Depth24PlusStencil8,
        ETextureFormat::Depth32Float => Depth32Float,
        ETextureFormat::RgbaBc1Unorm => Bc1RgbaUnorm,
        ETextureFormat::RgbaBc1Srgb => Bc1RgbaUnormSrgb,
        ETextureFormat::RgbaBc2Unorm => Bc2RgbaUnorm,
        ETextureFormat::RgbaBc2Srgb => Bc2RgbaUnormSrgb,
        ETextureFormat::RgbaBc3Unorm => Bc3RgbaUnorm,
        ETextureFormat::RgbaBc3Srgb => Bc3RgbaUnormSrgb,
        ETextureFormat::RgbaBc4Unorm => Bc4RUnorm,
        ETextureFormat::RgbaBc4Snorm => Bc4RSnorm,
        ETextureFormat::RgbaBc5Unorm => Bc5RgUnorm,
        ETextureFormat::RgbaBc5Snorm => Bc5RgSnorm,
        ETextureFormat::Rg11B10Float => Rg11b10Float,
        ETextureFormat::R32Float => R32Float,
        ETextureFormat::Rg8Unorm => Rg8Unorm,
        ETextureFormat::Rg8Snorm => Rg8Snorm,
        ETextureFormat::Rg8Uint => Rg8Uint,
        ETextureFormat::Rg8Sint => Rg8Sint,
        ETextureFormat::Rg16Float => Rg16Float,
        ETextureFormat::Rg16Unorm => Rg16Unorm,
        ETextureFormat::Rg16Snorm => Rg16Snorm,
        ETextureFormat::Rg16Uint => Rg16Uint,
        ETextureFormat::Rg16Sint => Rg16Sint,
        ETextureFormat::Rgb10A2Unorm => Rgb10a2Unorm,
        ETextureFormat::Rg32Uint => Rg32Uint,
        ETextureFormat::Rg32Sint => Rg32Sint,
        ETextureFormat::Rg32Float => Rg32Float,
        ETextureFormat::Rgba16Unorm => Rgba16Unorm,
        ETextureFormat::Rgba16Snorm => Rgba16Snorm,
        ETextureFormat::Rgba16Uint => Rgba16Uint,
        ETextureFormat::Rgba16Sint => Rgba16Sint,
        ETextureFormat::Rgba32Uint => Rgba32Uint,
        ETextureFormat::Rgba32Sint => Rgba32Sint,
        ETextureFormat::BptcUfloat => Bc6hRgbUfloat,
        ETextureFormat::BptcSfloat => Bc6hRgbSfloat,
        ETextureFormat::BptcUnorm => Bc7RgbaUnorm,
        ETextureFormat::BptcUnormSrgb => Bc7RgbaUnormSrgb,
        ETextureFormat::RgbaAstc4x4 => Astc { block: B4x4, channel: Unorm },
        ETextureFormat::RgbaAstc5x4 => Astc { block: B5x4, channel: Unorm },
        ETextureFormat::RgbaAstc5x5 => Astc { block: B5x5, channel: Unorm },
        ETextureFormat::RgbaAstc6x5 => Astc { block: B6x5, channel: Unorm },
        ETextureFormat::RgbaAstc6x6 => Astc { block: B6x6, channel: Unorm },
        ETextureFormat::RgbaAstc8x5 => Astc { block: B8x5, channel: Unorm },
        ETextureFormat::RgbaAstc8x6 => Astc { block: B8x6, channel: Unorm },
        ETextureFormat::RgbaAstc8x8 => Astc { block: B8x8, channel: Unorm },
        ETextureFormat::RgbaAstc10x5 => Astc { block: B10x5, channel: Unorm },
        ETextureFormat::RgbaAstc10x6 => Astc { block: B10x6, channel: Unorm },
        ETextureFormat::RgbaAstc10x8 => Astc { block: B10x8, channel: Unorm },
        ETextureFormat::RgbaAstc10x10 => Astc { block: B10x10, channel: Unorm },
        ETextureFormat::RgbaAstc12x10 => Astc { block: B12x10, channel: Unorm },
        ETextureFormat::RgbaAstc12x12 => Astc { block: B12x12, channel: Unorm },
        ETextureFormat::RgbaAstc4x4Srgb => Astc { block: B4x4, channel: UnormSrgb },
        ETextureFormat::RgbaAstc5x4Srgb => Astc { block: B5x4, channel: UnormSrgb },
        ETextureFormat::RgbaAstc5x5Srgb => Astc { block: B5x5, channel: UnormSrgb },
        ETextureFormat::RgbaAstc6x5Srgb => Astc { block: B6x5, channel: UnormSrgb },
        ETextureFormat::RgbaAstc6x6Srgb => Astc { block: B6x6, channel: UnormSrgb },
        ETextureFormat::RgbaAstc8x5Srgb => Astc { block: B8x5, channel: UnormSrgb },
        ETextureFormat::RgbaAstc8x6Srgb => Astc { block: B8x6, channel: UnormSrgb },
        ETextureFormat::RgbaAstc8x8Srgb => Astc { block: B8x8, channel: UnormSrgb },
        ETextureFormat::RgbaAstc10x5Srgb => Astc { block: B10x5, channel: UnormSrgb },
        ETextureFormat::RgbaAstc10x6Srgb => Astc { block: B10x6, channel: UnormSrgb },
        ETextureFormat::RgbaAstc10x8Srgb => Astc { block: B10x8, channel: UnormSrgb },
        ETextureFormat::RgbaAstc10x10Srgb => Astc { block: B10x10, channel: UnormSrgb },
        ETextureFormat::RgbaAstc12x10Srgb => Astc { block: B12x10, channel: UnormSrgb },
        ETextureFormat::RgbaAstc12x12Srgb => Astc { block: B12x12, channel: UnormSrgb },
        _ => return None,
    })
}

fn wgpu_dimension(kind: ETextureType) -> TextureDimension {
    match kind {
        ETextureType::D1 | ETextureType::D1Array => TextureDimension::D1,
        ETextureType::D2
        | ETextureType::D2Array
        | ETextureType::D2Multisample
        | ETextureType::D2MultisampleArray
        | ETextureType::Cube
        | ETextureType::CubeArray => TextureDimension::D2,
        ETextureType::D3 => TextureDimension::D3,
    }
}
