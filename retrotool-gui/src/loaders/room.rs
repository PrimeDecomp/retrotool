use anyhow::{ensure, Context, Error};
use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::*,
};
use retrolib::format::{
    mcon::K_FORM_MCON,
    room::{RoomData, MODCON_COMPONENT},
    txtr::K_FORM_TXTR,
};
use zerocopy::LittleEndian;

use crate::{
    loaders::{modcon::ModConAsset, package::dependency_path, texture::TextureAsset},
    AssetRef,
};

#[derive(Debug, Clone, Asset, TypePath)]
pub struct RoomAsset {
    pub inner: RoomData<LittleEndian>,
    pub modcons: Vec<RoomModCon>,
    pub lightmap: Option<Handle<TextureAsset>>,
}

/// An ordinary component definition and its geometry dependency. Array indices
/// retain definition identity even if editor GUIDs or resource IDs repeat.
#[derive(Debug, Clone)]
pub struct RoomModCon {
    pub layer_index: usize,
    pub component_index: usize,
    pub handle: Handle<ModConAsset>,
}

#[derive(TypePath)]
pub struct RoomAssetLoader;

impl FromWorld for RoomAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for RoomAssetLoader {
    type Asset = RoomAsset;
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
        let room = RoomData::<LittleEndian>::slice(bytes)?;
        let mut modcons = Vec::new();
        // This is an inspection dependency graph. Generated definitions and
        // runtime activity are not inferred by loading ordinary geometry.
        for (layer_index, layer) in room.layers.iter().enumerate() {
            for (component_index, component) in layer.components.iter().enumerate() {
                if component.component_type != MODCON_COMPONENT {
                    continue;
                }
                let properties = room
                    .component_properties
                    .get(component.property_index as usize)
                    .context("ModCon property index is out of range")?;
                ensure!(
                    properties.component_type == MODCON_COMPONENT,
                    "ModCon property type mismatch"
                );
                let properties = properties.modcon()?.unwrap();
                if properties.resource_id.is_nil() {
                    continue;
                }
                let path = dependency_path(load_context.path(), AssetRef {
                    id: properties.resource_id.into_inner(),
                    kind: K_FORM_MCON,
                })?;
                modcons.push(RoomModCon {
                    layer_index,
                    component_index,
                    handle: load_context.load(path),
                });
            }
        }
        let lightmap = room
            .baked_lighting
            .light_map
            .as_ref()
            .filter(|map| !map.txtr_id.is_nil())
            .map(|map| -> anyhow::Result<_> {
                let path = dependency_path(load_context.path(), AssetRef {
                    id: map.txtr_id.into_inner(),
                    kind: K_FORM_TXTR,
                })?;
                Ok(load_context.load(path))
            })
            .transpose()?;
        Ok(RoomAsset { inner: room, modcons, lightmap })
    }

    fn extensions(&self) -> &[&str] { &["room"] }
}
