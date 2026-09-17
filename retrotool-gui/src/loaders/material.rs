use anyhow::Error;
use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext},
    prelude::{Asset, FromWorld, TypePath, World},
};
use retrolib::format::{foot::locate_meta, mtrl::MaterialData};
use zerocopy::LittleEndian;

#[derive(Debug, Clone, Asset, TypePath)]
pub struct MaterialAsset {
    #[allow(unused)]
    pub inner: MaterialData<LittleEndian>,
}

#[derive(TypePath)]
pub struct MaterialAssetLoader;

impl FromWorld for MaterialAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for MaterialAssetLoader {
    type Asset = MaterialAsset;
    type Error = Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> anyhow::Result<Self::Asset, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let bytes = bytes.as_slice();
        let meta = locate_meta::<LittleEndian>(bytes)?;
        let inner = MaterialData::<LittleEndian>::slice(bytes, meta)?;
        inner.payload()?;
        Ok(MaterialAsset { inner })
    }

    fn extensions(&self) -> &[&str] { &["mtrl"] }
}
