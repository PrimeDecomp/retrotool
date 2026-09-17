//! Bevy adapter for ordered package sessions. The catalog and request identities
//! do not depend on Bevy's asset lifetime or completion order.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use bevy::{
    asset::{
        io::{AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, Reader, VecReader},
        AssetLoader, AssetPath, LoadContext,
    },
    prelude::*,
};
use event_listener::Event;
use retrolib::{format::pack::Package, util::file::map_file};
use zerocopy::LittleEndian;

use crate::AssetRef;

mod catalog;
pub use catalog::PackageIndex;
use catalog::{Catalog, PackageKey, Resolution};

const SOURCE: &str = "mpr";

pub fn asset_path(session: u64, asset: AssetRef) -> AssetPath<'static> {
    AssetPath::from(catalog::asset_path(session, asset)).with_source(SOURCE)
}

/// Dependencies stay in the requesting asset's session, including loads that
/// complete after Clear. They must never acquire the application's new session.
pub fn dependency_path(parent: &AssetPath<'_>, asset: AssetRef) -> Result<AssetPath<'static>> {
    let (session, _) =
        catalog::asset_request(parent.path()).context("Asset has no package session")?;
    Ok(asset_path(session, asset))
}

#[derive(Clone, Resource, Default)]
pub struct SharedPackageInfo {
    catalog: Arc<RwLock<Catalog>>,
    changed: Arc<Event>,
}

impl SharedPackageInfo {
    pub fn session(&self) -> u64 { self.catalog.read().unwrap().session() }

    pub fn indexes(&self) -> Vec<Arc<PackageIndex>> { self.catalog.read().unwrap().indexes() }

    fn clear(&self) {
        self.catalog.write().unwrap().clear();
        self.changed.notify(usize::MAX);
    }

    fn package_path(&self, key: PackageKey) -> Result<PathBuf> {
        self.catalog
            .read()
            .unwrap()
            .path(key)
            .map(Path::to_owned)
            .context("Package request belongs to a cleared session")
    }

    fn publish(&self, key: PackageKey, index: Arc<PackageIndex>) {
        if self.catalog.write().unwrap().publish(key, index) {
            self.changed.notify(usize::MAX);
        }
    }

    async fn resolve(&self, session: u64, asset: AssetRef) -> Option<PathBuf> {
        loop {
            // Listen before inspecting the catalog to avoid losing a completion
            // between the read and await. No lock is held across I/O or suspension.
            let changed = self.changed.listen();
            let resolution = self.catalog.read().unwrap().resolve(session, asset);
            match resolution {
                Resolution::Ready(path) => return Some(path),
                Resolution::Missing => return None,
                Resolution::Pending => changed.await,
            }
        }
    }
}

#[derive(Resource)]
pub struct PackageSession {
    shared: SharedPackageInfo,
    requests: Vec<PackageRequest>,
}

struct PackageRequest {
    key: PackageKey,
    handle: Handle<PackageDirectory>,
    settled: bool,
}

impl PackageSession {
    pub(crate) fn new(shared: SharedPackageInfo) -> Self { Self { shared, requests: vec![] } }

    pub fn open(&mut self, server: &AssetServer, path: &Path) -> Result<()> {
        let path = path.canonicalize().with_context(|| format!("Opening {}", path.display()))?;
        let (key, new) = self.shared.catalog.write().unwrap().request(path);
        if new {
            self.requests.push(PackageRequest {
                key,
                handle: server.load(AssetPath::from(key.path()).with_source(SOURCE)),
                settled: false,
            });
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        self.shared.clear();
        self.requests.clear();
    }
}

struct PackageReader(SharedPackageInfo);

impl AssetReader for PackageReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let result = if let Some((session, asset)) = catalog::asset_request(path) {
            let package = self
                .0
                .resolve(session, asset)
                .await
                .ok_or_else(|| AssetReaderError::NotFound(path.to_owned()))?;
            map_file(package).and_then(|data| Package::<LittleEndian>::read_asset(&data, asset.id))
        } else if let Some(key) = PackageKey::parse(path) {
            self.0.package_path(key).and_then(|path| {
                let data = map_file(path)?;
                Package::<LittleEndian>::read_header(&data)
            })
        } else {
            return Err(AssetReaderError::NotFound(path.to_owned()));
        };
        result.map(VecReader::new).map_err(|error| std::io::Error::other(error).into())
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<VecReader, AssetReaderError> {
        // The package's TOCC and resource footer provide game metadata; there
        // are no Bevy .meta sidecars in this source.
        Err(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(false)
    }
}

pub struct PackageSourcePlugin;

impl Plugin for PackageSourcePlugin {
    fn build(&self, app: &mut App) {
        let packages = SharedPackageInfo::default();
        let reader = packages.clone();
        app.register_asset_source(
            SOURCE,
            AssetSourceBuilder::new(move || Box::new(PackageReader(reader.clone()))),
        );
        app.insert_resource(PackageSession::new(packages.clone()));
        app.insert_resource(packages);
    }
}

pub fn package_loader_system(
    mut events: MessageReader<AssetEvent<PackageDirectory>>,
    assets: Res<Assets<PackageDirectory>>,
    server: Res<AssetServer>,
    mut session: ResMut<PackageSession>,
    packages: Res<SharedPackageInfo>,
) {
    for event in events.read() {
        let id = match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            _ => continue,
        };
        // Removed assets are already gone. Late events from a cleared session
        // are ignored even if the asset server has not released its cache yet.
        if let Some(request) = session.requests.iter_mut().find(|request| request.handle.id() == id)
        {
            if let Some(directory) = assets.get(id) {
                packages.publish(request.key, directory.0.clone());
                request.settled = true;
            }
        }
    }
    for request in session.requests.iter_mut().filter(|request| !request.settled) {
        if server.load_state(request.handle.id()).is_failed() {
            packages.catalog.write().unwrap().fail(request.key);
            request.settled = true;
            log::error!(
                "Package failed to load: {}",
                packages.package_path(request.key).unwrap().display()
            );
            packages.changed.notify(usize::MAX);
        }
    }
}

#[derive(Debug, Clone, Deref, Asset, TypePath)]
pub struct PackageDirectory(Arc<PackageIndex>);

#[derive(TypePath)]
pub struct PackageAssetLoader(SharedPackageInfo);

impl FromWorld for PackageAssetLoader {
    fn from_world(world: &mut World) -> Self { Self(world.resource::<SharedPackageInfo>().clone()) }
}

impl AssetLoader for PackageAssetLoader {
    type Asset = PackageDirectory;
    type Error = anyhow::Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<PackageDirectory> {
        let key =
            PackageKey::parse(context.path().path()).context("Package has no session identity")?;
        let path = self.0.package_path(key)?;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let index = PackageIndex::new(path, Package::<LittleEndian>::read_sparse(&bytes)?);
        Ok(PackageDirectory(Arc::new(index)))
    }

    fn extensions(&self) -> &[&str] { &["pak"] }
}

#[cfg(test)]
mod tests;
