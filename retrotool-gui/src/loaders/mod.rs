pub mod lightprobe;
pub mod material;
pub mod modcon;
pub mod model;
pub mod package;
pub mod room;
pub mod texture;

use bevy::prelude::*;

pub struct RetroAssetPlugin;

impl Plugin for RetroAssetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<package::PackageSourcePlugin>() {
            panic!("PackageSourcePlugin must be added before AssetPlugin");
        }
        app.init_asset::<material::MaterialAsset>()
            .init_asset::<modcon::ModConAsset>()
            .init_asset::<model::ModelAsset>()
            .init_asset::<package::PackageDirectory>()
            .init_asset::<texture::TextureAsset>()
            .init_asset::<lightprobe::LightProbeAsset>()
            .init_asset::<room::RoomAsset>()
            .init_asset_loader::<material::MaterialAssetLoader>()
            .init_asset_loader::<modcon::ModConAssetLoader>()
            .init_asset_loader::<model::ModelAssetLoader>()
            .init_asset_loader::<package::PackageAssetLoader>()
            .init_asset_loader::<room::RoomAssetLoader>()
            .add_systems(Update, package::package_loader_system.after(crate::load_files));
    }

    fn finish(&self, app: &mut App) {
        // RenderPlugin creates the device during finish. Texture decoding must
        // use its enabled compression features, not adapter capabilities.
        app.init_asset_loader::<texture::TextureAssetLoader>()
            .init_asset_loader::<lightprobe::LightProbeAssetLoader>();
    }
}
