use std::collections::HashMap;

use bevy::{
    asset::LoadState,
    core_pipeline::{clear_color::ClearColorConfig, tonemapping::Tonemapping},
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::{camera::Viewport, view::RenderLayers},
};
use bevy_egui::EguiUserTextures;
use egui::Widget;
use retrolib::format::{
    cmdl::{CMaterialCache, CMaterialDataInner, CMaterialTextureTokenData},
    txtr::K_FORM_TXTR,
};
use uuid::Uuid;

use crate::{
    icon,
    loaders::{
        model::{MaterialKey, ModelAsset},
        texture::TextureAsset,
    },
    material::CustomMaterial,
    render::{
        camera::ModelCamera,
        convert_aabb,
        grid::GridSettings,
        model::{load_model, ModelLod},
        scene_preview::{PreviewMesh, PreviewMeshStatus, ShaderPreview},
        TemporaryLabel,
    },
    tabs::{
        property_with_value,
        texture::{TextureTab, UiTexture},
        EditorTabSystem,
    },
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
    pub selected_material: Option<usize>,
    pub camera: ModelCamera,
    pub diffuse_map: Handle<Image>,
    pub specular_map: Handle<Image>,
    pub egui_textures: HashMap<Uuid, UiTexture>,
}

impl ModelTab {
    pub fn new(asset_ref: AssetRef, handle: Handle<ModelAsset>) -> Box<Self> {
        Box::new(Self { asset_ref, handle, ..default() })
    }

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
        asset.get_load_state(server)
    }
}

impl EditorTabSystem for ModelTab {
    type LoadParam = (
        SCommands,
        SResMut<Assets<Mesh>>,
        SResMut<Assets<ModelAsset>>,
        SResMut<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<AssetServer>,
        SResMut<EguiUserTextures>,
    );
    type UiParam = (
        SCommands,
        SRes<AssetServer>,
        SRes<Assets<ModelAsset>>,
        SQuery<Read<PreviewMesh>>,
        SRes<ShaderPreview>,
    );

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        let (
            mut commands,
            mut meshes,
            mut models,
            mut texture_assets,
            mut images,
            server,
            mut egui_textures,
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
        match asset.get_load_state(&server) {
            LoadState::Loaded => {}
            _ => return,
        }

        asset.build_texture_images(&mut texture_assets, &mut images);
        let result = load_model(asset, &mut meshes);
        let built = match result {
            Ok(value) => value,
            Err(e) => {
                log::error!("Failed to load model: {e:?}");
                return;
            }
        };
        let mut meshes = Vec::with_capacity(built.meshes.len());
        for (mesh_index, mesh) in built.meshes.into_iter().enumerate() {
            let entity = commands
                .spawn(MaterialMeshBundle::<CustomMaterial> {
                    mesh: mesh.mesh,
                    // transform: Transform::from_translation((-built.aabb.center).into()),
                    visibility: Visibility::Hidden,
                    ..default()
                })
                .id();
            commands.entity(entity).insert(PreviewMesh::new(
                self.handle.clone(),
                mesh_index,
                MaterialKey {
                    material_idx: mesh.material_idx,
                    mesh_class: mesh.mesh_class,
                    mesh_mirrored: false,
                    mesh_two_sided: mesh.two_sided,
                },
            ));
            meshes.push(LoadedMesh {
                entity,
                material_idx: mesh.material_idx,
                visible: mesh.visible,
                unk_c: mesh.flags,
                unk_e: mesh.unk_e,
            });
        }
        self.loaded = Some(LoadedModel { meshes, lod: built.lod, materials: built.materials });
        self.camera.init(&convert_aabb(&asset.inner.head.bounds), true);
        self.diffuse_map = server.load("papermill_diffuse_rgb9e5_zstd.ktx2");
        self.specular_map = server.load("papermill_specular_rgb9e5_zstd.ktx2");

        // Build egui textures
        for (texture_id, texture_handle) in &asset.textures {
            let texture = texture_assets.get(texture_handle).unwrap();
            let ui_texture = UiTexture::from_handle(
                texture.slices[0][0].clone(),
                images.as_mut(),
                egui_textures.as_mut(),
            )
            .unwrap();
            self.egui_textures.insert(*texture_id, ui_texture);
        }
    }

    fn close(&mut self, query: SystemParamItem<Self::LoadParam>) -> bool {
        let (mut commands, _, _, _, _, _, _) = query;
        if let Some(loaded) = &self.loaded {
            for mesh in &loaded.meshes {
                if let Some(commands) = commands.get_entity(mesh.entity) {
                    commands.despawn_recursive();
                }
            }
        }
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
            physical_position: UVec2::new(left_top.x as u32, left_top.y as u32),
            physical_size: UVec2::new(size.x as u32, size.y as u32),
            depth: 0.0..1.0,
        };
        let response =
            ui.interact(rect, ui.make_persistent_id("background"), egui::Sense::click_and_drag());
        self.camera.update(&rect, &response, ui.input(|i| i.scroll_delta));

        let (mut commands, server, models, preview_meshes, preview) = query;
        if let Some(loaded) = &mut self.loaded {
            commands.spawn((
                Camera3dBundle {
                    camera_3d: Camera3d { clear_color: ClearColorConfig::None, ..default() },
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
                    ui.small(
                        preview.description(
                            loaded.lod[self.selected_lod]
                                .meshes
                                .iter()
                                .filter(|&idx| loaded.meshes[idx].visible)
                                .map(|idx| preview_meshes.get(loaded.meshes[idx].entity).ok()),
                        ),
                    );
                    for idx in loaded.lod[self.selected_lod].meshes.iter() {
                        let mesh = &mut loaded.meshes[idx];
                        let renderer = preview_meshes.get(mesh.entity).ok();
                        ui.horizontal(|ui| {
                            ui.checkbox(
                                &mut mesh.visible,
                                format!(
                                    "Mesh {idx} ({})",
                                    loaded.materials[mesh.material_idx].name
                                ),
                            )
                            .on_hover_text(
                                renderer.map_or("Preparing renderer", |r| r.description.as_str()),
                            );
                            let status = renderer
                                .map_or(PreviewMeshStatus::Preparing, |r| r.status(&preview));
                            let color = match status {
                                PreviewMeshStatus::Fallback => egui::Color32::LIGHT_YELLOW,
                                PreviewMeshStatus::Unavailable => egui::Color32::LIGHT_RED,
                                _ => ui.visuals().text_color(),
                            };
                            ui.label(egui::RichText::new(status.label()).small().color(color))
                                .on_hover_text(
                                    renderer
                                        .map_or("Preparing renderer", |r| r.description.as_str()),
                                );
                            if !matches!(mesh.unk_c, 0 | 1) {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("(unk_c: {})", mesh.unk_c),
                                );
                            }
                            if mesh.unk_e != 64 {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("(unk_e: {})", mesh.unk_e),
                                );
                            }
                            if ui
                                .small_button(format!("{}", icon::MATERIAL_DATA))
                                .on_hover_text_at_pointer("View material")
                                .clicked()
                            {
                                self.selected_material = Some(mesh.material_idx);
                            }
                        });
                        if let Some(mut commands) = commands.get_entity(mesh.entity) {
                            commands.insert((
                                if mesh.visible { Visibility::Visible } else { Visibility::Hidden },
                                RenderLayers::layer(state.render_layer),
                            ));
                        }
                    }
                });
            });
            if let Some(material_idx) = self.selected_material {
                ui.push_id(format!("material_{}", material_idx), |ui| {
                    egui::Frame::group(ui.style()).fill(egui::Color32::from_black_alpha(200)).show(
                        ui,
                        |ui| {
                            egui::ScrollArea::vertical()
                                // .max_height(rect.height() * 0.25)
                                .show(ui, |ui| {
                                    if ui
                                        .small_button(format!("{}", icon::PANEL_CLOSE))
                                        .on_hover_text_at_pointer("Close")
                                        .clicked()
                                    {
                                        self.selected_material = None;
                                    }
                                    for renderer in loaded.meshes.iter()
                                        .filter(|m| m.material_idx == material_idx)
                                        .filter_map(|m| preview_meshes.get(m.entity).ok())
                                        .map(|r| r.description.as_str())
                                        .collect::<std::collections::BTreeSet<_>>() {
                                        ui.label(renderer);
                                    }
                                    if let Some(selection) = models.get(&self.handle)
                                        .and_then(|m| m.shader_selections.get(&material_idx)) {
                                        ui.collapsing("Cooked forward shader (static model state)", |ui| {
                                            match selection {
                                                Ok(s) => {
                                                    property_with_value(ui, "Implementation", s.name.clone());
                                                    property_with_value(ui, "Material hash", format!("{:08X}", s.material_hash));
                                                    property_with_value(ui, "Runtime ID", format!("{:X}", s.runtime_id));
                                                    property_with_value(ui, "Permutation", format!("{:016X}{:016X}", s.permutation[1], s.permutation[0]));
                                                    property_with_value(ui, "Shader hash", format!("{:016X}", s.shader_hash));
                                                    if !loaded.meshes.iter().any(|m| m.material_idx == material_idx
                                                        && preview_meshes.get(m.entity).is_ok_and(|r| r.original.is_some())) {
                                                        ui.label(s.surface_profile.description());
                                                    }
                                                    ui.collapsing("Cooked graphics state", |ui| {
                                                        ui.monospace(format!("Technique: {:#?}\nShader overrides: {:#?}", s.technique_state, s.override_state));
                                                        for warning in &s.render_state_warnings { ui.label(warning); }
                                                    });
                                                    ui.collapsing("Selected feature options", |ui| {
                                                        for feature in &s.features { ui.label(feature); }
                                                    });
                                                }
                                                Err(reason) => { ui.label(format!("Selection unavailable: {reason}")); }
                                            }
                                        });
                                    }
                                    material_ui(
                                        ui,
                                        &loaded.materials[material_idx],
                                        &self.egui_textures,
                                        state,
                                        server.as_ref(),
                                    );
                                });
                        },
                    );
                });
            }
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

    fn title(&self) -> egui::WidgetText {
        format!("{} {} {}", icon::FILE_3D, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }

    fn clear_background(&self) -> bool { false }

    fn asset(&self) -> Option<AssetRef> { Some(self.asset_ref) }
}

fn texture_ui(
    ui: &mut egui::Ui,
    texture: &CMaterialTextureTokenData,
    textures: &HashMap<Uuid, UiTexture>,
    state: &mut TabState,
    server: &AssetServer,
) {
    property_with_value(ui, "Texture ID", format!("{}", texture.id));
    if let Some(ui_texture) = textures.get(&texture.id) {
        if ui_texture
            .image_scaled(200.0)
            .sense(egui::Sense::click())
            .ui(ui)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            state.open_tab(TextureTab::new(
                AssetRef { id: texture.id, kind: K_FORM_TXTR },
                server.load(format!("{}.{}", texture.id, K_FORM_TXTR)),
            ));
        }
    }
    if let Some(usage) = &texture.usage {
        property_with_value(ui, "Tex coord", format!("{}", usage.tex_coord));
        property_with_value(ui, "Filter", format!("{}", usage.filter));
        property_with_value(ui, "Wrap X", format!("{}", usage.wrap_x));
        property_with_value(ui, "Wrap Y", format!("{}", usage.wrap_y));
        property_with_value(ui, "Wrap Z", format!("{}", usage.wrap_z));
    }
}

fn material_ui(
    ui: &mut egui::Ui,
    mat: &CMaterialCache,
    textures: &HashMap<Uuid, UiTexture>,
    state: &mut TabState,
    server: &AssetServer,
) {
    property_with_value(ui, "Material", mat.name.clone());
    property_with_value(ui, "Material resource ID", format!("{}", mat.material_resource_id));
    property_with_value(ui, "Material implementation ID", format!("{}", mat.material_id));
    property_with_value(ui, "Flags", format!("{:032b}", mat.flags));
    property_with_value(ui, "Material set hash", format!("{:08X}", mat.material_set_hash));
    ui.collapsing(format!("Traits: {}", mat.traits.len()), |ui| {
        for material_trait in &mat.traits {
            ui.label(material_trait.to_string());
        }
    });
    ui.collapsing(format!("External variables: {}", mat.variable_descs.len()), |ui| {
        for render_type in &mat.variable_descs {
            ui.group(|ui| {
                property_with_value(ui, "Name", format!("{}", render_type.name));
                property_with_value(ui, "Data ID", format!("{}", render_type.data_id));
                property_with_value(ui, "Variable type", format!("{}", render_type.data_type));
                property_with_value(ui, "Packing rule", format!("{}", render_type.packing_rule));
            });
        }
    });
    ui.collapsing(format!("Data: {}", mat.data.len()), |ui| {
        for material_data in &mat.data {
            ui.group(|ui| {
                property_with_value(ui, "Data ID", format!("{:?}", material_data.data_id));
                property_with_value(ui, "Data type", format!("{:?}", material_data.data_type));
                match &material_data.data {
                    CMaterialDataInner::Texture(texture) => {
                        texture_ui(ui, texture, textures, state, server);
                    }
                    CMaterialDataInner::Color(color) => {
                        property_with_value(ui, "Color", format!("{:?}", color.to_array()));
                    }
                    CMaterialDataInner::Scalar(scalar) => {
                        property_with_value(ui, "Scalar", format!("{}", scalar));
                    }
                    CMaterialDataInner::Int1(int) => {
                        property_with_value(ui, "Int", format!("{}", int));
                    }
                    CMaterialDataInner::Int4(int4) => {
                        property_with_value(ui, "Int4", format!("{:?}", int4.to_array()));
                    }
                    CMaterialDataInner::Mat4(mat4) => {
                        property_with_value(ui, "Mat4", format!("{:?}", mat4));
                    }
                    CMaterialDataInner::LayeredTexture(layers) => {
                        for (idx, color) in layers.base.colors.iter().enumerate() {
                            property_with_value(
                                ui,
                                &format!("Blend register {idx}"),
                                format!("{:?}", color.to_array()),
                            );
                        }
                        property_with_value(ui, "Flags", format!("{}", layers.base.flags));
                        property_with_value(
                            ui,
                            "Blend control",
                            format!("{:?}", layers.base.blend_control),
                        );
                        for (name, texture) in
                            ["Texture 0", "Texture 1", "Blend texture"].iter().zip(&layers.textures)
                        {
                            ui.group(|ui| {
                                ui.label(*name);
                                texture_ui(ui, texture, textures, state, server);
                            });
                        }
                    }
                }
            });
        }
    });
}
