pub mod material;
pub mod modcon;
pub mod model;
pub mod package;
pub mod texture;

use bevy::prelude::*;

pub struct RetroAssetPlugin;

impl Plugin for RetroAssetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<package::RetroAssetIoPlugin>() {
            panic!("RetroAssetIoPlugin must be added before AssetPlugin");
        }
        app.add_asset::<material::MaterialAsset>()
            .add_asset::<modcon::ModConAsset>()
            .add_asset::<model::ModelAsset>()
            .add_asset::<package::PackageDirectory>()
            .add_asset::<texture::TextureAsset>()
            .init_asset_loader::<material::MaterialAssetLoader>()
            .init_asset_loader::<modcon::ModConAssetLoader>()
            .init_asset_loader::<model::ModelAssetLoader>()
            .init_asset_loader::<package::PackageAssetLoader>()
            .init_asset_loader::<texture::TextureAssetLoader>()
            .add_system(package::package_loader_system);
    }
}
