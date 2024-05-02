use anyhow::Error;
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    render::{renderer::RenderDevice, texture::CompressedImageFormats},
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

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "f5d65a8b-ffcc-47ea-8c9d-1ab30cca723c"]
pub struct LightProbeAsset {
    pub head: LightProbeBundleHeader,
    pub textures: Vec<TextureAsset>,
    pub extra: Vec<LightProbeExtra>,
}

pub struct LightProbeAssetLoader {
    supported_formats: CompressedImageFormats,
}

impl FromWorld for LightProbeAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let supported_formats = match world.get_resource::<RenderDevice>() {
            Some(render_device) => CompressedImageFormats::from_features(render_device.features()),
            None => CompressedImageFormats::all(),
        };
        Self { supported_formats }
    }
}

impl AssetLoader for LightProbeAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let id = locate_asset_id::<LittleEndian>(bytes)?;
            let meta = locate_meta::<LittleEndian>(bytes)?;
            let data = LightProbeData::<LittleEndian>::slice(bytes, meta)?;
            info!("Loading light probe {} {:?}", id, data.head);

            let mut textures = Vec::with_capacity(data.textures.len());
            for (idx, texture_data) in data.textures.into_iter().enumerate() {
                let result = load_texture_asset(texture_data, &self.supported_formats)?;
                let mut slice_handles = Vec::with_capacity(result.slices.len());
                for (mip, images) in result.slices.into_iter().enumerate() {
                    let mut handles = Vec::with_capacity(images.len());
                    for (layer, image) in images.into_iter().enumerate() {
                        handles.push(load_context.set_labeled_asset(
                            &format!("image_{idx}_mip_{mip}_layer_{layer}"),
                            LoadedAsset::new(image),
                        ));
                    }
                    slice_handles.push(handles);
                }
                textures.push(TextureAsset {
                    asset_ref: AssetRef { id, kind: K_FORM_LTPB },
                    inner: result.inner,
                    // 3D BC1 textures are not supported by wgpu
                    // and we don't use the 3D texture anyway
                    texture: default(),
                    slices: slice_handles,
                });
            }
            load_context.set_default_asset(LoadedAsset::new(LightProbeAsset {
                head: data.head,
                textures,
                extra: data.extra,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["ltpb"] }
}
