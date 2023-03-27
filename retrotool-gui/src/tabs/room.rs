use bevy::{
    core_pipeline::{clear_color::ClearColorConfig, tonemapping::Tonemapping},
    ecs::system::{lifetimeless::*, *},
    prelude::*,
    render::{camera::Viewport, view::RenderLayers},
};
use bevy_egui::EguiContext;
use bevy_mod_raycast::{Intersection, RaycastSource};
use egui::Sense;
use retrolib::format::room::ConstructedPropertyValue;

use crate::{
    icon,
    loaders::{model::ModelAsset, room::RoomAsset, texture::TextureAsset},
    material::CustomMaterial,
    render::{camera::ModelCamera, grid::GridSettings, TemporaryLabel},
    tabs::{modcon::ModelLabel, property_with_value, SystemTab, TabState},
    AssetRef,
};

pub struct RoomTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<RoomAsset>,
    pub camera: ModelCamera,
}

impl Default for RoomTab {
    fn default() -> Self { Self { asset_ref: default(), handle: default(), camera: default() } }
}

// impl RoomTab {
//     fn get_load_state(
//         &self,
//         server: &AssetServer,
//         assets: &Assets<RoomAsset>,
//         models: &Assets<ModelAsset>,
//     ) -> LoadState {
//         match server.get_load_state(&self.handle) {
//             LoadState::Loaded => {}
//             state => return state,
//         };
//         let asset = match assets.get(&self.handle) {
//             Some(v) => v,
//             None => return LoadState::Failed,
//         };
//         // Ensure all dependencies loaded
//         match server.get_group_load_state(asset.models.iter().map(|h| h.id())) {
//             LoadState::Loaded => {}
//             state => return state,
//         }
//         for model in &asset.models {
//             let model = models.get(model).unwrap();
//             match model.get_load_state(server) {
//                 LoadState::Loaded => {}
//                 state => return state,
//             }
//         }
//         LoadState::Loaded
//     }
// }

pub struct RoomRaycastSet;

impl SystemTab for RoomTab {
    type LoadParam = (
        SCommands,
        SResMut<Assets<Mesh>>,
        SResMut<Assets<CustomMaterial>>,
        SResMut<Assets<ModelAsset>>,
        SResMut<Assets<TextureAsset>>,
        SResMut<Assets<Image>>,
        SResMut<AssetServer>,
        SResMut<Assets<RoomAsset>>,
    );
    type UiParam = (
        SCommands,
        SRes<AssetServer>,
        SRes<Assets<ModelAsset>>,
        SRes<Assets<RoomAsset>>,
        SQuery<Read<Parent>, With<Intersection<RoomRaycastSet>>>,
        SQuery<Read<ModelLabel>>,
    );

    fn load(&mut self, _ctx: &mut EguiContext, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (
            _commands,
            _meshes,
            _materials,
            _models,
            _texture_assets,
            _images,
            _server,
            _room_assets,
        ) = query;
    }

    fn close(&mut self, query: SystemParamItem<'_, '_, Self::LoadParam>) {
        let (_commands, _, _, _, _, _, _, _) = query;
        // for model in self.models.iter().flat_map(|l| &l.loaded) {
        //     if let Some(commands) = commands.get_entity(model.entity) {
        //         commands.despawn_recursive();
        //     }
        // }
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

        let (mut commands, _server, _models, room_assets, _intersection_query, _model_query) =
            query;
        let room_asset = match room_assets.get(&self.handle) {
            Some(v) => v,
            None => return,
        };

        // if let Some(parent) = intersection_query.iter().next() {
        //     self.selected_model = Some(model_query.get(parent.get()).unwrap().clone());
        // }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                // .max_height(rect.height() * 0.25)
                .show(ui, |ui| {
                    if !room_asset.inner.room_header.parent_room_id.is_nil() {
                        property_with_value(
                            ui,
                            "Parent",
                            format!("{}", room_asset.inner.room_header.parent_room_id),
                        );
                    }
                    property_with_value(
                        ui,
                        "Unk1",
                        format!("{}", room_asset.inner.room_header.unk1),
                    );
                    property_with_value(
                        ui,
                        "Unk2",
                        format!("{}", room_asset.inner.room_header.unk2),
                    );
                    property_with_value(
                        ui,
                        "Unk3",
                        format!("{}", room_asset.inner.room_header.unk3),
                    );
                    if !room_asset.inner.room_header.id_b.is_nil() {
                        property_with_value(
                            ui,
                            "ID b",
                            format!("{}", room_asset.inner.room_header.id_b),
                        );
                    }
                    if !room_asset.inner.room_header.id_c.is_nil() {
                        property_with_value(
                            ui,
                            "ID c",
                            format!("{}", room_asset.inner.room_header.id_c),
                        );
                    }
                    if !room_asset.inner.room_header.id_d.is_nil() {
                        property_with_value(
                            ui,
                            "ID d",
                            format!("{}", room_asset.inner.room_header.id_d),
                        );
                    }
                    if !room_asset.inner.room_header.id_e.is_nil() {
                        property_with_value(
                            ui,
                            "ID e",
                            format!("{}", room_asset.inner.room_header.id_e),
                        );
                    }
                    if !room_asset.inner.room_header.path_find_area_id.is_nil() {
                        property_with_value(
                            ui,
                            "Path Find Area",
                            format!("{}", room_asset.inner.room_header.path_find_area_id),
                        );
                    }
                    if let Some(light_map) = &room_asset.inner.baked_lighting.light_map {
                        ui.collapsing("Light map data", |ui| {
                            property_with_value(ui, "Texture ID", format!("{}", light_map.txtr_id));
                            // TODO display
                            for id in &light_map.ids {
                                property_with_value(ui, "Unk ID", format!("{}", id));
                            }
                            for lookup in &light_map.atlas_lookups {
                                property_with_value(
                                    ui,
                                    "Atlas lookup",
                                    format!("{:?}", lookup.0.to_array()),
                                );
                            }
                        });
                    }
                    if let Some(light_probe) = &room_asset.inner.baked_lighting.light_probe {
                        property_with_value(ui, "Light Probe", format!("{}", light_probe.ltpb_id));
                    }
                    for (layer_idx, layer) in room_asset.inner.layers.iter().enumerate() {
                        ui.collapsing(
                            format!("Layer {} ({})", layer_idx, layer.header.name),
                            |ui| {
                                property_with_value(ui, "Name", layer.header.name.clone());
                                property_with_value(ui, "ID", format!("{}", layer.header.id));
                                property_with_value(ui, "Unk", format!("{}", layer.header.unk));
                                for id in &layer.header.ids {
                                    property_with_value(ui, "Unk ID", format!("{}", id));
                                }
                                property_with_value(ui, "Unk2", format!("{}", layer.header.unk2));
                                for (component_idx, component) in
                                    layer.components.iter().enumerate()
                                {
                                    property_with_value(
                                        ui,
                                        &format!("Component {component_idx}"),
                                        format!("{}", component.component_type),
                                    );
                                    property_ui(
                                        ui,
                                        &room_asset.inner.constructed_properties
                                            [component.property_index as usize],
                                    );
                                }
                            },
                        );
                    }
                });
        });

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
        // if self.env_light {
        //     entity.insert(EnvironmentMapLight {
        //         diffuse_map: self.diffuse_map.clone(),
        //         specular_map: self.specular_map.clone(),
        //     });
        // }
        if response.hovered() {
            if let Some(pos) = ui.input(|i| {
                i.pointer.hover_pos().map(|pos| Vec2::new(pos.x, i.screen_rect.height() - pos.y))
            }) {
                entity.insert(RaycastSource::<RoomRaycastSet>::new_screenspace(
                    pos,
                    &camera,
                    &GlobalTransform::default(),
                ));
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

        // for info in &self.models {
        //     for model in &info.loaded {
        //         if let Some(mut commands) = commands.get_entity(model.entity) {
        //             commands.insert((
        //                 if model.visible { Visibility::Visible } else { Visibility::Hidden },
        //                 RenderLayers::layer(state.render_layer),
        //             ));
        //         }
        //     }
        // }

        state.render_layer += 1;
    }

    fn title(&mut self) -> egui::WidgetText {
        format!("{} {} {}", icon::SCENE_DATA, self.asset_ref.kind, self.asset_ref.id).into()
    }

    fn id(&self) -> String { format!("{} {}", self.asset_ref.kind, self.asset_ref.id) }
}

fn property_ui(ui: &mut egui::Ui, property: &ConstructedPropertyValue) {
    match property {
        ConstructedPropertyValue::ObjectId(id) => {
            property_with_value(ui, "ID", format!("{id}"));
        }
        ConstructedPropertyValue::Enum(value, name) => {
            property_with_value(ui, &format!("Enum {name}"), format!("{value:#X}"));
        }
        ConstructedPropertyValue::Float(value) => {
            property_with_value(ui, "Float", format!("{value}"));
        }
        ConstructedPropertyValue::Struct(prop_struct) => {
            if !prop_struct.name.is_empty() {
                property_with_value(ui, "Struct", prop_struct.name.to_string());
            }
            for prop in &prop_struct.properties {
                ui.group(|ui| {
                    property_with_value(ui, "Property ID", format!("{:#X}", prop.id));
                    property_ui(ui, &prop.value);
                });
            }
        }
        ConstructedPropertyValue::Opaque(data) => {
            ui.label(format!("Opaque data: {:?}", *data));
        }
        ConstructedPropertyValue::Unknown(data) => {
            ui.label(format!("Unknown data (size {:#X})", data.len()));
        }
        ConstructedPropertyValue::Int(int) => {
            property_with_value(ui, "Int", format!("{int}"));
        }
        ConstructedPropertyValue::Bool(b) => {
            property_with_value(ui, "Bool", format!("{b}"));
        }
        ConstructedPropertyValue::Color(color) => {
            property_with_value(ui, "Color", format!("{:?}", color.to_array()));
        }
        ConstructedPropertyValue::TypedefInterface(kind, value) => {
            property_with_value(ui, "Typedef", format!("{:?}", kind));
            property_ui(ui, value);
        }
        ConstructedPropertyValue::List(vec) => {
            for (idx, value) in vec.iter().enumerate() {
                ui.group(|ui| {
                    ui.label(format!("Item {idx}"));
                    property_ui(ui, value);
                });
            }
        }
    }
}
