mod loaders;

use std::path::PathBuf;

use bevy::{
    asset::LoadState,
    ecs::system::{
        lifetimeless::{SRes, SResMut},
        SystemParam, SystemParamItem, SystemState,
    },
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
};
use bevy_egui::{
    egui,
    egui::{text::LayoutJob, Color32, FontId, TextFormat, Widget},
    EguiContext, EguiPlugin,
};
use egui::{TextureId, Ui, WidgetText};
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

#[derive(Default, Resource)]
struct FileOpen(Vec<PathBuf>);

fn main() {
    let mut file_open = FileOpen::default();
    for arg in std::env::args_os() {
        file_open.0.push(arg.into());
    }
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.0, 0.0, 0.0)))
        .insert_resource(Msaa { samples: 1 })
        .insert_resource(bevy::render::settings::WgpuSettings {
            features: bevy::render::settings::WgpuFeatures::TEXTURE_COMPRESSION_BC,
            ..Default::default()
        })
        .insert_resource(file_open)
        .init_resource::<UiState>()
        .init_resource::<Packages>()
        .add_plugins(DefaultPlugins.build().add_before::<AssetPlugin, _>(RetroAssetIoPlugin))
        .add_plugin(PackageAssetLoader)
        .add_plugin(TextureAssetLoader)
        .add_plugin(EguiPlugin)
        .add_system(file_drop)
        .add_system(load_files)
        .add_system(package_loader_system)
        .add_system(ui_system)
        .run();
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct AssetRef {
    id: Uuid,
    kind: FourCC,
}

enum TabType {
    Project(ProjectTab),
    Texture(TextureTab),
    Empty,
}

#[derive(Resource)]
struct UiState {
    tree: egui_dock::Tree<TabType>,
}

impl Default for UiState {
    fn default() -> Self {
        let mut tree = egui_dock::Tree::new(vec![TabType::Empty]);
        tree.split_left(egui_dock::NodeIndex::root(), 0.3, vec![TabType::Project(ProjectTab)]);
        Self { tree }
    }
}

struct TabViewer<'a> {
    world: &'a mut World,
    state: TabState,
}

struct TabState {
    open_assets: Vec<AssetRef>,
    open_tab: Option<TabType>,
}

trait SystemTab {
    type LoadParam: SystemParam;
    type UiParam: SystemParam;

    fn load(&mut self, _ctx: &mut EguiContext, _query: SystemParamItem<'_, '_, Self::LoadParam>) {}

    fn close(&mut self, _ctx: &mut EguiContext, _query: SystemParamItem<'_, '_, Self::LoadParam>) {}

    fn ui(
        &mut self,
        ui: &mut Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        state: &mut TabState,
    );

    fn title(&mut self) -> WidgetText;
}

struct ProjectTab;

impl SystemTab for ProjectTab {
    type LoadParam = ();
    type UiParam = (SRes<AssetServer>, SRes<Assets<PackageDirectory>>);

    fn ui(
        &mut self,
        ui: &mut Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        state: &mut TabState,
    ) {
        let (server, packages) = query;
        let mut packages_sorted =
            packages.iter().map(|(_, p)| p).collect::<Vec<&PackageDirectory>>();
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
                    if egui::SelectableLabel::new(state.open_assets.contains(&asset_ref), job)
                        .ui(ui)
                        .clicked()
                    {
                        #[allow(clippy::single_match)]
                        match entry.kind {
                            K_FORM_TXTR => {
                                let handle = server.load::<TextureAsset, _>(format!(
                                    "{}.{}",
                                    entry.id, entry.kind
                                ));
                                state.open_tab = Some(TabType::Texture(TextureTab {
                                    asset_ref: asset_ref.clone(),
                                    handle,
                                    loaded_texture: None,
                                }));
                            }
                            _ => {}
                        }
                    }
                }
            });
        }
    }

    fn title(&mut self) -> WidgetText { "Browser".into() }
}

struct LoadedTexture {
    texture_ids: Vec<TextureId>,
}

struct TextureTab {
    asset_ref: AssetRef,
    handle: Handle<TextureAsset>,
    loaded_texture: Option<LoadedTexture>,
}

impl SystemTab for TextureTab {
    type LoadParam = (SRes<Assets<TextureAsset>>, SResMut<Assets<Image>>);
    type UiParam = (SRes<AssetServer>, SRes<Assets<TextureAsset>>);

    fn load(&mut self, ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        if self.loaded_texture.is_some() {
            return;
        }

        let (textures, mut images) = query;
        let Some(txtr) = textures.get(&self.handle) else { return; };
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
            texture_ids.push(ctx.add_image(image_handle));
        } else {
            let array_stride: usize =
                (txtr.inner.head.mip_sizes.iter().sum::<u32>() / txtr.inner.head.layers) as usize;
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
                            ETextureFormat::Depth24S8Unorm => TextureFormat::Depth24PlusStencil8,
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
                            ETextureFormat::BptcUnormSrgb => TextureFormat::Bc7RgbaUnormSrgb,
                            _ => todo!(),
                        },
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    },
                    sampler_descriptor: Default::default(),
                    texture_view_descriptor: None,
                });
                texture_ids.push(ctx.add_image(image_handle));
            }
        };
        self.loaded_texture = Some(LoadedTexture { texture_ids });
    }

    fn ui(
        &mut self,
        ui: &mut Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
        _state: &mut TabState,
    ) {
        let (server, textures) = query;

        ui.label(format!("{} {}", self.asset_ref.kind, self.asset_ref.id));

        match server.get_load_state(&self.handle) {
            LoadState::NotLoaded => {
                return;
            }
            LoadState::Loading => {
                ui.spinner();
                return;
            }
            LoadState::Loaded => {}
            LoadState::Failed => {
                ui.colored_label(Color32::RED, "Loading failed");
                return;
            }
            LoadState::Unloaded => {
                return;
            }
        };

        let loaded = self.loaded_texture.as_mut().unwrap();
        if let Some(txtr) = textures.get(&self.handle) {
            ui.label(format!("Type: {:?}", txtr.inner.head.kind));
            ui.label(format!("Format: {:?}", txtr.inner.head.format));
            ui.label(format!(
                "Dimensions: {}x{}x{} (mips: {})",
                txtr.inner.head.width,
                txtr.inner.head.height,
                txtr.inner.head.layers,
                txtr.inner.head.mip_sizes.len()
            ));
            if txtr.inner.head.kind == ETextureType::Cube && loaded.texture_ids.len() == 6 {
                let width = txtr.inner.head.width;
                let height = txtr.inner.head.height;
                let (_, rect) =
                    ui.allocate_space(egui::Vec2 { x: (width * 4) as f32, y: (height * 3) as f32 });
                let size = egui::Vec2 { x: width as f32, y: height as f32 };
                let mut draw_image = |i: usize, x: u32, y: u32| {
                    let min = egui::Vec2 { x: (width * x) as f32, y: (height * y) as f32 };
                    let max =
                        egui::Vec2 { x: (width * (x + 1)) as f32, y: (height * (y + 1)) as f32 };
                    egui::widgets::Image::new(loaded.texture_ids[i], size)
                        .paint_at(ui, egui::Rect { min: rect.min + min, max: rect.min + max });
                };
                draw_image(2, 1, 0);
                draw_image(1, 0, 1);
                draw_image(4, 1, 1);
                draw_image(0, 2, 1);
                draw_image(5, 3, 1);
                draw_image(3, 1, 2);
            } else {
                for image in &loaded.texture_ids {
                    ui.add(egui::widgets::Image::new(*image, egui::Vec2 {
                        x: txtr.inner.head.width as f32,
                        y: txtr.inner.head.height as f32,
                    }));
                }
            }
        }
    }

    fn title(&mut self) -> WidgetText {
        format!("{} {}", self.asset_ref.kind, self.asset_ref.id).into()
    }
}

fn load_tab<T: SystemTab + 'static>(world: &mut World, ctx: &mut EguiContext, tab: &mut T) {
    let mut state: SystemState<T::LoadParam> = SystemState::new(world);
    tab.load(ctx, state.get_mut(world));
}

fn render_tab<T: SystemTab + 'static>(
    world: &mut World,
    ui: &mut Ui,
    tab: &mut T,
    tab_state: &mut TabState,
) {
    let mut state: SystemState<T::UiParam> = SystemState::new(world);
    tab.ui(ui, state.get_mut(world), tab_state);
}

impl egui_dock::TabViewer for TabViewer<'_> {
    type Tab = TabType;

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            TabType::Project(tab) => render_tab(self.world, ui, tab, &mut self.state),
            TabType::Texture(tab) => render_tab(self.world, ui, tab, &mut self.state),
            TabType::Empty => {}
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        match tab {
            TabType::Project(tab) => tab.title(),
            TabType::Texture(tab) => tab.title(),
            TabType::Empty => "Placeholder".into(),
        }
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> bool {
        match tab {
            TabType::Project(_) => false,
            TabType::Texture(_) => true,
            TabType::Empty => false,
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
        if let FileDragAndDrop::DroppedFile { id: _, path_buf } = ev {
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
    world.resource_scope::<EguiContext, _>(|world, mut egui_ctx| {
        egui::TopBottomPanel::top("top_panel").show(egui_ctx.ctx_mut(), |ui| {
            egui::menu::bar(ui, |ui| {
                egui::menu::menu_button(ui, "File", |ui| {
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
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
                                load_tab(world, egui_ctx.as_mut(), tab);
                            }
                            TabType::Texture(tab) => {
                                load_tab(world, egui_ctx.as_mut(), tab);
                                tab_assets.push(tab.asset_ref.clone());
                            }
                            TabType::Empty => {}
                        }
                    }
                }
            }

            let mut viewer =
                TabViewer { world, state: TabState { open_assets: tab_assets, open_tab: None } };
            egui_dock::DockArea::new(&mut ui_state.tree)
                .style(egui_dock::Style::from_egui(egui_ctx.ctx_mut().style().as_ref()))
                .show(egui_ctx.ctx_mut(), &mut viewer);

            if let Some(tab) = viewer.state.open_tab {
                ui_state.tree.push_to_first_leaf(tab);
            }
        });
    });
}
