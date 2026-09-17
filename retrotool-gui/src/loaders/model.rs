use std::collections::{hash_map::Entry, HashMap};

use anyhow::{ensure, Context, Error, Result};
use bevy::{
    asset::{io::Reader, AssetLoader, AssetPath, LoadContext, LoadState},
    image::ImageSampler,
    prelude::*,
};
use retrolib::format::{
    cmdl::{
        CMaterialDataInner, CMaterialTextureTokenData, EMaterialDataId, ModelData, SMaterialChunk,
    },
    foot::{locate_asset_id, locate_meta},
    material_features::GraphicsState,
    material_predicates::MaterialSelectionState,
    rfrm::FormDescriptor,
    texture_sampler::TextureSamplerParams,
};
use uuid::Uuid;
use zerocopy::LittleEndian;

use crate::{
    loaders::{
        material::MaterialAsset,
        package::dependency_path,
        texture::{sampler_descriptor, TextureAsset},
    },
    material::{CustomMaterial, MprSurfaceProfile},
    render::{
        convert_color,
        material_state::{mesh_draw_state, MaterialRenderState},
    },
    AssetRef,
};

#[derive(Copy, Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct MaterialKey {
    pub material_idx: usize,
    pub mesh_class: u8,
    pub mesh_mirrored: bool,
    pub mesh_two_sided: bool,
}

/// Texture identity plus per-use sampler overrides. UV selection is a shader
/// input and intentionally does not produce another image/sampler allocation.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct TextureImageKey {
    pub id: Uuid,
    pub sampler_override: [i32; 4],
}

impl From<&CMaterialTextureTokenData> for TextureImageKey {
    fn from(texture: &CMaterialTextureTokenData) -> Self {
        Self {
            id: texture.id,
            sampler_override: texture
                .usage
                .as_ref()
                .map_or([-1; 4], |u| [u.filter, u.wrap_x, u.wrap_y, u.wrap_z]),
        }
    }
}

/// Cooked forward selection from the model's static state (no game scene lights).
#[derive(Debug, Clone)]
pub struct ModelShaderSelection {
    pub name: String,
    pub runtime_id: u64,
    pub permutation: [u64; 2],
    pub material_hash: u32,
    pub shader_hash: u64,
    pub features: Vec<String>,
    pub surface_profile: MprSurfaceProfile,
    pub technique_state: GraphicsState,
    pub override_state: GraphicsState,
    pub render_state_warnings: Vec<String>,
}

#[derive(Debug, Clone, Asset, TypePath)]
pub struct ModelAsset {
    pub asset_ref: AssetRef,
    pub inner: ModelData<LittleEndian>,
    pub textures: HashMap<Uuid, Handle<TextureAsset>>,
    pub material_resources: HashMap<Uuid, Handle<MaterialAsset>>,
    pub shader_selections: HashMap<usize, Result<ModelShaderSelection, String>>,
    pub texture_images: HashMap<TextureImageKey, Handle<Image>>,
    pub materials: HashMap<MaterialKey, Handle<CustomMaterial>>,
}

#[derive(TypePath)]
pub struct ModelAssetLoader;

impl FromWorld for ModelAssetLoader {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl AssetLoader for ModelAssetLoader {
    type Asset = ModelAsset;
    type Error = Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> anyhow::Result<Self::Asset, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let bytes = bytes.as_slice();
        let id = locate_asset_id::<LittleEndian>(bytes)?;
        let meta = locate_meta::<LittleEndian>(bytes)?;
        let data = ModelData::<LittleEndian>::slice(bytes, meta)?;
        let (header, _, _) = FormDescriptor::<LittleEndian>::slice(bytes)?;
        // log::info!("Loaded model {:?}", data.head);
        // log::info!("Loaded meshes {:#?}", data.mesh);
        let mut dependencies = HashMap::<Uuid, AssetPath>::new();
        for values in std::iter::once(&data.mtrl.shared_data)
            .chain(data.mtrl.materials.iter().map(|m| &m.data))
        {
            for data in values {
                match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        if texture.id.is_nil() {
                            continue;
                        }
                        dependencies.insert(
                            texture.id,
                            dependency_path(load_context.path(), AssetRef {
                                id: texture.id,
                                kind: retrolib::format::txtr::K_FORM_TXTR,
                            })?,
                        );
                    }
                    CMaterialDataInner::LayeredTexture(texture) => {
                        for texture in &texture.textures {
                            if texture.id.is_nil() {
                                continue;
                            }
                            dependencies.insert(
                                texture.id,
                                dependency_path(load_context.path(), AssetRef {
                                    id: texture.id,
                                    kind: retrolib::format::txtr::K_FORM_TXTR,
                                })?,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        let textures =
            dependencies.iter().map(|(u, p)| (*u, load_context.load(p.clone()))).collect();
        let material_dependencies: HashMap<Uuid, AssetPath> = data
            .mtrl
            .materials
            .iter()
            .map(|m| m.material_resource_id)
            .filter(|id| !id.is_nil())
            .map(|id| {
                Ok((
                    id,
                    dependency_path(load_context.path(), AssetRef {
                        id,
                        kind: retrolib::format::FourCC(*b"MTRL"),
                    })?,
                ))
            })
            .collect::<Result<_>>()?;
        let material_resources = material_dependencies
            .iter()
            .map(|(id, path)| (*id, load_context.load(path.clone())))
            .collect();
        Ok(ModelAsset {
            asset_ref: AssetRef { id, kind: header.id },
            inner: data,
            textures,
            material_resources,
            shader_selections: default(),
            texture_images: default(),
            materials: default(),
        })
    }

    fn extensions(&self) -> &[&str] { &["cmdl", "smdl", "wmdl"] }
}

impl ModelAsset {
    pub fn get_load_state(&self, server: &AssetServer) -> LoadState {
        for texture in self.textures.values() {
            let state = server.load_state(texture.id());
            if !state.is_loaded() {
                return state;
            }
        }
        // MTRL failure retains the approximate viewer path, with a diagnostic.
        // Wait for every request to finish; one failed resource must not hide
        // another resource that is still loading.
        if self
            .material_resources
            .values()
            .any(|h| matches!(server.load_state(h), LoadState::Loading | LoadState::NotLoaded))
        {
            LoadState::Loading
        } else {
            LoadState::Loaded
        }
    }

    pub fn build_texture_images(
        &mut self,
        texture_assets: &Assets<TextureAsset>,
        images: &mut Assets<Image>,
    ) {
        for values in std::iter::once(&self.inner.mtrl.shared_data)
            .chain(self.inner.mtrl.materials.iter().map(|m| &m.data))
        {
            for data in values {
                let textures = match &data.data {
                    CMaterialDataInner::Texture(t) => std::slice::from_ref(t),
                    CMaterialDataInner::LayeredTexture(t) => &t.textures,
                    _ => continue,
                };
                for texture in textures {
                    if texture.id.is_nil() {
                        continue;
                    }
                    let key = TextureImageKey::from(texture);
                    if self.texture_images.contains_key(&key) {
                        continue;
                    }
                    let Some(asset) =
                        self.textures.get(&texture.id).and_then(|h| texture_assets.get(h))
                    else {
                        continue;
                    };
                    let base = TextureSamplerParams::from_header(&asset.inner.head.sampler_data);
                    let params = texture.usage.as_ref().map_or(base, |u| base.with_usage(u));
                    let handle = if params == base {
                        asset.texture.clone()
                    } else {
                        let Some(mut image) = images.get(&asset.texture).cloned() else {
                            continue;
                        };
                        image.sampler = ImageSampler::Descriptor(
                            sampler_descriptor(&params, asset.sampler_features).into(),
                        );
                        images.add(image)
                    };
                    self.texture_images.insert(key, handle);
                }
            }
        }
    }

    pub fn material(
        &mut self,
        key: &MaterialKey,
        assets: &mut Assets<CustomMaterial>,
        resources: &Assets<MaterialAsset>,
        textures: &Assets<TextureAsset>,
    ) -> Result<Handle<CustomMaterial>> {
        if !self.shader_selections.contains_key(&key.material_idx) {
            let selection = self
                .select_shader(key.material_idx, resources, textures)
                .map_err(|e| format!("{e:#}"));
            if let Err(reason) = &selection {
                log::warn!("Material {}: approximate shading: {reason}", key.material_idx);
            }
            self.shader_selections.insert(key.material_idx, selection);
        }
        Ok(match self.materials.entry(*key) {
            Entry::Occupied(e) => e.get().clone(),
            Entry::Vacant(e) => {
                let selection =
                    self.shader_selections.get(&key.material_idx).and_then(|s| s.as_ref().ok());
                let (material, warnings) =
                    build_material(key, &self.inner.mtrl, &self.texture_images, selection)?;
                for warning in &warnings {
                    log::warn!("Material {} render state: {warning}", key.material_idx);
                }
                if let Some(Ok(selection)) = self.shader_selections.get_mut(&key.material_idx) {
                    for warning in warnings {
                        if !selection.render_state_warnings.contains(&warning) {
                            selection.render_state_warnings.push(warning);
                        }
                    }
                }
                let handle = assets.add(material);
                e.insert(handle.clone());
                handle
            }
        })
    }

    fn select_shader(
        &self,
        index: usize,
        resources: &Assets<MaterialAsset>,
        textures: &Assets<TextureAsset>,
    ) -> Result<ModelShaderSelection> {
        let material = self.inner.mtrl.materials.get(index).context("Invalid material index")?;
        let resource = self
            .material_resources
            .get(&material.material_resource_id)
            .and_then(|h| resources.get(h))
            .context("Cooked MTRL resource unavailable")?;
        let payload = resource.inner.payload()?;
        ensure!(payload.runtime.id == material.material_id, "Model/MTRL implementation mismatch");
        let state = MaterialSelectionState::from_model_material(&self.inner.mtrl, index, |id| {
            Ok(self.textures.get(&id).and_then(|h| textures.get(h)).map(|t| t.inner.head.kind))
        })?;
        let selected = payload
            .select_nvn_shader(b'f', &state)?
            .context("No forward shader for static model state")?;
        // Evidence is specific to this resource/revision and shader. Expanding
        // this to other implementations requires comparing their instructions.
        let layered = material.material_resource_id
            == Uuid::from_u128(0x07d578a947c04a4daa69573473fb8d2c)
            && payload.runtime.hash == 0x0b8c1a78
            && selected.identifier.permutation == [0x022c7d2b63205b00, 0]
            && selected.shader_hash == 0x5627e993cc49096a
            && [("BCRL", 2), ("MTLL", 2), ("NRML", 8)].iter().all(|(id, option)| {
                selected.features.iter().any(|v| {
                    v.definition == "LAYR" && v.data_ids == [*id] && v.definition_option == *option
                })
            });
        let single = material.material_resource_id
            == Uuid::from_u128(0x5beb2ff2107f4b4681384ff85676505a)
            && payload.runtime.hash == 0x0b8c1a78
            && selected.identifier.permutation == [0x022c4d2b6b2c5a00, 0]
            && selected.shader_hash == 0x7f0637bf372a9c57
            && ["BCLR", "METL", "NMAP"].iter().all(|id| {
                selected.features.iter().any(|v| {
                    v.definition == "TEXS" && v.data_ids == [*id] && v.definition_option == 1
                })
            });
        let surface_profile = if layered {
            MprSurfaceProfile::LayeredVine
        } else if single {
            MprSurfaceProfile::SingleVine
        } else {
            MprSurfaceProfile::Approximate
        };
        let mut features = Vec::new();
        for v in &selected.features {
            let label = format!(
                "{}: {} {} = {}",
                v.feature_path.join("/"),
                v.definition,
                v.data_ids.join(","),
                v.definition_option
            );
            if !features.contains(&label) {
                features.push(label);
            }
        }
        Ok(ModelShaderSelection {
            name: selected.material_name.into(),
            runtime_id: selected.runtime_id,
            permutation: selected.identifier.permutation,
            material_hash: payload.runtime.hash,
            shader_hash: selected.shader_hash,
            features,
            surface_profile,
            technique_state: selected.technique_state,
            override_state: selected.override_state,
            render_state_warnings: Vec::new(),
        })
    }
}

fn build_material(
    key: &MaterialKey,
    materials: &SMaterialChunk,
    texture_images: &HashMap<TextureImageKey, Handle<Image>>,
    selection: Option<&ModelShaderSelection>,
) -> Result<(CustomMaterial, Vec<String>)> {
    let bound = mesh_draw_state(key.mesh_class, key.mesh_two_sided, key.mesh_mirrored)?;
    let mut state = GraphicsState::default();
    if let Some(selected) = selection {
        state.merge(&selected.technique_state)?;
    }
    state.merge(&bound)?;
    if let Some(selected) = selection {
        state.merge(&selected.override_state)?;
    }
    let mut render_state = MaterialRenderState::default();
    // The bound draw state is already live when the composed state is bound.
    // An override excluding RT0 must preserve that target's previous settings.
    let mut warnings = render_state.apply(&bound);
    let bound_blend = render_state.blend;
    warnings.extend(render_state.apply(&state));
    if render_state.color_routing() == remax::maxwell::graphics::ColorOutputRouting::DualSource {
        // The approximate surface shader supplies one color, so it cannot use
        // native secondary-color factors. Preserve the mesh-class approximation.
        render_state.blend = bound_blend;
        warnings.push(
            "Preview material cannot supply a secondary blend color; mesh-class blending retained"
                .into(),
        );
    }
    let mut out_mat = CustomMaterial {
        alpha_mode: if key.mesh_class & 1 == 0 { AlphaMode::Opaque } else { AlphaMode::Blend },
        render_state,
        ..default()
    };
    let profile = selection.map_or(MprSurfaceProfile::Approximate, |s| s.surface_profile);
    let values = materials.material_values(key.material_idx)?;
    if profile == MprSurfaceProfile::LayeredVine {
        let smoothness =
            values.iter().find(|d| d.data_id == EMaterialDataId::BLSM).and_then(|d| match d.data {
                CMaterialDataInner::Scalar(v) => Some(v),
                _ => None,
            });
        let scales = values.iter().find(|d| d.data_id == EMaterialDataId::BSAO).and_then(|d| {
            match &d.data {
                CMaterialDataInner::Color(v) => Some(Vec4::from_array(v.to_array())),
                _ => None,
            }
        });
        if let (Some(s), Some(k)) = (smoothness, scales) {
            if s.is_finite() && s > 0.0 && k.is_finite() {
                out_mat.surface_profile = profile;
                out_mat.layer_smoothness = s;
                out_mat.layer_scales = k;
            }
        }
    } else {
        out_mat.surface_profile = profile;
    }
    for data in values {
        match data.data_id {
            EMaterialDataId::DIFT | EMaterialDataId::BCLR => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.base_color_l0 = LinearRgba::WHITE;
                    out_mat.base_color_texture_0 =
                        texture_images.get(&TextureImageKey::from(texture)).cloned();
                    out_mat.base_color_uv_0 =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for DIFT {:?}", data.data_type);
                }
            },
            EMaterialDataId::BCRL => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.base_color_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.base_color_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.base_color_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.base_color_texture_0 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[0])).cloned();
                    out_mat.base_color_texture_1 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[1])).cloned();
                    out_mat.base_color_texture_2 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[2])).cloned();
                    out_mat.base_color_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.base_color_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.base_color_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for BCRL {:?}", data.data_type);
                }
            },
            EMaterialDataId::DIFC => match &data.data {
                CMaterialDataInner::Color(color) => {
                    out_mat.base_color = convert_color(color);
                }
                _ => log::warn!("Unsupported material data type for DIFC {:?}", data.data_type),
            },
            EMaterialDataId::ICAN => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.emissive_texture =
                        texture_images.get(&TextureImageKey::from(texture)).cloned();
                    out_mat.emissive_uv =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => log::warn!("Unsupported material data type for ICAN {:?}", data.data_type),
            },
            EMaterialDataId::ICNC => match &data.data {
                CMaterialDataInner::Color(color) => {
                    out_mat.emissive_color = convert_color(color);
                }
                _ => log::warn!("Unsupported material data type for ICNC {:?}", data.data_type),
            },
            EMaterialDataId::ICMC => match &data.data {
                CMaterialDataInner::Color(color) => {
                    out_mat.emissive_unmasked_color = convert_color(color);
                }
                _ => log::warn!("Unsupported material data type for ICMC {:?}", data.data_type),
            },
            EMaterialDataId::NMAP => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.normal_map_l0 = LinearRgba::WHITE;
                    out_mat.normal_map_texture_0 =
                        texture_images.get(&TextureImageKey::from(texture)).cloned();
                    out_mat.normal_map_uv_0 =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for NMAP {:?}", data.data_type);
                }
            },
            EMaterialDataId::NRML => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.normal_map_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.normal_map_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.normal_map_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.normal_map_texture_0 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[0])).cloned();
                    out_mat.normal_map_texture_1 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[1])).cloned();
                    out_mat.normal_map_texture_2 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[2])).cloned();
                    out_mat.normal_map_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.normal_map_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.normal_map_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for NRML {:?}", data.data_type);
                }
            },
            EMaterialDataId::METL => match &data.data {
                CMaterialDataInner::Texture(texture) => {
                    out_mat.metallic_map_l0 = LinearRgba::WHITE;
                    out_mat.metallic_map_texture_0 =
                        texture_images.get(&TextureImageKey::from(texture)).cloned();
                    out_mat.metallic_map_uv_0 =
                        texture.usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for METL {:?}", data.data_type);
                }
            },
            EMaterialDataId::MTLL => match &data.data {
                CMaterialDataInner::LayeredTexture(layers) => {
                    out_mat.metallic_map_l0 = convert_color(&layers.base.colors[0]);
                    out_mat.metallic_map_l1 = convert_color(&layers.base.colors[1]);
                    out_mat.metallic_map_l2 = convert_color(&layers.base.colors[2]);
                    out_mat.metallic_map_texture_0 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[0])).cloned();
                    out_mat.metallic_map_texture_1 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[1])).cloned();
                    out_mat.metallic_map_texture_2 =
                        texture_images.get(&TextureImageKey::from(&layers.textures[2])).cloned();
                    out_mat.metallic_map_uv_0 =
                        layers.textures[0].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.metallic_map_uv_1 =
                        layers.textures[1].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                    out_mat.metallic_map_uv_2 =
                        layers.textures[2].usage.as_ref().map(|u| u.tex_coord).unwrap_or_default();
                }
                _ => {
                    log::warn!("Unsupported material data type for MTLL {:?}", data.data_type);
                }
            },
            EMaterialDataId::LITS => {
                if let CMaterialDataInner::Scalar(value) = data.data {
                    if value.is_finite() {
                        out_mat.light_scale = value;
                    }
                }
            }
            EMaterialDataId::BLSM | EMaterialDataId::BSAO => {}
            id => {
                log::warn!("Unsupported material data ID {id:?}");
            }
        }
    }
    Ok((out_mat, warnings))
}
