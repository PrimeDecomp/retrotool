use std::{
    borrow::Cow,
    future::Future,
    io::{Cursor, Write},
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant, SystemTime},
};

use retrolib::format::{
    pack::{Asset, AssetInfo},
    rfrm::FormDescriptor,
    FourCC,
};
use uuid::Uuid;
use zerocopy::U32;

use super::*;

const KIND: FourCC = FourCC(*b"TEST");

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) { self.0.fetch_add(1, Ordering::Relaxed); }
}

#[test]
fn waiting_readers_wake_on_completion_and_clear() {
    let shared = SharedPackageInfo::default();
    let (key, _) = shared.catalog.write().unwrap().request("a.pak".into());
    let asset = AssetRef { id: Uuid::from_u128(1), kind: KIND };
    let wake = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let mut context = TaskContext::from_waker(&waker);
    let mut read = Box::pin(shared.resolve(0, asset));
    assert_eq!(read.as_mut().poll(&mut context), Poll::Pending);
    shared.publish(key, Arc::new(PackageIndex::new("a.pak".into(), vec![])));
    assert_eq!(wake.0.load(Ordering::Relaxed), 1);
    assert_eq!(read.as_mut().poll(&mut context), Poll::Ready(None));

    shared.catalog.write().unwrap().request("b.pak".into());
    let mut read = Box::pin(shared.resolve(0, asset));
    assert_eq!(read.as_mut().poll(&mut context), Poll::Pending);
    shared.clear();
    assert_eq!(wake.0.load(Ordering::Relaxed), 2);
    assert_eq!(read.as_mut().poll(&mut context), Poll::Ready(None));
}

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir()
            .join(format!("retrotool-packages-{}-{time}-{serial}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn write_package(path: &Path, value: u8) {
    let mut package = Package::<LittleEndian>::default();
    for (id, payload) in [(1, vec![value, 2]), (2, vec![value + 1])] {
        let mut data = Cursor::new(Vec::new());
        FormDescriptor::<LittleEndian> {
            id: KIND,
            reader_version: U32::new(1),
            writer_version: U32::new(1),
            ..Default::default()
        }
        .write(&mut data, |writer| {
            writer.write_all(&payload)?;
            Ok(())
        })
        .unwrap();
        let id = Uuid::from_u128(id);
        package.assets.push(Asset {
            id,
            kind: KIND,
            names: vec![],
            data: Cow::Owned(data.into_inner()),
            meta: None,
            info: AssetInfo { id, compression_mode: 0, orig_offset: 0 },
            version: 1,
            other_version: 1,
        });
    }
    let mut data = Cursor::new(Vec::new());
    package.write(&mut data).unwrap();
    std::fs::write(path, data.into_inner()).unwrap();
}

#[derive(bevy::prelude::Asset, TypePath)]
struct TestAsset {
    value: u8,
    #[dependency]
    child: Option<Handle<TestAsset>>,
}

#[derive(TypePath)]
struct TestLoader;
impl AssetLoader for TestLoader {
    type Asset = TestAsset;
    type Error = anyhow::Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<TestAsset> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let (_, payload, _) = FormDescriptor::<LittleEndian>::slice(&bytes)?;
        let path = payload
            .get(1)
            .map(|&id| {
                dependency_path(context.path(), AssetRef {
                    id: Uuid::from_u128(u128::from(id)),
                    kind: KIND,
                })
            })
            .transpose()?;
        let child = path.map(|path| context.load(path));
        Ok(TestAsset { value: payload[0], child })
    }

    fn extensions(&self) -> &[&str] { &["test"] }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(PackageSourcePlugin)
        .add_plugins(AssetPlugin::default())
        .init_asset::<PackageDirectory>()
        .init_asset_loader::<PackageAssetLoader>()
        .init_asset::<TestAsset>()
        .register_asset_loader(TestLoader)
        .add_systems(Update, package_loader_system);
    app
}

fn wait_for(app: &mut App, ready: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready(app.world()) {
        assert!(Instant::now() < deadline, "asset task did not settle");
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn open(app: &mut App, path: &Path) {
    let server = app.world().resource::<AssetServer>().clone();
    app.world_mut().resource_mut::<PackageSession>().open(&server, path).unwrap();
}

fn load_root(app: &App) -> Handle<TestAsset> {
    let session = app.world().resource::<SharedPackageInfo>().session();
    app.world()
        .resource::<AssetServer>()
        .load(asset_path(session, AssetRef { id: Uuid::from_u128(1), kind: KIND }))
}

fn values(app: &mut App, handle: &Handle<TestAsset>) -> (u8, u8) {
    wait_for(app, |world| {
        let assets = world.resource::<Assets<TestAsset>>();
        assets
            .get(handle)
            .and_then(|root| root.child.as_ref())
            .and_then(|child| assets.get(child))
            .is_some()
    });
    let assets = app.world().resource::<Assets<TestAsset>>();
    let root = assets.get(handle).unwrap();
    (root.value, assets.get(root.child.as_ref().unwrap()).unwrap().value)
}

#[test]
fn real_asset_tasks_keep_dependencies_in_their_session_after_clear() {
    let directory = TestDirectory::new();
    let a = directory.0.join("a.pak");
    let b = directory.0.join("b.pak");
    write_package(&a, 42);
    write_package(&b, 89);
    let mut app = app();
    open(&mut app, &a);
    open(&mut app, &b);
    let old = load_root(&app);
    assert_eq!(values(&mut app, &old), (42, 43));
    let stale_index = app.world().resource::<PackageSession>().requests[0].handle.clone();
    let server = app.world().resource::<AssetServer>();
    for path in [server.get_path(old.id()).unwrap(), server.get_path(stale_index.id()).unwrap()] {
        assert_eq!(path.source(), &bevy::asset::io::AssetSourceId::from(SOURCE));
    }

    app.world_mut().resource_mut::<PackageSession>().clear();
    // Keep old handles alive deliberately: even retained caches must not alias
    // the next session. The new resolution order and all child handles differ.
    open(&mut app, &b);
    open(&mut app, &a);
    // Deliver the same notification as a late index task, while its old value
    // is still retained. It cannot publish into the replacement session.
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<PackageDirectory>>>()
        .write(AssetEvent::Added { id: stale_index.id() });
    let new = load_root(&app);
    assert_ne!(old.id(), new.id());
    assert_eq!(values(&mut app, &new), (89, 90));
    assert_eq!(values(&mut app, &old), (42, 43));

    // Asset Removed events contain no value. Clearing again must tolerate the
    // old indexes disappearing, without trying to fetch them from Assets.
    app.world_mut().resource_mut::<PackageSession>().clear();
    let keys = app.world().resource::<Assets<PackageDirectory>>().ids().collect::<Vec<_>>();
    for key in keys {
        app.world_mut().resource_mut::<Assets<PackageDirectory>>().remove(key);
    }
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().resource::<SharedPackageInfo>().indexes().is_empty());
}

#[test]
fn failed_package_unblocks_assets_in_later_packages() {
    let directory = TestDirectory::new();
    let bad = directory.0.join("bad.pak");
    let good = directory.0.join("good.pak");
    std::fs::write(&bad, b"not a package").unwrap();
    write_package(&good, 17);
    let mut app = app();
    open(&mut app, &bad);
    open(&mut app, &good);
    let root = load_root(&app);
    assert_eq!(values(&mut app, &root), (17, 18));
    assert_eq!(app.world().resource::<SharedPackageInfo>().indexes().len(), 1);
}

#[test]
#[ignore = "requires MPR_PAKS containing the vine CMDL and its dependencies"]
fn original_model_dependencies_survive_clear_and_reopen() -> Result<()> {
    use crate::loaders::{
        material::{MaterialAsset, MaterialAssetLoader},
        model::{ModelAsset, ModelAssetLoader},
        texture::{TextureAsset, TextureAssetLoader},
    };

    let paths = std::env::var_os("MPR_PAKS").context("Set MPR_PAKS to a platform path list")?;
    let paths = std::env::split_paths(&paths).collect::<Vec<_>>();
    let mut app = app();
    app.init_asset::<Image>()
        .init_asset::<ModelAsset>()
        .init_asset_loader::<ModelAssetLoader>()
        .init_asset::<MaterialAsset>()
        .init_asset_loader::<MaterialAssetLoader>()
        .init_asset::<TextureAsset>()
        .init_asset_loader::<TextureAssetLoader>();
    let model = AssetRef {
        id: Uuid::parse_str("2a3bbc26-c384-43ab-b9ba-a16b8560ed5f")?,
        kind: retrolib::format::cmdl::K_FORM_CMDL,
    };
    let mut retained = Vec::new();
    let mut identities = Vec::new();
    for _ in 0..2 {
        for path in &paths {
            open(&mut app, path);
        }
        let session = app.world().resource::<SharedPackageInfo>().session();
        let handle =
            app.world().resource::<AssetServer>().load::<ModelAsset>(asset_path(session, model));
        wait_for(&mut app, |world| {
            let server = world.resource::<AssetServer>();
            assert!(!server.load_state(&handle).is_failed(), "model failed to load");
            world.resource::<Assets<ModelAsset>>().get(&handle).is_some_and(|model| {
                let state = model.get_load_state(server);
                assert!(!state.is_failed(), "model dependency failed to load");
                state.is_loaded()
            })
        });
        let assets = app.world().resource::<Assets<ModelAsset>>();
        let loaded = assets.get(&handle).unwrap();
        assert!(!loaded.textures.is_empty());
        assert!(!loaded.material_resources.is_empty());
        let mut ids = vec![handle.id().untyped()];
        for texture in loaded.textures.values() {
            let asset = app.world().resource::<Assets<TextureAsset>>().get(texture).unwrap();
            assert!(app.world().resource::<Assets<Image>>().get(&asset.texture).is_some());
            ids.push(texture.id().untyped());
        }
        for material in loaded.material_resources.values() {
            app.world()
                .resource::<Assets<MaterialAsset>>()
                .get(material)
                .unwrap()
                .inner
                .payload()?;
            ids.push(material.id().untyped());
        }
        identities.push(ids);
        retained.push(handle);
        app.world_mut().resource_mut::<PackageSession>().clear();
    }
    assert!(identities[0].iter().all(|id| !identities[1].contains(id)));
    eprintln!(
        "Original vine: {} model/material/texture handles per session, all distinct after Clear",
        identities[0].len()
    );
    Ok(())
}
