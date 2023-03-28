use bevy::{
    asset::LoadState,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use bevy_egui::EguiUserTextures;
use retrolib::format::txtr::ETextureType;

use crate::{
    icon,
    loaders::lightprobe::LightProbeAsset,
    tabs::{texture::LoadedTexture, EditorTabSystem, TabState},
    AssetRef,
};

#[derive(Default)]
pub struct LightProbeTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<LightProbeAsset>,
    pub loaded_textures: Vec<Vec<LoadedTexture>>,
}

impl LightProbeTab {
    pub fn new(asset_ref: AssetRef, handle: Handle<LightProbeAsset>) -> Box<Self> {
        Box::new(Self { asset_ref, handle, ..default() })
    }
}

impl EditorTabSystem for LightProbeTab {
    type LoadParam =
        (SRes<Assets<LightProbeAsset>>, SResMut<Assets<Image>>, SResMut<EguiUserTextures>);
    type UiParam = (SRes<AssetServer>, SRes<Assets<LightProbeAsset>>);

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        if !self.loaded_textures.is_empty() {
            return;
        }

        let (assets, images, mut egui_textures) = query;
        let Some(asset) = assets.get(&self.handle) else { return; };

        self.loaded_textures.reserve_exact(asset.textures.len());
        for texture in &asset.textures {
            let mut slices = Vec::with_capacity(texture.slices.len());
            for mip in &texture.slices {
                let mut texture_ids = Vec::with_capacity(mip.len());
                for image in mip {
                    texture_ids.push(egui_textures.add_image(image.clone_weak()));
                }
                let size = mip
                    .first()
                    .and_then(|h| images.get(h))
                    .map(|m| m.texture_descriptor.size)
                    .unwrap_or_default();
                slices.push(LoadedTexture { texture_ids, width: size.width, height: size.height });
            }
            self.loaded_textures.push(slices);
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<Self::UiParam>,
        _state: &mut TabState,
    ) {
        let (server, assets) = query;

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

        let Some(asset) = assets.get(&self.handle) else { return; };

        for (txtr_idx, txtr) in asset.textures.iter().enumerate() {
            ui.group(|ui| {
                ui.label(format!("Type: {}", txtr.inner.head.kind));
                ui.label(format!("Format: {}", txtr.inner.head.format));
                ui.label(format!(
                    "Size: {}x{}x{} (mips: {})",
                    txtr.inner.head.width,
                    txtr.inner.head.height,
                    txtr.inner.head.layers,
                    txtr.inner.head.mip_sizes.len()
                ));

                let mip = &self.loaded_textures[txtr_idx][0];
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
                    draw_image(ui, &rect, 2, 1, 0, false);
                    draw_image(ui, &rect, 1, 0, 1, false);
                    draw_image(ui, &rect, 4, 1, 1, false);
                    draw_image(ui, &rect, 0, 2, 1, false);
                    draw_image(ui, &rect, 5, 3, 1, false);
                    draw_image(ui, &rect, 3, 1, 2, false);
                } else {
                    let (_, rect) = ui
                        .allocate_space(size * egui::Vec2::new(mip.texture_ids.len() as f32, 1.0));
                    for i in 0..mip.texture_ids.len() {
                        draw_image(ui, &rect, i, i as u32, 0, false);
                    }
                }
            });
        }
    }

    fn title(&self) -> egui::WidgetText {
        format!("{} {} {}", icon::LIGHTPROBE_CUBEMAP, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }

    fn asset(&self) -> Option<AssetRef> { Some(self.asset_ref) }
}
