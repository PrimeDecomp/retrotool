//! Native execution harness for the explicit, ignored compiler comparison test.
use std::{borrow::Cow, sync::mpsc};

use wgpu::util::DeviceExt;

use crate::program::{
    wgsl::{Input, Kernel},
    Inputs, Value,
};

pub(in crate::ir::tests) struct Gpu {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
}
impl Gpu {
    pub fn new() -> Self { Self::with_features(wgpu::Features::empty()) }

    pub fn with_features(features: wgpu::Features) -> Self {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("native GPU adapter");
            println!("{:?}", adapter.get_info());
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("remax comparison"),
                    required_features: features,
                    ..Default::default()
                })
                .await
                .unwrap();
            Self { device, queue }
        })
    }

    pub fn run(&self, k: &Kernel, cases: &[Inputs]) -> Vec<Vec<u32>> {
        self.run_with_resources(k, cases, |_, _, _| vec![])
    }

    pub fn run_with_snapshot(
        &self,
        k: &Kernel,
        cases: &[Inputs],
        snapshot: &crate::resources::memory::Snapshot,
    ) -> Vec<Vec<u32>> {
        let binding = k.global_memory.expect("global memory kernel");
        assert_eq!(binding.group, 2);
        self.run_with_resources(k, cases, |device, _, pipeline| {
            let bytes = snapshot.words().iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(2),
                entries: &[wgpu::BindGroupEntry {
                    binding: binding.binding,
                    resource: buffer.as_entire_binding(),
                }],
            });
            let mut groups = vec![(2, group)];
            if k.constant_memory.is_none() {
                groups.push((
                    1,
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(1),
                        entries: &[],
                    }),
                ));
            }
            groups
        })
    }

    pub fn run_with_resources(
        &self,
        k: &Kernel,
        cases: &[Inputs],
        resources: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &wgpu::ComputePipeline,
        ) -> Vec<(u32, wgpu::BindGroup)>,
    ) -> Vec<Vec<u32>> {
        assert!(!cases.is_empty());
        let n = k.inputs.len().max(1);
        let source = format!(
            r#"{}
struct Case {{
    words: array<u32, {n}>,
    valid: array<u32, {n}>,
}}
@group(0) @binding(0) var<storage, read> cases: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<RemaxResult>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    if (id.x >= arrayLength(&cases)) {{
        return;
    }}
    let input = cases[id.x];
    results[id.x] = remax_main(input.words, input.valid);
}}
"#,
            k.source,
        );
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("remax SSA WGSL"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
        });
        let pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("{error}");
        }
        let extra_groups = resources(&self.device, &self.queue, &pipeline);
        let mut data = Vec::new();
        for c in cases {
            let mut words = vec![0u32; n];
            let mut valid = vec![0u32; n];
            for (slot, input) in k.inputs.iter().enumerate() {
                let value = match input {
                    Input::Register(r) => c.registers.get(r).copied().map(|v| match v {
                        Value::Word(v) => v,
                        Value::Predicate(v) => u32::from(v),
                    }),
                    Input::VertexAttribute(offset) => c.attributes.get(offset).copied(),
                    Input::Interpolant(i) => c.interpolants.get(i).copied(),
                    Input::InterpolationPlane { attribute, component } => {
                        c.offset_interpolation.as_ref().and_then(|planes| {
                            assert_eq!(Some(planes.model()), k.offset_interpolation);
                            planes.get(*attribute).map(|p| p.component(*component).to_bits())
                        })
                    }
                    Input::SampleIndex => c.sample_index,
                };
                if let Some(v) = value {
                    words[slot] = v;
                    valid[slot] = 1;
                }
            }
            data.extend(words.into_iter().chain(valid).flat_map(u32::to_le_bytes));
        }
        let input = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &data,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let stride = (k.outputs.len().max(1) + 3) * 4;
        let size = (stride * cases.len()) as u64;
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
            ],
        });
        let memory = k.constant_memory.map(|binding| {
            assert_eq!(binding.group, 1);
            let words = k.constant_preconditions.bind(&cases[0].constants).unwrap();
            assert!(cases.iter().all(|c| c.constants.words() == words));
            let bytes = words.iter().copied().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(1),
                entries: &[wgpu::BindGroupEntry {
                    binding: binding.binding,
                    resource: buffer.as_entire_binding(),
                }],
            })
        });
        let mut commands = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = commands.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            if let Some(memory) = &memory {
                pass.set_bind_group(1, memory, &[]);
            }
            for (index, group) in &extra_groups {
                pass.set_bind_group(*index, group, &[]);
            }
            pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
        }
        commands.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        self.queue.submit(Some(commands.finish()));
        let (send, receive) = mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| send.send(r).unwrap());
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receive.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        mapped
            .chunks_exact(stride)
            .map(|c| c.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect())
            .collect()
    }

    pub fn render_with_resources(
        &self,
        k: &Kernel,
        words: &[String],
        valid: &[String],
        size: [u32; 2],
        resources: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &wgpu::RenderPipeline,
        ) -> Vec<(u32, wgpu::BindGroup)>,
    ) -> Vec<Vec<u32>> {
        assert_eq!(k.outputs.len(), 4);
        assert_eq!(words.len(), k.inputs.len());
        assert_eq!(valid.len(), words.len());
        let n = words.len().max(1);
        let setup = words
            .iter()
            .zip(valid)
            .enumerate()
            .map(|(i, (w, v))| format!("    words[{i}] = {w};\n    valid[{i}] = {v};\n"))
            .collect::<String>();
        let source = format!(
            r#"{}
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}}

struct Output {{
    @location(0) words: vec4<u32>,
    @location(1) status: vec4<u32>,
}}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> Output {{
    var words: array<u32, {n}>;
    var valid: array<u32, {n}>;
{setup}
    let result = remax_main(words, valid);
    var output: Output;
    output.words = vec4<u32>(result.words[0], result.words[1], result.words[2], result.words[3]);
    output.status = vec4<u32>(result.status, result.pc, result.discarded, 0u);
    return output;
}}
"#,
            k.source,
        );
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("remax texture render"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[
                    Some(wgpu::TextureFormat::Rgba32Uint.into()),
                    Some(wgpu::TextureFormat::Rgba32Uint.into()),
                ],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("{error}");
        }
        let groups = resources(&self.device, &self.queue, &pipeline);
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let targets = [0, 1].map(|_| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Uint,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let views = targets.each_ref().map(|t| t.create_view(&Default::default()));
        let pitch = (size[0] * 16).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(pitch * size[1] * 2),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut commands = self.device.create_command_encoder(&Default::default());
        {
            let attachments = views.each_ref().map(|v| {
                Some(wgpu::RenderPassColorAttachment {
                    view: v,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            });
            let mut pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            for (index, group) in &groups {
                pass.set_bind_group(*index, group, &[]);
            }
            pass.draw(0..3, 0..1);
        }
        for (i, t) in targets.iter().enumerate() {
            commands.copy_texture_to_buffer(
                t.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: (i as u64) * u64::from(pitch * size[1]),
                        bytes_per_row: Some(pitch),
                        rows_per_image: Some(size[1]),
                    },
                },
                extent,
            );
        }
        self.queue.submit(Some(commands.finish()));
        let (send, receive) = mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| send.send(r).unwrap());
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receive.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        let mut results = vec![];
        for y in 0..size[1] {
            for x in 0..size[0] {
                let mut row = vec![];
                for attachment in 0..2 {
                    let start = ((attachment * size[1] + y) * pitch + x * 16) as usize;
                    row.extend(
                        mapped[start..start + 16]
                            .chunks_exact(4)
                            .map(|w| u32::from_le_bytes(w.try_into().unwrap())),
                    );
                }
                row.truncate(7);
                results.push(row);
            }
        }
        results
    }
}
