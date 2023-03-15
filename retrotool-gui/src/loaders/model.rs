use std::path::PathBuf;

use anyhow::Error;
use bevy::{
    app::{App, Plugin},
    asset::{AddAsset, AssetLoader, AssetPath, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
    utils::HashMap,
};
use binrw::Endian;
use retrolib::format::{
    cmdl::{CMaterialDataInner, ModelData},
    foot::locate_meta,
};
use uuid::Uuid;

use crate::loaders::texture::TextureAsset;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e829"]
pub struct ModelAsset {
    pub inner: ModelData,
    pub textures: HashMap<Uuid, Handle<TextureAsset>>,
}

pub struct ModelAssetLoader;

impl Plugin for ModelAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<ModelAsset>().add_asset_loader(ModelAssetLoader);
    }
}

impl AssetLoader for ModelAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = ModelData::slice(bytes, meta, Endian::Little)?;
            log::info!("Loaded model {:?}", data.head);
            log::info!("Loaded meshes {:#?}", data.mesh);
            let mut dependencies = HashMap::<Uuid, AssetPath>::new();
            for mat in &data.mtrl.materials {
                for data in &mat.data {
                    match &data.data {
                        CMaterialDataInner::Texture(texture) => {
                            dependencies.insert(
                                texture.id,
                                AssetPath::new(PathBuf::from(format!("{}.TXTR", texture.id)), None),
                            );
                        }
                        CMaterialDataInner::LayeredTexture(texture) => {
                            for texture in &texture.textures {
                                if texture.id.is_nil() {
                                    continue;
                                }
                                dependencies.insert(
                                    texture.id,
                                    AssetPath::new(
                                        PathBuf::from(format!("{}.TXTR", texture.id)),
                                        None,
                                    ),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let textures = dependencies
                .iter()
                .map(|(u, p)| (*u, load_context.get_handle(p.clone())))
                .collect();
            load_context.set_default_asset(
                LoadedAsset::new(ModelAsset { inner: data, textures })
                    .with_dependencies(dependencies.into_values().collect()),
            );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["cmdl", "smdl", "wmdl"] }
}
