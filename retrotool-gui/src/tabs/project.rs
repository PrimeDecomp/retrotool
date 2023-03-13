use bevy::{
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use egui::{text::LayoutJob, Color32, TextFormat, Widget};
use retrolib::format::{
    cmdl::{K_FORM_CMDL, K_FORM_SMDL, K_FORM_WMDL},
    txtr::K_FORM_TXTR,
    FourCC,
};

use crate::{
    icon,
    loaders::{ModelAsset, PackageDirectory, TextureAsset},
    tabs::{model::ModelTab, texture::TextureTab, SystemTab, TabState, TabType},
    AssetRef,
};

pub const K_FORM_FMV0: FourCC = FourCC(*b"FMV0");
pub const K_FORM_ROOM: FourCC = FourCC(*b"ROOM");

#[derive(Default)]
pub struct ProjectTab {
    search: String,
}

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

        let mut set_open = None;
        ui.horizontal(|ui| {
            if ui.button("Expand all").clicked() {
                set_open = Some(true);
            }
            if ui.button("Collapse all").clicked() {
                set_open = Some(false);
            }
        });
        egui::TextEdit::singleline(&mut self.search).hint_text("Search").ui(ui);

        let mut packages_sorted =
            packages.iter().map(|(_, p)| p).collect::<Vec<&PackageDirectory>>();
        packages_sorted.sort_by_key(|p| &p.name);
        for package in packages_sorted {
            let search = self.search.to_ascii_lowercase();
            let search = search.trim_start_matches('{').trim_end_matches('}');
            let mut iter = package
                .entries
                .iter()
                .filter(|e| {
                    search.is_empty()
                        || (search.as_bytes().len() == 4
                            && e.kind.0.eq_ignore_ascii_case(search.as_bytes()))
                        || matches!(&e.name, Some(v) if v.contains(search))
                        || e.id.to_string().contains(search)
                })
                .peekable();
            if iter.peek().is_none() {
                continue;
            }
            egui::CollapsingHeader::new(&package.name).open(set_open).show(ui, |ui| {
                for entry in iter {
                    let monospace =
                        ui.style().text_styles.get(&egui::TextStyle::Monospace).unwrap().clone();
                    let mut job = LayoutJob::simple(
                        format!(
                            "{} {} {}",
                            match entry.kind {
                                K_FORM_TXTR => icon::TEXTURE,
                                K_FORM_CMDL | K_FORM_SMDL | K_FORM_WMDL => icon::FILE_3D,
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
                        .context_menu(|ui| {
                            if ui.button(format!("Copy \"{}\"", entry.id)).clicked() {
                                ui.output_mut(|out| out.copied_text = format!("{}", entry.id));
                                ui.close_menu();
                            }
                        })
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
                            K_FORM_CMDL | K_FORM_SMDL | K_FORM_WMDL => {
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

    fn id(&self) -> String { "project".to_string() }
}
