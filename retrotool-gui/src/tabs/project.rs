use bevy::{
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use egui::{text::LayoutJob, Color32, TextFormat, Widget};
use retrolib::format::{cmdl::K_FORM_CMDL, txtr::K_FORM_TXTR, FourCC};

use crate::{
    icon,
    loaders::{ModelAsset, PackageDirectory, TextureAsset},
    tabs::{model::ModelTab, texture::TextureTab, SystemTab, TabState, TabType},
    AssetRef,
};

pub const K_FORM_FMV0: FourCC = FourCC(*b"FMV0");
pub const K_FORM_ROOM: FourCC = FourCC(*b"ROOM");

pub struct ProjectTab;

impl SystemTab for ProjectTab {
    type LoadParam = ();
    type UiParam = (SRes<AssetServer>, SRes<Assets<PackageDirectory>>);

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
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
                    let monospace =
                        ui.style().text_styles.get(&egui::TextStyle::Monospace).unwrap().clone();
                    let mut job = LayoutJob::simple(
                        format!(
                            "{} {} {}",
                            match entry.kind {
                                K_FORM_TXTR => icon::TEXTURE,
                                K_FORM_CMDL => icon::FILE_3D,
                                K_FORM_FMV0 => icon::FILE_MOVIE,
                                K_FORM_ROOM => icon::SCENE_DATA,
                                _ => icon::FILE,
                            },
                            entry.kind,
                            entry.id
                        ),
                        monospace.clone(),
                        Color32::GRAY,
                        0.0,
                    );
                    if let Some(name) = &entry.name {
                        job.append(
                            &format!("\n{name}"),
                            0.0,
                            TextFormat::simple(monospace, Color32::WHITE),
                        );
                    }
                    let asset_ref = AssetRef { id: entry.id, kind: entry.kind };
                    if egui::SelectableLabel::new(state.open_assets.contains(&asset_ref), job)
                        .ui(ui)
                        .clicked()
                    {
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
                            K_FORM_CMDL => {
                                let handle = server
                                    .load::<ModelAsset, _>(format!("{}.{}", entry.id, entry.kind));
                                state.open_tab = Some(TabType::Model(ModelTab {
                                    asset_ref: asset_ref.clone(),
                                    handle,
                                    loaded: None,
                                }));
                            }
                            _ => {}
                        }
                    }
                }
            });
        }
    }

    fn title(&mut self) -> egui::WidgetText { format!("{} Browser", icon::FILEBROWSER).into() }
}
