use retrolib::format::FourCC;

use super::*;

fn asset(id: u128, kind: [u8; 4]) -> AssetRef {
    AssetRef { id: uuid::Uuid::from_u128(id), kind: FourCC(kind) }
}

fn index(path: &str, assets: &[AssetRef]) -> Arc<PackageIndex> {
    Arc::new(PackageIndex::new(
        PathBuf::from(path),
        assets
            .iter()
            .map(|asset| SparsePackageEntry {
                id: asset.id,
                kind: asset.kind,
                names: vec![],
                reader_version: 1,
                writer_version: 1,
            })
            .collect(),
    ))
}

#[test]
fn completion_order_never_changes_duplicate_priority() {
    let duplicate = asset(1, *b"CMDL");
    for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        let mut catalog = Catalog::default();
        let paths = ["a.pak", "b.pak", "c.pak"];
        let keys = paths.map(|path| catalog.request(path.into()).0);
        let mut first_ready = false;
        for item in order {
            assert!(catalog.publish(keys[item], index(paths[item], &[duplicate])));
            first_ready |= item == 0;
            let expected =
                if first_ready { Resolution::Ready("a.pak".into()) } else { Resolution::Pending };
            assert_eq!(catalog.resolve(0, duplicate), expected, "completion order {order:?}");
        }
    }
}

#[test]
fn unrelated_pending_packages_only_block_lower_priority_candidates() {
    let model = asset(1, *b"CMDL");
    let texture = asset(1, *b"TXTR");
    let mut catalog = Catalog::default();
    let (a, _) = catalog.request("a.pak".into());
    let (b, _) = catalog.request("b.pak".into());
    catalog.publish(a, index("a.pak", &[texture]));
    assert_eq!(catalog.resolve(0, texture), Resolution::Ready("a.pak".into()));
    assert_eq!(catalog.resolve(0, model), Resolution::Pending);
    catalog.fail(b);
    assert_eq!(catalog.resolve(0, model), Resolution::Missing);
    assert!(catalog.publish(b, index("b.pak", &[model])));
    assert_eq!(catalog.resolve(0, model), Resolution::Ready("b.pak".into()));
}

#[test]
fn clear_rejects_late_publication_and_namespaces_every_dependency() {
    let model = asset(1, *b"CMDL");
    let texture = asset(2, *b"TXTR");
    let mut catalog = Catalog::default();
    let (old, _) = catalog.request("a.pak".into());
    let old_model = asset_path(catalog.session(), model);
    catalog.clear();
    let (new, _) = catalog.request("a.pak".into());
    assert_ne!(old.path(), new.path());
    assert!(!catalog.publish(old, index("a.pak", &[model])));
    assert!(!catalog.fail(old));
    assert!(catalog.path(old).is_none());
    assert_eq!(catalog.resolve(0, model), Resolution::Missing);
    assert_eq!(catalog.resolve(1, model), Resolution::Pending);
    catalog.publish(new, index("a.pak", &[model]));
    assert_eq!(catalog.resolve(1, model), Resolution::Ready("a.pak".into()));
    let old_dependency =
        super::super::dependency_path(&bevy::asset::AssetPath::from(old_model.clone()), texture)
            .unwrap();
    assert_eq!(asset_request(old_dependency.path()), Some((0, texture)));
    assert_ne!(old_dependency.path(), asset_path(1, texture));
}

#[test]
fn duplicate_requests_and_mismatched_publications_preserve_identity() {
    let mut catalog = Catalog::default();
    let (first, new) = catalog.request("a.pak".into());
    assert!(new);
    assert_eq!(catalog.request("a.pak".into()), (first, false));
    assert!(!catalog.publish(first, index("b.pak", &[])));
    assert_eq!(PackageKey::parse(&first.path()), Some(first));
    assert!(PackageKey::parse(Path::new("__packages/0/index/0.pak/extra")).is_none());
    let model = asset(1, *b"CMDL");
    assert_eq!(asset_request(&asset_path(17, model)), Some((17, model)));
    assert!(asset_request(Path::new("unrelated/17/assets/00000000.CMDL")).is_none());
    assert!(asset_request(Path::new("__packages/17/assets/../anything.CMDL")).is_none());
}
