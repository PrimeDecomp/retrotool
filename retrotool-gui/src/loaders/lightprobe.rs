use anyhow::Error;
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    render::{renderer::RenderDevice, texture::CompressedImageFormats},
};
use binrw::Endian;
use retrolib::format::{
    foot::{locate_asset_id, locate_meta},
    ltpb::{LightProbeBundleHeader, LightProbeData, LightProbeExtra},
};

use crate::loaders::texture::{load_texture_asset, TextureAsset};

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
            let id = locate_asset_id(bytes, Endian::Little)?;
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = LightProbeData::slice(bytes, meta, Endian::Little)?;
            info!("Loading light probe {} {:?}", id, data.head);

            let mut textures = Vec::with_capacity(data.textures.len());
            for texture_data in data.textures {
                textures.push(load_texture_asset(id, texture_data, &self.supported_formats)?);
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
