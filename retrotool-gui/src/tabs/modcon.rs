use bevy::{
    asset::LoadState,
    camera::{visibility::RenderLayers, Viewport},
    ecs::system::{lifetimeless::*, *},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
};
use egui::{Sense, Widget};
use retrolib::format::SumBy;
use uuid::Uuid;

use crate::{
    icon,
    loaders::{modcon::ModConAsset, model::ModelAsset},
    render::{
        camera::ModelCamera,
        scene::{container_models, model_bounds, ModelGroup, ModelLabel, SceneModelLoader},
        scene_preview::{PreviewMesh, ShaderPreview},
    },
    tabs::{model::ModelTab, EditorTabSystem, TabState},
    AssetRef,
};

pub struct ModConTab {
    pub tab_id: Uuid,
    pub asset_ref: AssetRef,
    pub handle: Handle<ModConAsset>,
    pub models: Vec<ModelGroup>,
    pub camera: ModelCamera,
    pub diffuse_map: Handle<Image>,
    pub specular_map: Handle<Image>,
    pub env_light: bool,
    pub selected_model: Option<AssetRef>,
}

impl Default for ModConTab {
    fn default() -> Self {
        Self {
            tab_id: Uuid::new_v4(),
            asset_ref: default(),
            handle: default(),
            models: default(),
            camera: default(),
            diffuse_map: default(),
            specular_map: default(),
            env_light: true,
            selected_model: None,
        }
    }
}

impl ModConTab {
    pub fn new(asset_ref: AssetRef, handle: Handle<ModConAsset>) -> Box<Self> {
        Box::new(Self { asset_ref, handle, ..default() })
    }

    fn get_load_state(
        &self,
        server: &AssetServer,
        assets: &Assets<ModConAsset>,
        models: &Assets<ModelAsset>,
    ) -> LoadState {
        match server.load_state(&self.handle) {
            LoadState::Loaded => {}
            state => return state,
        };
        let asset = match assets.get(&self.handle) {
            Some(v) => v,
            None => return LoadState::NotLoaded,
        };
        // Ensure all dependencies loaded
        for model in asset.models.iter().chain(asset.world_models.values()) {
            let state = server.load_state(model.id());
            if !state.is_loaded() {
                return state;
            }
        }
        for model in asset.models.iter().chain(asset.world_models.values()) {
            let model = models.get(model).unwrap();
            match model.get_load_state(server) {
                LoadState::Loaded => {}
                state => return state,
            }
        }
        LoadState::Loaded
    }
}

impl EditorTabSystem for ModConTab {
    type LoadParam = (SceneModelLoader<'static, 'static>, SRes<Assets<ModConAsset>>);
    type UiParam = (
        SCommands,
        SRes<AssetServer>,
        SRes<Assets<ModelAsset>>,
        SRes<Assets<ModConAsset>>,
        MeshRayCast<'static, 'static>,
        SQuery<Read<ChildOf>>,
        SQuery<(Read<ModelLabel>, Read<Children>)>,
        SRes<ShaderPreview>,
        SQuery<Read<PreviewMesh>>,
    );

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        let (mut loader, containers) = query;
        if self.models.is_empty() {
            if let Some(container) = containers.get(&self.handle) {
                match container_models(container) {
                    Ok(models) => self.models = models,
                    Err(error) => log::error!("Invalid MCON: {error:#}"),
                }
            }
        }
        if loader.load(&mut self.models, self.tab_id, None) {
            if let Some(bounds) = model_bounds(&self.models) {
                self.camera.init(&bounds, true);
            }
        }
        if self.diffuse_map == Handle::default() {
            self.diffuse_map = loader.server.load("papermill_diffuse_rgb9e5_zstd.ktx2");
            self.specular_map = loader.server.load("papermill_specular_rgb9e5_zstd.ktx2");
        }
    }

    fn close(&mut self, query: SystemParamItem<Self::LoadParam>) -> bool {
        let (mut loader, _) = query;
        self.camera.close(&mut loader.commands);
        loader.close(&self.models);
        true
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<Self::UiParam>,
        state: &mut TabState,
    ) {
        let scale = ui.ctx().pixels_per_point();
        let rect = ui.available_rect_before_wrap();
        let left_top = rect.left_top().to_vec2() * scale;
        let size = rect.size() * scale;
        let viewport = Viewport {
            physical_position: UVec2 { x: left_top.x as u32, y: left_top.y as u32 },
            physical_size: UVec2 { x: size.x as u32, y: size.y as u32 },
            depth: 0.0..1.0,
        };
        let response =
            ui.interact(rect, ui.make_persistent_id("background"), Sense::click_and_drag());
        self.camera.update(&rect, &response, ui.input(|i| i.smooth_scroll_delta));

        let (
            mut commands,
            server,
            models,
            mod_con_assets,
            mut raycast,
            parents,
            model_query,
            preview,
            preview_meshes,
        ) = query;
        if let Some(error) = self.models.iter().find_map(|model| model.error.as_ref()) {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
            return;
        }
        if self.models.is_empty() || !self.models.iter().all(|m| !m.loaded.is_empty()) {
            ui.centered_and_justified(|ui| {
                match self.get_load_state(&server, &mod_con_assets, &models) {
                    LoadState::Failed(_) => egui::Label::new(
                        egui::RichText::from("Loading failed").heading().color(egui::Color32::RED),
                    )
                    .ui(ui),
                    _ => egui::Spinner::new().size(50.0).ui(ui),
                };
            });
            return;
        }

        if response.hovered() {
            self.selected_model = None;
            if let Some(ray) = ui
                .input(|i| i.pointer.hover_pos())
                .and_then(|pos| self.camera.pointer_ray(&rect, pos))
            {
                let filter = |entity| {
                    parents
                        .get(entity)
                        .ok()
                        .and_then(|parent| model_query.get(parent.parent()).ok())
                        .is_some_and(|(label, _)| label.tab_id == self.tab_id)
                };
                let settings = MeshRayCastSettings::default().with_filter(&filter);
                if let Some((entity, _)) = raycast.cast_ray(ray, &settings).first() {
                    if let Ok(parent) = parents.get(*entity) {
                        if let Ok((label, _)) = model_query.get(parent.parent()) {
                            self.selected_model = Some(label.asset_ref);
                        }
                    }
                }
            }
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(rect.height() * 0.25).show(ui, |ui| {
                ui.small(
                    preview.description(
                        self.models
                            .iter()
                            .flat_map(|m| &m.loaded)
                            .filter(|m| m.visible)
                            .filter_map(|m| model_query.get(m.entity).ok())
                            .flat_map(|(_, children)| children.iter())
                            .map(|e| preview_meshes.get(e).ok()),
                    ),
                );
                ui.checkbox(&mut self.env_light, "Environment lighting for preview materials");
                ui.label(format!("Models: {}", self.models.len()));
                ui.label(format!("Instances: {}", self.models.sum_by(|m| m.loaded.len())));
                if let Some(decals) = mod_con_assets
                    .get(&self.handle)
                    .and_then(|asset| asset.inner.decal_data.as_ref())
                {
                    if !decals.decals.is_empty() {
                        ui.label(format!("Decals: {} (not rendered)", decals.decals.len()));
                    }
                }
                if let Some(selected) = &self.selected_model {
                    ui.label(format!("Hovering: {}", selected.id));
                }
            });
        });

        if let Some(selected) = &self.selected_model {
            let mut shown = false;
            response.context_menu(|ui| {
                if ui.button("Open in new tab").clicked() {
                    let handle = server.load(state.asset_path(*selected));
                    state.open_tab(ModelTab::new(*selected, handle));
                    ui.close();
                }
                if ui.button("Copy GUID").clicked() {
                    ui.ctx().copy_text(selected.id.to_string());
                    ui.close();
                }
                shown = true;
            });
            if !shown {
                self.selected_model = None;
            }
        }

        let environment = self.env_light.then(|| EnvironmentMapLight {
            diffuse_map: self.diffuse_map.clone(),
            specular_map: self.specular_map.clone(),
            intensity: 1.0,
            ..default()
        });
        self.camera.show(&mut commands, viewport, state.render_layer, environment);

        for info in &self.models {
            for model in &info.loaded {
                if let Ok(mut commands) = commands.get_entity(model.entity) {
                    commands.insert(if model.visible {
                        Visibility::Visible
                    } else {
                        Visibility::Hidden
                    });
                }
                if let Ok((_, children)) = model_query.get(model.entity) {
                    for child in children.iter() {
                        if let Ok(mut commands) = commands.get_entity(child) {
                            commands.insert(RenderLayers::layer(state.render_layer));
                        }
                    }
                }
            }
        }

        state.render_layer += 1;
    }

    fn title(&self) -> egui::WidgetText {
        format!("{} {} {}", icon::SCENE_DATA, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }

    fn clear_background(&self) -> bool { false }

    fn asset(&self) -> Option<AssetRef> { Some(self.asset_ref) }
}
