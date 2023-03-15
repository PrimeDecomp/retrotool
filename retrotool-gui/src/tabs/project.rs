use bevy::{
    asset::LoadState,
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::render_resource::Extent3d,
};
use bevy_egui::{EguiContext, EguiUserTextures};
use egui::{text::LayoutJob, Color32, TextFormat, Widget};
use retrolib::format::{
    cmdl::{K_FORM_CMDL, K_FORM_SMDL, K_FORM_WMDL},
    txtr::{ETextureFormat, ETextureType, K_FORM_TXTR},
    FourCC,
};

use crate::{
    icon,
    loaders::{model::ModelAsset, package::PackageDirectory, texture::TextureAsset},
    tabs::{model::ModelTab, texture::TextureTab, SystemTab, TabState, TabType},
    AssetRef,
};

pub const K_FORM_FMV0: FourCC = FourCC(*b"FMV0");
pub const K_FORM_ROOM: FourCC = FourCC(*b"ROOM");

#[derive(Default)]
enum HoverState {
    #[default]
    None,
    Loading {
        asset: AssetRef,
        handle: HandleUntyped,
    },
    Texture {
        _handle: Handle<TextureAsset>,
        _image: Handle<Image>,
        size: Extent3d,
        texture_id: egui::TextureId,
        kind: ETextureType,
        format: ETextureFormat,
    },
}

#[derive(Default)]
pub struct ProjectTab {
    search: String,
    hover_asset: Option<AssetRef>,
    hover_state: HoverState,
}

const THUMBNAIL_SIZE: f32 = 250.0;

impl ProjectTab {
    fn hover_ui(&mut self, ui: &mut egui::Ui, asset_ref: &AssetRef, server: &AssetServer) {
        if matches!(&self.hover_asset, Some(aref) if aref == asset_ref) {
            match &self.hover_state {
                HoverState::None => {}
                HoverState::Loading { .. } => {
                    ui.spinner();
                }
                HoverState::Texture { size, texture_id, kind, format, .. } => {
                    ui.label(format!("Type: {kind}"));
                    ui.label(format!("Format: {format}"));
                    ui.label(format!(
                        "Size: {}x{}x{}",
                        size.width, size.height, size.depth_or_array_layers
                    ));
                    let size = if size.height > size.width {
                        let ratio = THUMBNAIL_SIZE / size.height as f32;
                        egui::Vec2::new(size.width as f32 * ratio, THUMBNAIL_SIZE)
                    } else {
                        let ratio = THUMBNAIL_SIZE / size.width as f32;
                        egui::Vec2::new(THUMBNAIL_SIZE, size.height as f32 * ratio)
                    };
                    ui.image(*texture_id, size);
                }
            }
        } else {
            self.hover_asset = Some(*asset_ref);
            self.hover_state = HoverState::Loading {
                asset: *asset_ref,
                handle: server
                    .load::<TextureAsset, _>(format!("{}.{}", asset_ref.id, asset_ref.kind))
                    .into(),
            };
        }
    }
}

impl SystemTab for ProjectTab {
    type LoadParam = (
        SRes<AssetServer>,
        SRes<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<EguiUserTextures>,
    );
    type UiParam = (SRes<AssetServer>, SRes<Assets<PackageDirectory>>);

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (server, textures, mut images, mut egui_textures) = query;
        if let HoverState::Loading { asset, handle } = &self.hover_state {
            if asset.kind != K_FORM_TXTR {
                return;
            }
            if server.get_load_state(handle) == LoadState::Loaded {
                let texture_handle = handle.clone().typed::<TextureAsset>();
                let asset = textures.get(&texture_handle).unwrap();
                if let Some(image) = asset.slices.first().and_then(|v| v.first()) {
                    let image_handle = images.add(image.clone());
                    let texture_id = egui_textures.add_image(image_handle.clone_weak());
                    self.hover_state = HoverState::Texture {
                        _handle: texture_handle,
                        _image: image_handle,
                        size: image.texture_descriptor.size,
                        texture_id,
                        kind: asset.inner.head.kind,
                        format: asset.inner.head.format,
                    };
                } else {
                    self.hover_state = HoverState::None;
                }
            }
        }
    }

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
                        || matches!(&e.name, Some(v) if v.to_ascii_lowercase().contains(search))
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
                    let mut response =
                        egui::SelectableLabel::new(state.open_assets.contains(&asset_ref), job)
                            .ui(ui)
                            .context_menu(|ui| {
                                if ui.button(format!("Copy \"{}\"", entry.id)).clicked() {
                                    ui.output_mut(|out| out.copied_text = format!("{}", entry.id));
                                    ui.close_menu();
                                }
                            });
                    if entry.kind == K_FORM_TXTR {
                        response = response.on_hover_ui_at_pointer(|ui| {
                            self.hover_ui(ui, &asset_ref, &server);
                        });
                    }
                    if response.clicked() {
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
