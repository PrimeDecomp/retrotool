use std::{borrow::Cow, collections::BTreeMap, sync::mpsc};

use wgpu::util::DeviceExt;

use super::{ConstantMemory, Gpu, GraphicsProgram};

mod targets;
use targets::{Targets, FAULT_OFFSET, READBACK_BYTES};

pub(super) struct RenderState {
    pub samples: u32,
    pub blend: Option<wgpu::BlendState>,
    pub clear: wgpu::Color,
}

impl Default for RenderState {
    fn default() -> Self { Self { samples: 1, blend: None, clear: wgpu::Color::TRANSPARENT } }
}

#[derive(Default)]
pub(super) struct Resources<'a> {
    pub vertex_constants: Option<&'a ConstantMemory>,
    pub fragment_constants: Option<&'a ConstantMemory>,
    pub fragment_memory: Option<&'a crate::resources::memory::Snapshot>,
}

pub(super) struct Rendered {
    pub colors: BTreeMap<u8, Vec<[f32; 4]>>,
    pub depth: Option<Vec<f32>>,
    pub faults: [u32; 4],
}

pub(super) fn render(
    gpu: &Gpu,
    program: &GraphicsProgram,
    vertices: &[BTreeMap<u16, u32>],
    samples: u32,
    resources: Resources<'_>,
) -> Rendered {
    render_with_bind_groups(gpu, program, vertices, samples, resources, |_| Vec::new())
}

/// Additional texture/sampler groups use the actual pipeline layout. Group zero
/// remains owned by the vertex input, constant/global snapshot and fault buffers.
pub(super) fn render_with_bind_groups(
    gpu: &Gpu,
    program: &GraphicsProgram,
    vertices: &[BTreeMap<u16, u32>],
    samples: u32,
    resources: Resources<'_>,
    bind_groups: impl FnOnce(&wgpu::RenderPipeline) -> Vec<(u32, wgpu::BindGroup)>,
) -> Rendered {
    render_with_state(
        gpu,
        program,
        vertices,
        RenderState { samples, ..Default::default() },
        resources,
        bind_groups,
    )
}

pub(super) fn render_with_state(
    gpu: &Gpu,
    program: &GraphicsProgram,
    vertices: &[BTreeMap<u16, u32>],
    state: RenderState,
    resources: Resources<'_>,
    bind_groups: impl FnOnce(&wgpu::RenderPipeline) -> Vec<(u32, wgpu::BindGroup)>,
) -> Rendered {
    let device = &gpu.device;
    let targets = Targets::new(device, program, &state);
    let pipeline = create_pipeline(device, program, state.samples, &targets);
    let (group, faults) = upload_inputs(device, &pipeline, program, vertices, resources);
    let bind_groups = bind_groups(&pipeline);
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: READBACK_BYTES,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &targets.color_attachments(),
            depth_stencil_attachment: targets.depth_attachment(),
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        for (index, group) in &bind_groups {
            assert_ne!(*index, 0, "additional groups must not replace graphics inputs");
            pass.set_bind_group(*index, group, &[]);
        }
        pass.draw(0..vertices.len() as u32, 0..1);
    }
    targets.copy_to(&mut encoder, &output);
    encoder.copy_buffer_to_buffer(&faults, 0, &output, FAULT_OFFSET, 16);
    gpu.queue.submit(Some(encoder.finish()));
    let (send, receive) = mpsc::channel();
    output.slice(..).map_async(wgpu::MapMode::Read, move |r| send.send(r).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let mapped = output.slice(..).get_mapped_range();
    let colors = targets.read_colors(&mapped);
    let depth = targets.read_depth(&mapped);
    let faults = std::array::from_fn(|index| {
        let offset = FAULT_OFFSET as usize + index * 4;
        u32::from_le_bytes(mapped[offset..offset + 4].try_into().unwrap())
    });
    Rendered { colors, depth, faults }
}

fn create_pipeline(
    device: &wgpu::Device,
    program: &GraphicsProgram,
    samples: u32,
    targets: &Targets,
) -> wgpu::RenderPipeline {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let modules = [&program.vertex, &program.fragment].map(|module| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(module.entry_point),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(&module.source)),
        })
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("remax linked graphics"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &modules[0],
            entry_point: Some(program.vertex.entry_point),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &modules[1],
            entry_point: Some(program.fragment.entry_point),
            compilation_options: Default::default(),
            targets: &targets.states(),
        }),
        primitive: Default::default(),
        depth_stencil: targets.depth_state(),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        multiview_mask: None,
        cache: None,
    });
    if let Some(error) = pollster::block_on(scope.pop()) {
        panic!("{error}");
    }
    pipeline
}

fn upload_inputs(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    program: &GraphicsProgram,
    vertices: &[BTreeMap<u16, u32>],
    resources: Resources<'_>,
) -> (wgpu::BindGroup, wgpu::Buffer) {
    let buffer = |words: Vec<u32>, usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>(),
            usage,
        })
    };
    let input_count = program.attributes.len().max(1);
    let input = buffer(
        vertices
            .iter()
            .flat_map(|vertex| {
                let mut words = vec![0; 2 * input_count];
                for (slot, attribute) in program.attributes.iter().enumerate() {
                    if let Some(&word) = vertex.get(attribute) {
                        words[slot] = word;
                        words[input_count + slot] = 1;
                    }
                }
                words
            })
            .collect(),
        wgpu::BufferUsages::STORAGE,
    );
    let faults = buffer(
        vec![0, u32::MAX, 0, u32::MAX],
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let constant_buffers =
        [resources.vertex_constants, resources.fragment_constants].map(|constants| {
            constants
                .map(|constants| buffer(constants.words().to_vec(), wgpu::BufferUsages::STORAGE))
        });
    let memory_buffer = resources
        .fragment_memory
        .map(|snapshot| buffer(snapshot.words().to_vec(), wgpu::BufferUsages::STORAGE));
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: program.vertex_inputs.binding,
            resource: input.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: program.faults.binding,
            resource: faults.as_entire_binding(),
        },
    ];
    for (kernel, buffer) in
        [&program.vertex.kernel, &program.fragment.kernel].into_iter().zip(&constant_buffers)
    {
        if let Some(binding) = kernel.constant_memory {
            assert_eq!(binding.group, 0);
            entries.push(wgpu::BindGroupEntry {
                binding: binding.binding,
                resource: buffer.as_ref().expect("stage constants").as_entire_binding(),
            });
        }
    }
    if let Some(binding) = program.fragment.kernel.global_memory {
        assert_eq!(binding.group, 0);
        entries.push(wgpu::BindGroupEntry {
            binding: binding.binding,
            resource: memory_buffer.as_ref().unwrap().as_entire_binding(),
        });
    }
    assert_eq!(program.vertex_inputs.group, 0);
    assert_eq!(program.faults.group, 0);
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    (group, faults)
}
