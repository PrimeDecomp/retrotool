mod loaders;

use bevy::{
    asset::LoadState,
    prelude::*,
    render::{
        render_resource::{
            Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        },
    },
};
use bevy_egui::{
    egui,
    egui::{text::LayoutJob, Color32, FontId, TextFormat, Widget},
    EguiContext, EguiPlugin, EguiSettings,
};
use egui::TextureId;
use retrolib::format::{
    txtr::{ETextureFormat, ETextureType, K_FORM_TXTR},
    FourCC,
};
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

use crate::loaders::{
    package_loader_system, PackageAssetLoader, PackageDirectory, RetroAssetIoPlugin, TextureAsset,
    TextureAssetLoader,
};

/// This example demonstrates the following functionality and use-cases of bevy_egui:
/// - rendering loaded assets;
/// - toggling hidpi scaling (by pressing '/' button);
/// - configuring egui contexts during the startup.
fn main() {
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.0, 0.0, 0.0)))
        .insert_resource(Msaa { samples: 1 })
        .insert_resource(bevy::render::settings::WgpuSettings {
            features: bevy::render::settings::WgpuFeatures::TEXTURE_COMPRESSION_BC,
            ..Default::default()
        })
        .init_resource::<UiState>()
        .init_resource::<Packages>()
        .init_resource::<OpenAsset>()
        .add_plugins(DefaultPlugins.build().add_before::<AssetPlugin, _>(RetroAssetIoPlugin))
        .add_plugin(PackageAssetLoader)
        .add_plugin(TextureAssetLoader)
        .add_plugin(EguiPlugin)
        .add_startup_system(preload_package)
        .add_system(update_ui_scale_factor)
        .add_system(ui_example)
        .add_system(package_loader_system)
        .add_system(check_assets_ready)
        .run();
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct AssetRef {
    id: Uuid,
    kind: FourCC,
}

enum TabType {
    Directory,
    Texture(AssetRef, Handle<TextureAsset>, Vec<TextureId>),
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
    textures: Res<'a, Assets<TextureAsset>>,
    open_asset: ResMut<'a, OpenAsset>,
    tab_assets: Vec<AssetRef>,
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = TabType;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            TabType::Directory => {
                let mut packages_sorted =
                    self.packages.iter().map(|(_, p)| p).collect::<Vec<&PackageDirectory>>();
                packages_sorted.sort_by_key(|p| &p.name);
                for package in packages_sorted {
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
                            let asset_ref = AssetRef { id: entry.id, kind: entry.kind };
                            if egui::SelectableLabel::new(self.tab_assets.contains(&asset_ref), job)
                                .ui(ui)
                                .clicked()
                            {
                                *self.open_asset = OpenAsset(asset_ref, match entry.kind {
                                    K_FORM_TXTR => Some(
                                        self.server
                                            .load::<TextureAsset, _>(format!(
                                                "{}.{}",
                                                entry.id, entry.kind
                                            ))
                                            .into(),
                                    ),
                                    _ => None,
                                });
                            }
                        }
                    });
                }
            }
            TabType::Texture(asset_ref, handle, images) => {
                ui.label(format!("{} {}", asset_ref.kind, asset_ref.id));
                if let Some(txtr) = self.textures.get(handle) {
                    ui.label(format!("Type: {:?}", txtr.inner.head.kind));
                    ui.label(format!("Format: {:?}", txtr.inner.head.format));
                    ui.label(format!(
                        "Dimensions: {}x{}x{} (mips: {})",
                        txtr.inner.head.width,
                        txtr.inner.head.height,
                        txtr.inner.head.layers,
                        txtr.inner.head.mip_sizes.len()
                    ));
                    if txtr.inner.head.kind == ETextureType::Cube && images.len() == 6 {
                        let width = txtr.inner.head.width * 2;
                        let height = txtr.inner.head.height * 2;
                        let (_, rect) = ui.allocate_space(egui::Vec2 {
                            x: (width * 4) as f32,
                            y: (height * 3) as f32,
                        });
                        let size = egui::Vec2 { x: width as f32, y: height as f32 };
                        let mut draw_image = |i: usize, x: u32, y: u32| {
                            let min = egui::Vec2 { x: (width * x) as f32, y: (height * y) as f32 };
                            let max = egui::Vec2 {
                                x: (width * (x + 1)) as f32,
                                y: (height * (y + 1)) as f32,
                            };
                            egui::widgets::Image::new(images[i], size).paint_at(ui, egui::Rect {
                                min: rect.min + min,
                                max: rect.min + max,
                            });
                        };
                        draw_image(2, 1, 0);
                        draw_image(1, 0, 1);
                        draw_image(4, 1, 1);
                        draw_image(0, 2, 1);
                        draw_image(5, 3, 1);
                        draw_image(3, 1, 2);
                    } else {
                        for image in images {
                            ui.add(egui::widgets::Image::new(*image, egui::Vec2 {
                                x: txtr.inner.head.width as f32,
                                y: txtr.inner.head.height as f32,
                            }));
                        }
                    }
                }
            }
            TabType::Empty => {}
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            TabType::Directory => "Directory".into(),
            TabType::Texture(asset_ref, _, _) => {
                format!("{} {}", asset_ref.kind, asset_ref.id).into()
            }
            TabType::Empty => "Placeholder".into(),
        }
    }
}

#[derive(Default, Resource)]
struct Packages(Vec<Handle<PackageDirectory>>);

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
    textures: Res<Assets<TextureAsset>>,
    mut egui_ctx: ResMut<EguiContext>,
    mut images: ResMut<Assets<Image>>,
) {
    let OpenAsset(asset_ref, opt_handle) = &mut *loading;
    if let Some(handle) = &*opt_handle {
        let state = server.get_load_state(handle);
        match state {
            LoadState::NotLoaded => {}
            LoadState::Loading => {}
            LoadState::Loaded => {
                let handle = std::mem::take(opt_handle).unwrap().typed::<TextureAsset>();
                let txtr = textures.get(&handle).unwrap();
                let mut texture_ids = Vec::new();
                if let Some(rgba) = &txtr.rgba {
                    let image_handle = images.add(Image {
                        data: rgba.clone(),
                        texture_descriptor: TextureDescriptor {
                            label: None,
                            size: Extent3d {
                                width: txtr.inner.head.width,
                                height: txtr.inner.head.height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: if txtr.inner.head.format.is_srgb() {
                                TextureFormat::Rgba8UnormSrgb
                            } else {
                                TextureFormat::Rgba8Unorm
                            },
                            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        },
                        sampler_descriptor: Default::default(),
                        texture_view_descriptor: None,
                    });
                    texture_ids.push(egui_ctx.add_image(image_handle));
                } else {
                    let array_stride: usize = (txtr.inner.head.mip_sizes.iter().sum::<u32>()
                        / txtr.inner.head.layers)
                        as usize;
                    for layer in 0..txtr.inner.head.layers as usize {
                        let image_handle = images.add(Image {
                            data: txtr.inner.data
                                [layer * array_stride..(layer * array_stride) + array_stride]
                                .to_vec(),
                            texture_descriptor: TextureDescriptor {
                                label: None,
                                size: Extent3d {
                                    width: txtr.inner.head.width,
                                    height: txtr.inner.head.height,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: txtr.inner.head.mip_sizes.len() as u32,
                                sample_count: 1,
                                dimension: match txtr.inner.head.kind {
                                    ETextureType::_1D => TextureDimension::D1,
                                    ETextureType::_2D => TextureDimension::D2,
                                    ETextureType::_3D => TextureDimension::D3,
                                    ETextureType::Cube => TextureDimension::D2,
                                    ETextureType::_1DArray => TextureDimension::D1,
                                    ETextureType::_2DArray => TextureDimension::D2,
                                    ETextureType::_2DMultisample => TextureDimension::D2,
                                    ETextureType::_2DMultisampleArray => TextureDimension::D2,
                                    ETextureType::CubeArray => TextureDimension::D2,
                                },
                                format: match txtr.inner.head.format {
                                    ETextureFormat::R8Unorm => TextureFormat::Rgba8Unorm,
                                    ETextureFormat::R8Snorm => TextureFormat::R8Snorm,
                                    ETextureFormat::R8Uint => TextureFormat::R8Uint,
                                    ETextureFormat::R8Sint => TextureFormat::R8Sint,
                                    ETextureFormat::R16Unorm => TextureFormat::R16Unorm,
                                    ETextureFormat::R16Snorm => TextureFormat::R16Snorm,
                                    ETextureFormat::R16Uint => TextureFormat::R16Uint,
                                    ETextureFormat::R16Sint => TextureFormat::R16Sint,
                                    ETextureFormat::R16Float => TextureFormat::R16Float,
                                    ETextureFormat::R32Uint => TextureFormat::R32Uint,
                                    ETextureFormat::R32Sint => TextureFormat::R32Sint,
                                    ETextureFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
                                    ETextureFormat::Rgba8Srgb => TextureFormat::Rgba8UnormSrgb,
                                    ETextureFormat::Rgba16Float => TextureFormat::Rgba16Float,
                                    ETextureFormat::Rgba32Float => TextureFormat::Rgba32Float,
                                    ETextureFormat::Depth16Unorm => TextureFormat::Depth16Unorm,
                                    ETextureFormat::Depth16Unorm2 => TextureFormat::Depth16Unorm,
                                    ETextureFormat::Depth24S8Unorm => {
                                        TextureFormat::Depth24PlusStencil8
                                    }
                                    ETextureFormat::Depth32Float => TextureFormat::Depth32Float,
                                    ETextureFormat::RgbaBc1Unorm => TextureFormat::Bc1RgbaUnorm,
                                    ETextureFormat::RgbaBc1Srgb => TextureFormat::Bc1RgbaUnormSrgb,
                                    ETextureFormat::RgbaBc2Unorm => TextureFormat::Bc2RgbaUnorm,
                                    ETextureFormat::RgbaBc2Srgb => TextureFormat::Bc2RgbaUnormSrgb,
                                    ETextureFormat::RgbaBc3Unorm => TextureFormat::Bc3RgbaUnorm,
                                    ETextureFormat::RgbaBc3Srgb => TextureFormat::Bc3RgbaUnormSrgb,
                                    ETextureFormat::RgbaBc4Unorm => TextureFormat::Bc4RUnorm,
                                    ETextureFormat::RgbaBc4Snorm => TextureFormat::Bc4RSnorm,
                                    ETextureFormat::RgbaBc5Unorm => TextureFormat::Bc5RgUnorm,
                                    ETextureFormat::RgbaBc5Snorm => TextureFormat::Bc5RgSnorm,
                                    ETextureFormat::Rg11B10Float => TextureFormat::Rg11b10Float,
                                    ETextureFormat::R32Float => TextureFormat::R32Float,
                                    ETextureFormat::Rg8Unorm => TextureFormat::Rg8Unorm,
                                    ETextureFormat::Rg8Snorm => TextureFormat::Rg8Snorm,
                                    ETextureFormat::Rg8Uint => TextureFormat::Rg8Uint,
                                    ETextureFormat::Rg8Sint => TextureFormat::Rg8Sint,
                                    ETextureFormat::Rg16Float => TextureFormat::Rg16Float,
                                    ETextureFormat::Rg16Unorm => TextureFormat::Rg16Unorm,
                                    ETextureFormat::Rg16Snorm => TextureFormat::Rg16Snorm,
                                    ETextureFormat::Rg16Uint => TextureFormat::Rg16Uint,
                                    ETextureFormat::Rg16Sint => TextureFormat::Rg16Sint,
                                    ETextureFormat::Rgb10A2Unorm => TextureFormat::Rgb10a2Unorm,
                                    ETextureFormat::Rg32Uint => TextureFormat::Rg32Uint,
                                    ETextureFormat::Rg32Sint => TextureFormat::Rg32Sint,
                                    ETextureFormat::Rg32Float => TextureFormat::Rg32Float,
                                    ETextureFormat::Rgba16Unorm => TextureFormat::Rgba16Unorm,
                                    ETextureFormat::Rgba16Snorm => TextureFormat::Rgba16Snorm,
                                    ETextureFormat::Rgba16Uint => TextureFormat::Rgba16Uint,
                                    ETextureFormat::Rgba16Sint => TextureFormat::Rgba16Sint,
                                    ETextureFormat::Rgba32Uint => TextureFormat::Rgba32Uint,
                                    ETextureFormat::Rgba32Sint => TextureFormat::Rgba32Sint,
                                    ETextureFormat::BptcUfloat => TextureFormat::Bc6hRgbUfloat,
                                    ETextureFormat::BptcSfloat => TextureFormat::Bc6hRgbSfloat,
                                    ETextureFormat::BptcUnorm => TextureFormat::Bc7RgbaUnorm,
                                    ETextureFormat::BptcUnormSrgb => {
                                        TextureFormat::Bc7RgbaUnormSrgb
                                    }
                                    _ => todo!(),
                                },
                                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                            },
                            sampler_descriptor: Default::default(),
                            texture_view_descriptor: None,
                        });
                        texture_ids.push(egui_ctx.add_image(image_handle));
                    }
                };
                ui_state.tree.push_to_first_leaf(TabType::Texture(
                    asset_ref.clone(),
                    handle,
                    texture_ids,
                ));
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
struct OpenAsset(AssetRef, Option<HandleUntyped>);

fn ui_example(
    mut egui_ctx: ResMut<EguiContext>,
    mut ui_state: ResMut<UiState>,
    packages: Res<Assets<PackageDirectory>>,
    textures: Res<Assets<TextureAsset>>,
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

    let mut tab_assets = vec![];
    for node in ui_state.tree.iter() {
        if let egui_dock::Node::Leaf { tabs, .. } = node {
            for tab in tabs {
                match tab {
                    TabType::Directory => {}
                    TabType::Texture(asset_ref, _, _) => {
                        tab_assets.push(asset_ref.clone());
                    }
                    TabType::Empty => {}
                }
            }
        }
    }
    egui_dock::DockArea::new(&mut ui_state.tree)
        .style(egui_dock::Style::from_egui(egui_ctx.ctx_mut().style().as_ref()))
        .show(egui_ctx.ctx_mut(), &mut TabViewer {
            server,
            packages,
            textures,
            open_asset,
            tab_assets,
        });
}
