use anyhow::Error;
use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    prelude::*,
};
use retrolib::format::room::RoomData;
use zerocopy::LittleEndian;

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "12ae034e-f1f7-404a-8b7e-d04d9f8f34a7"]
pub struct RoomAsset {
    pub inner: RoomData<LittleEndian>,
}

pub struct RoomAssetLoader;

impl FromWorld for RoomAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for RoomAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let room = RoomData::<LittleEndian>::slice(bytes)?;
            // println!("Loaded ROOM: {:?}", room);
            let dependencies = vec![];
            load_context.set_default_asset(
                LoadedAsset::new(RoomAsset { inner: room }).with_dependencies(dependencies),
            );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["room"] }
}
