use bevy::{
    asset::LoadState,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use bevy_egui::{EguiContext, EguiUserTextures};
use retrolib::format::txtr::ETextureType;

use crate::{icon, loaders::texture::TextureAsset, tabs::SystemTab, AssetRef, TabState};

pub struct LoadedTexture {
    pub texture_ids: Vec<egui::TextureId>,
}

pub struct TextureTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<TextureAsset>,
    pub loaded_texture: Option<LoadedTexture>,
    pub v_flip: bool,
}

impl SystemTab for TextureTab {
    type LoadParam =
        (SRes<Assets<TextureAsset>>, SResMut<Assets<Image>>, SResMut<EguiUserTextures>);
    type UiParam = (SRes<AssetServer>, SRes<Assets<TextureAsset>>);

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        if self.loaded_texture.is_some() {
            return;
        }

        let (textures, mut images, mut egui_textures) = query;
        let Some(asset) = textures.get(&self.handle) else { return; };
        let mut texture_ids = Vec::new();
        if let Some(first_mip) = asset.slices.first() {
            for image in first_mip {
                let handle = images.add(image.clone());
                texture_ids.push(egui_textures.add_image(handle));
            }
        }
        self.loaded_texture = Some(LoadedTexture { texture_ids });
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
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
                ui.colored_label(egui::Color32::RED, "Loading failed");
                return;
            }
            LoadState::Unloaded => {
                return;
            }
        };

        let loaded = self.loaded_texture.as_mut().unwrap();
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
            let w = txtr.inner.head.width;
            let h = txtr.inner.head.height;
            let size = egui::Vec2 { x: w as f32, y: h as f32 };
            let draw_image = |ui: &mut egui::Ui, rect: &egui::Rect, i: usize, x: u32, y: u32, flip: &bool| {
                let min = egui::Vec2 { x: (w * x) as f32, y: (h * y) as f32 };
                let max = egui::Vec2 { x: (w * (x + 1)) as f32, y: (h * (y + 1)) as f32 };
                let y_range = if *flip {1.0..=0.0} else {0.0..=1.0};
                egui::widgets::Image::new(loaded.texture_ids[i], size)
                    .uv(egui::Rect::from_x_y_ranges(0.0..=1.0, y_range))
                    .paint_at(ui, egui::Rect { min: rect.min + min, max: rect.min + max });
            };
            if txtr.inner.head.kind == ETextureType::Cube && loaded.texture_ids.len() == 6 {
                let (_, rect) =
                    ui.allocate_space(egui::Vec2 { x: (w * 4) as f32, y: (h * 3) as f32 });
                draw_image(ui, &rect, 2, 1, 0, &self.v_flip);
                draw_image(ui, &rect, 1, 0, 1, &self.v_flip);
                draw_image(ui, &rect, 4, 1, 1, &self.v_flip);
                draw_image(ui, &rect, 0, 2, 1, &self.v_flip);
                draw_image(ui, &rect, 5, 3, 1, &self.v_flip);
                draw_image(ui, &rect, 3, 1, 2, &self.v_flip);
            } else {
                let (_, rect) = ui.allocate_space(egui::Vec2 {
                    x: (w as usize * loaded.texture_ids.len()) as f32,
                    y: h as f32,
                });
                for i in 0..loaded.texture_ids.len() {
                    draw_image(ui, &rect, i, i as u32, 0, &self.v_flip);
                }
            }
        }
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::TEXTURE, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }
}
