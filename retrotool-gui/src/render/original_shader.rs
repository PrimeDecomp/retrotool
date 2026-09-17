//! Experimental Bevy draws using each material's translated original shaders.
//! Static material providers are shared with retrolib; scene providers and
//! unsupported shaders return a diagnostic before replacing the preview material.

use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

mod diagnostics;
mod pipelines;
mod program;

#[cfg(test)]
pub(super) mod tests;

use anyhow::{ensure, Context, Result};
use bevy::{
    camera::{
        primitives::{Aabb, Frustum},
        visibility::NoFrustumCulling,
    },
    core_pipeline::core_3d::{
        Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey, Transparent3d, TransparentSortingInfo3d,
    },
    ecs::{
        query::QueryItem,
        system::{lifetimeless::*, SystemParamItem},
    },
    prelude::*,
    render::{
        batching::NoAutomaticBatching,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
            PhaseItemExtraIndex, RenderCommand, RenderCommandResult, TrackedRenderPass,
            ViewBinnedRenderPhases, ViewSortedRenderPhases,
        },
        render_resource::{
            BindGroup, BindGroupLayout, Buffer, CachedRenderPipelineId, Texture, TextureView,
        },
        renderer::{RenderDevice, RenderQueue},
        sync_component::SyncComponent,
        sync_world::MainEntity,
        texture::GpuImage,
        view::{ExtractedView, RenderVisibleEntities, ViewTarget},
        Render, RenderApp, RenderSystems,
    },
};
use diagnostics::{Diagnostics, FaultCapture};
use pipelines::{MaterialPipelines, PipelineRequest, ViewKey};
pub use program::Compiler;
use program::MaterialProgram;
use remax::{
    maxwell::texture::TextureDimension,
    wgsl::texture::{TextureBinding, TextureValueType},
};
use retrolib::format::{
    material_scene_bindings::{SceneMaterialInputs, SceneTexture},
    material_shader_draw::{
        ModelShaderBindings, ModelShaderFrame, ModelShaderOptions, ModelShaderTextureSource,
    },
    model_shader_inputs::{model_mesh_indices, model_vertex_inputs},
    shader_nvn_globals::NvnGlobalTransforms,
    shader_nvn_samples::{NvnSampleShading, NvnSampleState},
    shader_nvn_texture::texture_sampling_from_sampler,
    txtr::ETextureFormat,
};

use super::material_state::{mesh_draw_state, MaterialRenderState};
use crate::loaders::{
    material::MaterialAsset,
    model::ModelAsset,
    texture::{sampler_descriptor, TextureAsset},
};

/// Each generated draw owns bindings outside Bevy's mesh/material batches.
/// In particular, transparent batch sets must not merge distinct programs just
/// because their phase items share our custom draw function and cache-ID sentinel.
/// Frustum culling is performed per view with authored model bounds. Bevy's
/// undeformed per-mesh culling must not reject these draws first.
#[derive(Component, Clone)]
#[require(NoAutomaticBatching, NoFrustumCulling)]
pub struct OriginalShaderMesh {
    prepared: Arc<PreparedMesh>,
    diagnostics: Diagnostics,
    // CMDL/MCON owners cull the authored whole-model box, not the undeformed
    // vertices of an individual draw. World-model octree culling is separate.
    model_bounds: Aabb,
    pub baked_atlas_lookup: Option<retrolib::format::room::SAtlasLookup>,
}

/// Per-instance playback time. Kept outside PreparedMesh so uniform animation
/// preserves shared geometry, texture resources and compiled pipelines.
#[derive(Component, Clone, Copy, Default)]
pub struct OriginalShaderTime(pub f64);

#[derive(Clone, Debug)]
pub struct OriginalShaderInfo {
    pub sources: [i32; 4],
    pub permutation: [u64; 2],
    pub animated_uv: bool,
    pub wind: bool,
    pub baked_atlas: bool,
    pub compiler: Compiler,
    pub diagnostics: Diagnostics,
}

impl OriginalShaderMesh {
    /// Share compiled resources while keeping each scene instance's diagnostics separate.
    pub fn instance(&self) -> Self {
        Self {
            prepared: self.prepared.clone(),
            diagnostics: Diagnostics::new(self.prepared.draw.has_faults()),
            model_bounds: self.model_bounds,
            baked_atlas_lookup: self.baked_atlas_lookup,
        }
    }

    pub fn info(&self) -> OriginalShaderInfo {
        OriginalShaderInfo {
            sources: self.prepared.draw.bindings.source_indices,
            permutation: self.prepared.draw.bindings.permutation,
            animated_uv: self.prepared.draw.bindings.has_animated_uv(),
            wind: self.prepared.draw.bindings.has_wind(),
            baked_atlas: self.prepared.draw.bindings.uses_draw_uniform(
                retrolib::format::material_scene_bindings::DrawUniform::BakedAtlasLookup,
            ),
            compiler: self.prepared.compiler,
            diagnostics: self.diagnostics.clone(),
        }
    }
}

struct PreparedMesh {
    draw: MaterialProgram,
    compiler: Compiler,
    vertices: Vec<[u32; 256]>,
    indices: Vec<u32>,
    images: Vec<PreparedImage>,
    state: MaterialRenderState,
}

enum PreparedImage {
    Image(Handle<Image>),
    Cache(SceneTexture),
}

/// Compiler choice and actual enabled host capabilities.
#[derive(Clone, Copy)]
pub struct ShaderOptions {
    pub compiler: Compiler,
    pub features: wgpu::Features,
    pub backend: wgpu::Backend,
}

/// Resolve material inputs, compile and validate resources before replacing the
/// preview material. Runtime tokens name caller-supplied complete texture views.
#[allow(clippy::too_many_arguments)]
pub fn prepare_mesh_with_scene(
    model: &ModelAsset,
    mesh_index: usize,
    mirrored: bool,
    materials: &Assets<MaterialAsset>,
    textures: &Assets<TextureAsset>,
    scene: &SceneMaterialInputs<'_>,
    options: ShaderOptions,
    mut scene_image: impl FnMut(u64) -> Result<Handle<Image>>,
) -> Result<OriginalShaderMesh> {
    let model_bounds = super::convert_aabb(&model.inner.head.bounds);
    ensure!(
        model_bounds.center.is_finite()
            && model_bounds.half_extents.is_finite()
            && model_bounds.half_extents.min_element() >= 0.,
        "Invalid authored model bounds"
    );
    if matches!(options.compiler, Compiler::Remax(_)) {
        ensure!(
            options.backend == wgpu::Backend::Metal,
            "Remax helper-aware graphics diagnostics have only been established on Metal"
        );
        ensure!(
            options
                .features
                .contains(wgpu::Features::SUBGROUP | wgpu::Features::VERTEX_WRITABLE_STORAGE),
            "Remax graphics diagnostics require subgroup operations and vertex storage writes"
        );
    }
    let mesh = model.inner.mesh.meshes.get(mesh_index).context("Missing mesh")?;
    let material = &model.inner.mtrl.materials[mesh.material_idx as usize];
    let resource = model
        .material_resources
        .get(&material.material_resource_id)
        .and_then(|h| materials.get(h))
        .context("Material archive unavailable")?;
    let bindings = ModelShaderBindings::prepare_mesh_with_scene(
        &model.inner,
        mesh_index,
        &resource.inner.payload()?,
        ModelShaderOptions { technique: b'f', sample_shading: NvnSampleShading::ShaderDefault },
        |id| {
            Ok(model.textures.get(&id).and_then(|h| textures.get(h)).map(|t| t.inner.head.clone()))
        },
        scene,
        0.0, // Baseline; extracted instance time updates material uniforms at draw time.
    )?;
    let bound = mesh_draw_state(
        model.inner.mesh.mesh_class(mesh_index).context("Missing mesh class")?,
        model.inner.mesh.is_two_sided(mesh_index).context("Missing two-sided flag")?,
        mirrored,
    )?;
    let mut merged = bindings.technique_state.clone();
    merged.merge(&bound)?;
    merged.merge(&bindings.override_state)?;
    let mut state = MaterialRenderState::default();
    let mut warnings = state.apply(&bound);
    warnings.extend(state.apply(&merged));
    ensure!(warnings.is_empty(), "Original shader draw state: {}", warnings.join("; "));
    let color_routing = state.color_routing();
    if color_routing == remax::maxwell::graphics::ColorOutputRouting::DualSource {
        ensure!(
            options.features.contains(wgpu::Features::DUAL_SOURCE_BLENDING),
            "Original shader requires the DualSourceBlending GPU feature"
        );
    }
    let sources = bindings.source_indices;
    let draw = MaterialProgram::compile(bindings, options.compiler, color_routing)
        .with_context(|| format!("VS source {}, PS source {}", sources[0], sources[1]))?;
    ensure!(
        !draw.uses_clip_distances() || options.features.contains(wgpu::Features::CLIP_DISTANCES),
        "Original shader requires the ClipDistances GPU feature (unavailable on wgpu 29 Metal)"
    );
    let images = draw
        .bindings
        .textures
        .iter()
        .map(|t| {
            ensure!(
                matches!(
                    t.binding.dimension,
                    TextureDimension::D2
                        | TextureDimension::D2Array
                        | TextureDimension::D3
                        | TextureDimension::Cube
                ) && matches!(
                    t.binding.value_type,
                    TextureValueType::Float | TextureValueType::Uint | TextureValueType::Sint
                ) && !t.binding.multisample,
                "Original shader viewer has no adapter for {:?} {:?} (multisample: {})",
                t.binding.dimension,
                t.binding.value_type,
                t.binding.multisample
            );
            ensure!(
                t.sampler.wrap.iter().all(|w| matches!(w, 0..=2 | 4)),
                "Original shader viewer cannot reproduce this texture wrap mode"
            );
            if t.sampler.wrap.contains(&4) {
                ensure!(
                    options.features.contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER),
                    "Original shader viewer requires the ClampToBorder GPU feature"
                );
                ensure!(
                    matches!(t.sampler.border, -1..=2),
                    "Original shader viewer cannot reproduce border color {}",
                    t.sampler.border
                );
            }
            ensure!(
                !matches!(
                    t.format,
                    ETextureFormat::R32Float
                        | ETextureFormat::Rg32Float
                        | ETextureFormat::Rgba32Float
                ),
                "Original shader viewer needs an unfilterable float texture adapter"
            );
            ensure!(
                t.sampler.max_anisotropy() <= 1
                    || (t.sampler.filter == 1 && t.sampler.mip_filter == 1),
                "Host anisotropic sampler requires linear min/mag/mip filters"
            );
            if let Some(sampling) = t.binding.sampling {
                ensure!(
                    sampling
                        == texture_sampling_from_sampler(sampling.mip_level_count, &t.sampler)?,
                    "Original shader sampler disagrees with its sampling declaration"
                );
            }
            if let ModelShaderTextureSource::Scene(source) = t.source {
                return Ok(match source {
                    SceneTexture::Token(token) => PreparedImage::Image(scene_image(token)?),
                    SceneTexture::White | SceneTexture::Zero => PreparedImage::Cache(source),
                });
            }
            let id = t.source.asset_id().context("Missing texture asset identity")?;
            // The current TXTR loader may convert integer formats to float
            // images. Keep asset integers explicit until that path is verified.
            ensure!(
                t.binding.value_type == TextureValueType::Float,
                "Original shader viewer needs a typed integer TXTR loader"
            );
            let texture = model
                .textures
                .get(&id)
                .and_then(|h| textures.get(h))
                .context("Texture unavailable")?;
            Ok(PreparedImage::Image(texture.texture.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    let vertices = model_vertex_inputs(
        &model.inner,
        mesh.vtx_buf_idx as usize,
        &draw.bindings.vertex_layout,
        draw.attributes(),
    )?;
    let indices = model_mesh_indices(&model.inner, mesh_index)?;
    ensure!(!vertices.is_empty() && !indices.is_empty(), "Empty original shader draw");
    // Validate direct constant availability before replacing a preview material.
    // Indexed reads retain byte validity and fault rather than using unknown data.
    let transforms = preview_transforms(Mat4::IDENTITY, Mat4::IDENTITY, Mat4::IDENTITY);
    draw.constants(&ModelShaderFrame {
        animation_time: Some(0.0),
        cull_camera_position: Some([0.0; 3]),
        // Structural preflight only. Every actual draw supplies its own lookup.
        baked_atlas_lookup: Some(retrolib::format::room::SAtlasLookup([0., 0., 1., 0.].into())),
        ..ModelShaderFrame::new(&transforms, sample_state(1)?.constants)
    })?;
    Ok(OriginalShaderMesh {
        diagnostics: Diagnostics::new(draw.has_faults()),
        model_bounds,
        baked_atlas_lookup: None,
        prepared: Arc::new(PreparedMesh {
            draw,
            compiler: options.compiler,
            vertices,
            indices,
            images,
            state,
        }),
    })
}

/// Explicit viewer matrices in the recovered uc_globalTransforms upload layout.
/// Bevy's projection supplies host clip space and reversed depth. Baked/custom
/// transforms are preview inputs, not reconstructed game scene state.
pub fn preview_transforms(model: Mat4, view: Mat4, projection: Mat4) -> NvnGlobalTransforms {
    let rows = |m: Mat4| {
        let c = m.transpose().to_cols_array_2d();
        [c[0], c[1], c[2]]
    };
    let model_view = view * model;
    let mut normal = rows(model_view.inverse().transpose());
    for row in &mut normal {
        row[3] = 0.0;
    }
    NvnGlobalTransforms {
        model_to_clip_columns: (projection * model_view).to_cols_array_2d(),
        model_view_inverse_transpose_rows: normal,
        model_to_view_rows: rows(model_view),
        model_to_world_rows: rows(model),
        world_to_model_rows: rows(model.inverse()),
        world_to_view_rows: rows(view),
        baked_to_view_rows: rows(view),
        projection_columns: projection.to_cols_array_2d(),
        inverse_projection_columns: projection.inverse().to_cols_array_2d(),
        custom0_columns: Mat4::IDENTITY.to_cols_array_2d(),
    }
}

fn sample_state(count: u32) -> Result<retrolib::format::shader_nvn_samples::NvnSampleLayout> {
    let count = u8::try_from(count)?;
    NvnSampleState {
        samples: count,
        raster_samples: count,
        enabled: count > 1,
        custom_locations: None,
        grid: false,
    }
    .resolve()
}

#[derive(Component)]
pub struct ExtractedOriginalMesh {
    mesh: Arc<PreparedMesh>,
    asset_id: bevy::asset::UntypedAssetId,
    transform: Mat4,
    animation_seconds: f64,
    diagnostics: Diagnostics,
    world_bounds: Aabb,
    baked_atlas_lookup: Option<retrolib::format::room::SAtlasLookup>,
}

impl SyncComponent for OriginalShaderMesh {
    type Target = ExtractedOriginalMesh;
}

impl ExtractComponent for OriginalShaderMesh {
    type Out = ExtractedOriginalMesh;
    type QueryData = (
        &'static Self,
        &'static GlobalTransform,
        &'static Mesh3d,
        Option<&'static OriginalShaderTime>,
    );
    type QueryFilter = ();

    fn extract_component(
        (mesh, transform, geometry, time): QueryItem<Self::QueryData>,
    ) -> Option<Self::Out> {
        // Queue visibility is per view. Keep hidden instances extracted so LOD
        // changes and tab switches preserve their compiled resources.
        Some(ExtractedOriginalMesh {
            mesh: mesh.prepared.clone(),
            diagnostics: mesh.diagnostics.clone(),
            asset_id: geometry.0.id().untyped(),
            transform: transform.to_matrix(),
            world_bounds: super::transform_aabb(&mesh.model_bounds, &transform.affine()),
            animation_seconds: time.map_or(0.0, |t| t.0),
            baked_atlas_lookup: mesh.baked_atlas_lookup,
        })
    }
}

pub struct OriginalShaderPlugin;
impl Plugin for OriginalShaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<OriginalShaderMesh>::default());
        app.sub_app_mut(RenderApp)
            .init_resource::<GpuMeshes>()
            .init_resource::<MaterialPipelines>()
            .init_resource::<diagnostics::FaultReadbacks>()
            .add_systems(Render, diagnostics::read_draw_faults.in_set(RenderSystems::Cleanup))
            .add_render_command::<Transparent3d, DrawOriginal>()
            .add_render_command::<Opaque3d, DrawOriginal>()
            .add_systems(Render, clear_original_bins.in_set(RenderSystems::PrepareAssets))
            .add_systems(Render, queue_original.in_set(RenderSystems::QueueMeshes));
    }
}

struct GpuMesh {
    source: Arc<PreparedMesh>,
    vertices: Buffer,
    indices: Buffer,
    layouts: [BindGroupLayout; 4],
    textures: [BindGroup; 2],
    layout_entries: [Vec<wgpu::BindGroupLayoutEntry>; 4],
}

struct GpuInstance {
    mesh: Arc<GpuMesh>,
    pipelines: HashMap<ViewKey, Arc<PipelineRequest>>,
    faults: Option<Arc<FaultCapture>>,
}

#[derive(Resource, Default)]
struct GpuMeshes {
    instances: HashMap<Entity, GpuInstance>,
    prepared: HashMap<usize, Weak<GpuMesh>>,
}

struct ViewDraw {
    pipeline: Arc<PipelineRequest>,
    main_entity: MainEntity,
    groups: [BindGroup; 2],
    faults: Option<Arc<FaultCapture>>,
}
#[derive(Component, Default)]
struct OriginalViewDraws(HashMap<Entity, ViewDraw>);

fn storage_layout(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        count: None,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    }
}

fn buffer(device: &RenderDevice, words: &[u32], usage: wgpu::BufferUsages) -> Buffer {
    device.create_buffer_with_data(&wgpu::util::BufferInitDescriptor {
        label: Some("MPR original shader data"),
        contents: bytemuck::cast_slice(if words.is_empty() { &[0] } else { words }),
        usage,
    })
}

fn texture_sample_type(value: TextureValueType) -> wgpu::TextureSampleType {
    match value {
        TextureValueType::Float => wgpu::TextureSampleType::Float { filterable: true },
        TextureValueType::Uint => wgpu::TextureSampleType::Uint,
        TextureValueType::Sint => wgpu::TextureSampleType::Sint,
        TextureValueType::Depth => wgpu::TextureSampleType::Depth,
    }
}

fn texture_view_dimension(dimension: TextureDimension) -> Result<wgpu::TextureViewDimension> {
    Ok(match dimension {
        TextureDimension::D2 => wgpu::TextureViewDimension::D2,
        TextureDimension::D2Array => wgpu::TextureViewDimension::D2Array,
        TextureDimension::D3 => wgpu::TextureViewDimension::D3,
        TextureDimension::Cube => wgpu::TextureViewDimension::Cube,
        _ => anyhow::bail!("Original shader viewer has no {dimension:?} texture adapter"),
    })
}

/// Construct the declared full view from the actual GPU allocation, independently
/// of Bevy's default view (which treats six 2D layers as an array, not a cube).
/// Do not turn a mismatched resource into a valid one by taking its first layer.
pub(crate) fn original_texture_view(
    texture: &Texture,
    binding: &TextureBinding,
) -> Result<TextureView> {
    let dimension = texture_view_dimension(binding.dimension)?;
    ensure!(
        !binding.multisample && texture.sample_count() == 1,
        "Original shader texture requires a single-sample allocation"
    );
    if let Some(sampling) = binding.sampling {
        ensure!(
            texture.mip_level_count() == sampling.mip_level_count,
            "Original shader texture mip count disagrees with its declaration"
        );
    }
    ensure!(
        texture.format().sample_type(None, None) == Some(texture_sample_type(binding.value_type)),
        "Original shader texture sample type disagrees with its declaration"
    );
    ensure!(
        match binding.dimension {
            TextureDimension::D2 =>
                texture.dimension() == wgpu::TextureDimension::D2
                    && texture.depth_or_array_layers() == 1,
            TextureDimension::D2Array => texture.dimension() == wgpu::TextureDimension::D2,
            TextureDimension::Cube =>
                texture.dimension() == wgpu::TextureDimension::D2
                    && texture.depth_or_array_layers() == 6
                    && texture.width() == texture.height(),
            TextureDimension::D3 => texture.dimension() == wgpu::TextureDimension::D3,
            _ => false,
        },
        "Original shader {:?} texture has incompatible allocation {:?} {:?}",
        binding.dimension,
        texture.dimension(),
        texture.size()
    );
    Ok(texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("MPR declared full texture view"),
        dimension: Some(dimension),
        ..Default::default()
    }))
}

impl GpuMesh {
    fn new(
        source: Arc<PreparedMesh>,
        device: &RenderDevice,
        queue: &RenderQueue,
        images: &RenderAssets<GpuImage>,
    ) -> Result<Option<Self>> {
        pipelines::checked(device, "Original material resources", || {
            let mut textures = Vec::with_capacity(source.images.len());
            for (image, declaration) in source.images.iter().zip(&source.draw.bindings.textures) {
                let view = match image {
                    PreparedImage::Image(handle) => {
                        let Some(image) = images.get(handle) else { return Ok(None) };
                        original_texture_view(&image.texture, &declaration.binding)?
                    }
                    PreparedImage::Cache(kind) => {
                        let data = kind.cache_data().context("Unavailable native scene cache")?;
                        let format = match data.format {
                            ETextureFormat::Rgba32Uint => wgpu::TextureFormat::Rgba32Uint,
                            ETextureFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
                            other => anyhow::bail!("Unsupported scene cache format: {other:?}"),
                        };
                        let texture = device.create_texture_with_data(
                            queue,
                            &wgpu::TextureDescriptor {
                                label: Some("MPR native scene cache image"),
                                size: wgpu::Extent3d {
                                    width: 1,
                                    height: 1,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format,
                                usage: wgpu::TextureUsages::TEXTURE_BINDING
                                    | wgpu::TextureUsages::COPY_DST,
                                view_formats: &[],
                            },
                            wgpu::util::TextureDataOrder::LayerMajor,
                            data.pixels,
                        );
                        original_texture_view(&texture, &declaration.binding)?
                    }
                };
                textures.push(view);
            }
            let stages = [source.draw.textures(0), source.draw.textures(1)];
            let visibility = [wgpu::ShaderStages::VERTEX, wgpu::ShaderStages::FRAGMENT];
            let mut entries = [
                vec![
                    storage_layout(0, visibility[0]),
                    storage_layout(1, visibility[0]),
                    storage_layout(2, visibility[1]),
                ],
                vec![],
                vec![],
                vec![],
            ];
            if source.draw.has_faults() {
                let mut faults = storage_layout(3, wgpu::ShaderStages::VERTEX_FRAGMENT);
                faults.ty = wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                };
                entries[0].push(faults);
            }
            for (stage, textures) in stages.iter().enumerate() {
                if let Some(binding) = source.draw.memory_bindings()[stage] {
                    entries[binding.group as usize]
                        .push(storage_layout(binding.binding, visibility[stage]));
                }
                for t in textures {
                    entries[stage + 2].push(wgpu::BindGroupLayoutEntry {
                        binding: t.texture,
                        visibility: visibility[stage],
                        count: None,
                        ty: wgpu::BindingType::Texture {
                            sample_type: texture_sample_type(t.view.value_type),
                            view_dimension: texture_view_dimension(t.view.dimension)?,
                            multisampled: false,
                        },
                    });
                    if let Some(binding) = t.sampler {
                        entries[stage + 2].push(wgpu::BindGroupLayoutEntry {
                            binding,
                            visibility: visibility[stage],
                            count: None,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        });
                    }
                }
            }
            let layouts = entries.each_ref().map(|entries| {
                device.create_bind_group_layout("MPR original shader resources", entries)
            });
            let samplers = source
                .draw
                .bindings
                .textures
                .iter()
                .map(|t| device.create_sampler(&sampler_descriptor(&t.sampler, device.features())))
                .collect::<Vec<_>>();
            let groups = std::array::from_fn(|stage| {
                let mut entries = Vec::new();
                for t in &stages[stage] {
                    let index = source
                        .draw
                        .bindings
                        .textures
                        .iter()
                        .position(|s| {
                            s.stage == stage && s.binding.texture_index == t.view.texture_index
                        })
                        .unwrap();
                    entries.push(wgpu::BindGroupEntry {
                        binding: t.texture,
                        resource: wgpu::BindingResource::TextureView(&textures[index]),
                    });
                    if let Some(binding) = t.sampler {
                        entries.push(wgpu::BindGroupEntry {
                            binding,
                            resource: wgpu::BindingResource::Sampler(&samplers[index]),
                        });
                    }
                }
                device.create_bind_group("MPR texture bindings", &layouts[stage + 2], &entries)
            });
            let vertices = buffer(
                device,
                &source.draw.vertex_words(&source.vertices),
                wgpu::BufferUsages::STORAGE,
            );
            let indices = buffer(device, &source.indices, wgpu::BufferUsages::INDEX);
            Ok(Some(Self {
                source,
                vertices,
                indices,
                layouts,
                textures: groups,
                layout_entries: entries,
            }))
        })
        .map_err(anyhow::Error::msg)?
    }
}

impl GpuInstance {
    fn view_draw(
        &self,
        device: &RenderDevice,
        extracted: &ExtractedOriginalMesh,
        main_entity: MainEntity,
        view: &ExtractedView,
        key: ViewKey,
    ) -> Result<ViewDraw> {
        let transforms = preview_transforms(
            extracted.transform,
            view.world_from_view.to_matrix().inverse(),
            view.clip_from_view,
        );
        let constants = self.mesh.source.draw.constants(&ModelShaderFrame {
            transforms: &transforms,
            samples: sample_state(key.0)?.constants,
            animation_time: Some(extracted.animation_seconds),
            // The standalone viewer uses the rendering camera for culling.
            cull_camera_position: Some(view.world_from_view.translation().to_array()),
            baked_atlas_lookup: extracted.baked_atlas_lookup,
        })?;
        let compact =
            constants.inputs.map(|words| buffer(device, &words, wgpu::BufferUsages::STORAGE));
        let faults = self.faults.clone();
        let mut inputs = vec![
            wgpu::BindGroupEntry { binding: 0, resource: self.mesh.vertices.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: compact[0].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: compact[1].as_entire_binding() },
        ];
        if let Some(faults) = &faults {
            inputs.push(wgpu::BindGroupEntry {
                binding: 3,
                resource: faults.buffer.as_entire_binding(),
            });
        }
        let input = device.create_bind_group("MPR vertex inputs", &self.mesh.layouts[0], &inputs);
        let memories: Vec<_> = constants
            .memories
            .into_iter()
            .map(|(binding, words)| (binding, buffer(device, &words, wgpu::BufferUsages::STORAGE)))
            .collect();
        let entries: Vec<_> = memories
            .iter()
            .map(|(binding, data)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: data.as_entire_binding(),
            })
            .collect();
        let memory =
            device.create_bind_group("MPR constant memory", &self.mesh.layouts[1], &entries);
        Ok(ViewDraw {
            pipeline: self.pipelines[&key].clone(),
            main_entity,
            groups: [input, memory],
            faults,
        })
    }
}

// Binned phases retain entries across frames. Remove only our previous entries
// before Bevy queues preview materials, including when an original draw falls back.
fn clear_original_bins(
    views: Query<(&ExtractedView, &OriginalViewDraws)>,
    mut phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
) {
    for (view, draws) in &views {
        if let Some(phase) = phases.get_mut(&view.retained_view_entity) {
            for draw in draws.0.values() {
                phase.remove(draw.main_entity);
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn queue_original(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<GpuImage>>,
    functions: Res<DrawFunctions<Transparent3d>>,
    opaque_functions: Res<DrawFunctions<Opaque3d>>,
    mut gpu: ResMut<GpuMeshes>,
    mut pipelines: ResMut<MaterialPipelines>,
    meshes: Query<(Entity, &MainEntity, &ExtractedOriginalMesh)>,
    views: Query<(Entity, &ExtractedView, &RenderVisibleEntities, &Frustum, &ViewTarget, &Msaa)>,
    mut transparent_phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
) {
    gpu.instances.retain(|entity, _| meshes.get(*entity).is_ok());
    gpu.prepared.retain(|_, mesh| mesh.strong_count() != 0);
    pipelines.prune();
    let entities: HashMap<_, _> = meshes.iter().map(|(entity, main, _)| (*main, entity)).collect();
    for (view_entity, view, visible, frustum, target, msaa) in &views {
        let (Some(phase), Some(opaque_phase), Some(visible)) = (
            transparent_phases.get_mut(&view.retained_view_entity),
            opaque_phases.get_mut(&view.retained_view_entity),
            visible.get::<Mesh3d>(),
        ) else {
            continue;
        };
        let mut draws = OriginalViewDraws::default();
        let key = (msaa.samples(), target.main_texture_format());
        for &(_, main_entity) in &visible.entities_cpu_culling {
            let Some(&entity) = entities.get(&main_entity) else { continue };
            let Ok((_, _, extracted)) = meshes.get(entity) else {
                continue;
            };
            // Bevy handles inherited visibility and render layers; its mesh
            // frustum test is disabled for these draws. Native model owners
            // test a world-aligned box, including for rotated instances.
            // Retain the viewer's existing side/near-plane clipping policy.
            if !frustum.intersects_obb(
                &extracted.world_bounds,
                &bevy::math::Affine3A::IDENTITY,
                true,
                false,
            ) {
                continue;
            }
            if gpu
                .instances
                .get(&entity)
                .is_some_and(|instance| !Arc::ptr_eq(&instance.mesh.source, &extracted.mesh))
            {
                gpu.instances.remove(&entity);
            }
            if !gpu.instances.contains_key(&entity) {
                let identity = Arc::as_ptr(&extracted.mesh) as usize;
                let mesh = match gpu.prepared.get(&identity).and_then(Weak::upgrade) {
                    Some(mesh) => mesh,
                    None => {
                        let mesh =
                            match GpuMesh::new(extracted.mesh.clone(), &device, &queue, &images) {
                                Ok(Some(mesh)) => mesh,
                                Ok(None) => continue,
                                Err(error) => {
                                    extracted
                                        .diagnostics
                                        .fail(format!("Original shader resources: {error:#}"));
                                    continue;
                                }
                            };
                        let mesh = Arc::new(mesh);
                        gpu.prepared.insert(identity, Arc::downgrade(&mesh));
                        mesh
                    }
                };
                let faults = mesh
                    .source
                    .draw
                    .has_faults()
                    .then(|| FaultCapture::new(&device, extracted.diagnostics.clone()));
                gpu.instances.insert(entity, GpuInstance { mesh, pipelines: default(), faults });
            }
            let instance = gpu.instances.get_mut(&entity).unwrap();
            let request = instance.pipelines.entry(key).or_insert_with(|| {
                let mesh = &instance.mesh;
                pipelines.request(
                    &device,
                    &mesh.source.draw,
                    &mesh.layout_entries,
                    &mesh.layouts,
                    mesh.source.state,
                    key,
                )
            });
            match request.pipeline() {
                Ok(Some(_)) => extracted.diagnostics.pipeline_ready(),
                Ok(None) => continue,
                Err(error) => {
                    extracted.diagnostics.fail(format!("Original shader compilation: {error}"));
                    continue;
                }
            }
            match instance.view_draw(&device, extracted, main_entity, view, key) {
                Ok(draw) => {
                    draws.0.insert(entity, draw);
                }
                Err(e) => {
                    extracted.diagnostics.fail(format!("Original shader constants: {e:#}"));
                    log::error!("Original shader view constants: {e:#}");
                    continue;
                }
            }
            if instance.mesh.source.state.blend.is_none() {
                opaque_phase.add(
                    Opaque3dBatchSetKey {
                        pipeline: CachedRenderPipelineId::INVALID,
                        draw_function: opaque_functions.read().id::<DrawOriginal>(),
                        material_bind_group_index: None,
                        slabs: default(),
                        lightmap_slab: None,
                    },
                    Opaque3dBinKey { asset_id: extracted.asset_id },
                    (entity, main_entity),
                    InputUniformIndex(0),
                    BinnedRenderPhaseType::NonMesh,
                );
            } else {
                phase.add_transient(Transparent3d {
                    entity: (entity, main_entity),
                    pipeline: CachedRenderPipelineId::INVALID,
                    draw_function: functions.read().id::<DrawOriginal>(),
                    distance: 0.0,
                    sorting_info: TransparentSortingInfo3d::Sorted {
                        mesh_center: extracted.transform.transform_point3(Vec3::ZERO),
                        depth_bias: 0.0,
                    },
                    batch_range: 0..1,
                    extra_index: PhaseItemExtraIndex::None,
                    indexed: true,
                });
            }
        }
        commands.entity(view_entity).insert(draws);
    }
}

struct DrawOriginal;

impl<P: PhaseItem> RenderCommand<P> for DrawOriginal {
    type ItemQuery = ();
    type Param = SRes<GpuMeshes>;
    type ViewQuery = Read<OriginalViewDraws>;

    fn render<'w>(
        item: &P,
        view: &'w OriginalViewDraws,
        _: Option<()>,
        gpu: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(instance) = gpu.into_inner().instances.get(&item.entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(draw) = view.0.get(&item.entity()) else {
            return RenderCommandResult::Skip;
        };
        let Ok(Some(pipeline)) = draw.pipeline.pipeline() else {
            return RenderCommandResult::Skip;
        };
        let mesh = &instance.mesh;
        pass.set_render_pipeline(pipeline);
        for (i, group) in draw.groups.iter().chain(mesh.textures.iter()).enumerate() {
            pass.set_bind_group(i, group, &[]);
        }
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.source.indices.len() as u32, 0, 0..1);
        if let Some(faults) = &draw.faults {
            faults.mark_drawn();
        }
        RenderCommandResult::Success
    }
}
