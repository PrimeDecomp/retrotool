mod icon;
mod loaders;
mod material;
mod render;
mod tabs;

#[cfg(test)]
mod tests;

use std::{borrow::Cow, path::PathBuf, time::Duration};

use bevy::{
    app::AppExit,
    diagnostic::{
        DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin,
        SystemInformationDiagnosticsPlugin,
    },
    prelude::*,
    window::WindowResolution,
};
use bevy_egui::{
    egui, EguiContext, EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPostUpdateSet,
    EguiPrimaryContextPass, EguiStartupSet, PrimaryEguiContext,
};
use egui::{FontFamily, FontId, Frame};
use egui_dock::{DockState, NodeIndex};
use retrolib::format::FourCC;
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

use crate::{
    loaders::{
        model::ModelAsset,
        package::{PackageSession, PackageSourcePlugin, SharedPackageInfo},
        texture::TextureAsset,
        RetroAssetPlugin,
    },
    material::CustomMaterial,
    render::grid::GridPlugin,
    tabs::{project::ProjectTab, splash::SplashTab, EditorTab, TabState, TabType, TabViewer},
};

#[derive(Default, Resource)]
struct FileOpen(Vec<PathBuf>);

fn main() -> anyhow::Result<()> {
    let mut file_open = FileOpen::default();
    let mut preview = render::scene_preview::ShaderPreview::default();
    if let Some(path) = render::scene_preview::brdf_preference_path() {
        preview.restore_brdf_preference(path);
    }
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--brdf-lut" {
            let path = args.next().ok_or_else(|| anyhow::anyhow!("--brdf-lut requires a path"))?;
            preview.request_brdf(path.into());
        } else if arg == "--" {
            file_open.0.extend(args.map(PathBuf::from));
            break;
        } else {
            file_open.0.push(arg.into());
        }
    }
    let mut app = App::new();
    #[cfg(feature = "embed")]
    app.add_plugins(bevy_embedded_assets::EmbeddedAssetPlugin {
        mode: bevy_embedded_assets::PluginMode::ReplaceDefault,
    });
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.05)))
        .insert_resource(bevy::winit::WinitSettings {
            focused_mode: bevy::winit::UpdateMode::Continuous,
            unfocused_mode: bevy::winit::UpdateMode::reactive_low_power(Duration::from_secs(5)),
        })
        .insert_resource(file_open)
        .insert_resource(preview)
        .init_resource::<UiState>()
        .init_resource::<SessionCommands>()
        .add_plugins(PackageSourcePlugin)
        .add_plugins(
            DefaultPlugins
                .build()
                .set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::RenderCreation::Automatic(Box::new(
                        bevy::render::settings::WgpuSettings {
                            features: bevy::render::settings::WgpuFeatures::TEXTURE_COMPRESSION_BC,
                            ..default()
                        },
                    )),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        position: WindowPosition::Centered(MonitorSelection::Current),
                        resolution: WindowResolution::new(1600, 900),
                        title: "retrotool".to_string(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        // App
        .add_plugins(RetroAssetPlugin)
        .add_plugins(MaterialPlugin::<CustomMaterial>::default())
        .add_plugins(render::original_shader::OriginalShaderPlugin)
        .add_plugins(render::scene_preview::ShaderPreviewPlugin)
        .add_plugins(EguiPlugin::default())
        .add_plugins(GridPlugin)
        .insert_resource(EguiGlobalSettings { auto_create_primary_context: false, ..default() })
        .add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            EntityCountDiagnosticsPlugin::default(),
            SystemInformationDiagnosticsPlugin,
        ))
        .add_systems(Startup, setup_cameras.before(EguiStartupSet::InitContexts))
        .add_systems(Startup, setup_egui.after(EguiStartupSet::InitContexts))
        .add_systems(Update, (file_drop, apply_session_commands, load_files, prepare_tabs).chain())
        .add_systems(EguiPrimaryContextPass, ui_system)
        // UI passes set the active scene cameras, transforms and mesh visibility.
        // Bevy must consume their final state in this frame, after all egui passes.
        .configure_sets(
            PostUpdate,
            EguiPostUpdateSet::EndPass
                .before(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::CameraUpdateSystems)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        )
        .run();
    Ok(())
}

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Hash)]
pub struct AssetRef {
    id: Uuid,
    kind: FourCC,
}

#[derive(Resource)]
struct UiState {
    tree: DockState<TabType>,
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
struct SessionCommands {
    clear_packages: bool,
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name().to_str().map(|s| s.starts_with('.')).unwrap_or(false)
}

fn file_drop(mut dnd_evr: MessageReader<FileDragAndDrop>, mut file_open: ResMut<FileOpen>) {
    for ev in dnd_evr.read() {
        if let FileDragAndDrop::DroppedFile { window: _, path_buf } = ev {
            file_open.0.push(path_buf.clone());
        }
    }
}

fn load_files(
    server: Res<AssetServer>,
    mut packages: ResMut<PackageSession>,
    mut file_open: ResMut<FileOpen>,
) {
    if file_open.0.is_empty() {
        return;
    }
    for path_buf in std::mem::take(&mut file_open.0) {
        if path_buf.is_dir() {
            let walker = WalkDir::new(path_buf).sort_by_file_name().into_iter();
            for entry in walker.filter_entry(|e| !is_hidden(e)).filter_map(|e| e.ok()) {
                if entry.file_type().is_file() && entry.path().extension() == Some("pak".as_ref()) {
                    if let Err(error) = packages.open(&server, entry.path()) {
                        log::error!("Cannot open {}: {error:#}", entry.path().display());
                    }
                }
            }
        } else {
            if let Err(error) = packages.open(&server, &path_buf) {
                log::error!("Cannot open {}: {error:#}", path_buf.display());
            }
        }
    }
}

// Run once per update, outside egui's pass. A future multi-pass UI can request
// this repeatedly without clearing twice or creating another asset session.
fn apply_session_commands(world: &mut World) {
    if !std::mem::take(&mut world.resource_mut::<SessionCommands>().clear_packages) {
        return;
    }
    world.resource_scope::<UiState, _>(|world, mut ui_state| {
        close_matching_tabs(world, &mut ui_state.tree, |tab| tab.asset().is_some());
        for (_, tab) in ui_state.tree.iter_all_tabs_mut() {
            if tab.asset().is_none() {
                tab.clear_assets(world);
            }
        }
    });
    world.resource_mut::<PackageSession>().clear();
    world.resource_mut::<FileOpen>().0.clear();
}

fn close_all_tabs(world: &mut World, ui_state: &mut UiState) {
    close_matching_tabs(world, &mut ui_state.tree, |_| true);
}

fn close_matching_tabs(
    world: &mut World,
    tree: &mut DockState<TabType>,
    matches: impl Fn(&TabType) -> bool,
) {
    let closed: Vec<_> = tree
        .iter_all_tabs_mut()
        .filter_map(|(_, tab)| (matches(tab) && tab.close(world)).then(|| tab.id()))
        .collect();
    // remove_tab repairs the active selection. egui_dock 0.21's retain_tabs
    // can leave it past the last remaining tab, producing an empty viewport.
    // Re-find each path because removing a leaf can rebalance the dock tree.
    for id in closed {
        if let Some(path) = tree.find_tab_from(|tab| tab.id() == id) {
            tree.remove_tab(path);
        }
    }
}

fn default_tree() -> DockState<TabType> {
    let mut tree = DockState::new(vec![SplashTab::new() as TabType]);
    tree.main_surface_mut().split_left(NodeIndex::root(), 0.25, vec![ProjectTab::new()]);
    tree
}

fn setup_cameras(mut commands: Commands) {
    // Clear once before any scene view, then draw egui over all viewports.
    commands.spawn((Camera2d, Camera { order: -1, ..default() }));
    commands.spawn((PrimaryEguiContext, Camera2d, Camera {
        order: 1000,
        clear_color: bevy::camera::ClearColorConfig::None,
        ..default()
    }));
}

fn prepare_tabs(world: &mut World) {
    for mut camera in
        world.query_filtered::<&mut Camera, With<render::camera::SceneCamera>>().iter_mut(world)
    {
        camera.is_active = false;
    }
    world.resource_scope::<UiState, _>(|world, mut state| {
        for (_, tab) in state.tree.iter_all_tabs_mut() {
            tab.load(world);
        }
    });
}

fn ui_system(world: &mut World) {
    let mut ctx = world
        .query_filtered::<&EguiContext, With<PrimaryEguiContext>>()
        .single(world)
        .unwrap()
        .clone();

    world.resource_scope::<UiState, _>(|world, mut ui_state| {
        let style = ctx.get_mut().global_style();
        let mut root_ui = egui::Ui::new(
            ctx.get_mut().clone(),
            egui::Id::new("viewer"),
            egui::UiBuilder::default(),
        );
        egui::Panel::top("top_panel")
            .show_separator_line(false)
            .frame(Frame::side_top_panel(style.as_ref()).fill(egui::Color32::BLACK))
            .show(&mut root_ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            world.write_message(AppExit::Success);
                        }
                    });
                    ui.menu_button("View", |ui| {
                        // Keep editable settings open until the user clicks elsewhere.
                        egui::containers::menu::SubMenuButton::new("Shader preview")
                            .config(
                                egui::containers::menu::MenuConfig::new()
                                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                            )
                            .ui(ui, |ui| {
                                ui.set_max_width(420.0);
                                world
                                    .resource_mut::<render::scene_preview::ShaderPreview>()
                                    .menu(ui);
                            });
                        if ui.button("Restore default layout").clicked() {
                            close_all_tabs(world, ui_state.as_mut());
                            if ui_state.tree.iter_all_tabs().next().is_none() {
                                ui_state.tree = default_tree();
                            }
                            ui.close();
                        }
                    });
                });
            });

        let diagnostics = world.resource::<DiagnosticsStore>();
        egui::Panel::bottom("bottom_panel").show_separator_line(false).show(&mut root_ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "[FPS {:.0} | RAM {}] [Loaded Textures: {} | Models: {} | Entities: {}]",
                    diagnostics
                        .get(&FrameTimeDiagnosticsPlugin::FPS)
                        .and_then(|d| d.smoothed())
                        .unwrap_or_default(),
                    diagnostics
                        .get(&SystemInformationDiagnosticsPlugin::SYSTEM_MEM_USAGE)
                        .and_then(|d| d
                            .measurement()
                            .map(|m| Cow::Owned(format!("{:.0}{}", m.value, d.suffix))))
                        .unwrap_or(Cow::Borrowed("?")),
                    world.resource::<Assets<TextureAsset>>().len(),
                    world.resource::<Assets<ModelAsset>>().len(),
                    diagnostics
                        .get(&EntityCountDiagnosticsPlugin::ENTITY_COUNT)
                        .and_then(|d| d.measurement())
                        .map(|m| m.value)
                        .unwrap_or_default(),
                ));
            });
        });

        let tab_assets = ui_state.tree.iter_all_tabs().filter_map(|(_, tab)| tab.asset()).collect();

        let package_session = world.resource::<SharedPackageInfo>().session();
        let mut viewer = TabViewer {
            world,
            state: TabState {
                open_assets: tab_assets,
                open_tab: None,
                render_layer: 0,
                close_all: None,
                close_others: None,
                package_session,
                clear_packages: false,
            },
        };

        // Setup and draw the dock area
        let mut style = egui_dock::Style::from_egui(style.as_ref());
        style.main_surface_border_stroke = egui::Stroke::new(5.0, egui::Color32::BLACK);
        style.buttons.add_tab_align = egui_dock::TabAddAlign::Right;
        style.dock_area_padding = Some(egui::Margin::same(5));
        style.separator.color_hovered = egui::Color32::GRAY;
        style.separator.color_idle = egui::Color32::BLACK;
        style.separator.width = 5.0;
        style.tab_bar.bg_fill = egui::Color32::BLACK;
        egui_dock::DockArea::new(&mut ui_state.tree)
            .style(style)
            .show_add_buttons(true)
            .show_add_popup(true)
            .show_inside(&mut root_ui, &mut viewer);

        if viewer.state.clear_packages {
            viewer.world.resource_mut::<SessionCommands>().clear_packages = true;
        }

        if let Some(path) = viewer.state.close_all {
            if let egui_dock::Node::Leaf(leaf) = &mut ui_state.tree[path] {
                leaf.tabs.retain_mut(|tab| !tab.close(viewer.world));
                leaf.active = egui_dock::TabIndex(0);
            }
        }
        if let Some((path, keep_id)) = viewer.state.close_others {
            if let egui_dock::Node::Leaf(leaf) = &mut ui_state.tree[path] {
                leaf.tabs.retain_mut(|tab| tab.id() == keep_id || !tab.close(viewer.world));
                leaf.active = egui_dock::TabIndex(0);
            }
        }
        // The upstream operation also removes empty leaves and window surfaces.
        ui_state.tree.retain_tabs(|_| true);

        // Open a new tab if requested
        if let Some(open) = viewer.state.open_tab {
            if let Some(node) = open.node {
                ui_state.tree.set_focused_node_and_surface(node);
                ui_state.tree.push_to_focused_leaf(open.tab);
            } else {
                let destination = ui_state
                    .tree
                    .iter_all_tabs()
                    .find(|(_, tab)| tab.id() != "project")
                    .map(|(path, _)| path.node_path());
                if let Some(path) = destination {
                    ui_state.tree.set_focused_node_and_surface(path);
                    ui_state.tree.push_to_focused_leaf(open.tab);
                } else {
                    ui_state.tree.push_to_first_leaf(open.tab);
                }
            }
        }
    });
}

fn setup_egui(mut context: EguiContexts, state: Res<UiState>) -> Result {
    let ctx = context.ctx_mut()?;

    let font = egui::FontData::from_static(include_bytes!("../icon.ttf"));
    let font_name = "blender".to_string();
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(font_name.clone(), font.into());
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, font_name.clone());
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, font_name);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.global_style()).clone();
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
    ctx.set_global_style(style);
    Ok(())
}
