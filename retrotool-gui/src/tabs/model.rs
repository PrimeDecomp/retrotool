use bevy::{
    asset::LoadState,
    core_pipeline::{clear_color::ClearColorConfig, tonemapping::Tonemapping},
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::{camera::Viewport, view::RenderLayers},
};
use bevy_egui::EguiContext;
use egui::{Sense, Widget};
use retrolib::format::cmdl::CMaterialCache;

use crate::{
    icon,
    loaders::{
        model::{MaterialKey, ModelAsset},
        texture::TextureAsset,
    },
    material::CustomMaterial,
    render::{
        camera::ModelCamera,
        model::{convert_aabb, load_model, ModelLod},
        TemporaryLabel,
    },
    tabs::SystemTab,
    AssetRef, TabState,
};

pub struct LoadedMesh {
    pub entity: Entity,
    pub material_idx: usize,
    pub visible: bool,
    pub unk_c: u16,
    pub unk_e: u16,
}

pub struct LoadedModel {
    pub meshes: Vec<LoadedMesh>,
    pub lod: Vec<ModelLod>,
    pub materials: Vec<CMaterialCache>,
}

#[derive(Default)]
pub struct ModelTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<ModelAsset>,
    pub loaded: Option<LoadedModel>,
    pub selected_lod: usize,
    pub camera: ModelCamera,
    pub diffuse_map: Handle<Image>,
    pub specular_map: Handle<Image>,
}

impl ModelTab {
    fn get_load_state(&self, server: &AssetServer, models: &Assets<ModelAsset>) -> LoadState {
        match server.get_load_state(&self.handle) {
            LoadState::Loaded => {}
            state => return state,
        };
        let asset = match models.get(&self.handle) {
            Some(v) => v,
            None => return LoadState::Failed,
        };
        // Ensure all dependencies loaded
        server.get_group_load_state(asset.textures.iter().map(|(_, h)| h.id()))
    }
}

impl SystemTab for ModelTab {
    type LoadParam = (
        SCommands,
        SResMut<Assets<Mesh>>,
        SResMut<Assets<CustomMaterial>>,
        SResMut<Assets<ModelAsset>>,
        SResMut<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<AssetServer>,
    );
    type UiParam = (SCommands, SRes<AssetServer>, SRes<Assets<ModelAsset>>);

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (
            mut commands,
            mut meshes,
            mut materials,
            mut models,
            texture_assets,
            mut images,
            server,
        ) = query;
        if let Some(loaded) = &self.loaded {
            for mesh in &loaded.meshes {
                if let Some(mut commands) = commands.get_entity(mesh.entity) {
                    commands.insert(Visibility::Hidden);
                }
            }
            return;
        }

        let asset = match models.get_mut(&self.handle) {
            Some(v) => v,
            None => return,
        };
        // Ensure all dependencies loaded
        match server.get_group_load_state(asset.textures.iter().map(|(_, h)| h.id())) {
            LoadState::Loaded => {}
            _ => return,
        }

        asset.build_texture_images(&texture_assets, &mut images);
        let result = load_model(asset, &mut meshes);
        let built = match result {
            Ok(value) => value,
            Err(e) => {
                log::error!("Failed to load model: {e:?}");
                return;
            }
        };
        let mut meshes = Vec::with_capacity(built.meshes.len());
        for mesh in built.meshes {
            let material = match asset.material(
                &MaterialKey {
                    material_idx: mesh.material_idx,
                    mesh_flags: mesh.flags,
                    mesh_mirrored: false,
                },
                &mut materials,
            ) {
                Ok(handle) => handle,
                Err(e) => {
                    log::warn!("Failed to build material: {:?}", e);
                    continue;
                }
            };
            let entity = commands
                .spawn(MaterialMeshBundle::<CustomMaterial> {
                    mesh: mesh.mesh,
                    material,
                    transform: Transform::from_translation((-built.aabb.center).into()),
                    ..default()
                })
                .id();
            meshes.push(LoadedMesh {
                entity,
                material_idx: mesh.material_idx,
                visible: mesh.visible,
                unk_c: mesh.flags,
                unk_e: mesh.unk_e,
            });
        }
        self.loaded = Some(LoadedModel { meshes, lod: built.lod, materials: built.materials });
        self.camera.init(&convert_aabb(&asset.inner.head.bounds), false);
        self.diffuse_map = server.load("papermill_diffuse_rgb9e5_zstd.ktx2");
        self.specular_map = server.load("papermill_specular_rgb9e5_zstd.ktx2");
    }

    fn close(&mut self, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (mut commands, _, _, _, _, _, _) = query;
        if let Some(loaded) = &self.loaded {
            for mesh in &loaded.meshes {
                if let Some(commands) = commands.get_entity(mesh.entity) {
                    commands.despawn_recursive();
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
        let scale = ui.ctx().pixels_per_point();
        let rect = ui.available_rect_before_wrap();
        let left_top = rect.left_top().to_vec2() * scale;
        let size = rect.size() * scale;
        let viewport = Viewport {
            physical_position: UVec2::new(left_top.x as u32, left_top.y as u32),
            physical_size: UVec2::new(size.x as u32, size.y as u32),
            depth: 0.0..1.0,
        };
        let response =
            ui.interact(rect, ui.make_persistent_id("background"), Sense::click_and_drag());
        self.camera.update(&rect, &response, ui.input(|i| i.scroll_delta));

        let (mut commands, server, models) = query;
        if let Some(loaded) = &mut self.loaded {
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

            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(rect.height() * 0.25).show(ui, |ui| {
                    if loaded.lod.len() > 1 {
                        egui::Slider::new(&mut self.selected_lod, 0..=loaded.lod.len() - 1)
                            .text("LOD")
                            .ui(ui);
                        if let Some(value) = loaded.lod[self.selected_lod].distance {
                            ui.label(format!("Distance: {value}"));
                        }
                    }
                    for idx in loaded.lod[self.selected_lod].meshes.iter() {
                        let mesh = &mut loaded.meshes[idx];
                        ui.checkbox(
                            &mut mesh.visible,
                            format!(
                                "Mesh {idx} ({}, {}, {})",
                                mesh.unk_c, mesh.unk_e, loaded.materials[mesh.material_idx].name
                            ),
                        );
                        if let Some(mut commands) = commands.get_entity(mesh.entity) {
                            commands.insert((
                                if mesh.visible { Visibility::Visible } else { Visibility::Hidden },
                                RenderLayers::layer(state.render_layer),
                            ));
                        }
                    }
                });
            });
            state.render_layer += 1;
        } else {
            ui.centered_and_justified(|ui| {
                match self.get_load_state(&server, &models) {
                    LoadState::Failed => egui::Label::new(
                        egui::RichText::from("Loading failed").heading().color(egui::Color32::RED),
                    )
                    .ui(ui),
                    _ => egui::Spinner::new().size(50.0).ui(ui),
                };
            });
        }
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::FILE_3D, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }
}
