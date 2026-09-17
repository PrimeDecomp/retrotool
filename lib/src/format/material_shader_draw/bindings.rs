//! Material selection and runtime bindings, independent of shader translation.
//!
//! No material-name/profile matching. Unresolved providers return diagnostics.
//! The renderer supplies transforms, sample state,
//! texture images and GPU objects; this module never treats unknown scene data as
//! absent. Scene features can be added through their recovered binding providers.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Result};
use remax::resources::{texture::TextureBinding, ConstantBank, ConstantMemory};
use uuid::Uuid;
use zerocopy::LittleEndian;

use super::{
    super::{
        cmdl::{EMaterialDataId, ModelData},
        material_features::GraphicsState,
        material_predicates::{MaterialSelectionState, SelectionValue},
        material_scene_bindings::{
            bind_scene_feature, SceneBindingValue, SceneMaterialInputs, SceneMaterialSnapshot,
            SceneTexture, SceneTextureView,
        },
        material_skinning::{SkinningPalette, SkinningSnapshot},
        material_texture_bindings::MaterialTextureValue,
        material_uniform_bindings::MaterialUniformKind,
        mtrl::MaterialPayload,
        shader_nvn::{NvnReflection, NvnShader, VariableBinding, VertexLayoutEntry},
        shader_nvn_program::NvnProgram,
        shader_nvn_samples::{NvnSampleConstants, NvnSampleShading},
        shader_nvn_uniforms::{NvnUniformBuffers, NvnUniformValue, UniformBufferBytes},
        texture_sampler::{TextureSamplerBinding, TextureSamplerParams},
        txtr::{ETextureFormat, ETextureType, STextureHeader},
    },
    animation::{MeshAnimation, MeshSnapshots, UvBinding},
    frame::{DrawUniformBinding, ModelShaderFrame},
};
use crate::format::shader_nvn_texture::{
    texture_binding_from_header, texture_binding_from_view, texture_sampling_from_sampler,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelShaderTextureSource {
    Asset(Uuid),
    Scene(SceneTexture),
}

impl ModelShaderTextureSource {
    pub fn asset_id(self) -> Option<Uuid> {
        match self {
            Self::Asset(id) => Some(id),
            Self::Scene(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelShaderTexture {
    pub stage: usize,
    pub source: ModelShaderTextureSource,
    pub format: ETextureFormat,
    pub binding: TextureBinding,
    pub sampler: TextureSamplerParams,
}

/// Material technique and NVN program variant used during selection and binding.
#[derive(Clone, Copy, Debug)]
pub struct ModelShaderOptions {
    pub technique: u8,
    /// Program frequency override, independent of the render target sample count.
    pub sample_shading: NvnSampleShading,
}

/// Selected native programs and resolved material inputs, before translation.
#[derive(Clone, Debug)]
pub struct ModelShaderBindings {
    shaders: [NvnShader; 2],
    sample_shading: NvnSampleShading,
    pub vertex_layout: Vec<VertexLayoutEntry>,
    pub textures: Vec<ModelShaderTexture>,
    pub technique_state: GraphicsState,
    pub override_state: GraphicsState,
    pub permutation: [u64; 2],
    pub source_indices: [i32; 4],
    pub uniforms: NvnUniformBuffers,
    /// Selected vertex/pixel compiler literals; each stage has its own bank1.
    shader_constants: [Vec<u8>; 2],
    transform_buffers: Vec<usize>,
    skinning_variables: Vec<u16>,
    animation: MeshAnimation,
    wind_bounds_padding: [f32; 3],
    draw_uniforms: Vec<DrawUniformBinding>,
}

#[derive(Clone, Debug)]
pub struct ModelShaderMemory {
    pub vertex: ConstantMemory,
    pub fragment: ConstantMemory,
}

impl ModelShaderBindings {
    /// Borrow the selected native vertex/pixel programs without invoking a
    /// translator. Returned code and literal slices belong to these bindings.
    pub fn programs(&self) -> Result<[NvnProgram<'_>; 2]> {
        let program = |stage: usize| {
            self.shaders[stage]
                .programs()?
                .into_iter()
                .find(|program| program.stage.kind == stage as u32)
                .context("Selected graphics stage missing")
        };
        Ok([program(0)?, program(1)?])
    }

    pub fn sample_shading(&self) -> NvnSampleShading { self.sample_shading }

    /// Full default texture views and inactive streaming (0xff), as used for an
    /// isolated model preview. Texture types/usage come from actual model/assets.
    /// Scene/global values are not inferred from absence in the static model.
    pub fn prepare(
        model: &ModelData<LittleEndian>,
        material_index: usize,
        payload: &MaterialPayload<'_>,
        options: ModelShaderOptions,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
    ) -> Result<Self> {
        Self::prepare_with_scene(
            model,
            material_index,
            payload,
            options,
            texture_header,
            &SceneMaterialInputs::default(),
        )
    }

    /// Explicit draw-context snapshots participate in selection and binding.
    /// Missing source entries remain unresolved. Native cache images have known
    /// descriptors; other scene textures require supplied view/sampler metadata.
    /// The renderer maps each source identity to the matching actual GPU view.
    pub fn prepare_with_scene(
        model: &ModelData<LittleEndian>,
        material_index: usize,
        payload: &MaterialPayload<'_>,
        options: ModelShaderOptions,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
    ) -> Result<Self> {
        Self::prepare_resolved(
            model,
            material_index,
            payload,
            options,
            texture_header,
            scene,
            &MeshSnapshots::default(),
        )
    }

    /// Resolve known HEAD providers in render-mesh order at the requested time.
    /// Unknown unframed source records stop traversal. Scene snapshots override
    /// model providers; model wind needs an explicitly supplied environment.
    pub fn prepare_mesh_with_scene(
        model: &ModelData<LittleEndian>,
        mesh_index: usize,
        payload: &MaterialPayload<'_>,
        options: ModelShaderOptions,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
        seconds: f64,
    ) -> Result<Self> {
        let mesh = model.mesh.meshes.get(mesh_index).context("Model mesh missing")?;
        let mut animation = MeshAnimation::parse(&model.head.data_sources, mesh_index, scene)?;
        let snapshots = animation.snapshots(seconds)?;
        let mut draw = Self::prepare_resolved(
            model,
            mesh.material_idx as usize,
            payload,
            options,
            texture_header,
            scene,
            &snapshots,
        )?;
        animation.uv_bindings = std::mem::take(&mut draw.animation.uv_bindings);
        animation.wind_bindings = std::mem::take(&mut draw.animation.wind_bindings);
        animation.retain_bound_sources();
        draw.animation = animation;
        Ok(draw)
    }

    fn prepare_resolved(
        model: &ModelData<LittleEndian>,
        material_index: usize,
        payload: &MaterialPayload<'_>,
        options: ModelShaderOptions,
        mut texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
        snapshots: &MeshSnapshots,
    ) -> Result<Self> {
        let material =
            model.mtrl.materials.get(material_index).context("Model material missing")?;
        ensure!(payload.runtime.id == material.material_id, "Model/MTRL implementation mismatch");
        let model_selection =
            MaterialSelectionState::from_model_material(&model.mtrl, material_index, |id| {
                Ok(texture_header(id)?.map(|h| h.kind))
            })?;
        let mut selection = scene.selection_state();
        if let Some(uv) = &snapshots.uv {
            let mut source = MaterialSelectionState::default();
            source.set(EMaterialDataId::AUXF, SelectionValue::AnimatedUv { mode: uv.mode });
            selection.append_fallback(&source);
        }
        if snapshots.wind.is_some() {
            let mut source = MaterialSelectionState::default();
            source.set(EMaterialDataId::WIND, SelectionValue::Present);
            selection.append_fallback(&source);
        }
        selection.append_fallback(&model_selection);
        let selected = payload
            .select_nvn_shader(options.technique, &selection)?
            .context("No shader for model/technique state")?;
        ensure!(
            selected.programs[0] >= 0
                && selected.programs[1] >= 0
                && selected.programs[2] < 0
                && selected.programs[3] < 0,
            "Model draw requires a vertex/pixel shader pair"
        );
        let backend = payload
            .archive
            .backends
            .iter()
            .find(|b| b.id.0 == *b"SNVN")
            .context("NVN backend missing")?;
        let implementation = backend
            .materials
            .iter()
            .find(|m| m.id == payload.runtime.id && m.hash == payload.runtime.hash)
            .context("NVN material implementation missing")?;
        let shader = implementation
            .shaders
            .iter()
            .find(|s| s.identifier == selected.identifier)
            .context("Selected shader missing")?;
        let reflection = NvnReflection::parse(shader.metadata)?;
        let mut uniforms = NvnUniformBuffers::new(&reflection)?;
        let values = model.mtrl.material_values(material_index)?;
        let bounds = model.mtrl.backlight_bounds(&model.head.bounds);
        let mut textures = BTreeMap::new();
        let mut skinning_variables = Vec::new();
        let mut draw_uniforms = Vec::new();
        let mut animation = MeshAnimation::default();
        let mut wind_bounds_padding = [0.0; 3];
        let mut next_variable = 0;
        for (index, variable) in selected.variables.iter().enumerate() {
            if index < next_variable {
                continue;
            }
            if variable.definition == "ANUV" && matches!(variable.definition_option, 1 | 2) {
                // Native option1 binds semantic0 as one or three matrices;
                // option2 binds only semantic1 as the first matrix's U offset.
                let active = match variable.semantic {
                    "TexCoordTransform" => variable.definition_option == 1,
                    "TexCoordTransU" => variable.definition_option == 2,
                    _ => bail!("Unknown ANUV semantic"),
                };
                if active {
                    let id = EMaterialDataId::from_fourcc(
                        variable.data_ids.first().context("ANUV needs a data ID")?,
                    )?;
                    let snapshot = match scene.get(id) {
                        Ok(Some(SceneMaterialSnapshot::AnimatedUv(snapshot))) => snapshot,
                        Ok(Some(_)) => bail!("Wrong ANUV material data"),
                        _ if id == EMaterialDataId::AUXF && snapshots.uv.is_some() => {
                            animation.uv_bindings.push(UvBinding {
                                packed: variable.packed,
                                option: variable.definition_option,
                                flags: variable.binding_flags,
                            });
                            snapshots.uv.as_ref().unwrap()
                        }
                        _ => bail!("Unresolved ANUV provider ({id:?})"),
                    };
                    for value in snapshot
                        .binding_writes(variable.definition_option, variable.binding_flags)?
                    {
                        uniforms.write(variable.packed, u32::MAX, &value)?;
                    }
                }
            } else if variable.definition == "WIND" && variable.definition_option == 1 {
                ensure!(variable.semantic == "WindAnimData", "Unknown WIND semantic");
                let id = EMaterialDataId::from_fourcc(
                    variable.data_ids.first().context("WIND needs a data ID")?,
                )?;
                let snapshot = match scene.get(id) {
                    Ok(Some(SceneMaterialSnapshot::Wind(snapshot))) => snapshot,
                    Ok(Some(_)) => bail!("Wrong WIND material data"),
                    _ if id == EMaterialDataId::WIND && snapshots.wind.is_some() => {
                        animation.wind_bindings.push(variable.packed);
                        snapshots.wind.as_ref().unwrap()
                    }
                    _ => bail!("Unresolved WIND provider ({id:?})"),
                };
                wind_bounds_padding = snapshot.bounds_padding;
                let data = SceneMaterialSnapshot::Wind(*snapshot);
                let result = bind_scene_feature(std::slice::from_ref(variable), Some(&data))?
                    .context("Inactive WIND feature")?;
                ensure!(result.success, "WIND binding failed");
                for write in result.writes {
                    uniforms.write(
                        variable.packed,
                        u32::MAX,
                        &write.uniform_value().context("Invalid WIND uniform write")?,
                    )?;
                }
            } else if variable.scene_material_kind().is_some() {
                let names =
                    super::super::material_feature_semantics::semantic_names(variable.definition)
                        .context("Unknown scene feature")?;
                ensure!(variable.semantic == names[0], "Truncated scene feature variable group");
                let count = names.len();
                next_variable = index + count;
                let group = selected
                    .variables
                    .get(index..next_variable)
                    .context("Truncated scene feature variable group")?;
                let data_id = variable.data_ids.first().context("Scene feature needs a data ID")?;
                let data = scene.get(EMaterialDataId::from_fourcc(data_id)?)?;
                let result = bind_scene_feature(group, data)?.context("Inactive scene feature")?;
                ensure!(result.success, "Scene material {data_id} binding failed");
                for write in result.writes {
                    if let Some(value) = write.uniform_value() {
                        uniforms.write(write.packed, u32::MAX, &value)?;
                    } else if let SceneBindingValue::Texture { texture, sampler } = write.value {
                        let view = if let Some(cache) = texture.cache_data() {
                            ensure!(
                                sampler.is_none(),
                                "Native cache texture has no custom sampler"
                            );
                            SceneTextureView {
                                kind: ETextureType::D2,
                                format: cache.format,
                                mip_level_count: Some(1),
                                sampler: cache.sampler.for_binding(
                                    TextureSamplerBinding::Regular {
                                        sample_shading: options
                                            .sample_shading
                                            .uses_sampler_variant(),
                                    },
                                    0xff,
                                )?,
                            }
                        } else {
                            let SceneTexture::Token(token) = texture else { unreachable!() };
                            scene.texture_view(token, sampler)?
                        };
                        bind_scene_texture(
                            &mut textures,
                            &reflection,
                            write.packed,
                            texture,
                            view,
                        )?;
                    }
                }
            } else if let Some(request) = variable.material_uniform_binding()? {
                let id = EMaterialDataId::from_fourcc(request.data_id)?;
                if let Some(source) = scene.draw_uniform(id) {
                    ensure!(
                        request.kind == MaterialUniformKind::Color,
                        "Draw uniform requires FLT4"
                    );
                    draw_uniforms.push(DrawUniformBinding { packed: variable.packed, source });
                    continue;
                }
                let data = scene
                    .uniform_data(id)
                    .or_else(|| values.iter().find(|v| v.data_id == id).copied())
                    .or_else(|| bounds.as_ref().filter(|data| data.data_id == id))
                    .with_context(|| format!("Missing model uniform {}", request.data_id))?;
                uniforms.write(variable.packed, u32::MAX, &request.value_from_data(data)?)?;
            } else if let Some(request) = variable.material_texture_binding()? {
                let id = EMaterialDataId::from_fourcc(request.data_id)?;
                if let Some(texture) = scene.texture(id) {
                    ensure!(request.layer.is_none(), "Ordinary scene texture cannot supply LAYR");
                    match request.value {
                        MaterialTextureValue::TexCoordIndex => {
                            let uv =
                                texture.tex_coord.context("Scene texture UV usage unresolved")?;
                            uniforms.write(
                                variable.packed,
                                u32::MAX,
                                &NvnUniformValue::Int(uv as i32),
                            )?;
                        }
                        MaterialTextureValue::Texture { bindless: false } => {
                            bind_scene_texture(
                                &mut textures,
                                &reflection,
                                variable.packed,
                                SceneTexture::Token(texture.token),
                                texture.view,
                            )?;
                        }
                        MaterialTextureValue::Texture { bindless: true } => {
                            bail!("Bindless scene texture provider not connected")
                        }
                        MaterialTextureValue::SizeParams => {
                            bail!("Native texture size reciprocal provider not connected")
                        }
                    }
                    continue;
                }
                let data = values
                    .iter()
                    .find(|v| v.data_id == id)
                    .with_context(|| format!("Missing model texture data {}", request.data_id))?;
                let token = request.texture_from_data(data)?;
                match request.value {
                    MaterialTextureValue::TexCoordIndex => {
                        let usage = token.usage.as_ref().context("Texture UV usage unresolved")?;
                        uniforms.write(
                            variable.packed,
                            u32::MAX,
                            &NvnUniformValue::Int(usage.tex_coord as i32),
                        )?;
                    }
                    MaterialTextureValue::SizeParams => {
                        bail!("Native texture size reciprocal provider not connected")
                    }
                    MaterialTextureValue::Texture { bindless: true } => {
                        bail!("Bindless texture provider not connected")
                    }
                    MaterialTextureValue::Texture { bindless: false } => {
                        ensure!(!token.id.is_nil(), "Texture token is nil");
                        let header = texture_header(token.id)?
                            .with_context(|| format!("Texture {} unavailable", token.id))?;
                        let mut sampler = TextureSamplerParams::from_header(&header.sampler_data);
                        if let Some(usage) = &token.usage {
                            sampler = sampler.with_usage(usage);
                        }
                        sampler = sampler.for_binding(
                            TextureSamplerBinding::Regular {
                                sample_shading: options.sample_shading.uses_sampler_variant(),
                            },
                            0xff,
                        )?;
                        let Some(VariableBinding::Resource { resource }) =
                            reflection.variable(variable.packed)?
                        else {
                            bail!("Texture semantic has no resource binding");
                        };
                        for (stage, index) in
                            resource.graphics_texture_word_offsets()?.into_iter().enumerate()
                        {
                            let Some(index) = index else { continue };
                            let binding = texture_binding_from_header(index, &header, &sampler)?;
                            textures.insert((stage, index), ModelShaderTexture {
                                stage,
                                source: ModelShaderTextureSource::Asset(token.id),
                                format: header.format,
                                binding,
                                sampler,
                            });
                        }
                    }
                }
            } else if let Some(request) = variable.skinning_binding()? {
                ensure!(
                    request.data_id == "SKIN",
                    "Unresolved skinning source {}",
                    request.data_id
                );
                let header =
                    model.skinned_header.as_ref().context("SKIN source needs an SMDL header")?;
                // The model preview has no animation reference. Match the
                // native freshly-created fallback palette, whose size comes
                // from SKHD, rather than inferring a size from vertex indices.
                let snapshot =
                    SkinningSnapshot::supplied(SkinningPalette::identity(header.bone_count)?);
                for value in snapshot.writes() {
                    uniforms.write(variable.packed, u32::MAX, &value)?;
                }
                skinning_variables.push(variable.packed);
            } else if variable.definition == "XFRM" && variable.definition_option == 1 {
                // Global buffer is supplied at draw time, independently of the
                // packed variable. The native callback calls global flush0.
            } else if variable.definition == "GSV_" && variable.definition_option == 1 {
                // Native callback 0x7100171c54 calls global flush1. Reflection
                // flags locate its whole buffer; the packed variable is unused.
                ensure!(variable.semantic == "Values", "Invalid global system semantic");
                scene.global_system_values.context("Global system values unresolved")?;
            } else if variable.packed != u16::MAX {
                bail!(
                    "Unresolved {}.{} provider ({})",
                    variable.definition,
                    variable.semantic,
                    variable.data_ids.join(",")
                );
            }
        }
        let vertex_shader = NvnShader::parse(
            backend
                .sources
                .get(selected.programs[0] as usize)
                .context("Vertex source missing")?
                .binary,
        )?;
        let fragment_shader = NvnShader::parse(
            backend
                .sources
                .get(selected.programs[1] as usize)
                .context("Pixel source missing")?
                .binary,
        )?;
        let vp = vertex_shader.programs()?;
        let fp = fragment_shader.programs()?;
        let vertex = vp.iter().find(|p| p.stage.kind == 0).context("Vertex stage missing")?;
        let fragment = fp.iter().find(|p| p.stage.kind == 1).context("Pixel stage missing")?;
        let shader_constants = [vertex.constant_data.to_vec(), fragment.constant_data.to_vec()];
        let transform_buffers = reflection
            .constant_buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.flags & 1 != 0)
            .map(|(i, _)| i)
            .collect();
        for (index, buffer) in reflection.constant_buffers.iter().enumerate() {
            if buffer.flags & 8 != 0 {
                let bytes = scene
                    .global_system_values
                    .context("Global system values unresolved")?
                    .to_bytes();
                ensure!(buffer.size as usize == bytes.len(), "Global system buffer size mismatch");
                uniforms.buffer_mut(index)?.write(0, &bytes)?;
            }
        }
        Ok(Self {
            shaders: [vertex_shader, fragment_shader],
            sample_shading: options.sample_shading,
            vertex_layout: reflection.vertex_layout,
            textures: textures.into_values().collect(),
            technique_state: selected.technique_state,
            override_state: selected.override_state,
            permutation: selected.identifier.permutation,
            source_indices: selected.programs,
            uniforms,
            shader_constants,
            transform_buffers,
            skinning_variables,
            animation,
            wind_bounds_padding,
            draw_uniforms,
        })
    }

    /// Bind explicitly supplied animation palettes (or an instanced range) in
    /// the native order. Untouched bytes retain their previous known state, as
    /// they do in a partially updated native uniform buffer.
    pub fn bind_skinning(&mut self, snapshot: &SkinningSnapshot) -> Result<()> {
        ensure!(!self.skinning_variables.is_empty(), "Selected shader has no SKIN binding");
        for &packed in &self.skinning_variables {
            for value in snapshot.writes() {
                self.uniforms.write(packed, u32::MAX, &value)?;
            }
        }
        Ok(())
    }

    pub fn has_animated_uv(&self) -> bool { self.animation.has_animated_uv() }

    pub fn has_wind(&self) -> bool { self.animation.has_wind() }

    pub fn uses_draw_uniform(
        &self,
        source: super::super::material_scene_bindings::DrawUniform,
    ) -> bool {
        self.draw_uniforms.iter().any(|binding| binding.source == source)
    }

    /// Padding from the selected wind set and supplied environment. Applying it
    /// is the renderer owner's policy: native world models pad local bounds,
    /// while MCON ignores the callback. This is not an aggregate model bound.
    pub fn wind_bounds_padding(&self) -> [f32; 3] { self.wind_bounds_padding }

    /// Resolve one draw's constants without mutating shared prepared material
    /// state. Unknown padding/driver regions retain byte validity. Declared draw
    /// sources require their frame input only when the selected shader uses them.
    pub fn constants(&self, frame: &ModelShaderFrame<'_>) -> Result<ModelShaderMemory> {
        let mut uniforms = self.uniforms.clone();
        if let Some(seconds) = frame.animation_time {
            self.animation.write_at_time(&mut uniforms, seconds)?;
        }
        for binding in &self.draw_uniforms {
            binding.write(&mut uniforms, frame)?;
        }
        let transform_bytes = frame.transforms.to_bytes();
        for &index in &self.transform_buffers {
            uniforms.buffer_mut(index)?.write(0, &transform_bytes)?;
        }
        let mut driver = UniformBufferBytes::unknown(0x640)?;
        driver.write(NvnSampleConstants::BYTE_OFFSET as u32, &frame.samples.to_bytes())?;
        let stage_memory = |stage| {
            uniforms.checked_graphics_memory(stage, &[driver.constant_bank(0), ConstantBank {
                bank: 1,
                bytes: &self.shader_constants[stage],
                known: None,
            }])
        };
        let vertex_memory = stage_memory(0)?;
        let fragment_memory = stage_memory(1)?;
        Ok(ModelShaderMemory { vertex: vertex_memory, fragment: fragment_memory })
    }
}

/// Resolve reflected stage slots from one complete scene texture declaration.
fn bind_scene_texture(
    textures: &mut BTreeMap<(usize, u16), ModelShaderTexture>,
    reflection: &NvnReflection,
    packed: u16,
    source: SceneTexture,
    view: SceneTextureView,
) -> Result<()> {
    let Some(VariableBinding::Resource { resource }) = reflection.variable(packed)? else {
        bail!("Scene texture semantic has no resource binding");
    };
    for (stage, index) in resource.graphics_texture_word_offsets()?.into_iter().enumerate() {
        let Some(index) = index else { continue };
        let mut binding = texture_binding_from_view(index, view.kind, view.format, &view.sampler)?;
        binding.sampling = view
            .mip_level_count
            .map(|count| texture_sampling_from_sampler(count, &view.sampler))
            .transpose()?;
        textures.insert((stage, index), ModelShaderTexture {
            stage,
            source: ModelShaderTextureSource::Scene(source),
            format: view.format,
            binding,
            sampler: view.sampler,
        });
    }
    Ok(())
}
