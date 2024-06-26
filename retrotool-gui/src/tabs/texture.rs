use bevy::{
    asset::LoadState,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use bevy_egui::EguiUserTextures;
use egui::Widget;
use retrolib::format::txtr::ETextureType;

use crate::{icon, loaders::texture::TextureAsset, tabs::EditorTabSystem, AssetRef, TabState};

pub struct LoadedTexture {
    pub width: u32,
    pub height: u32,
    pub texture_ids: Vec<egui::TextureId>,
}

#[derive(Default)]
pub struct TextureTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<TextureAsset>,
    pub loaded_textures: Vec<LoadedTexture>,
    pub selected_mip: usize,
    pub v_flip: bool,
}

impl TextureTab {
    pub fn new(asset_ref: AssetRef, handle: Handle<TextureAsset>) -> Box<Self> {
        Box::new(Self { asset_ref, handle, ..default() })
    }
}

pub struct UiTexture {
    _image: Handle<Image>,
    texture_id: egui::TextureId,
    width: u32,
    height: u32,
}

impl UiTexture {
    #[allow(dead_code)]
    pub fn new(image: Image, images: &mut Assets<Image>, textures: &mut EguiUserTextures) -> Self {
        let width = image.texture_descriptor.size.width;
        let height = image.texture_descriptor.size.height;
        let handle = images.add(image);
        let weak_handle = handle.clone_weak();
        Self { _image: handle, texture_id: textures.add_image(weak_handle), width, height }
    }

    pub fn from_handle(
        handle: Handle<Image>,
        images: &mut Assets<Image>,
        textures: &mut EguiUserTextures,
    ) -> Option<Self> {
        let Some(image) = images.get(&handle) else {
            return None;
        };
        let width = image.texture_descriptor.size.width;
        let height = image.texture_descriptor.size.height;
        let weak_handle = handle.clone_weak();
        Some(Self { _image: handle, texture_id: textures.add_image(weak_handle), width, height })
    }

    #[allow(dead_code)]
    pub fn image(&self) -> egui::Image {
        egui::Image::new(self.texture_id, egui::Vec2::new(self.width as f32, self.height as f32))
    }

    pub fn image_scaled(&self, max_size: f32) -> egui::Image {
        let size = if self.height > self.width {
            let ratio = max_size / self.height as f32;
            egui::Vec2::new(self.width as f32 * ratio, max_size)
        } else {
            let ratio = max_size / self.width as f32;
            egui::Vec2::new(max_size, self.height as f32 * ratio)
        };
        egui::Image::new(self.texture_id, size)
    }
}

impl EditorTabSystem for TextureTab {
    type LoadParam =
        (SRes<Assets<TextureAsset>>, SResMut<Assets<Image>>, SResMut<EguiUserTextures>);
    type UiParam = (SRes<AssetServer>, SRes<Assets<TextureAsset>>);

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        if !self.loaded_textures.is_empty() {
            return;
        }

        let (textures, images, mut egui_textures) = query;
        let Some(asset) = textures.get(&self.handle) else {
            return;
        };
        self.loaded_textures.reserve_exact(asset.slices.len());
        for mip in &asset.slices {
            let mut texture_ids = Vec::with_capacity(mip.len());
            for image in mip {
                texture_ids.push(egui_textures.add_image(image.clone_weak()));
            }
            let size = mip
                .first()
                .and_then(|h| images.get(h))
                .map(|m| m.texture_descriptor.size)
                .unwrap_or_default();
            self.loaded_textures.push(LoadedTexture {
                texture_ids,
                width: size.width,
                height: size.height,
            });
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<Self::UiParam>,
        _state: &mut TabState,
    ) {
        let (server, textures) = query;

        ui.label(format!("{} {}", self.asset_ref.kind, self.asset_ref.id));

        match server.get_load_state(&self.handle) {
            LoadState::NotLoaded | LoadState::Loading => {
                ui.spinner();
                return;
            }
            LoadState::Loaded => {}
            LoadState::Failed => {
                ui.colored_label(egui::Color32::RED, "Loading failed");
                return;
            }
            LoadState::Unloaded => {
                ui.colored_label(egui::Color32::RED, "Unloaded");
                return;
            }
        };

        if let Some(txtr) = textures.get(&self.handle) {
            ui.label(format!("Type: {}", txtr.inner.head.kind));
            ui.label(format!("Format: {}", txtr.inner.head.format));
            ui.label(format!(
                "Size: {}x{}x{} (mips: {})",
                txtr.inner.head.width,
                txtr.inner.head.height,
                txtr.inner.head.layers,
                txtr.inner.head.mip_sizes.len()
            ));
            ui.checkbox(&mut self.v_flip, "Flip texture vertically");
            if self.loaded_textures.len() > 1 {
                egui::Slider::new(&mut self.selected_mip, 0..=self.loaded_textures.len() - 1)
                    .text("Mipmap")
                    .ui(ui);
            }

            let mip = &self.loaded_textures[self.selected_mip];
            if self.loaded_textures.len() > 1 {
                ui.label(format!(
                    "Mipmap size: {}x{}x{}",
                    mip.width,
                    mip.height,
                    mip.texture_ids.len(),
                ));
            }
            let size = egui::Vec2::new(mip.width as f32, mip.height as f32);
            let draw_image =
                |ui: &mut egui::Ui, rect: &egui::Rect, i: usize, x: u32, y: u32, flip: bool| {
                    let min = rect.min + size * egui::Vec2::new(x as f32, y as f32);
                    let y_range = if flip { 1.0..=0.0 } else { 0.0..=1.0 };
                    egui::widgets::Image::new(mip.texture_ids[i], size)
                        .uv(egui::Rect::from_x_y_ranges(0.0..=1.0, y_range))
                        .paint_at(ui, egui::Rect::from_min_size(min, size));
                };
            if txtr.inner.head.kind == ETextureType::Cube && mip.texture_ids.len() == 6 {
                let (_, rect) = ui.allocate_space(size * egui::Vec2::new(4.0, 3.0));
                draw_image(ui, &rect, 2, 1, 0, self.v_flip);
                draw_image(ui, &rect, 1, 0, 1, self.v_flip);
                draw_image(ui, &rect, 4, 1, 1, self.v_flip);
                draw_image(ui, &rect, 0, 2, 1, self.v_flip);
                draw_image(ui, &rect, 5, 3, 1, self.v_flip);
                draw_image(ui, &rect, 3, 1, 2, self.v_flip);
            } else {
                let (_, rect) =
                    ui.allocate_space(size * egui::Vec2::new(mip.texture_ids.len() as f32, 1.0));
                for i in 0..mip.texture_ids.len() {
                    draw_image(ui, &rect, i, i as u32, 0, self.v_flip);
                }
            }
        }
    }

    fn title(&self) -> egui::WidgetText {
        format!("{} {} {}", icon::TEXTURE, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }

    fn asset(&self) -> Option<AssetRef> { Some(self.asset_ref) }
}
