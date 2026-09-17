use std::collections::HashMap;

use anyhow::Error;
use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
};
use retrolib::format::{cmdl, mcon::ModConData};
use uuid::Uuid;
use zerocopy::LittleEndian;

use crate::{
    loaders::{model::ModelAsset, package::dependency_path},
    AssetRef,
};

#[derive(Debug, Clone, Asset, TypePath)]
pub struct ModConAsset {
    pub inner: ModConData<LittleEndian>,
    pub models: Vec<Handle<ModelAsset>>,
    pub world_models: HashMap<Uuid, Handle<ModelAsset>>,
}

#[derive(TypePath)]
pub struct ModConAssetLoader;

impl FromWorld for ModConAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for ModConAssetLoader {
    type Asset = ModConAsset;
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
        let mcon = ModConData::<LittleEndian>::slice(bytes)?;
        // println!("Loaded MCON: {:?}", mcon);
        let mut models = vec![];
        let mut world_models = HashMap::new();
        if let Some(visual_data) = &mcon.visual_data {
            models = load_models(load_context, &visual_data.models, cmdl::K_FORM_CMDL)?;
            // BuildWorldModels requests placement IDs directly. Include them
            // even if a resource was omitted from the native preload table.
            let mut ids = visual_data.world_models.clone();
            for instance in &visual_data.world_instances {
                if !ids.contains(&instance.model) {
                    ids.push(instance.model);
                }
            }
            let handles = load_models(load_context, &ids, cmdl::K_FORM_WMDL)?;
            world_models = ids.into_iter().zip(handles).collect();
        }
        Ok(ModConAsset { inner: mcon, models, world_models })
    }

    fn extensions(&self) -> &[&str] { &["mcon"] }
}

fn load_models(
    context: &mut LoadContext<'_>,
    ids: &[Uuid],
    kind: retrolib::format::FourCC,
) -> anyhow::Result<Vec<Handle<ModelAsset>>> {
    ids.iter()
        .map(|&id| {
            let path = dependency_path(context.path(), AssetRef { id, kind })?;
            Ok(context.load(path))
        })
        .collect()
}
