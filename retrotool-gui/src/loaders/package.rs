use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::Error;
use bevy::{
    app::{App, Plugin},
    asset::{AssetIo, AssetIoError, AssetLoader, BoxedFuture, LoadContext, LoadedAsset, Metadata},
    prelude::*,
};
use retrolib::{
    format::pack::{Package, SparsePackageEntry},
    util::file::map_file,
};
use uuid::Uuid;
use zerocopy::LittleEndian;

#[derive(Debug, Clone, Resource)]
pub struct SharedPackageInfo {
    packages: Arc<RwLock<Vec<PackageDirectory>>>,
}

struct RetroAssetIo {
    default: Box<dyn AssetIo>,
    packages: SharedPackageInfo,
}

fn read_pak_header(path: &Path) -> anyhow::Result<Vec<u8>> {
    let data = map_file(path)?;
    Package::<LittleEndian>::read_header(&data)
}

fn read_asset(path: &Path, id: Uuid) -> anyhow::Result<Vec<u8>> {
    let data = map_file(path)?;
    Package::<LittleEndian>::read_asset(&data, id)
}

impl AssetIo for RetroAssetIo {
    fn load_path<'a>(
        &'a self,
        path: &'a Path,
    ) -> BoxedFuture<'a, anyhow::Result<Vec<u8>, AssetIoError>> {
        if let Some(id) =
            path.file_stem().and_then(|name| Uuid::try_parse(&name.to_string_lossy()).ok())
        {
            // Find pak for UUID and load asset
            Box::pin(async move {
                let mut package_path: Option<PathBuf> = None;
                if let Ok(packages) = self.packages.packages.read() {
                    if let Some(package) =
                        packages.iter().find(|p| p.entries.iter().any(|e| e.id == id))
                    {
                        // log::info!("Loading {} from {}", id, package.path.display());
                        package_path = Some(package.path.clone());
                    }
                }
                let Some(package_path) = package_path else {
                    return Err(AssetIoError::NotFound(path.to_owned()));
                };
                read_asset(&package_path, id).map_err(|e| {
                    AssetIoError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
                })
            })
        } else if path.extension() == Some("pak".as_ref()) {
            // Load pak header only
            Box::pin(async move {
                read_pak_header(path).map_err(|e| {
                    AssetIoError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
                })
            })
        } else {
            self.default.load_path(path)
        }
    }

    fn read_directory(
        &self,
        path: &Path,
    ) -> anyhow::Result<Box<dyn Iterator<Item = PathBuf>>, AssetIoError> {
        self.default.read_directory(path)
    }

    fn get_metadata(&self, path: &Path) -> anyhow::Result<Metadata, AssetIoError> {
        self.default.get_metadata(path)
    }

    fn watch_path_for_changes(
        &self,
        to_watch: &Path,
        to_reload: Option<PathBuf>,
    ) -> anyhow::Result<(), AssetIoError> {
        self.default.watch_path_for_changes(to_watch, to_reload)
    }

    fn watch_for_changes(&self) -> anyhow::Result<(), AssetIoError> {
        self.default.watch_for_changes()
    }
}

pub struct RetroAssetIoPlugin;

impl Plugin for RetroAssetIoPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "embed")]
        let default = Box::new(bevy_embedded_assets::EmbeddedAssetIo::preloaded());
        #[cfg(not(feature = "embed"))]
        let default = AssetPlugin::default().create_platform_default_asset_io();
        let shared_package_info = SharedPackageInfo { packages: Arc::new(Default::default()) };
        let asset_io = RetroAssetIo { default, packages: shared_package_info.clone() };
        app.insert_resource(shared_package_info);
        app.insert_resource(AssetServer::new(asset_io));
    }
}

pub fn package_loader_system(
    mut ev_asset: EventReader<AssetEvent<PackageDirectory>>,
    assets: Res<Assets<PackageDirectory>>,
    package_info: Res<SharedPackageInfo>,
) {
    for ev in ev_asset.iter() {
        match ev {
            AssetEvent::Created { handle } => {
                let package = assets.get(handle).unwrap();
                log::info!("Loaded package {}", package.path.display());
                let mut package_info =
                    package_info.packages.write().expect("Failed to lock shared package info");
                package_info.push(package.clone());
            }
            AssetEvent::Modified { .. } => {}
            AssetEvent::Removed { handle } => {
                let package = assets.get(handle).unwrap();
                let mut package_info =
                    package_info.packages.write().expect("Failed to lock shared package info");
                package_info.retain(|p| p.path != package.path);
            }
        }
    }
}

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e827"]
pub struct PackageDirectory {
    pub path: PathBuf,
    pub name: String,
    pub entries: Vec<SparsePackageEntry>,
}

pub struct PackageAssetLoader;

impl FromWorld for PackageAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for PackageAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            load_context.set_default_asset(LoadedAsset::new(PackageDirectory {
                path: load_context.path().to_owned(),
                name: load_context
                    .path()
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                entries: Package::<LittleEndian>::read_sparse(bytes)?,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["pak"] }
}
