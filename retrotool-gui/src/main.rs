mod icon;
mod loaders;
mod material;
mod render;
mod tabs;

use std::{path::PathBuf, time::Duration};

use bevy::{
    app::AppExit,
    prelude::*,
    window::{PrimaryWindow, WindowResolution},
};
use bevy_egui::{egui, EguiContext, EguiContexts, EguiPlugin};
use egui::{FontFamily, FontId};
use retrolib::format::FourCC;
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

use crate::{
    loaders::{
        material::MaterialAssetLoader,
        modcon::ModConAssetLoader,
        model::ModelAssetLoader,
        package::{
            package_loader_system, PackageAssetLoader, PackageDirectory, RetroAssetIoPlugin,
        },
        texture::TextureAssetLoader,
    },
    material::CustomMaterial,
    render::TemporaryLabel,
    tabs::{load_tab, project::ProjectTab, TabState, TabType, TabViewer},
};

#[derive(Default, Resource)]
struct FileOpen(Vec<PathBuf>);

fn main() {
    let mut file_open = FileOpen::default();
    for arg in std::env::args_os() {
        file_open.0.push(arg.into());
    }
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.05, 0.05, 0.05)))
        .insert_resource(Msaa::default())
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::ReactiveLowPower {
                max_wait: Duration::from_secs(5),
            },
            ..Default::default()
        })
        // .insert_resource(AmbientLight { color: Color::rgb(1.0, 1.0, 1.0), brightness: 0.1 })
        .insert_resource(file_open)
        .init_resource::<UiState>()
        .init_resource::<Packages>()
        .add_plugins(
            DefaultPlugins
                .build()
                .set(bevy::render::RenderPlugin {
                    wgpu_settings: bevy::render::settings::WgpuSettings {
                        features: bevy::render::settings::WgpuFeatures::TEXTURE_COMPRESSION_BC,
                        ..default()
                    },
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        position: WindowPosition::Centered(MonitorSelection::Primary),
                        resolution: WindowResolution::new(1600.0, 900.0),
                        title: "retrotool".to_string(),
                        ..default()
                    }),
                    ..default()
                })
                .add_before::<AssetPlugin, _>(RetroAssetIoPlugin),
        )
        .add_plugin(MaterialPlugin::<CustomMaterial>::default())
        .add_plugin(PackageAssetLoader)
        .add_plugin(TextureAssetLoader)
        .add_plugin(ModelAssetLoader)
        .add_plugin(MaterialAssetLoader)
        .add_plugin(ModConAssetLoader)
        .add_plugin(EguiPlugin)
        .add_startup_system(setup_icon_font)
        .add_system(file_drop)
        .add_system(load_files)
        .add_system(package_loader_system)
        .add_system(ui_system)
        .run();
}

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq)]
pub struct AssetRef {
    id: Uuid,
    kind: FourCC,
}

#[derive(Resource)]
struct UiState {
    tree: egui_dock::Tree<TabType>,
    ui_font: FontId,
    code_font: FontId,
}

impl Default for UiState {
    fn default() -> Self {
        let mut tree = egui_dock::Tree::new(vec![TabType::Empty]);
        tree.split_left(egui_dock::NodeIndex::root(), 0.25, vec![TabType::Project(
            ProjectTab::default(),
        )]);
        Self {
            tree,
            ui_font: FontId { size: 13.0, family: FontFamily::Proportional },
            code_font: FontId { size: 14.0, family: FontFamily::Monospace },
        }
    }
}

#[derive(Default, Resource)]
struct Packages(Vec<Handle<PackageDirectory>>);

fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name().to_str().map(|s| s.starts_with('.')).unwrap_or(false)
}

fn file_drop(mut dnd_evr: EventReader<FileDragAndDrop>, mut file_open: ResMut<FileOpen>) {
    for ev in dnd_evr.iter() {
        if let FileDragAndDrop::DroppedFile { window: _, path_buf } = ev {
            file_open.0.push(path_buf.clone());
        }
    }
}

fn load_files(
    server: Res<AssetServer>,
    mut loading: ResMut<Packages>,
    mut file_open: ResMut<FileOpen>,
) {
    if file_open.0.is_empty() {
        return;
    }
    for path_buf in std::mem::take(&mut file_open.0) {
        if path_buf.is_dir() {
            let walker = WalkDir::new(path_buf).into_iter();
            for entry in walker.filter_entry(|e| !is_hidden(e)).filter_map(|e| e.ok()) {
                if entry.file_type().is_file() && entry.path().extension() == Some("pak".as_ref()) {
                    loading.0.push(server.load(entry.path()));
                }
            }
        } else {
            loading.0.push(server.load(path_buf));
        }
    }
}

fn ui_system(world: &mut World) {
    let mut ctx = world
        .query::<(&mut EguiContext, With<PrimaryWindow>)>()
        .iter(world)
        .next()
        .unwrap()
        .0
        .clone();

    egui::TopBottomPanel::top("top_panel").show(ctx.get_mut(), |ui| {
        egui::menu::bar(ui, |ui| {
            egui::menu::menu_button(ui, "File", |ui| {
                if ui.button("Quit").clicked() {
                    world.send_event(AppExit);
                }
            });
        });
    });

    world.resource_scope::<UiState, _>(|world, mut ui_state| {
        let mut tab_assets = vec![];
        for node in ui_state.tree.iter_mut() {
            if let egui_dock::Node::Leaf { tabs, .. } = node {
                for tab in tabs {
                    match tab {
                        TabType::Project(tab) => {
                            load_tab(world, &mut ctx, tab);
                        }
                        TabType::Texture(tab) => {
                            load_tab(world, &mut ctx, tab);
                            tab_assets.push(tab.asset_ref);
                        }
                        TabType::Model(tab) => {
                            load_tab(world, &mut ctx, tab);
                            tab_assets.push(tab.asset_ref);
                        }
                        TabType::ModCon(tab) => {
                            load_tab(world, &mut ctx, tab);
                            tab_assets.push(tab.asset_ref);
                        }
                        TabType::Empty => {}
                    }
                }
            }
        }

        // Remove all temporary entities
        let mut to_remove = vec![];
        for (entity, _) in world.query::<(Entity, With<TemporaryLabel>)>().iter(world) {
            to_remove.push(entity);
        }
        for entity in to_remove {
            world.despawn(entity);
        }

        let mut viewer = TabViewer {
            world,
            state: TabState {
                open_assets: tab_assets,
                open_tab: None,
                viewport: default(),
                render_layer: 0,
            },
        };
        egui_dock::DockArea::new(&mut ui_state.tree)
            .style(egui_dock::Style::from_egui(ctx.get_mut().style().as_ref()))
            .show(ctx.get_mut(), &mut viewer);

        if let Some(tab) = viewer.state.open_tab {
            ui_state.tree.push_to_first_leaf(tab);
        }

        if viewer.state.render_layer == 0 {
            // Spawn a camera to just clear the screen
            world.spawn((Camera3dBundle::default(), TemporaryLabel));
        }
    });
}

fn setup_icon_font(mut context: EguiContexts, state: ResMut<UiState>) {
    let ctx = context.ctx_mut();

    let font = egui::FontData::from_static(include_bytes!("../icon.ttf"));
    let font_name = "blender".to_string();
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(font_name.clone(), font);
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, font_name.clone());
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, font_name);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(egui::TextStyle::Body, FontId {
        size: (state.ui_font.size * 0.75).floor(),
        family: state.ui_font.family.clone(),
    });
    style.text_styles.insert(egui::TextStyle::Body, state.ui_font.clone());
    style.text_styles.insert(egui::TextStyle::Button, state.ui_font.clone());
    style.text_styles.insert(egui::TextStyle::Heading, FontId {
        size: (state.ui_font.size * 1.5).floor(),
        family: state.ui_font.family.clone(),
    });
    style.text_styles.insert(egui::TextStyle::Monospace, state.code_font.clone());
    ctx.set_style(style);
}
