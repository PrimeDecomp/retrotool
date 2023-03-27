use bevy::{
    asset::LoadState,
    core_pipeline::{clear_color::ClearColorConfig, tonemapping::Tonemapping},
    ecs::system::{lifetimeless::*, *},
    math::Vec3A,
    prelude::*,
    render::{camera::Viewport, primitives::Aabb, view::RenderLayers},
};
use bevy_egui::EguiContext;
use bevy_mod_raycast::{Intersection, RaycastMesh, RaycastSource};
use egui::{Sense, Widget};
use retrolib::format::SumBy;
use uuid::Uuid;

use crate::{
    icon,
    loaders::{
        modcon::ModConAsset,
        model::{MaterialKey, ModelAsset},
        texture::TextureAsset,
    },
    material::CustomMaterial,
    render::{
        camera::ModelCamera, convert_transform, grid::GridSettings, model::load_model,
        TemporaryLabel,
    },
    tabs::{model::ModelTab, SystemTab, TabState, TabType},
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

pub struct ModConTab {
    pub tab_id: Uuid,
    pub asset_ref: AssetRef,
    pub handle: Handle<ModConAsset>,
    pub models: Vec<ModelInfo>,
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

pub struct ModConRaycastSet;

#[derive(Component, Clone, Debug)]
pub struct ModelLabel {
    pub asset_ref: AssetRef,
    pub tab_id: Uuid,
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
    type UiParam = (
        SCommands,
        SRes<AssetServer>,
        SRes<Assets<ModelAsset>>,
        SRes<Assets<ModConAsset>>,
        SQuery<Read<Parent>, With<Intersection<ModConRaycastSet>>>,
        SQuery<(Read<ModelLabel>, Read<Children>)>,
    );

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

            asset.build_texture_images(&texture_assets, &mut images);
            let result = load_model(asset, &mut meshes);
            let built = match result {
                Ok(value) => value,
                Err(e) => {
                    log::error!("Failed to load model: {e:?}");
                    continue;
                }
            };
            for &transform in &info.transforms {
                let is_mirrored = transform.scale.x.is_sign_negative()
                    ^ transform.scale.y.is_sign_negative()
                    ^ transform.scale.z.is_sign_negative();
                let entity = commands
                    .spawn((
                        SpatialBundle { transform, visibility: Visibility::Hidden, ..default() },
                        ModelLabel { asset_ref: asset.asset_ref, tab_id: self.tab_id },
                    ))
                    .with_children(|builder| {
                        for idx in built.lod[0].meshes.iter() {
                            let mesh = &built.meshes[idx];
                            let material = match asset.material(
                                &MaterialKey {
                                    material_idx: mesh.material_idx,
                                    mesh_flags: mesh.flags,
                                    mesh_mirrored: is_mirrored,
                                },
                                &mut materials,
                            ) {
                                Ok(handle) => handle,
                                Err(e) => {
                                    log::warn!("Failed to build material: {:?}", e);
                                    continue;
                                }
                            };
                            builder.spawn((
                                MaterialMeshBundle::<CustomMaterial> {
                                    mesh: mesh.mesh.clone(),
                                    material,
                                    ..default()
                                },
                                RaycastMesh::<ModConRaycastSet>::default(),
                            ));
                        }
                    })
                    .id();
                info.loaded.push(LoadedModel { entity, visible: true });
            }
            info.aabb = built.aabb;
            loaded = true;
        }

        if loaded && self.models.iter().all(|m| !m.loaded.is_empty()) {
            let mut min = Vec3A::splat(f32::MAX);
            let mut max = Vec3A::splat(f32::MIN);
            for info in &self.models {
                let m_min = Vec3::from(info.aabb.min());
                let m_max = Vec3::from(info.aabb.max());
                for &xf in &info.transforms {
                    min = min.min(Vec3A::from(xf * m_min));
                    max = max.max(Vec3A::from(xf * m_max));
                }
            }
            let aabb = Aabb::from_min_max(min.into(), max.into());
            self.camera.init(&aabb, true);
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
        let mut response =
            ui.interact(rect, ui.make_persistent_id("background"), Sense::click_and_drag());
        self.camera.update(&rect, &response, ui.input(|i| i.scroll_delta));

        let (mut commands, server, models, mod_con_assets, intersection_query, model_query) = query;
        if !self.models.iter().all(|m| !m.loaded.is_empty()) {
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

        if let Some(parent) = intersection_query.iter().next() {
            let (label, _) = model_query.get(parent.get()).unwrap();
            if label.tab_id == self.tab_id {
                self.selected_model = Some(label.asset_ref);
            }
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(rect.height() * 0.25).show(ui, |ui| {
                ui.checkbox(&mut self.env_light, "Environment lighting");
                ui.label(format!("Models: {}", self.models.len()));
                ui.label(format!("Instances: {}", self.models.sum_by(|m| m.loaded.len())));
                if let Some(selected) = &self.selected_model {
                    ui.label(format!("Hovering: {}", selected.id));
                }
            });
        });

        if let Some(selected) = &self.selected_model {
            let mut shown = false;
            response = response.context_menu(|ui| {
                if ui.button("Open in new tab").clicked() {
                    let handle = server.load(format!("{}.{}", selected.id, selected.kind));
                    state.open_tab(TabType::Model(Box::new(ModelTab {
                        asset_ref: *selected,
                        handle,
                        ..default()
                    })));
                    ui.close_menu();
                }
                if ui.button("Copy GUID").clicked() {
                    ui.output_mut(|out| out.copied_text = format!("{}", selected.id));
                    ui.close_menu();
                }
                shown = true;
            });
            if !shown {
                self.selected_model = None;
            }
        }

        let camera = Camera {
            viewport: Some(viewport),
            order: state.render_layer as isize,
            // hdr: true,
            ..default()
        };
        let mut entity = commands.spawn((
            Camera3dBundle {
                camera_3d: Camera3d { clear_color: ClearColorConfig::None, ..default() },
                camera: camera.clone(),
                tonemapping: Tonemapping::TonyMcMapface,
                transform: self.camera.transform,
                ..default()
            },
            // BloomSettings::default(),
            GridSettings {
                clear_color: if state.render_layer == 0 {
                    ClearColorConfig::Default
                } else {
                    ClearColorConfig::None
                },
            },
            RenderLayers::layer(state.render_layer),
            TemporaryLabel,
        ));
        if self.env_light {
            entity.insert(EnvironmentMapLight {
                diffuse_map: self.diffuse_map.clone(),
                specular_map: self.specular_map.clone(),
            });
        }
        let mut is_raycasting = false;
        if response.hovered() {
            if let Some(pos) = ui.input(|i| {
                i.pointer.hover_pos().map(|pos| Vec2::new(pos.x, i.screen_rect.height() - pos.y))
            }) {
                entity.insert(RaycastSource::<ModConRaycastSet>::new_screenspace(
                    pos,
                    &camera,
                    &GlobalTransform::default(),
                ));
                is_raycasting = true;
            }
        }
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
                    commands.insert(if model.visible {
                        Visibility::Visible
                    } else {
                        Visibility::Hidden
                    });
                }
                if let Ok((_, children)) = model_query.get(model.entity) {
                    for &child in children.iter() {
                        if let Some(mut commands) = commands.get_entity(child) {
                            commands.insert(RenderLayers::layer(state.render_layer));
                            if is_raycasting {
                                commands.insert(RaycastMesh::<ModConRaycastSet>::default());
                            } else {
                                commands.remove::<RaycastMesh<ModConRaycastSet>>();
                            }
                        }
                    }
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
