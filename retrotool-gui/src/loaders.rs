use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::Error;
use bevy::{
    app::{App, Plugin},
    asset::{
        AddAsset, AssetIo, AssetIoError, AssetLoader, AssetPath, BoxedFuture, LoadContext,
        LoadedAsset, Metadata,
    },
    prelude::*,
    utils::HashMap,
};
use binrw::Endian;
use image::DynamicImage;
use retrolib::{
    format::{
        cmdl::{CMaterialDataInner, ModelData},
        foot::locate_meta,
        mtrl::MaterialData,
        pack::{Package, SparsePackageEntry},
        txtr::{texture_to_image, TextureData},
    },
    util::file::map_file,
};
use uuid::Uuid;

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
    Package::read_header(&data, Endian::Little)
}

fn read_asset(path: &Path, id: Uuid) -> anyhow::Result<Vec<u8>> {
    let data = map_file(path)?;
    Package::read_asset(&data, id, Endian::Little)
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
                        // println!("Loading {} from {}", id, package.path.display());
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
                println!("Loaded package {}", package.path.display());
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

impl Plugin for PackageAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<PackageDirectory>().add_asset_loader(PackageAssetLoader);
    }
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
                entries: Package::read_sparse(bytes, Endian::Little)?,
            }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["pak"] }
}

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e828"]
pub struct TextureAsset {
    pub inner: TextureData,
    pub decompressed: Option<DynamicImage>,
}

pub struct TextureAssetLoader;

impl Plugin for TextureAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<TextureAsset>().add_asset_loader(TextureAssetLoader);
    }
}

impl AssetLoader for TextureAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = TextureData::slice(bytes, meta, Endian::Little)?;
            let decompressed =
                if data.head.format.is_astc() { Some(texture_to_image(&data)?) } else { None };
            // println!("Loaded texture {:?}", data.head);
            load_context
                .set_default_asset(LoadedAsset::new(TextureAsset { inner: data, decompressed }));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["txtr"] }
}

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e829"]
pub struct ModelAsset {
    pub inner: ModelData,
    pub textures: HashMap<Uuid, Handle<TextureAsset>>,
}

pub struct ModelAssetLoader;

impl Plugin for ModelAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<ModelAsset>().add_asset_loader(ModelAssetLoader);
    }
}

impl AssetLoader for ModelAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta(bytes, Endian::Little)?;
            let data = ModelData::slice(bytes, meta, Endian::Little)?;
            println!("Loaded model {:?}", data.head);
            let mut dependencies = HashMap::<Uuid, AssetPath>::new();
            for mat in &data.mtrl.materials {
                for data in &mat.data {
                    match &data.data {
                        CMaterialDataInner::Texture(texture) => {
                            dependencies.insert(
                                texture.id,
                                AssetPath::new(PathBuf::from(format!("{}.TXTR", texture.id)), None),
                            );
                        }
                        CMaterialDataInner::LayeredTexture(texture) => {
                            for texture in &texture.textures {
                                if texture.id.is_nil() {
                                    continue;
                                }
                                dependencies.insert(
                                    texture.id,
                                    AssetPath::new(
                                        PathBuf::from(format!("{}.TXTR", texture.id)),
                                        None,
                                    ),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            let textures = dependencies
                .iter()
                .map(|(u, p)| (*u, load_context.get_handle(p.clone())))
                .collect();
            load_context.set_default_asset(
                LoadedAsset::new(ModelAsset { inner: data, textures })
                    .with_dependencies(dependencies.into_values().collect()),
            );
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] { &["cmdl", "smdl", "wmdl"] }
}

#[derive(Debug, Clone, bevy::reflect::TypeUuid)]
#[uuid = "83269869-1209-408e-8835-bc6f2496e82a"]
pub struct MaterialAsset {
    pub inner: ModelData,
}

pub struct MaterialAssetLoader;

impl Plugin for MaterialAssetLoader {
    fn build(&self, app: &mut App) {
        app.add_asset::<MaterialAsset>().add_asset_loader(MaterialAssetLoader);
    }
}

impl AssetLoader for MaterialAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        _load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, anyhow::Result<(), Error>> {
        Box::pin(async move {
            let meta = locate_meta(bytes, Endian::Little)?;
            // let (desc, data, remain) = FormDescriptor::slice(bytes, Endian::Little)?;
            // println!("Loading material {:?}", desc);
            let _mtrl = MaterialData::slice(bytes, meta, Endian::Little)?;
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
