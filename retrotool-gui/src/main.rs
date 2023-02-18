mod loaders;

use std::fs::FileType;

use bevy::{
    asset::LoadState,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
};
use bevy_egui::{
    egui,
    egui::{text::LayoutJob, Color32, FontId, TextFormat, Widget},
    EguiContext, EguiPlugin, EguiSettings,
};
use egui::TextureId;
use retrolib::format::txtr::K_FORM_TXTR;
use walkdir::{DirEntry, WalkDir};

use crate::loaders::{
    package_loader_system, PackageAssetLoader, PackageDirectory, RetroAssetIoPlugin,
    TxtrAssetLoader, TxtrData,
};

struct Images {
    bevy_icon: Handle<Image>,
    bevy_icon_inverted: Handle<Image>,
}

impl FromWorld for Images {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.get_resource_mut::<AssetServer>().unwrap();
        Self {
            bevy_icon: asset_server.load("icon.png"),
            bevy_icon_inverted: asset_server.load("icon_inverted.png"),
        }
    }
}

/// This example demonstrates the following functionality and use-cases of bevy_egui:
/// - rendering loaded assets;
/// - toggling hidpi scaling (by pressing '/' button);
/// - configuring egui contexts during the startup.
fn main() {
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.0, 0.0, 0.0)))
        .insert_resource(Msaa { samples: 1 })
        .init_resource::<UiState>()
        .init_resource::<Packages>()
        .init_resource::<Textures>()
        .init_resource::<OpenAsset>()
        .add_plugins(
            DefaultPlugins
                .build()
                // the custom asset io plugin must be inserted in-between the
                // `CorePlugin' and `AssetPlugin`. It needs to be after the
                // CorePlugin, so that the IO task pool has already been constructed.
                // And it must be before the `AssetPlugin` so that the asset plugin
                // doesn't create another instance of an asset server. In general,
                // the AssetPlugin should still run so that other aspects of the
                // asset system are initialized correctly.
                .add_before::<AssetPlugin, _>(RetroAssetIoPlugin),
        )
        .add_plugin(PackageAssetLoader)
        .add_plugin(TxtrAssetLoader)
        .add_plugin(EguiPlugin)
        .add_startup_system(configure_visuals)
        .add_startup_system(preload_package)
        .add_system(update_ui_scale_factor)
        .add_system(ui_example)
        .add_system(package_loader_system)
        .add_system(check_assets_ready)
        .run();
}

enum TabType {
    Directory,
    Asset(HandleUntyped, Option<TextureId>),
    Empty,
}

#[derive(Resource)]
struct UiState {
    tree: egui_dock::Tree<TabType>,
}

impl Default for UiState {
    fn default() -> Self {
        let mut tree = egui_dock::Tree::new(vec![TabType::Empty]);
        tree.split_left(egui_dock::NodeIndex::root(), 0.3, vec![TabType::Directory]);
        Self { tree }
    }
}

struct TabViewer<'a> {
    server: Res<'a, AssetServer>,
    packages: Res<'a, Assets<PackageDirectory>>,
    textures: Res<'a, Assets<TxtrData>>,
    open_asset: ResMut<'a, OpenAsset>,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = TabType;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            TabType::Directory => {
                for (_, package) in self.packages.iter() {
                    egui::CollapsingHeader::new(&package.name).show(ui, |ui| {
                        for entry in &package.entries {
                            let mut job = LayoutJob::simple(
                                format!("{} {}", entry.kind, entry.id),
                                FontId::monospace(12.0),
                                Color32::GRAY,
                                0.0,
                            );
                            if let Some(name) = &entry.name {
                                job.append(
                                    &format!("\n{name}"),
                                    0.0,
                                    TextFormat::simple(FontId::monospace(12.0), Color32::WHITE),
                                );
                            }
                            if egui::SelectableLabel::new(false, job).ui(ui).clicked() {
                                self.open_asset.0 = match entry.kind {
                                    K_FORM_TXTR => Some(
                                        self.server
                                            .load::<TxtrData, _>(format!(
                                                "{}.{}",
                                                entry.id, entry.kind
                                            ))
                                            .into(),
                                    ),
                                    _ => None,
                                };
                            }
                        }
                    });
                }
            }
            TabType::Asset(handle, image) => {
                if let Some(txtr) = self.textures.get(&handle.typed_weak::<TxtrData>()) {
                    if let Some(image) = image {
                        ui.add(egui::widgets::Image::new(*image, egui::Vec2 {
                            x: txtr.data.head.width as f32,
                            y: txtr.data.head.height as f32,
                        }));
                    }
                }
            }
            TabType::Empty => {}
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            TabType::Directory => "Directory".into(),
            TabType::Asset(_, _) => "Asset view".into(),
            TabType::Empty => "Placeholder".into(),
        }
    }
}

fn configure_visuals(mut egui_ctx: ResMut<EguiContext>) {
    // egui_ctx
    //     .ctx_mut()
    //     .set_visuals(egui::Visuals { window_rounding: 0.0.into(), ..Default::default() });
}

#[derive(Default, Resource)]
struct Packages(Vec<Handle<PackageDirectory>>);
#[derive(Default, Resource)]
struct Textures(Vec<Handle<TxtrData>>);

fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name().to_str().map(|s| s.starts_with('.')).unwrap_or(false)
}

fn preload_package(server: Res<AssetServer>, mut loading: ResMut<Packages>) {
    let walker = WalkDir::new("/home/lstreet/Development/mpr/extract/romfs").into_iter();
    for entry in walker.filter_entry(|e| !is_hidden(e)).filter_map(|e| e.ok()) {
        if entry.file_type().is_file() && entry.path().extension() == Some("pak".as_ref()) {
            loading.0.push(server.load(entry.path()));
        }
    }
}

fn check_assets_ready(
    server: Res<AssetServer>,
    mut loading: ResMut<OpenAsset>,
    mut ui_state: ResMut<UiState>,
    textures: Res<Assets<TxtrData>>,
    mut egui_ctx: ResMut<EguiContext>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(handle) = &loading.0 {
        let state = server.get_load_state(handle);
        match state {
            LoadState::NotLoaded => {}
            LoadState::Loading => {}
            LoadState::Loaded => {
                let handle = std::mem::take(&mut loading.0).unwrap();
                let txtr = textures.get(&handle.typed_weak::<TxtrData>()).unwrap();
                let image = if let Some(rgba) = &txtr.rgba {
                    let image_handle = images.add(Image {
                        data: rgba.clone(),
                        texture_descriptor: TextureDescriptor {
                            label: None,
                            size: Extent3d {
                                width: txtr.data.head.width,
                                height: txtr.data.head.height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: TextureFormat::Rgba8Unorm,
                            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        },
                        sampler_descriptor: Default::default(),
                        texture_view_descriptor: None,
                    });
                    Some(egui_ctx.add_image(image_handle))
                } else {
                    None
                };
                ui_state.tree.push_to_first_leaf(TabType::Asset(handle, image));
            }
            LoadState::Failed => {}
            LoadState::Unloaded => {}
        }
    }
}

fn update_ui_scale_factor(
    keyboard_input: Res<Input<KeyCode>>,
    mut toggle_scale_factor: Local<Option<bool>>,
    mut egui_settings: ResMut<EguiSettings>,
    windows: Res<Windows>,
) {
    if keyboard_input.just_pressed(KeyCode::Slash) || toggle_scale_factor.is_none() {
        *toggle_scale_factor = Some(!toggle_scale_factor.unwrap_or(true));

        if let Some(window) = windows.get_primary() {
            let scale_factor =
                if toggle_scale_factor.unwrap() { 1.0 } else { 1.0 / window.scale_factor() };
            egui_settings.scale_factor = scale_factor;
        }
    }
}

#[derive(Default, Resource)]
struct OpenAsset(Option<HandleUntyped>);

fn ui_example(
    mut egui_ctx: ResMut<EguiContext>,
    mut ui_state: ResMut<UiState>,
    packages: Res<Assets<PackageDirectory>>,
    textures: Res<Assets<TxtrData>>,
    server: Res<AssetServer>,
    open_asset: ResMut<OpenAsset>,
) {
    egui::TopBottomPanel::top("top_panel").show(egui_ctx.ctx_mut(), |ui| {
        // The top panel is often a good place for a menu bar:
        egui::menu::bar(ui, |ui| {
            egui::menu::menu_button(ui, "File", |ui| {
                if ui.button("Quit").clicked() {
                    std::process::exit(0);
                }
            });
        });
    });

    egui_dock::DockArea::new(&mut ui_state.tree)
        .style(egui_dock::Style::from_egui(egui_ctx.ctx_mut().style().as_ref()))
        .show(egui_ctx.ctx_mut(), &mut TabViewer { server, packages, textures, open_asset });
}
