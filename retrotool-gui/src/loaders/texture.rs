use std::num::NonZeroU8;

use anyhow::{anyhow, Error, Result};
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    render::{
        render_resource::{
            Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        },
        renderer::RenderDevice,
        texture::{CompressedImageFormats, ImageSampler},
    },
};
use retrolib::format::{
    foot::{locate_asset_id, locate_meta},
    texture_sampler::TextureSamplerParams,
    txtr::{
        decompress_bc6h_rgba16, decompress_image, slice_texture, ETextureFormat, ETextureType,
        TextureData, K_FORM_TXTR,
    },
};
use wgpu::SamplerDescriptor;
use wgpu_types::{AddressMode, Features, FilterMode, SamplerBorderColor};
use zerocopy::LittleEndian;

use crate::AssetRef;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e828"]
pub struct TextureAsset {
    #[allow(unused)]
    pub asset_ref: AssetRef,
    pub inner: TextureData<LittleEndian>,
    pub texture: Handle<Image>,
    pub sampler_features: Features,
    pub slices: Vec<Vec<Handle<Image>>>, // [mip][layer]
}

pub struct TextureAssetLoader {
    supported_formats: CompressedImageFormats,
    sampler_features: Features,
}

impl FromWorld for TextureAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let supported_formats = match world.get_resource::<RenderDevice>() {
            Some(render_device) => CompressedImageFormats::from_features(render_device.features()),
            None => CompressedImageFormats::all(),
        };
        let sampler_features =
            world.get_resource::<RenderDevice>().map_or(Features::empty(), RenderDevice::features);
        Self { supported_formats, sampler_features }
    }
}

impl AssetLoader for TextureAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let id = locate_asset_id::<LittleEndian>(bytes)?;
            let meta = locate_meta::<LittleEndian>(bytes)?;
            let data = TextureData::<LittleEndian>::slice(bytes, meta)?;
            info!("Loading texture {} {:?}", id, data.head);

            let result = load_texture_asset(data, &self.supported_formats, self.sampler_features)?;
            let image_handle =
                load_context.set_labeled_asset("image", LoadedAsset::new(result.texture));
            let mut slice_handles = Vec::with_capacity(result.slices.len());
            for (mip, images) in result.slices.into_iter().enumerate() {
                let mut handles = Vec::with_capacity(images.len());
                for (layer, image) in images.into_iter().enumerate() {
                    handles.push(load_context.set_labeled_asset(
                        &format!("mip_{}_layer_{}", mip, layer),
                        LoadedAsset::new(image),
                    ));
                }
                slice_handles.push(handles);
            }
            load_context.set_default_asset(LoadedAsset::new(TextureAsset {
                asset_ref: AssetRef { id, kind: K_FORM_TXTR },
                inner: result.inner,
                texture: image_handle,
                sampler_features: self.sampler_features,
                slices: slice_handles,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["txtr"] }
}

pub struct LoadTextureResult {
    pub inner: TextureData<LittleEndian>,
    pub texture: Image,
    pub slices: Vec<Vec<Image>>, // [mip][layer]
}

pub fn load_texture_asset(
    data: TextureData<LittleEndian>,
    supported_formats: &CompressedImageFormats,
    sampler_features: Features,
) -> Result<LoadTextureResult> {
    let is_srgb = data.head.format.is_srgb();
    let is_bc6h =
        matches!(data.head.format, ETextureFormat::BptcUfloat | ETextureFormat::BptcSfloat);
    let slices = slice_texture(&data)?;
    let (bw, bh, _) = data.head.format.block_size();
    let format = wgpu_format(data.head.format)
        .ok_or_else(|| anyhow!("Texture format unsupported: {:?}", data.head.format))?;
    // wgpu requires compressed base dimensions to be block-aligned. Rounding
    // the dimensions changes normalized sampling and every mip's logical size.
    // Decode these images to preserve their actual texel dimensions instead.
    let supported = texture_format_supported(data.head.kind, format, supported_formats)
        && data.head.width % u32::from(bw) == 0
        && data.head.height % u32::from(bh) == 0;

    let mut images = Vec::with_capacity(slices.len());
    for mip in &slices {
        let mut slice_images = Vec::with_capacity(mip.len());
        for slice in mip {
            let slice_data = &data.data[slice.data_range.clone()];
            // Each thumbnail is a separate base-level image, so its dimensions
            // must independently satisfy compressed texture alignment.
            slice_images.push(
                if supported
                    && slice.width % u32::from(bw) == 0
                    && slice.height % u32::from(bh) == 0
                {
                    texture_slice_to_image(format, slice_data.to_vec(), slice.width, slice.height)
                } else if is_bc6h {
                    texture_slice_to_image(
                        TextureFormat::Rgba16Float,
                        decompress_bc6h_rgba16(
                            data.head.format,
                            slice.width,
                            slice.height,
                            slice_data,
                        )?,
                        slice.width,
                        slice.height,
                    )
                } else {
                    Image::from_dynamic(
                        decompress_image(data.head.format, slice.width, slice.height, slice_data)?,
                        is_srgb,
                    )
                },
            );
        }
        images.push(slice_images);
    }

    let (image_data, format) = if supported {
        (data.data.clone(), format)
    } else {
        (
            decoded_texture_bytes(&images, data.head.kind),
            if is_bc6h {
                TextureFormat::Rgba16Float
            } else if is_srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            },
        )
    };
    let mut texture = texture_to_image(&data, format, image_data)?;
    texture.sampler_descriptor = ImageSampler::Descriptor(sampler_descriptor(
        &TextureSamplerParams::from_header(&data.head.sampler_data),
        sampler_features,
    ));
    Ok(LoadTextureResult { inner: data, texture, slices: images })
}

// wgpu's upload helper consumes all mips of each array layer, in that order.
// Volumes instead contain all depth slices of one mip before the next mip.
fn decoded_texture_bytes(images: &[Vec<Image>], kind: ETextureType) -> Vec<u8> {
    if kind == ETextureType::D3 {
        images.iter().flatten().flat_map(|i| i.data.iter().copied()).collect()
    } else {
        (0..images.first().map_or(0, Vec::len))
            .flat_map(|layer| images.iter().flat_map(move |mip| mip[layer].data.iter().copied()))
            .collect()
    }
}

/// Create an [Image] from a 2D texture slice.
fn texture_slice_to_image(format: TextureFormat, data: Vec<u8>, width: u32, height: u32) -> Image {
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
        sampler_descriptor: DEFAULT_SAMPLER,
        ..default()
    }
}

fn texture_format_supported(
    kind: ETextureType,
    format: TextureFormat,
    supported_formats: &CompressedImageFormats,
) -> bool {
    supported_formats.supports(format)
        // wgpu 0.15 permits compressed allocations only with dimension D2.
        // In particular, BCn probe volumes must be decoded even with BC support.
        && !(wgpu_dimension(kind) != TextureDimension::D2 && format.describe().is_compressed())
}

const DEFAULT_SAMPLER: ImageSampler = ImageSampler::Descriptor(SamplerDescriptor {
    label: None,
    address_mode_u: AddressMode::Repeat,
    address_mode_v: AddressMode::Repeat,
    address_mode_w: AddressMode::Repeat,
    mag_filter: FilterMode::Linear,
    min_filter: FilterMode::Linear,
    mipmap_filter: FilterMode::Linear,
    lod_min_clamp: 0.0,
    lod_max_clamp: f32::MAX,
    compare: None,
    anisotropy_clamp: NonZeroU8::new(8),
    border_color: None,
});

/// Convert the recovered base sampler to wgpu. MirrorClamp has no native wgpu
/// equivalent; ClampToBorder additionally needs an enabled device feature.
/// LOD-bias/sample-shading and streaming variants require renderer state which
/// the standalone model viewer does not yet provide.
pub fn sampler_descriptor(
    params: &TextureSamplerParams,
    features: Features,
) -> SamplerDescriptor<'static> {
    let wrap = params.nvn_wrap_modes().map(|w| match w {
        7 => AddressMode::ClampToEdge,
        6 => AddressMode::MirrorRepeat,
        2 => {
            log::warn!("NVN MirrorClamp sampling approximated with ClampToEdge");
            AddressMode::ClampToEdge
        }
        5 if features.contains(Features::ADDRESS_MODE_CLAMP_TO_BORDER) => {
            AddressMode::ClampToBorder
        }
        5 => {
            log::warn!(
                "ClampToBorder sampling approximated with ClampToEdge: GPU feature unavailable"
            );
            AddressMode::ClampToEdge
        }
        _ => AddressMode::Repeat,
    });
    let [min, mag] = params.nvn_filters();
    SamplerDescriptor {
        label: None,
        address_mode_u: wrap[0],
        address_mode_v: wrap[1],
        address_mode_w: wrap[2],
        mag_filter: if mag == 1 { FilterMode::Linear } else { FilterMode::Nearest },
        min_filter: if min == 3 || min == 5 { FilterMode::Linear } else { FilterMode::Nearest },
        mipmap_filter: if min == 4 || min == 5 { FilterMode::Linear } else { FilterMode::Nearest },
        lod_min_clamp: params.lod_min,
        lod_max_clamp: params.lod_max,
        compare: None, // TXTR and serialized usage do not specify comparison.
        // Some(1) still enables anisotropy on wgpu's Vulkan backend. Native
        // max-anisotropy 1 means isotropic sampling, including one-mip views.
        anisotropy_clamp: (params.max_anisotropy() > 1)
            .then(|| NonZeroU8::new(params.max_anisotropy()).unwrap()),
        border_color: match params.border {
            // Native SetDefaults (SDK+4737c0) initializes all four channels to
            // +0. Engine border -1 leaves these values unchanged.
            -1 | 0 => Some(SamplerBorderColor::TransparentBlack),
            1 => Some(SamplerBorderColor::OpaqueBlack),
            2 => Some(SamplerBorderColor::OpaqueWhite),
            _ => None,
        },
    }
}

/// Creates an [Image] from a full texture.
fn texture_to_image(
    data: &TextureData<LittleEndian>,
    format: TextureFormat,
    image_data: Vec<u8>,
) -> Result<Image> {
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
        sampler_descriptor: DEFAULT_SAMPLER,
        ..default()
    })
}

fn wgpu_format(format: ETextureFormat) -> Option<TextureFormat> {
    use wgpu_types::{AstcBlock::*, AstcChannel::*, TextureFormat::*};
    Some(match format {
        ETextureFormat::R8Unorm => R8Unorm,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_isotropic_sampler_disables_host_anisotropy() {
        for filter in [0, 1] {
            let mut params = TextureSamplerParams::simple(filter, [0; 3], 0);
            let sampler = sampler_descriptor(&params, Features::empty());
            assert_eq!(sampler.anisotropy_clamp, None);
            assert_eq!(sampler.min_filter, sampler.mag_filter);
            for (mode, maximum) in [(1, 2), (2, 4), (3, 8), (4, 16)] {
                params.anisotropy = mode;
                assert_eq!(
                    sampler_descriptor(&params, Features::empty()).anisotropy_clamp,
                    NonZeroU8::new(maximum)
                );
            }
        }
    }

    #[test]
    fn decoded_array_upload_is_layer_major_but_volume_upload_is_mip_major() {
        let image = |value| Image { data: vec![value; 4], ..default() };
        let images = vec![vec![image(1), image(2)], vec![image(3), image(4)]];
        assert_eq!(decoded_texture_bytes(&images, ETextureType::D2Array), [
            1, 1, 1, 1, 3, 3, 3, 3, 2, 2, 2, 2, 4, 4, 4, 4
        ]);
        let volume = vec![vec![image(1), image(2)], vec![image(3)]];
        assert_eq!(decoded_texture_bytes(&volume, ETextureType::D3), [
            1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3
        ]);
    }
}
