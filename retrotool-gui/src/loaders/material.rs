use anyhow::Error;
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::{FromWorld, World},
};
use retrolib::format::{foot::locate_meta, mtrl::MaterialData};
use zerocopy::LittleEndian;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e82a"]
pub struct MaterialAsset {
    #[allow(unused)]
    pub inner: MaterialData<LittleEndian>,
}

pub struct MaterialAssetLoader;

impl FromWorld for MaterialAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for MaterialAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta::<LittleEndian>(bytes)?;
            let inner = MaterialData::<LittleEndian>::slice(bytes, meta)?;
            inner.payload()?;
            load_context.set_default_asset(LoadedAsset::new(MaterialAsset { inner }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["mtrl"] }
}
