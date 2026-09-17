//! Shared, demand-owned compilation of generated native-material shaders.
//!
//! Bevy 0.19's global cache compiles synchronously on macOS and cannot evict an
//! individual pipeline. Keep this adapter local to generated shaders: one worker
//! limits concurrent compiler/driver work, and weak entries release closed draws.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};

use bevy::{
    prelude::Resource,
    render::{
        render_resource::{BindGroupLayout, RenderPipeline},
        renderer::RenderDevice,
    },
    tasks::{block_on, Task, TaskPool, TaskPoolBuilder},
};

use super::{program::MaterialProgram, MaterialRenderState};

pub(super) type ViewKey = (u32, wgpu::TextureFormat);

/// Include complete source and host interface in equality. Uniform contents and
/// texture identities are draw inputs; neither affects this compilation contract.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PipelineKey {
    sources: [Arc<str>; 2],
    entry_points: [Arc<str>; 2],
    layout_entries: [Vec<wgpu::BindGroupLayoutEntry>; 4],
    state: MaterialRenderState,
    view: ViewKey,
}

pub(super) struct PipelineRequest {
    result: Arc<OnceLock<Result<RenderPipeline, String>>>,
    // Dropping the final request cancels queued work. An already running driver
    // call may finish, but its result has no path back to a replaced instance.
    _task: Task<()>,
}

impl PipelineRequest {
    pub fn pipeline(&self) -> Result<Option<&RenderPipeline>, &str> {
        match self.result.get() {
            None => Ok(None),
            Some(Ok(pipeline)) => Ok(Some(pipeline)),
            Some(Err(error)) => Err(error),
        }
    }
}

#[derive(Resource)]
pub(super) struct MaterialPipelines {
    worker: TaskPool,
    entries: HashMap<PipelineKey, Weak<PipelineRequest>>,
}

impl Default for MaterialPipelines {
    fn default() -> Self {
        Self {
            worker: TaskPoolBuilder::new()
                .num_threads(1)
                .thread_name("MPR shader compiler".into())
                .build(),
            entries: HashMap::new(),
        }
    }
}

impl MaterialPipelines {
    pub fn prune(&mut self) { self.entries.retain(|_, request| request.strong_count() != 0); }

    pub fn request(
        &mut self,
        device: &RenderDevice,
        program: &MaterialProgram,
        layout_entries: &[Vec<wgpu::BindGroupLayoutEntry>; 4],
        layouts: &[BindGroupLayout; 4],
        state: MaterialRenderState,
        view: ViewKey,
    ) -> Arc<PipelineRequest> {
        let stages = program.stages();
        let key = PipelineKey {
            sources: stages.map(|(source, _)| Arc::from(source)),
            entry_points: stages.map(|(_, entry)| Arc::from(entry)),
            layout_entries: layout_entries.clone(),
            state,
            view,
        };
        self.request_key(device, key, layouts.clone())
    }

    fn request_key(
        &mut self,
        device: &RenderDevice,
        key: PipelineKey,
        layouts: [BindGroupLayout; 4],
    ) -> Arc<PipelineRequest> {
        if let Some(request) = self.entries.get(&key).and_then(Weak::upgrade) {
            return request;
        }
        let result = Arc::new(OnceLock::new());
        let output = result.clone();
        let device = device.clone();
        let compile_key = key.clone();
        let task = self.worker.spawn(async move {
            let compiled = compile(&device, &compile_key, &layouts);
            let _ = output.set(compiled);
        });
        let request = Arc::new(PipelineRequest { result, _task: task });
        self.entries.insert(key, Arc::downgrade(&request));
        request
    }
}

/// wgpu 29's scopes are thread-local. Creation and popping stay on the worker;
/// only completed errors/results cross threads. Keep wgpu's runtime checks on.
pub(super) fn checked<T>(
    device: &RenderDevice,
    label: &str,
    create: impl FnOnce() -> T,
) -> Result<T, String> {
    let device = device.wgpu_device();
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let value = create();
    let errors = [internal.pop(), memory.pop(), validation.pop()];
    for error in errors {
        if let Some(error) = block_on(error) {
            return Err(format!("{label}: {error}"));
        }
    }
    Ok(value)
}

fn compile(
    device: &RenderDevice,
    key: &PipelineKey,
    layouts: &[BindGroupLayout; 4],
) -> Result<RenderPipeline, String> {
    let module = |stage: usize| {
        checked(device, &key.entry_points[stage], || {
            device.wgpu_device().create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&key.entry_points[stage]),
                source: wgpu::ShaderSource::Wgsl(key.sources[stage].as_ref().into()),
            })
        })
    };
    let vertex = module(0)?;
    let fragment = if key.sources[0] == key.sources[1] { vertex.clone() } else { module(1)? };
    checked(device, "Original material pipeline", || {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("MPR original material layout"),
            bind_group_layouts: &layouts.iter().map(|layout| Some(&**layout)).collect::<Vec<_>>(),
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("MPR original material"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some(&key.entry_points[0]),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some(&key.entry_points[1]),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: key.view.1,
                    blend: key.state.blend,
                    write_mask: key.state.color_writes,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: key.state.cull_mode,
                front_face: key.state.front_face,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(key.state.depth_write),
                depth_compare: Some(key.state.depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: key.view.0,
                alpha_to_coverage_enabled: key.state.alpha_to_coverage,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    })
}

#[cfg(test)]
pub(in crate::render) enum TestVariant {
    FreshNames,
    InvalidShader,
    MissingEntryPoint,
    Sources([String; 2]),
}

#[cfg(test)]
impl MaterialPipelines {
    pub fn is_empty_for_test(&self) -> bool { self.entries.is_empty() }

    pub fn variant_for_test(
        &mut self,
        device: &RenderDevice,
        original: &Arc<PipelineRequest>,
        layouts: [BindGroupLayout; 4],
        variant: TestVariant,
    ) -> Arc<PipelineRequest> {
        let mut key = self
            .entries
            .iter()
            .find_map(|(key, value)| {
                value.upgrade().filter(|value| Arc::ptr_eq(value, original)).map(|_| key.clone())
            })
            .unwrap();
        match variant {
            TestVariant::FreshNames => {
                // Change emitted identifiers, not shader semantics, to avoid the
                // Metal compiler's cache when measuring frame progress.
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let prefix = format!("remax_probe_{nonce}_");
                key.sources = key.sources.map(|source| source.replace("remax_", &prefix).into());
                key.entry_points =
                    key.entry_points.map(|entry| entry.replace("remax_", &prefix).into());
            }
            TestVariant::InvalidShader => {
                key.sources[1] = "this is deliberately invalid WGSL".into()
            }
            TestVariant::MissingEntryPoint => {
                key.entry_points[1] = "missing_fragment_for_test".into()
            }
            TestVariant::Sources(sources) => {
                key.sources = sources.map(Arc::from);
            }
        }
        self.request_key(device, key, layouts)
    }
}
