use bevy::{
    asset::load_internal_asset,
    core_pipeline::clear_color::ClearColorConfig,
    ecs::{query::QueryItem, system::lifetimeless::Read},
    prelude::*,
    reflect::TypeUuid,
    render::{
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_graph::{Node, NodeRunError, RenderGraph, RenderGraphContext, SlotInfo, SlotType},
        render_resource::{
            BindGroupLayout, CachedRenderPipelineId, FragmentState, PipelineCache,
            RenderPipelineDescriptor, ShaderType, SpecializedRenderPipeline,
            SpecializedRenderPipelines, VertexState,
        },
        renderer::{RenderContext, RenderDevice},
        texture::BevyDefault,
        view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        RenderApp, RenderSet,
    },
};

const GRID_SHADER_HANDLE: HandleUntyped =
    HandleUntyped::weak_from_u64(Shader::TYPE_UUID, 4444964537800926070);

#[derive(Default)]
pub struct GridPlugin;

impl Plugin for GridPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, GRID_SHADER_HANDLE, "grid.wgsl", Shader::from_wgsl);

        app.register_type::<GridSettings>()
            .add_plugin(ExtractComponentPlugin::<GridSettings>::default());

        let render_app = match app.get_sub_app_mut(RenderApp) {
            Ok(render_app) => render_app,
            Err(_) => return,
        };

        render_app
            .init_resource::<GridPipeline>()
            .init_resource::<SpecializedRenderPipelines<GridPipeline>>()
            .add_system(prepare_grid_pipeline.in_set(RenderSet::Prepare));

        {
            let grid_node = GridCameraDriver::from_world(&mut render_app.world);
            let mut graph = render_app.world.resource_mut::<RenderGraph>();
            let draw_3d_graph =
                graph.get_sub_graph_mut(bevy::core_pipeline::core_3d::graph::NAME).unwrap();
            draw_3d_graph.add_node(GridCameraDriver::IN_NODE, grid_node);
            draw_3d_graph.add_slot_edge(
                draw_3d_graph.input_node().id,
                bevy::core_pipeline::core_3d::graph::input::VIEW_ENTITY,
                GridCameraDriver::IN_NODE,
                GridCameraDriver::IN_VIEW,
            );
            // PREPASS -> GRID -> MAIN_PASS
            draw_3d_graph.add_node_edge(
                bevy::core_pipeline::core_3d::graph::node::PREPASS,
                GridCameraDriver::IN_NODE,
            );
            draw_3d_graph.add_node_edge(
                GridCameraDriver::IN_NODE,
                bevy::core_pipeline::core_3d::graph::node::MAIN_PASS,
            );
        }
    }
}

struct GridCameraDriver {
    #[allow(clippy::type_complexity)]
    view_query: QueryState<(
        Read<ViewTarget>,
        Read<ViewUniformOffset>,
        Read<ExtractedCamera>,
        Read<GridPipelineIds>,
        Read<GridSettings>,
    )>,
}

impl GridCameraDriver {
    pub const IN_NODE: &'static str = "grid";
    pub const IN_VIEW: &'static str = "view";
}

impl FromWorld for GridCameraDriver {
    fn from_world(world: &mut World) -> Self { Self { view_query: QueryState::new(world) } }
}

impl Node for GridCameraDriver {
    fn input(&self) -> Vec<SlotInfo> { vec![SlotInfo::new(Self::IN_VIEW, SlotType::Entity)] }

    fn update(&mut self, world: &mut World) { self.view_query.update_archetypes(world); }

    fn run(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let clear_res = world.resource::<ClearColor>();
        let pipeline_res = world.resource::<GridPipeline>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let uniforms = world.resource::<ViewUniforms>();

        let view_entity = graph.get_input_entity(Self::IN_VIEW)?;
        let Ok((
           target,
           offset,
           camera,
           pipeline_ids,
           settings,
        )) = self.view_query.get_manual(world, view_entity)
        else { return Ok(()); };

        let (
            Some(pipeline),
            Some(resource),
        ) = (
            pipeline_cache.get_render_pipeline(pipeline_ids.id),
            uniforms.uniforms.binding(),
        ) else { return Ok(()); };

        render_context.command_encoder().push_debug_group("grid");
        {
            let bind_group =
                render_context.render_device().create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("grid_bind_group"),
                    layout: &pipeline_res.bind_group_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource }],
                });

            let mut render_pass =
                render_context.begin_tracked_render_pass(wgpu::RenderPassDescriptor {
                    label: Some("grid_render_pass"),
                    color_attachments: &[Some(target.get_color_attachment(wgpu::Operations {
                        load: match settings.clear_color {
                            ClearColorConfig::Default => wgpu::LoadOp::Clear(clear_res.0.into()),
                            ClearColorConfig::Custom(color) => wgpu::LoadOp::Clear(color.into()),
                            ClearColorConfig::None => wgpu::LoadOp::Load,
                        },
                        store: true,
                    }))],
                    depth_stencil_attachment: None,
                });
            if let Some(viewport) = &camera.viewport {
                render_pass.set_camera_viewport(viewport);
            }
            render_pass.set_bind_group(0, &bind_group, &[offset.offset]);
            render_pass.set_render_pipeline(pipeline);
            render_pass.draw(0..4, 0..1);
        }
        render_context.command_encoder().pop_debug_group();
        Ok(())
    }
}

#[derive(Component)]
pub struct GridPipelineIds {
    pub id: CachedRenderPipelineId,
}

#[derive(Resource)]
pub struct GridPipeline {
    pub bind_group_layout: BindGroupLayout,
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct GridPipelineKey {
    pub msaa_samples: u32,
}

impl FromWorld for GridPipeline {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();

        let bind_group_layout =
            render_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("grid_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: Some(ViewUniform::min_size()),
                    },
                    count: None,
                }],
            });

        Self { bind_group_layout }
    }
}

impl SpecializedRenderPipeline for GridPipeline {
    type Key = GridPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("grid_pipeline".into()),
            layout: vec![self.bind_group_layout.clone()],
            vertex: VertexState {
                shader: GRID_SHADER_HANDLE.typed::<Shader>(),
                shader_defs: vec![],
                entry_point: "vertex".into(),
                buffers: vec![],
            },
            fragment: Some(FragmentState {
                shader: GRID_SHADER_HANDLE.typed::<Shader>(),
                shader_defs: vec![],
                entry_point: "fragment".into(),
                targets: vec![Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::bevy_default(),
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: key.msaa_samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            push_constant_ranges: Vec::new(),
        }
    }
}

pub fn prepare_grid_pipeline(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<GridPipeline>>,
    pipeline: Res<GridPipeline>,
    views: Query<(Entity, &GridSettings)>,
    msaa: Res<Msaa>,
) {
    for (entity, _settings) in &views {
        let pipeline_id = pipelines.specialize(&pipeline_cache, &pipeline, GridPipelineKey {
            msaa_samples: msaa.samples(),
        });
        commands.entity(entity).insert(GridPipelineIds { id: pipeline_id });
    }
}

#[derive(Component, Reflect, Clone)]
pub struct GridSettings {
    pub clear_color: ClearColorConfig,
}

// noinspection RsSortImplTraitMembers
impl ExtractComponent for GridSettings {
    type Filter = ();
    type Out = Self;
    type Query = (Read<Self>, Read<Camera>);

    fn extract_component((settings, _camera): QueryItem<'_, Self::Query>) -> Option<Self::Out> {
        Some(settings.clone())
    }
}
