use anyhow::Error;
use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    image::CompressedImageFormats,
    prelude::*,
    render::renderer::RenderDevice,
};
use retrolib::format::{
    foot::{locate_asset_id, locate_meta},
    ltpb::{LightProbeBundleHeader, LightProbeData, LightProbeExtra, K_FORM_LTPB},
};
use zerocopy::LittleEndian;

use crate::{
    loaders::texture::{load_texture_asset, TextureAsset},
    AssetRef,
};

#[derive(Debug, Clone, Asset, TypePath)]
pub struct LightProbeAsset {
    pub head: LightProbeBundleHeader,
    pub textures: Vec<TextureAsset>,
    pub extra: Vec<LightProbeExtra>,
}

#[derive(TypePath)]
pub struct LightProbeAssetLoader {
    supported_formats: CompressedImageFormats,
    sampler_features: wgpu_types::Features,
}

impl FromWorld for LightProbeAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let supported_formats = match world.get_resource::<RenderDevice>() {
            Some(render_device) => CompressedImageFormats::from_features(render_device.features()),
            None => CompressedImageFormats::all(),
        };
        let sampler_features = world
            .get_resource::<RenderDevice>()
            .map_or(wgpu_types::Features::empty(), RenderDevice::features);
        Self { supported_formats, sampler_features }
    }
}

impl AssetLoader for LightProbeAssetLoader {
    type Asset = LightProbeAsset;
    type Error = Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> anyhow::Result<Self::Asset, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let bytes = bytes.as_slice();
        let id = locate_asset_id::<LittleEndian>(bytes)?;
        let meta = locate_meta::<LittleEndian>(bytes)?;
        let data = LightProbeData::<LittleEndian>::slice(bytes, meta)?;
        info!("Loading light probe {} {:?}", id, data.head);

        let mut textures = Vec::with_capacity(data.textures.len());
        for (idx, texture_data) in data.textures.into_iter().enumerate() {
            let result =
                load_texture_asset(texture_data, &self.supported_formats, self.sampler_features)?;
            let mut slice_handles = Vec::with_capacity(result.slices.len());
            for (mip, images) in result.slices.into_iter().enumerate() {
                let mut handles = Vec::with_capacity(images.len());
                for (layer, image) in images.into_iter().enumerate() {
                    handles.push(
                        load_context.add_labeled_asset(
                            format!("image_{idx}_mip_{mip}_layer_{layer}"),
                            image,
                        ),
                    );
                }
                slice_handles.push(handles);
            }
            textures.push(TextureAsset {
                asset_ref: AssetRef { id, kind: K_FORM_LTPB },
                inner: result.inner,
                texture: load_context.add_labeled_asset(format!("texture_{idx}"), result.texture),
                sampler_features: self.sampler_features,
                slices: slice_handles,
            });
        }
        Ok(LightProbeAsset { head: data.head, textures, extra: data.extra })
    }

    fn extensions(&self) -> &[&str] { &["ltpb"] }
}
