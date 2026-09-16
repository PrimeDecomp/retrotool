//! Experimental Bevy draws using each material's translated original shaders.
//! Static material providers are shared with retrolib; scene providers and
//! unsupported shaders return a diagnostic before replacing the preview material.

use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use bevy::{
    core_pipeline::core_3d::{Opaque3d, Transparent3d},
    ecs::{
        query::QueryItem,
        system::{lifetimeless::*, SystemParamItem},
    },
    prelude::*,
    render::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, RenderCommand, RenderCommandResult,
            RenderPhase, TrackedRenderPass,
        },
        render_resource::{
            BindGroup, BindGroupLayout, Buffer, CachedRenderPipelineId, RenderPipeline, Texture,
            TextureView,
        },
        renderer::{RenderDevice, RenderQueue},
        view::{ExtractedView, ViewTarget, VisibleEntities},
        RenderApp, RenderSet,
    },
    utils::HashMap,
};
use remax::{
    maxwell::texture::TextureDimension,
    wgsl::texture::{TextureBinding, TextureValueType},
};
use retrolib::format::{
    material_scene_bindings::{SceneMaterialInputs, SceneTexture},
    material_shader_draw::{ModelShaderDraw, ModelShaderTextureSource},
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

#[derive(Component, Clone)]
pub struct OriginalShaderMesh(Arc<PreparedMesh>);

/// Per-instance playback time. Kept outside PreparedMesh so uniform animation
/// preserves shared geometry, texture resources and compiled pipelines.
#[derive(Component, Clone, Copy, Default)]
pub struct OriginalShaderTime(pub f64);

#[derive(Clone, Debug)]
pub struct OriginalShaderInfo {
    pub sources: [i32; 4],
    pub permutation: [u64; 2],
    pub animated_uv: bool,
}

impl OriginalShaderMesh {
    pub fn info(&self) -> OriginalShaderInfo {
        OriginalShaderInfo {
            sources: self.0.draw.source_indices,
            permutation: self.0.draw.permutation,
            animated_uv: self.0.draw.has_animated_uv(),
        }
    }
}

struct PreparedMesh {
    draw: ModelShaderDraw,
    vertices: Vec<[u32; 256]>,
    indices: Vec<u32>,
    images: Vec<PreparedImage>,
    state: MaterialRenderState,
}

enum PreparedImage {
    Image(Handle<Image>),
    Cache(SceneTexture),
}

/// Caller-supplied scene data and GPU images for runtime texture tokens. Cache
/// white/zero images are constructed from their recovered native descriptors.
/// A supplied image's complete backing texture must match the declared type.
/// This adapter constructs full views; mip/layer subresource views are unsupported.
#[allow(dead_code)] // Conservative entry point also used by standalone research probes.
pub fn prepare_mesh_with_scene(
    model: &ModelAsset,
    mesh_index: usize,
    mirrored: bool,
    materials: &Assets<MaterialAsset>,
    textures: &Assets<TextureAsset>,
    scene: &SceneMaterialInputs<'_>,
    scene_image: impl FnMut(u64) -> Result<Handle<Image>>,
) -> Result<OriginalShaderMesh> {
    prepare_mesh_with_scene_and_features(
        model,
        mesh_index,
        mirrored,
        materials,
        textures,
        scene,
        wgpu::Features::empty(),
        scene_image,
    )
}

/// As above, with the enabled features of the device which will draw the mesh.
/// Border sampling must be supported before replacing a working preview draw.
pub fn prepare_mesh_with_scene_and_features(
    model: &ModelAsset,
    mesh_index: usize,
    mirrored: bool,
    materials: &Assets<MaterialAsset>,
    textures: &Assets<TextureAsset>,
    scene: &SceneMaterialInputs<'_>,
    features: wgpu::Features,
    mut scene_image: impl FnMut(u64) -> Result<Handle<Image>>,
) -> Result<OriginalShaderMesh> {
    let mesh = model.inner.mesh.meshes.get(mesh_index).context("Missing mesh")?;
    let material = &model.inner.mtrl.materials[mesh.material_idx as usize];
    let resource = model
        .material_resources
        .get(&material.material_resource_id)
        .and_then(|h| materials.get(h))
        .context("Material archive unavailable")?;
    let draw = ModelShaderDraw::prepare_mesh_with_scene(
        &model.inner,
        mesh_index,
        &resource.inner.payload()?,
        b'f',
        NvnSampleShading::ShaderDefault,
        |id| {
            Ok(model.textures.get(&id).and_then(|h| textures.get(h)).map(|t| t.inner.head.clone()))
        },
        scene,
        0.0, // Baseline; extracted instance time updates UV uniforms at draw time.
    )?;
    let images = draw
        .textures
        .iter()
        .map(|t| {
            ensure!(
                matches!(
                    t.binding.dimension,
                    TextureDimension::D2 | TextureDimension::D3 | TextureDimension::Cube
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
                    features.contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER),
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
        &draw.vertex_layout,
        &draw.kernel.vertex.input_attributes,
    )?;
    let indices = model_mesh_indices(&model.inner, mesh_index)?;
    ensure!(!vertices.is_empty() && !indices.is_empty(), "Empty original shader draw");
    let bound = mesh_draw_state(
        model.inner.mesh.mesh_class(mesh_index).context("Missing mesh class")?,
        model.inner.mesh.is_two_sided(mesh_index).context("Missing two-sided flag")?,
        mirrored,
    )?;
    let mut merged = draw.technique_state.clone();
    merged.merge(&bound)?;
    merged.merge(&draw.override_state)?;
    let mut state = MaterialRenderState::default();
    let mut warnings = state.apply(&bound);
    warnings.extend(state.apply(&merged));
    ensure!(warnings.is_empty(), "Original shader draw state: {}", warnings.join("; "));
    // Validate direct constant availability before replacing a preview material.
    // Indexed reads retain byte validity and fault rather than using unknown data.
    draw.constants(
        &preview_transforms(Mat4::IDENTITY, Mat4::IDENTITY, Mat4::IDENTITY),
        sample_state(1)?.constants,
    )?;
    Ok(OriginalShaderMesh(Arc::new(PreparedMesh { draw, vertices, indices, images, state })))
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
    transform: Mat4,
    uv_seconds: f64,
}

impl ExtractComponent for OriginalShaderMesh {
    type Filter = ();
    type Out = ExtractedOriginalMesh;
    type Query = (
        &'static Self,
        &'static GlobalTransform,
        &'static ComputedVisibility,
        Option<&'static OriginalShaderTime>,
    );

    fn extract_component(
        (mesh, transform, visibility, time): QueryItem<Self::Query>,
    ) -> Option<Self::Out> {
        visibility.is_visible().then(|| ExtractedOriginalMesh {
            mesh: mesh.0.clone(),
            transform: transform.compute_matrix(),
            uv_seconds: time.map_or(0.0, |t| t.0),
        })
    }
}

pub struct OriginalShaderPlugin;
impl Plugin for OriginalShaderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(ExtractComponentPlugin::<OriginalShaderMesh>::default());
        app.sub_app_mut(RenderApp)
            .init_resource::<GpuMeshes>()
            .add_render_command::<Transparent3d, DrawOriginal>()
            .add_render_command::<Opaque3d, DrawOriginal>()
            .add_system(queue_original.in_set(RenderSet::Queue));
    }
}

struct GpuMesh {
    source: Arc<PreparedMesh>,
    vertices: Buffer,
    indices: Buffer,
    layouts: [BindGroupLayout; 4],
    textures: [BindGroup; 2],
    pipelines: HashMap<(u32, wgpu::TextureFormat), RenderPipeline>,
}

#[derive(Resource, Default)]
struct GpuMeshes(HashMap<Entity, GpuMesh>);

struct ViewDraw {
    key: (u32, wgpu::TextureFormat),
    groups: [BindGroup; 2],
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
        texture.format().describe().sample_type == texture_sample_type(binding.value_type),
        "Original shader texture sample type disagrees with its declaration"
    );
    ensure!(
        match binding.dimension {
            TextureDimension::D2 =>
                texture.dimension() == wgpu::TextureDimension::D2
                    && texture.depth_or_array_layers() == 1,
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
        images: &RenderAssets<Image>,
    ) -> Option<Self> {
        let textures = source
            .images
            .iter()
            .zip(&source.draw.textures)
            .map(|(image, declaration)| match image {
                PreparedImage::Image(handle) => {
                    let image = images.get(handle)?;
                    original_texture_view(&image.texture, &declaration.binding).ok()
                }
                PreparedImage::Cache(kind) => {
                    let data = kind.cache_data()?;
                    let format = match data.format {
                        ETextureFormat::Rgba32Uint => wgpu::TextureFormat::Rgba32Uint,
                        ETextureFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
                        _ => return None,
                    };
                    let texture = device.create_texture_with_data(
                        queue,
                        &wgpu::TextureDescriptor {
                            label: Some("MPR native scene cache image"),
                            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                        data.pixels,
                    );
                    original_texture_view(&texture, &declaration.binding).ok()
                }
            })
            .collect::<Option<Vec<_>>>()?;
        let stages = [&source.draw.kernel.vertex, &source.draw.kernel.fragment];
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
        for (stage, kernel) in stages.iter().enumerate() {
            if let Some(binding) = kernel.constant_memory_binding {
                entries[binding.group as usize]
                    .push(storage_layout(binding.binding, visibility[stage]));
            }
            for t in &kernel.textures {
                entries[stage + 2].push(wgpu::BindGroupLayoutEntry {
                    binding: t.texture_binding,
                    visibility: visibility[stage],
                    count: None,
                    ty: wgpu::BindingType::Texture {
                        sample_type: texture_sample_type(t.binding.value_type),
                        view_dimension: texture_view_dimension(t.binding.dimension).ok()?,
                        multisampled: false,
                    },
                });
                if let Some(binding) = t.sampler_binding {
                    entries[stage + 2].push(wgpu::BindGroupLayoutEntry {
                        binding,
                        visibility: visibility[stage],
                        count: None,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    });
                }
            }
        }
        let layouts = entries.map(|entries| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("MPR original shader resources"),
                entries: &entries,
            })
        });
        let samplers = source
            .draw
            .textures
            .iter()
            .map(|t| device.create_sampler(&sampler_descriptor(&t.sampler, device.features())))
            .collect::<Vec<_>>();
        let groups = std::array::from_fn(|stage| {
            let mut entries = Vec::new();
            for t in &stages[stage].textures {
                let index = source
                    .draw
                    .textures
                    .iter()
                    .position(|s| {
                        s.stage == stage && s.binding.texture_index == t.binding.texture_index
                    })
                    .unwrap();
                entries.push(wgpu::BindGroupEntry {
                    binding: t.texture_binding,
                    resource: wgpu::BindingResource::TextureView(&textures[index]),
                });
                if let Some(binding) = t.sampler_binding {
                    entries.push(wgpu::BindGroupEntry {
                        binding,
                        resource: wgpu::BindingResource::Sampler(&samplers[index]),
                    });
                }
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("MPR texture bindings"),
                layout: &layouts[stage + 2],
                entries: &entries,
            })
        });
        let vertices =
            buffer(device, bytemuck::cast_slice(&source.vertices), wgpu::BufferUsages::STORAGE);
        let indices = buffer(device, &source.indices, wgpu::BufferUsages::INDEX);
        Some(Self { source, vertices, indices, layouts, textures: groups, pipelines: default() })
    }

    fn pipeline(
        &mut self,
        device: &RenderDevice,
        key: (u32, wgpu::TextureFormat),
        format: wgpu::TextureFormat,
    ) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let module = device.wgpu_device().create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("MPR original vertex/pixel shaders"),
            source: wgpu::ShaderSource::Wgsl(self.source.draw.kernel.wgsl.clone().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &self.layouts.iter().map(|l| &**l).collect::<Vec<_>>(),
            push_constant_ranges: &[],
        });
        let state = self.source.state;
        self.pipelines.insert(
            key,
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("MPR original material"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: "mpr_vertex_main",
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: "mpr_fragment_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: state.blend,
                        write_mask: state.color_writes,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: state.cull_mode,
                    front_face: state.front_face,
                    ..default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: state.depth_write,
                    depth_compare: state.depth_compare,
                    stencil: default(),
                    bias: default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: key.0,
                    alpha_to_coverage_enabled: state.alpha_to_coverage,
                    ..default()
                },
                multiview: None,
            }),
        );
    }

    fn view_draw(
        &self,
        device: &RenderDevice,
        transform: Mat4,
        view: &ExtractedView,
        key: (u32, wgpu::TextureFormat),
        uv_seconds: f64,
    ) -> Result<ViewDraw> {
        let constants = self.source.draw.constants_at_time(
            &preview_transforms(
                transform,
                view.transform.compute_matrix().inverse(),
                view.projection,
            ),
            sample_state(key.0)?.constants,
            uv_seconds,
        )?;
        let compact = [
            buffer(device, &constants.vertex_compact, wgpu::BufferUsages::STORAGE),
            buffer(device, &constants.fragment_compact, wgpu::BufferUsages::STORAGE),
        ];
        let input = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layouts[0],
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.vertices.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: compact[0].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: compact[1].as_entire_binding() },
            ],
        });
        let memories = [
            buffer(device, constants.vertex_memory.words(), wgpu::BufferUsages::STORAGE),
            buffer(device, constants.fragment_memory.words(), wgpu::BufferUsages::STORAGE),
        ];
        let entries = [&self.source.draw.kernel.vertex, &self.source.draw.kernel.fragment]
            .iter()
            .enumerate()
            .filter_map(|(stage, k)| {
                k.constant_memory_binding.map(|b| wgpu::BindGroupEntry {
                    binding: b.binding,
                    resource: memories[stage].as_entire_binding(),
                })
            })
            .collect::<Vec<_>>();
        let memory = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layouts[1],
            entries: &entries,
        });
        Ok(ViewDraw { key, groups: [input, memory] })
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_original(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<Image>>,
    msaa: Res<Msaa>,
    functions: Res<DrawFunctions<Transparent3d>>,
    opaque_functions: Res<DrawFunctions<Opaque3d>>,
    mut gpu: ResMut<GpuMeshes>,
    meshes: Query<(Entity, &ExtractedOriginalMesh)>,
    mut views: Query<(
        Entity,
        &ExtractedView,
        &VisibleEntities,
        &ViewTarget,
        &mut RenderPhase<Transparent3d>,
        &mut RenderPhase<Opaque3d>,
    )>,
) {
    gpu.0.retain(|entity, _| meshes.get(*entity).is_ok());
    for (view_entity, view, visible, target, mut phase, mut opaque_phase) in &mut views {
        let mut draws = OriginalViewDraws::default();
        let key = (msaa.samples(), target.main_texture_format());
        for &entity in &visible.entities {
            let Ok((_, extracted)) = meshes.get(entity) else {
                continue;
            };
            if gpu.0.get(&entity).is_some_and(|g| !Arc::ptr_eq(&g.source, &extracted.mesh)) {
                gpu.0.remove(&entity);
            }
            if !gpu.0.contains_key(&entity) {
                let Some(mesh) = GpuMesh::new(extracted.mesh.clone(), &device, &queue, &images)
                else {
                    continue;
                };
                gpu.0.insert(entity, mesh);
            }
            let mesh = gpu.0.get_mut(&entity).unwrap();
            mesh.pipeline(&device, key, target.main_texture_format());
            match mesh.view_draw(&device, extracted.transform, view, key, extracted.uv_seconds) {
                Ok(draw) => {
                    draws.0.insert(entity, draw);
                }
                Err(e) => {
                    log::error!("Original shader view constants: {e:#}");
                    continue;
                }
            }
            let distance = view.rangefinder3d().distance(&extracted.transform);
            if mesh.source.state.blend.is_none() {
                opaque_phase.add(Opaque3d {
                    entity,
                    pipeline: CachedRenderPipelineId::INVALID,
                    draw_function: opaque_functions.read().id::<DrawOriginal>(),
                    distance,
                });
            } else {
                phase.add(Transparent3d {
                    entity,
                    pipeline: CachedRenderPipelineId::INVALID,
                    draw_function: functions.read().id::<DrawOriginal>(),
                    distance,
                });
            }
        }
        commands.entity(view_entity).insert(draws);
    }
}

struct DrawOriginal;
impl<P: PhaseItem> RenderCommand<P> for DrawOriginal {
    type ItemWorldQuery = ();
    type Param = SRes<GpuMeshes>;
    type ViewWorldQuery = Read<OriginalViewDraws>;

    fn render<'w>(
        item: &P,
        view: &'w OriginalViewDraws,
        _: (),
        gpu: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(mesh) = gpu.into_inner().0.get(&item.entity()) else {
            return RenderCommandResult::Failure;
        };
        let Some(draw) = view.0.get(&item.entity()) else {
            return RenderCommandResult::Failure;
        };
        pass.set_render_pipeline(&mesh.pipelines[&draw.key]);
        for (i, group) in draw.groups.iter().chain(mesh.textures.iter()).enumerate() {
            pass.set_bind_group(i, group, &[]);
        }
        pass.set_index_buffer(mesh.indices.slice(..), 0, wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.source.indices.len() as u32, 0, 0..1);
        RenderCommandResult::Success
    }
}
