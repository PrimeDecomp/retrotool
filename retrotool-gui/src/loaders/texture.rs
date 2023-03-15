use anyhow::{anyhow, Error, Result};
use bevy::{
    app::{App, Plugin},
    asset::{AddAsset, AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
};
use binrw::Endian;
use retrolib::format::{
    foot::locate_meta,
    txtr::{decompress_images, ETextureFormat, ETextureType, TextureData},
};

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e828"]
pub struct TextureAsset {
    pub inner: TextureData,
    pub texture: Image,
    pub slices: Vec<Vec<Image>>, // [mip][layer]
}

pub struct TextureAssetLoader;

impl Plugin for TextureAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<TextureAsset>().add_asset_loader(TextureAssetLoader);
    }
}

impl AssetLoader for TextureAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = TextureData::slice(bytes, meta, Endian::Little)?;
            let is_srgb = data.head.format.is_srgb();
            let images = decompress_images(&data)?
                .into_iter()
                .map(|v| v.into_iter().map(|i| Image::from_dynamic(i, is_srgb)).collect())
                .collect::<Vec<Vec<Image>>>();
            let texture = texture_to_image(&data, &images)?;
            load_context.set_default_asset(LoadedAsset::new(TextureAsset {
                inner: data,
                texture,
                slices: images,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["txtr"] }
}

fn texture_to_image(data: &TextureData, images: &[Vec<Image>]) -> Result<Image> {
    let mut image_data = data.data.clone();
    let format;
    if data.head.format.is_astc() {
        // Combine all decoded mips & layers
        image_data = images.iter().flatten().flat_map(|i| &i.data).cloned().collect();
        format = if data.head.format.is_srgb() {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        };
    } else {
        format = wgpu_format(data.head.format)
            .ok_or_else(|| anyhow!("Texture format unsupported: {:?}", data.head.format))?;
    }
    Ok(Image {
        data: image_data,
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width: data.head.width,
                height: data.head.height,
                depth_or_array_layers: data.head.layers,
            },
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
