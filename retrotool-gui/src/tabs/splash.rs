use bevy::{
    ecs::system::{lifetimeless::*, SystemParamItem},
    prelude::*,
};
use bevy_egui::EguiUserTextures;
use egui::Widget;

use crate::{
    icon,
    tabs::{texture::UiTexture, EditorTabSystem, TabState},
};

#[derive(Default)]
pub struct SplashTab {
    pub icon: Option<UiTexture>,
    pub icon_image: Option<Handle<Image>>,
}

impl EditorTabSystem for SplashTab {
    type LoadParam = (SRes<AssetServer>, SResMut<Assets<Image>>, SResMut<EguiUserTextures>);
    type UiParam = ();

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        if self.icon.is_some() {
            return;
        }

        let (asset_server, mut images, mut egui_textures) = query;
        if self.icon_image.is_none() {
            self.icon_image = Some(asset_server.load("icon.png"));
        }

        if let Some(icon_image) = self.icon_image.as_ref() {
            if images.contains(icon_image) {
                let handle = std::mem::take(&mut self.icon_image).unwrap();
                self.icon = UiTexture::from_handle(handle, &mut images, &mut egui_textures);
            }
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _query: SystemParamItem<Self::UiParam>,
        _state: &mut TabState,
    ) {
        let icon = match &self.icon {
            Some(icon) => icon,
            None => {
                ui.centered_and_justified(|ui| {
                    egui::Spinner::new().size(50.0).ui(ui);
                });
                return;
            }
        };

        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::new(0.0, 10.0);
            ui.add_space(10.0);
            icon.image_scaled(100.0).ui(ui);
            ui.heading("retrotool");
            ui.hyperlink_to(
                format!("{} GitHub", egui::special_emojis::GITHUB),
                "https://github.com/PrimeDecomp/retrotool",
            );
            ui.add_space(10.0);
            ui.label("Drag and drop a directory to load all .pak files.");
        });
    }

    fn title(&self) -> egui::WidgetText { format!("{} Splash", icon::HOME).into() }

    fn id(&self) -> String { "splash".into() }
}
