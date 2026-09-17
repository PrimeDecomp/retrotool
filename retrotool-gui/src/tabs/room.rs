use bevy::{
    camera::{visibility::RenderLayers, Viewport},
    ecs::system::{lifetimeless::*, *},
    prelude::*,
};
use egui::Sense;
use retrolib::format::room::{ConstructedProperty, ConstructedPropertyValue};
use uuid::Uuid;

use crate::{
    icon,
    loaders::{
        modcon::ModConAsset,
        model::ModelAsset,
        room::{RoomAsset, RoomModCon},
    },
    render::{
        camera::ModelCamera,
        scene::{container_models, model_bounds, ModelGroup, SceneModelLoader},
        scene_preview::{PreviewMesh, ShaderPreview},
    },
    tabs::{property_with_id, property_with_value, EditorTabSystem, TabState},
    AssetRef,
};

pub struct RoomGeometry {
    pub source: RoomModCon,
    pub models: Vec<ModelGroup>,
    prepared: bool,
    error: Option<String>,
}

pub struct RoomTab {
    pub asset_ref: AssetRef,
    pub handle: Handle<RoomAsset>,
    pub camera: ModelCamera,
    pub geometry: Vec<RoomGeometry>,
    pub visible_layers: Vec<bool>,
    pub use_baked_atlas: bool,
    tab_id: Uuid,
    initialized: bool,
    diffuse_map: Handle<Image>,
    specular_map: Handle<Image>,
}

impl Default for RoomTab {
    fn default() -> Self {
        Self {
            asset_ref: default(),
            handle: default(),
            camera: default(),
            geometry: Vec::new(),
            visible_layers: Vec::new(),
            use_baked_atlas: true,
            tab_id: Uuid::new_v4(),
            initialized: false,
            diffuse_map: default(),
            specular_map: default(),
        }
    }
}

impl RoomTab {
    pub fn new(asset_ref: AssetRef, handle: Handle<RoomAsset>) -> Box<Self> {
        Box::new(Self { asset_ref, handle, ..default() })
    }
}

impl EditorTabSystem for RoomTab {
    type LoadParam =
        (SceneModelLoader<'static, 'static>, SRes<Assets<RoomAsset>>, SRes<Assets<ModConAsset>>);
    type UiParam = (
        SCommands,
        SRes<AssetServer>,
        SRes<Assets<ModelAsset>>,
        SRes<Assets<RoomAsset>>,
        SQuery<Read<Children>>,
        SRes<ShaderPreview>,
        SQuery<Read<PreviewMesh>>,
    );

    fn load(&mut self, query: SystemParamItem<Self::LoadParam>) {
        let (mut loader, rooms, containers) = query;
        if !self.initialized {
            let Some(room) = rooms.get(&self.handle) else { return };
            self.visible_layers = vec![true; room.inner.layers.len()];
            self.geometry = room
                .modcons
                .iter()
                .cloned()
                .map(|source| RoomGeometry {
                    source,
                    models: Vec::new(),
                    prepared: false,
                    error: None,
                })
                .collect();
            self.initialized = true;
        }
        let mut changed = false;
        for geometry in &mut self.geometry {
            if !geometry.prepared {
                if let Some(container) = containers.get(&geometry.source.handle) {
                    match container_models(container) {
                        Ok(models) => geometry.models = models,
                        Err(error) => geometry.error = Some(format!("{error:#}")),
                    }
                    geometry.prepared = true;
                }
            }
            let lightmap = rooms
                .get(&self.handle)
                .and_then(|room| room.lightmap.as_ref())
                .filter(|_| self.use_baked_atlas);
            changed |= loader.load(&mut geometry.models, self.tab_id, lightmap);
        }
        if changed && self.geometry.iter().all(|geometry| geometry.prepared) {
            if let Some(bounds) = model_bounds(self.geometry.iter().flat_map(|g| &g.models)) {
                self.camera.init(&bounds, true);
            }
        }
        if self.diffuse_map == Handle::default() {
            self.diffuse_map = loader.server.load("papermill_diffuse_rgb9e5_zstd.ktx2");
            self.specular_map = loader.server.load("papermill_specular_rgb9e5_zstd.ktx2");
        }
    }

    fn close(&mut self, query: SystemParamItem<Self::LoadParam>) -> bool {
        let (mut loader, _, _) = query;
        self.camera.close(&mut loader.commands);
        for geometry in &self.geometry {
            loader.close(&geometry.models);
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
            physical_position: UVec2 { x: left_top.x as u32, y: left_top.y as u32 },
            physical_size: UVec2 { x: size.x as u32, y: size.y as u32 },
            depth: 0.0..1.0,
        };
        let response =
            ui.interact(rect, ui.make_persistent_id("background"), Sense::click_and_drag());
        self.camera.update(&rect, &response, ui.input(|i| i.smooth_scroll_delta));

        let (mut commands, server, models, room_assets, children, preview, preview_meshes) = query;
        let room_asset = match room_assets.get(&self.handle) {
            Some(v) => v,
            None => {
                if let bevy::asset::LoadState::Failed(error) = server.load_state(&self.handle) {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("ROOM failed to load: {error}"),
                    );
                } else {
                    ui.spinner();
                }
                return;
            }
        };

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_max_width(420.0);
            ui.label("Static geometry inspection");
            ui.small("All authored ModCon layers start visible.");
            ui.small("Scripts, entity models, decals and probe lighting are not yet applied.");
            if room_asset.lightmap.is_some() {
                ui.checkbox(&mut self.use_baked_atlas, "Baked lightmap")
                    .on_hover_text("Requires Remax. Uses authored atlas UVs and white modulation. Placements without an atlas lookup retain preview lighting.");
            }
            if let Some(lightmap) = &room_asset.lightmap {
                if let bevy::asset::LoadState::Failed(error) = server.load_state(lightmap) {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Lightmap failed to load: {error}"));
                }
            }
            let visible_meshes = self
                .geometry
                .iter()
                .filter(|geometry| self.visible_layers[geometry.source.layer_index])
                .flat_map(|geometry| &geometry.models)
                .flat_map(|model| &model.loaded)
                .filter_map(|instance| children.get(instance.entity).ok())
                .flat_map(|children| children.iter())
                .map(|entity| preview_meshes.get(entity).ok());
            ui.small(preview.description(visible_meshes));
            let count: usize = self
                .geometry
                .iter()
                .flat_map(|geometry| &geometry.models)
                .map(|model| model.loaded.len())
                .sum();
            ui.label(format!("{} containers; {count} model placements", self.geometry.len()));
            if self.geometry.is_empty() {
                ui.small("This ROOM has no ordinary ModCon geometry.");
            }
            ui.collapsing("Layers", |ui| {
                for (index, layer) in room_asset.inner.layers.iter().enumerate() {
                    let count = self
                        .geometry
                        .iter()
                        .filter(|geometry| geometry.source.layer_index == index)
                        .count();
                    if count != 0 {
                        ui.checkbox(
                            &mut self.visible_layers[index],
                            format!("Layer {index} ({count} containers)"),
                        )
                        .on_hover_text(&layer.header.name);
                    }
                }
            });
            for geometry in &self.geometry {
                let name = format!(
                    "Layer {} component {}",
                    geometry.source.layer_index, geometry.source.component_index
                );
                if let Some(error) = &geometry.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}"));
                }
                if let bevy::asset::LoadState::Failed(error) =
                    server.load_state(&geometry.source.handle)
                {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}"));
                }
                for group in &geometry.models {
                    let state = server.load_state(&group.handle);
                    let state = models
                        .get(&group.handle)
                        .map_or(state, |model| model.get_load_state(&server));
                    if let bevy::asset::LoadState::Failed(error) = state {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}"));
                    }
                    if let Some(error) = &group.error {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("{name}: {error}"));
                    }
                }
            }
        });

        egui::CollapsingHeader::new("Room properties").show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(rect.height() * 0.45).show(ui, |ui| {
                if !room_asset.inner.room_header.parent_room_id.is_nil() {
                    property_with_id(
                        ui,
                        "Parent",
                        room_asset.inner.room_header.parent_room_id.into_inner(),
                    );
                }
                property_with_value(
                    ui,
                    "Room type",
                    format!("{}", room_asset.inner.room_header.room_type),
                );
                property_with_value(ui, "Unk2", format!("{}", room_asset.inner.room_header.unk2));
                property_with_value(ui, "Unk3", format!("{}", room_asset.inner.room_header.unk3));
                if !room_asset.inner.room_header.id_b.is_nil() {
                    property_with_id(ui, "ID b", room_asset.inner.room_header.id_b.into_inner());
                }
                if !room_asset.inner.room_header.id_c.is_nil() {
                    property_with_id(ui, "ID c", room_asset.inner.room_header.id_c.into_inner());
                }
                if !room_asset.inner.room_header.id_d.is_nil() {
                    property_with_id(ui, "ID d", room_asset.inner.room_header.id_d.into_inner());
                }
                if !room_asset.inner.room_header.id_e.is_nil() {
                    property_with_id(ui, "ID e", room_asset.inner.room_header.id_e.into_inner());
                }
                if !room_asset.inner.room_header.path_find_area_id.is_nil() {
                    property_with_id(
                        ui,
                        "Path Find Area",
                        room_asset.inner.room_header.path_find_area_id.into_inner(),
                    );
                }
                if let Some(light_map) = &room_asset.inner.baked_lighting.light_map {
                    ui.collapsing("Light map data", |ui| {
                        property_with_id(ui, "Texture ID", light_map.txtr_id.into_inner());
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
                    property_with_id(ui, "Light Probe", light_probe.ltpb_id.into_inner());
                }
                for (layer_idx, layer) in room_asset.inner.layers.iter().enumerate() {
                    ui.collapsing(format!("Layer {} ({})", layer_idx, layer.header.name), |ui| {
                        property_with_value(ui, "Name", layer.header.name.clone());
                        property_with_value(ui, "ID", layer.header.id.to_string());
                        for id in &layer.header.load_unit_ids {
                            property_with_value(ui, "Load unit", id.to_string());
                        }
                        property_with_value(
                            ui,
                            "Reload set",
                            layer.header.reload_set_id.to_string(),
                        );
                        property_with_value(ui, "Entities", layer.header.entity_count.to_string());
                        property_with_value(
                            ui,
                            "Generated definitions",
                            layer.header.generated_object_count.to_string(),
                        );
                        property_with_value(
                            ui,
                            "Reset on activation",
                            (layer.header.reset_on_activation != 0).to_string(),
                        );
                        for (component_idx, component) in layer.components.iter().enumerate() {
                            let property = &room_asset.inner.constructed_properties
                                [component.property_index as usize];
                            ui.collapsing(
                                if let Some(name) = &property.name {
                                    format!("Component {} ({})", component_idx, name)
                                } else {
                                    format!(
                                        "Component {} ({:#X})",
                                        component_idx, component.component_type
                                    )
                                },
                                |ui| {
                                    property_with_value(
                                        ui,
                                        "Instance index",
                                        component.instance_index.to_string(),
                                    );
                                    property_ui(ui, property);
                                },
                            );
                        }
                    });
                }
            });
        });

        let environment = EnvironmentMapLight {
            diffuse_map: self.diffuse_map.clone(),
            specular_map: self.specular_map.clone(),
            intensity: 1.0,
            ..default()
        };
        self.camera.show(&mut commands, viewport, state.render_layer, Some(environment));
        for geometry in &self.geometry {
            let visible = self.visible_layers[geometry.source.layer_index];
            for instance in geometry.models.iter().flat_map(|group| &group.loaded) {
                if let Ok(mut entity) = commands.get_entity(instance.entity) {
                    entity.insert(if visible { Visibility::Visible } else { Visibility::Hidden });
                }
                if let Ok(children) = children.get(instance.entity) {
                    for child in children.iter() {
                        if let Ok(mut entity) = commands.get_entity(child) {
                            entity.insert(RenderLayers::layer(state.render_layer));
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

fn property_ui(ui: &mut egui::Ui, property: &ConstructedProperty) {
    property_with_value(ui, "ID", format!("{:#X}", property.id));
    if let Some(name) = &property.name {
        property_with_value(ui, "Name", name.clone());
    }
    property_value_ui(ui, &property.value);
}

fn property_value_ui(ui: &mut egui::Ui, value: &ConstructedPropertyValue) {
    match value {
        ConstructedPropertyValue::Unknown(data) => {
            ui.label(format!("Unknown data (size {:#X})", data.len()));
        }
        ConstructedPropertyValue::Enum(data) => {
            property_with_value(
                ui,
                &format!("Enum {}", data.enum_name),
                data.enum_value.clone().unwrap_or_else(|| format!("{:#X}", data.value)),
            );
        }
        ConstructedPropertyValue::PropertyList(prop_list) => {
            if !prop_list.name.is_empty() {
                property_with_value(ui, "Property List", prop_list.name.to_string());
            }
            for prop in &prop_list.properties {
                ui.group(|ui| {
                    property_with_value(ui, "Property ID", format!("{:#X}", prop.id));
                    if let Some(name) = &prop.name {
                        property_with_value(ui, "Name", name.clone());
                    }
                    property_value_ui(ui, &prop.value);
                });
            }
        }
        ConstructedPropertyValue::Struct(data) => {
            if !data.name.is_empty() {
                property_with_value(ui, "Struct", data.name.to_string());
            }
            for elem in &data.elements {
                ui.group(|ui| {
                    if let Some(name) = &elem.name {
                        property_with_value(ui, "Name", name.clone());
                    }
                    property_value_ui(ui, &elem.value);
                });
            }
        }
        ConstructedPropertyValue::Typedef(data) => {
            property_with_value(ui, "Typedef ID", format!("{:#X}", data.id));
            if let Some(name) = &data.name {
                property_with_value(ui, "Typedef name", name.clone());
            }
            property_value_ui(ui, &data.value);
        }
        ConstructedPropertyValue::List(vec) => {
            for (idx, value) in vec.iter().enumerate() {
                ui.group(|ui| {
                    ui.label(format!("Item {idx}"));
                    property_value_ui(ui, value);
                });
            }
        }
        ConstructedPropertyValue::Id(id) => {
            property_with_id(ui, "ID", id.into_inner());
        }
        ConstructedPropertyValue::Color(color) => {
            property_with_value(ui, "Color", format!("{:?}", color.to_array()));
        }
        ConstructedPropertyValue::Vector(vec) => {
            property_with_value(ui, "Vector", format!("{:?}", vec.to_array()));
        }
        ConstructedPropertyValue::Bool(b) => {
            property_with_value(ui, "Bool", format!("{b}"));
        }
        ConstructedPropertyValue::I8(value) => {
            property_with_value(ui, "Int8", format!("{value}"));
        }
        ConstructedPropertyValue::I16(value) => {
            property_with_value(ui, "Int16", format!("{value}"));
        }
        ConstructedPropertyValue::I32(value) => {
            property_with_value(ui, "Int32", format!("{value}"));
        }
        ConstructedPropertyValue::I64(value) => {
            property_with_value(ui, "Int64", format!("{value}"));
        }
        ConstructedPropertyValue::U8(value) => {
            property_with_value(ui, "UInt8", format!("{value}"));
        }
        ConstructedPropertyValue::U16(value) => {
            property_with_value(ui, "UInt16", format!("{value}"));
        }
        ConstructedPropertyValue::U32(value) => {
            property_with_value(ui, "UInt32", format!("{value}"));
        }
        ConstructedPropertyValue::U64(value) => {
            property_with_value(ui, "UInt64", format!("{value}"));
        }
        ConstructedPropertyValue::F32(value) => {
            property_with_value(ui, "Float", format!("{value}"));
        }
        ConstructedPropertyValue::F64(value) => {
            property_with_value(ui, "Double", format!("{value}"));
        }
        ConstructedPropertyValue::String(value) => {
            property_with_value(ui, "String", value.clone());
        }
    }
}
