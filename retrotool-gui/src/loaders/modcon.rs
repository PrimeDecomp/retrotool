use std::path::PathBuf;

use anyhow::Error;
use bevy::{
    asset::{AssetLoader, AssetPath, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
};
use binrw::Endian;
use retrolib::format::mcon::ModConData;

use crate::loaders::model::ModelAsset;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e82b"]
pub struct ModConAsset {
    pub inner: ModConData,
    pub models: Vec<Handle<ModelAsset>>,
}

pub struct ModConAssetLoader;

impl Plugin for ModConAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<ModConAsset>().add_asset_loader(ModConAssetLoader);
    }
}

impl AssetLoader for ModConAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let mcon = ModConData::slice(bytes, Endian::Little)?;
            // println!("Loaded MCON: {:?}", mcon);
            let mut dependencies = vec![];
            let mut models = vec![];
            if let Some(visual_data) = &mcon.visual_data {
                dependencies.reserve_exact(visual_data.models.len());
                models.reserve_exact(visual_data.models.len());
                for id in &visual_data.models {
                    let path = AssetPath::new(PathBuf::from(format!("{id}.CMDL")), None);
                    dependencies.push(path.clone());
                    models.push(load_context.get_handle(path));
                }
            }
            load_context.set_default_asset(
                LoadedAsset::new(ModConAsset { inner: mcon, models })
                    .with_dependencies(dependencies),
            );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["mcon"] }
}
