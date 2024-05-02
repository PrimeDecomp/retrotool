use anyhow::Error;
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext},
    prelude::{FromWorld, World},
};
use retrolib::format::{cmdl::ModelData, foot::locate_meta, mtrl::MaterialData};
use zerocopy::LittleEndian;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e82a"]
pub struct MaterialAsset {
    pub inner: ModelData<LittleEndian>,
}

pub struct MaterialAssetLoader;

impl FromWorld for MaterialAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for MaterialAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        _load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta::<LittleEndian>(bytes)?;
            // let (desc, data, remain) = FormDescriptor::slice(bytes, Endian::Little)?;
            // log::info!("Loading material {:?}", desc);
            let _mtrl = MaterialData::<LittleEndian>::slice(bytes, meta)?;
            // fs::write("mtrl.out", &mtrl.decompressed)?;
            // load_context.set_default_asset(
            //     LoadedAsset::new(ModelAsset { inner: data, textures })
            //         .with_dependencies(dependencies.into_values().collect()),
            // );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["mtrl"] }
}
