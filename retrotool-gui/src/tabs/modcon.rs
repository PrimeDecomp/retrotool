use bevy::{
    asset::LoadState,
    core_pipeline::{clear_color::ClearColorConfig, tonemapping::Tonemapping},
    ecs::system::{lifetimeless::*, *},
    math::Vec3A,
    prelude::*,
    render::{camera::Viewport, primitives::Aabb, view::RenderLayers},
};
use bevy_egui::EguiContext;
use egui::{Sense, Widget};

use crate::{
    icon,
    loaders::{modcon::ModConAsset, model::ModelAsset, texture::TextureAsset},
    material::CustomMaterial,
    render::{
        camera::ModelCamera,
        model::{convert_transform, load_model},
        TemporaryLabel,
    },
    tabs::{SystemTab, TabState},
    AssetRef,
};

pub struct LoadedModel {
    pub entity: Entity,
    pub visible: bool,
}

pub struct ModelInfo {
    pub handle: Handle<ModelAsset>,
    pub loaded: Vec<LoadedModel>,
    pub transforms: Vec<Transform>,
    pub aabb: Aabb,
}

#[derive(Default)]
pub struct ModConTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<ModConAsset>,
    pub models: Vec<ModelInfo>,
    pub camera: ModelCamera,
    pub diffuse_map: Handle<Image>,
    pub specular_map: Handle<Image>,
    pub combined_aabb: Aabb,
}

impl ModConTab {
    fn get_load_state(
        &self,
        server: &AssetServer,
        assets: &Assets<ModConAsset>,
        models: &Assets<ModelAsset>,
    ) -> LoadState {
        match server.get_load_state(&self.handle) {
            LoadState::Loaded => {}
            state => return state,
        };
        let asset = match assets.get(&self.handle) {
            Some(v) => v,
            None => return LoadState::Failed,
        };
        // Ensure all dependencies loaded
        match server.get_group_load_state(asset.models.iter().map(|h| h.id())) {
            LoadState::Loaded => {}
            state => return state,
        }
        for model in &asset.models {
            let model = models.get(model).unwrap();
            match model.get_load_state(server) {
                LoadState::Loaded => {}
                state => return state,
            }
        }
        LoadState::Loaded
    }
}

impl SystemTab for ModConTab {
    type LoadParam = (
        SCommands,
        SResMut<Assets<Mesh>>,
        SResMut<Assets<CustomMaterial>>,
        SResMut<Assets<ModelAsset>>,
        SResMut<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<AssetServer>,
        SResMut<Assets<ModConAsset>>,
    );
    type UiParam =
        (SCommands, SRes<AssetServer>, SRes<Assets<ModelAsset>>, SRes<Assets<ModConAsset>>);

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (
            mut commands,
            mut meshes,
            mut materials,
            mut models,
            texture_assets,
            mut images,
            server,
            mod_con_assets,
        ) = query;

        if self.models.is_empty() {
            if let Some(mod_con) = mod_con_assets.get(&self.handle) {
                let data = match &mod_con.inner.visual_data {
                    Some(value) => value,
                    None => return,
                };
                for handle in &mod_con.models {
                    self.models.push(ModelInfo {
                        handle: handle.clone(),
                        loaded: vec![],
                        transforms: vec![],
                        aabb: Default::default(),
                    });
                }
                for (idx, &model_idx) in data.shorts_1.iter().enumerate() {
                    self.models[model_idx as usize]
                        .transforms
                        .push(convert_transform(&data.transforms[idx]));
                }
                self.models.retain(|info| !info.transforms.is_empty());
            }
        }

        let mut loaded = false;
        for info in &mut self.models {
            if !info.loaded.is_empty() {
                for loaded in &info.loaded {
                    if let Some(mut commands) = commands.get_entity(loaded.entity) {
                        commands.insert(Visibility::Hidden);
                    }
                }
                continue;
            }

            let asset = match models.get_mut(&info.handle) {
                Some(v) => v,
                None => continue,
            };
            // Ensure all dependencies loaded
            match asset.get_load_state(&server) {
                LoadState::Loaded => println!("Loading model"),
                _ => continue,
            }

            let result = load_model(
                asset,
                &mut commands,
                &texture_assets,
                &mut images,
                &mut materials,
                &mut meshes,
            );
            let built = match result {
                Ok(value) => value,
                Err(e) => {
                    log::error!("Failed to load model: {e:?}");
                    continue;
                }
            };
            for &transform in &info.transforms {
                let entity = commands
                    .spawn(SpatialBundle { transform, visibility: Visibility::Hidden, ..default() })
                    .with_children(|builder| {
                        for idx in built.lod[0].meshes.iter() {
                            let mesh = &built.meshes[idx];
                            builder.spawn(MaterialMeshBundle {
                                mesh: mesh.mesh.clone(),
                                material: mesh.material.clone(),
                                ..default()
                            });
                        }
                    })
                    .id();
                info.loaded.push(LoadedModel { entity, visible: true });
            }
            info.aabb = built.aabb;
            loaded = true;
        }

        if loaded {
            let all_loaded = self.models.iter().all(|m| !m.loaded.is_empty());
            if all_loaded {
                let mut min = Vec3A::splat(f32::MAX);
                let mut max = Vec3A::splat(f32::MIN);
                for info in &self.models {
                    min = info.aabb.min().min(min);
                    max = info.aabb.max().max(max);
                }
                self.camera.init(&Aabb::from_min_max(min.into(), max.into()), true);
            }
        }

        // FIXME
        if self.diffuse_map.is_weak() {
            self.diffuse_map = server.load("papermill_diffuse_rgb9e5_zstd.ktx2");
            self.specular_map = server.load("papermill_specular_rgb9e5_zstd.ktx2");
        }
    }

    fn close(&mut self, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (mut commands, _, _, _, _, _, _, _) = query;
        for model in self.models.iter().flat_map(|l| &l.loaded) {
            if let Some(commands) = commands.get_entity(model.entity) {
                commands.despawn_recursive();
            }
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        query: SystemParamItem<'_, '_, Self::UiParam>,
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
        self.camera.update(&rect, &response, ui.input(|i| i.scroll_delta));

        let (mut commands, server, models, mod_con_assets) = query;
        let all_loaded = self.models.iter().all(|m| !m.loaded.is_empty());
        if !all_loaded {
            ui.centered_and_justified(|ui| {
                match self.get_load_state(&server, &mod_con_assets, &models) {
                    LoadState::Failed => egui::Label::new(
                        egui::RichText::from("Loading failed").heading().color(egui::Color32::RED),
                    )
                    .ui(ui),
                    _ => egui::Spinner::new().size(50.0).ui(ui),
                };
            });
            return;
        }

        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(rect.height() * 0.25).show(ui, |ui| {
                ui.label(format!("Models: {}", self.models.len()));
                ui.label(format!(
                    "Instances: {}",
                    self.models.iter().map(|m| m.loaded.len()).sum::<usize>()
                ))
            });
        });

        commands.spawn((
            Camera3dBundle {
                camera_3d: Camera3d {
                    clear_color: if state.render_layer == 0 {
                        ClearColorConfig::Default
                    } else {
                        ClearColorConfig::None
                    },
                    ..default()
                },
                camera: Camera {
                    viewport: Some(viewport),
                    order: state.render_layer as isize,
                    // hdr: true,
                    ..default()
                },
                tonemapping: Tonemapping::TonyMcMapface,
                transform: self.camera.transform,
                ..default()
            },
            // BloomSettings::default(),
            EnvironmentMapLight {
                diffuse_map: self.diffuse_map.clone(),
                specular_map: self.specular_map.clone(),
            },
            RenderLayers::layer(state.render_layer),
            TemporaryLabel,
        ));
        // FIXME: https://github.com/bevyengine/bevy/issues/3462
        if state.render_layer == 0 {
            // commands.spawn((
            //     DirectionalLightBundle {
            //         directional_light: DirectionalLight { ..default() },
            //         transform: Transform::from_xyz(-30.0, 5.0, 20.0)
            //             .looking_at(Vec3::ZERO, Vec3::Y),
            //         ..default()
            //     },
            //     RenderLayers::layer(state.render_layer),
            //     TemporaryLabel,
            // ));
        }

        for info in &self.models {
            for model in &info.loaded {
                if let Some(mut commands) = commands.get_entity(model.entity) {
                    commands.insert((
                        if model.visible { Visibility::Visible } else { Visibility::Hidden },
                        RenderLayers::layer(state.render_layer),
                    ));
                }
            }
        }

        state.render_layer += 1;
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::SCENE_DATA, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }
}
