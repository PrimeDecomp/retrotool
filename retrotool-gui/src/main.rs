mod icon;
mod loaders;
mod material;
mod render;
mod tabs;

use std::{borrow::Cow, path::PathBuf, time::Duration};

use bevy::{
    app::AppExit,
    asset::diagnostic::AssetCountDiagnosticsPlugin,
    diagnostic::{
        Diagnostics, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin,
        SystemInformationDiagnosticsPlugin,
    },
    prelude::*,
    window::{PrimaryWindow, WindowResolution},
};
use bevy_egui::{egui, EguiContext, EguiContexts, EguiPlugin};
use bevy_mod_raycast::{DefaultPluginState, DefaultRaycastingPlugin};
use egui::{FontFamily, FontId, Frame, Rounding};
use egui_dock::{NodeIndex, TabIndex, TabViewer as DockTabViewer};
use retrolib::format::FourCC;
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

use crate::{
    loaders::{
        model::ModelAsset,
        package::{PackageDirectory, RetroAssetIoPlugin},
        texture::TextureAsset,
        RetroAssetPlugin,
    },
    material::CustomMaterial,
    render::{grid::GridPlugin, TemporaryLabel},
    tabs::{
        modcon::ModConRaycastSet, project::ProjectTab, splash::SplashTab, EditorTab, TabState,
        TabType, TabViewer,
    },
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
            ..default()
        })
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
        // App
        .add_plugin(RetroAssetPlugin)
        .add_plugin(MaterialPlugin::<CustomMaterial>::default())
        .add_plugin(EguiPlugin)
        .add_plugin(GridPlugin)
        // Ray casting
        .add_plugin(DefaultRaycastingPlugin::<ModConRaycastSet>::default())
        .insert_resource(DefaultPluginState::<ModConRaycastSet>::default().with_debug_cursor())
        // Diagnostics
        .add_plugin(FrameTimeDiagnosticsPlugin::default())
        .add_plugin(EntityCountDiagnosticsPlugin::default())
        .add_plugin(AssetCountDiagnosticsPlugin::<TextureAsset>::default())
        .add_plugin(AssetCountDiagnosticsPlugin::<ModelAsset>::default())
        .add_plugin(SystemInformationDiagnosticsPlugin::default())
        // Systems
        .add_startup_system(setup_egui)
        .add_system(file_drop.before(load_files))
        .add_system(load_files)
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
        Self {
            tree: default_tree(),
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

fn close_all_tabs(world: &mut World, ui_state: &mut UiState) {
    for node in ui_state.tree.iter_mut() {
        if let egui_dock::Node::Leaf { tabs, .. } = node {
            tabs.retain_mut(|tab| !tab.close(world));
        }
    }
    'outer: loop {
        for (i, node) in ui_state.tree.iter().enumerate() {
            if matches!(node, egui_dock::Node::Leaf { tabs, .. } if tabs.is_empty()) {
                ui_state.tree.remove_leaf(NodeIndex(i));
                continue 'outer;
            }
        }
        break;
    }
}

fn default_tree() -> egui_dock::Tree<TabType> {
    let mut tree = egui_dock::Tree::<TabType>::new(vec![SplashTab::new()]);
    tree.split_left(NodeIndex::root(), 0.25, vec![ProjectTab::new()]);
    tree
}

fn ui_system(world: &mut World) {
    let mut ctx = world
        .query_filtered::<&EguiContext, With<PrimaryWindow>>()
        .get_single(world)
        .unwrap()
        .clone();

    world.resource_scope::<UiState, _>(|world, mut ui_state| {
        let style = ctx.get_mut().style();
        egui::TopBottomPanel::top("top_panel")
            .show_separator_line(false)
            .frame(Frame::side_top_panel(style.as_ref()).fill(egui::Color32::BLACK))
            .show(ctx.get_mut(), |ui| {
                egui::menu::bar(ui, |ui| {
                    egui::menu::menu_button(ui, "File", |ui| {
                        if ui.button("Quit").clicked() {
                            world.send_event(AppExit);
                        }
                    });
                    egui::menu::menu_button(ui, "View", |ui| {
                        if ui.button("Restore default layout").clicked() {
                            close_all_tabs(world, ui_state.as_mut());
                            if ui_state.tree.is_empty() {
                                ui_state.tree = default_tree();
                            }
                            ui.close_menu();
                        }
                    });
                });
            });

        let diagnostics = world.resource::<Diagnostics>();
        egui::TopBottomPanel::bottom("bottom_panel").show_separator_line(false).show(
            ctx.get_mut(),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "[FPS {:.0} | RAM {}] [Loaded Textures: {} | Models: {} | Entities: {}]",
                        diagnostics
                            .get(FrameTimeDiagnosticsPlugin::FPS)
                            .and_then(|d| d.smoothed())
                            .unwrap_or_default(),
                        diagnostics
                            .get(SystemInformationDiagnosticsPlugin::MEM_USAGE)
                            .and_then(|d| d
                                .measurement()
                                .map(|m| Cow::Owned(format!("{:.0}{}", m.value, d.suffix))))
                            .unwrap_or(Cow::Borrowed("?")),
                        diagnostics
                            .get_measurement(
                                AssetCountDiagnosticsPlugin::<TextureAsset>::diagnostic_id()
                            )
                            .map(|d| d.value)
                            .unwrap_or_default(),
                        diagnostics
                            .get_measurement(
                                AssetCountDiagnosticsPlugin::<ModelAsset>::diagnostic_id()
                            )
                            .map(|d| d.value)
                            .unwrap_or_default(),
                        diagnostics
                            .get_measurement(EntityCountDiagnosticsPlugin::ENTITY_COUNT)
                            .map(|d| d.value)
                            .unwrap_or_default(),
                    ));
                });
            },
        );

        let mut tab_assets = vec![];
        for node in ui_state.tree.iter_mut() {
            if let egui_dock::Node::Leaf { tabs, .. } = node {
                for tab in tabs {
                    tab.load(world);
                    if let Some(asset) = tab.asset() {
                        tab_assets.push(asset);
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
                close_all: None,
                close_others: None,
            },
        };

        // Setup and draw the dock area
        let mut style = egui_dock::Style::from_egui(style.as_ref());
        const MARGIN: f32 = 5.0;
        style.border.color = egui::Color32::BLACK;
        style.border.width = MARGIN * 2.0;
        style.buttons.add_tab_align = egui_dock::TabAddAlign::Right;
        style.dock_area_padding = Some(egui::Margin::same(MARGIN));
        style.separator.color_hovered = egui::Color32::GRAY;
        style.separator.color_idle = egui::Color32::BLACK;
        style.separator.width = MARGIN;
        style.tab_bar.bg_fill = egui::Color32::BLACK;
        style.tabs.bg_fill_unfocused = style.tabs.bg_fill.gamma_multiply(0.5);
        style.tabs.fill_tab_bar = true;
        style.tabs.grabbed_color = style.tabs.bg_fill.gamma_multiply(0.8);
        style.tabs.rounding = Rounding { nw: MARGIN, ne: MARGIN, sw: 0.0, se: 0.0 };
        style.tabs.text_align = egui::Align2::CENTER_CENTER;
        egui_dock::DockArea::new(&mut ui_state.tree)
            .style(style)
            .show_add_buttons(true)
            .show_add_popup(true)
            .show(ctx.get_mut(), &mut viewer);

        // Close all tabs in a group
        if let Some(node) = viewer.state.close_all {
            if let egui_dock::Node::Leaf { tabs, .. } = &mut ui_state.tree[node] {
                tabs.retain_mut(|tab| !viewer.on_close(tab));
                if tabs.is_empty() {
                    ui_state.tree.remove_leaf(node);
                } else {
                    ui_state.tree.set_active_tab(node, TabIndex(0));
                }
            }
        }

        // Close other tabs in a group
        if let Some((node, tab_index)) = viewer.state.close_others {
            if let egui_dock::Node::Leaf { tabs, .. } = &mut ui_state.tree[node] {
                let mut i = 0usize;
                tabs.retain_mut(|tab| {
                    let keep = i == tab_index.0;
                    i += 1;
                    keep || !viewer.on_close(tab)
                });
                ui_state.tree.set_active_tab(node, TabIndex(0));
            }
        }

        // Open a new tab if requested
        if let Some(open) = viewer.state.open_tab {
            if let Some(node) = open.node {
                ui_state.tree.set_focused_node(node);
                ui_state.tree.push_to_focused_leaf(open.tab);
            } else {
                ui_state.tree.push_to_first_leaf(open.tab);
            }
        }

        // If we're not rendering any scenes, spawn a camera to just clear the screen
        if viewer.state.render_layer == 0 {
            world.spawn((Camera3dBundle::default(), TemporaryLabel));
        }
    });
}

fn setup_egui(mut context: EguiContexts, state: ResMut<UiState>) {
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
