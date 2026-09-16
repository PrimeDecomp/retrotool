//! Generic preparation of selected original shaders and static model materials.
//!
//! No material-name/profile matching. Unsupported shader operations or unresolved
//! providers return diagnostics. The renderer supplies transforms, sample state,
//! texture images and GPU objects; this module never treats unknown scene data as
//! absent. Scene features can be added through their recovered binding providers.

use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Result};
use remax::{
    resources::{ConstantBank, ConstantMemory},
    wgsl::texture::TextureBinding,
};
use uuid::Uuid;
use zerocopy::LittleEndian;

use super::{
    cmdl::{EMaterialDataId, ModelData},
    material_features::GraphicsState,
    material_predicates::{MaterialSelectionState, SelectionValue},
    material_scene_bindings::{
        bind_scene_feature, SceneBindingValue, SceneMaterialInputs, SceneMaterialSnapshot,
        SceneTexture, SceneTextureView,
    },
    material_skinning::{SkinningPalette, SkinningSnapshot},
    material_texture_bindings::MaterialTextureValue,
    model_animated_uv::{AnimatedUvSnapshot, ModelAnimatedUvData},
    mtrl::MaterialPayload,
    shader_nvn::{NvnReflection, NvnShader, VariableBinding, VertexLayoutEntry},
    shader_nvn_bindings::graphics_texture_handle_offset,
    shader_nvn_globals::NvnGlobalTransforms,
    shader_nvn_graphics::{lower_graphics_with_bound_constants, GraphicsKernel, GraphicsOptions},
    shader_nvn_samples::{NvnSampleConstants, NvnSampleShading},
    shader_nvn_uniforms::{NvnUniformBuffers, NvnUniformValue, UniformBufferBytes},
    texture_sampler::{TextureSamplerBinding, TextureSamplerParams},
    txtr::{ETextureFormat, ETextureType, STextureHeader},
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

#[derive(Clone, Debug)]
pub struct ModelShaderDraw {
    pub kernel: GraphicsKernel,
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
    animated_uv_bindings: Vec<(u16, u32, u32)>,
    animated_uv_source: Option<(ModelAnimatedUvData, usize)>,
}

#[derive(Clone, Debug)]
pub struct ModelShaderConstants {
    pub vertex_compact: Vec<u32>,
    pub fragment_compact: Vec<u32>,
    pub vertex_memory: ConstantMemory,
    pub fragment_memory: ConstantMemory,
}

impl ModelShaderDraw {
    /// Full default texture views and inactive streaming (0xff), as used for an
    /// isolated model preview. Texture types/usage come from actual model/assets.
    /// Scene/global values are not inferred from absence in the static model.
    pub fn prepare(
        model: &ModelData<LittleEndian>,
        material_index: usize,
        payload: &MaterialPayload<'_>,
        technique: u8,
        sample_shading: NvnSampleShading,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
    ) -> Result<Self> {
        Self::prepare_with_scene(
            model,
            material_index,
            payload,
            technique,
            sample_shading,
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
        technique: u8,
        sample_shading: NvnSampleShading,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
    ) -> Result<Self> {
        Self::prepare_resolved(
            model,
            material_index,
            payload,
            technique,
            sample_shading,
            texture_header,
            scene,
            None,
        )
    }

    /// Resolve the mesh's first HEAD/ANUV provider at the requested time.
    /// ANUV's mapping is per mesh, so material-only preparation cannot infer it.
    /// Other data-source types and ANUV after an unparsed source stay unresolved.
    pub fn prepare_mesh_with_scene(
        model: &ModelData<LittleEndian>,
        mesh_index: usize,
        payload: &MaterialPayload<'_>,
        technique: u8,
        sample_shading: NvnSampleShading,
        texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
        uv_time_seconds: f64,
    ) -> Result<Self> {
        let mesh = model.mesh.meshes.get(mesh_index).context("Model mesh missing")?;
        let mut uv_source = None;
        let uv = if model.head.data_sources.get(4..8) == Some(b"ANUV") {
            let data = ModelAnimatedUvData::first_in_header(&model.head.data_sources)?
                .context("ANUV source missing")?;
            if data.mesh_sets.get(mesh_index) == Some(&255) {
                None
            } else {
                let mut instance = data.instance()?;
                instance.set_time(uv_time_seconds)?;
                let snapshot = data.mesh_snapshot(&instance, mesh_index)?;
                uv_source = Some((data, mesh_index));
                snapshot
            }
        } else {
            None
        };
        let mut draw = Self::prepare_resolved(
            model,
            mesh.material_idx as usize,
            payload,
            technique,
            sample_shading,
            texture_header,
            scene,
            uv.as_ref(),
        )?;
        if !draw.animated_uv_bindings.is_empty() {
            draw.animated_uv_source = uv_source;
        }
        Ok(draw)
    }

    fn prepare_resolved(
        model: &ModelData<LittleEndian>,
        material_index: usize,
        payload: &MaterialPayload<'_>,
        technique: u8,
        sample_shading: NvnSampleShading,
        mut texture_header: impl FnMut(Uuid) -> Result<Option<STextureHeader>>,
        scene: &SceneMaterialInputs<'_>,
        animated_uv: Option<&AnimatedUvSnapshot>,
    ) -> Result<Self> {
        let material =
            model.mtrl.materials.get(material_index).context("Model material missing")?;
        ensure!(payload.runtime.id == material.material_id, "Model/MTRL implementation mismatch");
        let model_selection =
            MaterialSelectionState::from_model_material(&model.mtrl, material_index, |id| {
                Ok(texture_header(id)?.map(|h| h.kind))
            })?;
        let mut selection = scene.selection_state();
        if let Some(uv) = animated_uv {
            let mut source = MaterialSelectionState::default();
            source.set(EMaterialDataId::AUXF, SelectionValue::AnimatedUv { mode: uv.mode });
            selection.append_fallback(&source);
        }
        selection.append_fallback(&model_selection);
        let selected = payload
            .select_nvn_shader(technique, &selection)?
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
        let mut textures = BTreeMap::new();
        let mut skinning_variables = Vec::new();
        let mut animated_uv_bindings = Vec::new();
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
                        _ if id == EMaterialDataId::AUXF && animated_uv.is_some() => {
                            animated_uv_bindings.push((
                                variable.packed,
                                variable.definition_option,
                                variable.binding_flags,
                            ));
                            animated_uv.unwrap()
                        }
                        _ => bail!("Unresolved ANUV provider ({id:?})"),
                    };
                    for value in snapshot
                        .binding_writes(variable.definition_option, variable.binding_flags)?
                    {
                        uniforms.write(variable.packed, u32::MAX, &value)?;
                    }
                }
            } else if variable.scene_material_kind().is_some() {
                let names = super::material_feature_semantics::semantic_names(variable.definition)
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
                                        sample_shading: sample_shading.uses_sampler_variant(),
                                    },
                                    0xff,
                                )?,
                            }
                        } else {
                            let SceneTexture::Token(token) = texture else { unreachable!() };
                            scene.texture_view(token, sampler)?
                        };
                        let Some(VariableBinding::Resource { resource }) =
                            reflection.variable(write.packed)?
                        else {
                            bail!("Scene texture semantic has no resource binding");
                        };
                        ensure!(
                            resource.kind == 0,
                            "Scene texture semantic uses nontexture resource"
                        );
                        for (stage, &slot) in resource.bindings[..3].iter().enumerate() {
                            if slot == u16::MAX {
                                continue;
                            }
                            let index =
                                u16::try_from(graphics_texture_handle_offset(u32::from(slot)) / 4)?;
                            let mut binding = texture_binding_from_view(
                                index,
                                view.kind,
                                view.format,
                                &view.sampler,
                            )?;
                            binding.sampling = view
                                .mip_level_count
                                .map(|count| texture_sampling_from_sampler(count, &view.sampler))
                                .transpose()?;
                            textures.insert((stage, index), ModelShaderTexture {
                                stage,
                                source: ModelShaderTextureSource::Scene(texture),
                                format: view.format,
                                binding,
                                sampler: view.sampler,
                            });
                        }
                    }
                }
            } else if let Some(request) = variable.material_uniform_binding()? {
                let id = EMaterialDataId::from_fourcc(request.data_id)?;
                let data = scene
                    .uniform_data(id)
                    .or_else(|| values.iter().find(|v| v.data_id == id).copied())
                    .with_context(|| format!("Missing model uniform {}", request.data_id))?;
                uniforms.write(variable.packed, u32::MAX, &request.value_from_data(data)?)?;
            } else if let Some(request) = variable.material_texture_binding()? {
                let id = EMaterialDataId::from_fourcc(request.data_id)?;
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
                                sample_shading: sample_shading.uses_sampler_variant(),
                            },
                            0xff,
                        )?;
                        let Some(VariableBinding::Resource { resource }) =
                            reflection.variable(variable.packed)?
                        else {
                            bail!("Texture semantic has no resource binding");
                        };
                        ensure!(resource.kind == 0, "Texture semantic uses nontexture resource");
                        for (stage, &slot) in resource.bindings[..3].iter().enumerate() {
                            if slot == u16::MAX {
                                continue;
                            }
                            let index =
                                u16::try_from(graphics_texture_handle_offset(u32::from(slot)) / 4)?;
                            let binding = texture_binding_from_header(index, &header, &sampler)?;
                            textures.insert((stage, index), ModelShaderTexture {
                                stage,
                                source: ModelShaderTextureSource::Asset(token.id),
                                format: header.format,
                                binding,
                                sampler: sampler.clone(),
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
            } else if variable.packed != u16::MAX {
                bail!(
                    "Unresolved {}.{} provider ({})",
                    variable.definition,
                    variable.semantic,
                    variable.data_ids.join(",")
                );
            }
        }
        let vertex = NvnShader::parse(
            backend
                .sources
                .get(selected.programs[0] as usize)
                .context("Vertex source missing")?
                .binary,
        )?;
        let fragment = NvnShader::parse(
            backend
                .sources
                .get(selected.programs[1] as usize)
                .context("Pixel source missing")?
                .binary,
        )?;
        let vp = vertex.programs()?;
        let fp = fragment.programs()?;
        let vertex = vp.iter().find(|p| p.stage.kind == 0).context("Vertex stage missing")?;
        let fragment = fp.iter().find(|p| p.stage.kind == 1).context("Pixel stage missing")?;
        let vt = textures.values().filter(|t| t.stage == 0).map(|t| t.binding).collect::<Vec<_>>();
        let ft = textures.values().filter(|t| t.stage == 1).map(|t| t.binding).collect::<Vec<_>>();
        let vertex_memory = uniforms.checked_graphics_memory(0, &[ConstantBank {
            bank: 1,
            bytes: vertex.constant_data,
            known: None,
        }])?;
        let fragment_memory = uniforms.checked_graphics_memory(1, &[ConstantBank {
            bank: 1,
            bytes: fragment.constant_data,
            known: None,
        }])?;
        let kernel = lower_graphics_with_bound_constants(
            vertex,
            fragment,
            GraphicsOptions { sample_shading, ..Default::default() },
            &vt,
            &ft,
            [&vertex_memory, &fragment_memory],
        )?;
        let transform_buffers = reflection
            .constant_buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.flags & 1 != 0)
            .map(|(i, _)| i)
            .collect();
        Ok(Self {
            kernel,
            vertex_layout: reflection.vertex_layout,
            textures: textures.into_values().collect(),
            technique_state: selected.technique_state,
            override_state: selected.override_state,
            permutation: selected.identifier.permutation,
            source_indices: selected.programs,
            uniforms,
            shader_constants: [vertex.constant_data.to_vec(), fragment.constant_data.to_vec()],
            transform_buffers,
            skinning_variables,
            animated_uv_bindings,
            animated_uv_source: None,
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

    /// Combine actual supplied transform/sample values with resolved material
    /// bytes and the selected stages' compiler literals. Unknown padding and
    /// driver regions remain guarded at runtime.
    pub fn constants(
        &self,
        transforms: &NvnGlobalTransforms,
        samples: NvnSampleConstants,
    ) -> Result<ModelShaderConstants> {
        self.constants_with_time(transforms, samples, None)
    }

    /// Evaluate the mesh's recovered ANUV provider at an absolute time. Updates
    /// only the selected uniform bindings; shader selection and textures remain
    /// fixed. Explicit scene ANUV snapshots keep their supplied values.
    pub fn constants_at_time(
        &self,
        transforms: &NvnGlobalTransforms,
        samples: NvnSampleConstants,
        seconds: f64,
    ) -> Result<ModelShaderConstants> {
        ensure!(seconds.is_finite(), "Nonfinite animated UV time");
        self.constants_with_time(transforms, samples, Some(seconds))
    }

    pub fn has_animated_uv(&self) -> bool { self.animated_uv_source.is_some() }

    fn constants_with_time(
        &self,
        transforms: &NvnGlobalTransforms,
        samples: NvnSampleConstants,
        seconds: Option<f64>,
    ) -> Result<ModelShaderConstants> {
        let mut uniforms = self.uniforms.clone();
        if let (Some(seconds), Some((data, mesh))) = (seconds, &self.animated_uv_source) {
            let mut instance = data.instance()?;
            instance.set_time(seconds)?;
            let snapshot = data
                .mesh_snapshot(&instance, *mesh)?
                .context("Animated UV mesh source disappeared")?;
            for &(packed, option, flags) in &self.animated_uv_bindings {
                for value in snapshot.binding_writes(option, flags)? {
                    uniforms.write(packed, u32::MAX, &value)?;
                }
            }
        }
        let transform_bytes = transforms.to_bytes();
        for &index in &self.transform_buffers {
            uniforms.buffer_mut(index)?.write(0, &transform_bytes)?;
        }
        let mut driver = UniformBufferBytes::unknown(0x640)?;
        driver.write(NvnSampleConstants::BYTE_OFFSET as u32, &samples.to_bytes())?;
        let stage_memory = |stage| {
            uniforms.checked_graphics_memory(stage, &[driver.constant_bank(0), ConstantBank {
                bank: 1,
                bytes: &self.shader_constants[stage],
                known: None,
            }])
        };
        let vertex_memory = stage_memory(0)?;
        let fragment_memory = stage_memory(1)?;
        self.kernel.vertex.validate_constant_assumptions(&vertex_memory)?;
        self.kernel.fragment.validate_constant_assumptions(&fragment_memory)?;
        Ok(ModelShaderConstants {
            vertex_compact: vertex_memory
                .compact_reads(&self.kernel.vertex.constant_reads)
                .context("vertex shader constants")?,
            fragment_compact: fragment_memory
                .compact_reads(&self.kernel.fragment.constant_reads)
                .context("fragment shader constants")?,
            vertex_memory,
            fragment_memory,
        })
    }
}
