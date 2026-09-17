use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use super::*;

struct SessionTab {
    asset: Option<AssetRef>,
    entity: Option<Entity>,
    clears: Arc<AtomicUsize>,
}

impl EditorTab for SessionTab {
    fn load(&mut self, _world: &mut World) {}

    fn ui(&mut self, _world: &mut World, _ui: &mut egui::Ui, _state: &mut TabState) {}

    fn close(&mut self, world: &mut World) -> bool {
        if let Some(entity) = self.entity.take() {
            world.despawn(entity);
        }
        true
    }

    fn clear_assets(&mut self, _world: &mut World) { self.clears.fetch_add(1, Ordering::Relaxed); }

    fn title(&self) -> egui::WidgetText { "Session test".into() }

    fn id(&self) -> String { format!("session-test-{}", self.asset.is_some()) }

    fn asset(&self) -> Option<AssetRef> { self.asset }
}

#[test]
fn clear_runs_once_outside_the_ui_and_keeps_non_asset_tabs() {
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let clears = Arc::new(AtomicUsize::new(0));
    let mut tree = egui_dock::DockState::<TabType>::new(vec![
        Box::new(SessionTab { asset: None, entity: None, clears: clears.clone() }),
        Box::new(SessionTab {
            asset: Some(AssetRef::default()),
            entity: Some(entity),
            clears: clears.clone(),
        }),
    ]);
    tree.main_surface_mut().set_active_tab(NodeIndex::root(), egui_dock::TabIndex(1)).unwrap();
    world.insert_resource(UiState { tree, ..Default::default() });
    let packages = SharedPackageInfo::default();
    world.insert_resource(PackageSession::new(packages.clone()));
    world.insert_resource(packages);
    world.insert_resource(FileOpen(vec!["pending.pak".into()]));
    world.insert_resource(SessionCommands { clear_packages: true });

    apply_session_commands(&mut world);
    apply_session_commands(&mut world);

    assert!(world.get_entity(entity).is_err());
    assert_eq!(clears.load(Ordering::Relaxed), 1);
    assert_eq!(world.resource::<SharedPackageInfo>().session(), 1);
    assert!(world.resource::<FileOpen>().0.is_empty());
    let tabs =
        world.resource::<UiState>().tree.iter_all_tabs().map(|(_, tab)| tab).collect::<Vec<_>>();
    assert_eq!(tabs.len(), 1);
    assert!(tabs[0].asset().is_none());
    let egui_dock::Node::Leaf(leaf) =
        &world.resource::<UiState>().tree.main_surface()[NodeIndex::root()]
    else {
        panic!("remaining tab lost its leaf");
    };
    assert_eq!(leaf.active, egui_dock::TabIndex(0));
}
