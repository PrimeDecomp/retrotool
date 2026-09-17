use bevy::{
    asset::{load_internal_asset, uuid_handle},
    core_pipeline::{core_3d::main_opaque_pass_3d, schedule::Core3d, Core3dSystems},
    prelude::*,
    render::{
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_resource::{
            BindGroupLayoutDescriptor, CachedRenderPipelineId, FragmentState, PipelineCache,
            RenderPipelineDescriptor, ShaderType, SpecializedRenderPipeline,
            SpecializedRenderPipelines, VertexState,
        },
        renderer::{RenderContext, ViewQuery},
        view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        Render, RenderApp, RenderSystems,
    },
};

const GRID_SHADER_HANDLE: Handle<Shader> = uuid_handle!("e784fdf0-c4d0-411b-a8be-9e87c98b486a");

pub struct GridPlugin;

impl Plugin for GridPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, GRID_SHADER_HANDLE, "grid.wgsl", Shader::from_wgsl);
        app.add_plugins(ExtractComponentPlugin::<GridSettings>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<GridPipeline>()
            .init_resource::<SpecializedRenderPipelines<GridPipeline>>()
            .add_systems(Render, prepare_grid_pipeline.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(
                Core3d,
                draw_grid.before(main_opaque_pass_3d).in_set(Core3dSystems::MainPass),
            );
    }
}

/// Marks a scene viewport which needs the ground grid.
#[derive(Component, ExtractComponent, Clone, Default)]
pub struct GridSettings;

#[derive(Component)]
struct GridPipelineId(CachedRenderPipelineId);

#[derive(Resource)]
struct GridPipeline {
    layout: BindGroupLayoutDescriptor,
}

impl Default for GridPipeline {
    fn default() -> Self {
        Self {
            layout: BindGroupLayoutDescriptor::new("grid view", &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: Some(ViewUniform::min_size()),
                },
                count: None,
            }]),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct GridPipelineKey {
    samples: u32,
    format: wgpu::TextureFormat,
}

impl SpecializedRenderPipeline for GridPipeline {
    type Key = GridPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("grid".into()),
            layout: vec![self.layout.clone()],
            vertex: VertexState {
                shader: GRID_SHADER_HANDLE,
                entry_point: Some("vertex".into()),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: GRID_SHADER_HANDLE,
                entry_point: Some("fragment".into()),
                targets: vec![Some(wgpu::ColorTargetState {
                    format: key.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
                ..default()
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..default()
            },
            multisample: wgpu::MultisampleState { count: key.samples, ..default() },
            ..default()
        }
    }
}

fn prepare_grid_pipeline(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<GridPipeline>>,
    pipeline: Res<GridPipeline>,
    views: Query<(Entity, &ViewTarget, &Msaa), With<GridSettings>>,
) {
    for (entity, target, msaa) in &views {
        let id = pipelines.specialize(&cache, &pipeline, GridPipelineKey {
            samples: msaa.samples(),
            format: target.main_texture_format(),
        });
        commands.entity(entity).insert(GridPipelineId(id));
    }
}

fn draw_grid(
    view: ViewQuery<(&ViewTarget, &ViewUniformOffset, &ExtractedCamera, &GridPipelineId)>,
    pipeline: Res<GridPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (target, offset, camera, id) = view.into_inner();
    let (Some(render_pipeline), Some(resource)) =
        (cache.get_render_pipeline(id.0), uniforms.uniforms.binding())
    else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "grid view",
        &cache.get_bind_group_layout(&pipeline.layout),
        &[wgpu::BindGroupEntry { binding: 0, resource }],
    );
    let attachments = [Some(target.get_color_attachment())];
    let mut pass = ctx.begin_tracked_render_pass(wgpu::RenderPassDescriptor {
        label: Some("grid"),
        color_attachments: &attachments,
        ..default()
    });
    if let Some(viewport) = &camera.viewport {
        pass.set_camera_viewport(viewport);
    }
    pass.set_bind_group(0, &group, &[offset.offset]);
    pass.set_render_pipeline(render_pipeline);
    pass.draw(0..4, 0..1);
}
