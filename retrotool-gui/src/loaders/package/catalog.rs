//! Ordered package resolution, independent of the asset server. Requests reserve
//! their priority before I/O starts. Session-qualified paths prevent a late load
//! or cached dependency from a cleared session becoming part of the next one.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use retrolib::format::pack::SparsePackageEntry;

use crate::AssetRef;

const ROOT: &str = "__packages";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PackageKey {
    session: u64,
    index: usize,
}

impl PackageKey {
    pub fn path(self) -> PathBuf {
        PathBuf::from(format!("{ROOT}/{}/index/{}.pak", self.session, self.index))
    }

    pub fn parse(path: &Path) -> Option<Self> {
        let mut parts = path.iter();
        if parts.next()? != ROOT {
            return None;
        }
        let session = parts.next()?.to_str()?.parse().ok()?;
        if parts.next()? != "index" {
            return None;
        }
        let index = parts.next()?.to_str()?.strip_suffix(".pak")?.parse().ok()?;
        parts.next().is_none().then_some(Self { session, index })
    }
}

pub(super) fn asset_path(session: u64, asset: AssetRef) -> PathBuf {
    PathBuf::from(format!("{ROOT}/{session}/assets/{}.{}", asset.id, asset.kind))
}

pub(super) fn asset_request(path: &Path) -> Option<(u64, AssetRef)> {
    let mut parts = path.iter();
    if parts.next()? != ROOT {
        return None;
    }
    let session = parts.next()?.to_str()?.parse().ok()?;
    if parts.next()? != "assets" {
        return None;
    }
    let filename = parts.next()?.to_str()?;
    if parts.next().is_some() {
        return None;
    }
    let (id, kind) = filename.rsplit_once('.')?;
    Some((session, AssetRef {
        id: uuid::Uuid::try_parse(id).ok()?,
        kind: retrolib::format::FourCC(kind.as_bytes().try_into().ok()?),
    }))
}

#[derive(Debug)]
pub struct PackageIndex {
    pub path: PathBuf,
    pub name: String,
    pub entries: Vec<SparsePackageEntry>,
    assets: HashSet<AssetRef>,
}

impl PackageIndex {
    pub fn new(path: PathBuf, entries: Vec<SparsePackageEntry>) -> Self {
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let assets =
            entries.iter().map(|entry| AssetRef { id: entry.id, kind: entry.kind }).collect();
        Self { path, name, entries, assets }
    }
}

#[derive(Debug)]
enum LoadState {
    Pending,
    Ready(Arc<PackageIndex>),
    Failed,
}

#[derive(Debug)]
struct PackageSlot {
    path: PathBuf,
    state: LoadState,
}

#[derive(Debug, Default)]
pub(super) struct Catalog {
    session: u64,
    packages: Vec<PackageSlot>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Ready(PathBuf),
    Pending,
    Missing,
}

impl Catalog {
    pub fn session(&self) -> u64 { self.session }

    /// Paths must already be canonicalized by the caller. Returns whether the
    /// request is new; opening a package twice preserves its original priority.
    pub fn request(&mut self, path: PathBuf) -> (PackageKey, bool) {
        if let Some(index) = self.packages.iter().position(|entry| entry.path == path) {
            return (PackageKey { session: self.session, index }, false);
        }
        let key = PackageKey { session: self.session, index: self.packages.len() };
        self.packages.push(PackageSlot { path, state: LoadState::Pending });
        (key, true)
    }

    pub fn path(&self, key: PackageKey) -> Option<&Path> {
        self.slot(key).map(|slot| slot.path.as_path())
    }

    pub fn publish(&mut self, key: PackageKey, index: Arc<PackageIndex>) -> bool {
        let Some(slot) = self.slot_mut(key) else { return false };
        if slot.path != index.path {
            return false;
        }
        slot.state = LoadState::Ready(index);
        true
    }

    pub fn fail(&mut self, key: PackageKey) -> bool {
        let Some(slot) = self.slot_mut(key) else { return false };
        if matches!(slot.state, LoadState::Failed) {
            return false;
        }
        slot.state = LoadState::Failed;
        true
    }

    pub fn resolve(&self, session: u64, asset: AssetRef) -> Resolution {
        if session != self.session {
            return Resolution::Missing;
        }
        for slot in &self.packages {
            match &slot.state {
                LoadState::Pending => return Resolution::Pending,
                LoadState::Ready(index) if index.assets.contains(&asset) => {
                    return Resolution::Ready(index.path.clone());
                }
                _ => {}
            }
        }
        Resolution::Missing
    }

    pub fn indexes(&self) -> Vec<Arc<PackageIndex>> {
        self.packages
            .iter()
            .filter_map(|slot| match &slot.state {
                LoadState::Ready(index) => Some(index.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.session = self.session.checked_add(1).expect("package session counter exhausted");
        self.packages.clear();
    }

    fn slot(&self, key: PackageKey) -> Option<&PackageSlot> {
        if key.session != self.session {
            return None;
        }
        self.packages.get(key.index)
    }

    fn slot_mut(&mut self, key: PackageKey) -> Option<&mut PackageSlot> {
        if key.session != self.session {
            return None;
        }
        self.packages.get_mut(key.index)
    }
}

#[cfg(test)]
mod tests;
