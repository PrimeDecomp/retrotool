//! Helpers for the asset-backed Bevy replay in `render::tests`.

pub(in crate::render) use super::pipelines::TestVariant;
use super::*;

pub(in crate::render) fn shader_sources(world: &World, entity: Entity) -> [String; 2] {
    world
        .get::<OriginalShaderMesh>(entity)
        .unwrap()
        .prepared
        .draw
        .stages()
        .map(|stage| stage.0.to_owned())
}

pub(in crate::render) fn assert_atlas_draw_input(
    original: &OriginalShaderMesh,
    lookup: retrolib::format::room::SAtlasLookup,
) {
    let bits = |lookup: retrolib::format::room::SAtlasLookup| lookup.0.to_array().map(f32::to_bits);
    assert_eq!(original.baked_atlas_lookup.map(bits), Some(bits(lookup)));
    if !original.info().baked_atlas {
        return;
    }
    let transforms = preview_transforms(Mat4::IDENTITY, Mat4::IDENTITY, Mat4::IDENTITY);
    let mut frame = ModelShaderFrame {
        animation_time: Some(0.0),
        cull_camera_position: Some([0.; 3]),
        ..ModelShaderFrame::new(&transforms, sample_state(1).unwrap().constants)
    };
    let bindings = &original.prepared.draw.bindings;
    assert!(bindings.constants(&frame).unwrap_err().to_string().contains("BLAL"));
    frame.baked_atlas_lookup = Some(lookup);
    let first = bindings.constants(&frame).unwrap();
    let mut changed = lookup;
    changed.0.x += 0.125;
    frame.baked_atlas_lookup = Some(changed);
    let second = bindings.constants(&frame).unwrap();
    assert!(
        first.vertex.words() != second.vertex.words()
            || first.fragment.words() != second.fragment.words()
    );
    frame.baked_atlas_lookup = Some(lookup);
    let restored = bindings.constants(&frame).unwrap();
    assert_eq!(restored.vertex.words(), first.vertex.words());
    assert_eq!(restored.fragment.words(), first.fragment.words());
}

/// Research-only source substitution with the original pipeline/binding state.
pub(in crate::render) fn replace_shader_sources(
    world: &mut World,
    main: Entity,
    samples: u32,
    sources: [String; 2],
) {
    let entity = world
        .query::<(Entity, &MainEntity)>()
        .iter(world)
        .find_map(|(entity, main_entity)| (main_entity.id() == main).then_some(entity))
        .unwrap();
    let device = world.resource::<RenderDevice>().clone();
    world.resource_scope::<MaterialPipelines, _>(|world, mut pipelines| {
        let mut gpu = world.resource_mut::<GpuMeshes>();
        let instance = gpu.instances.get_mut(&entity).unwrap();
        let request = instance.pipelines.iter_mut().find(|(key, _)| key.0 == samples).unwrap().1;
        *request = pipelines.variant_for_test(
            &device,
            request,
            instance.mesh.layouts.clone(),
            TestVariant::Sources(sources),
        );
    });
}

pub(in crate::render) fn override_model_bounds(world: &mut World, entity: Entity, bounds: Aabb) {
    world.get_mut::<OriginalShaderMesh>(entity).unwrap().model_bounds = bounds;
}

pub(in crate::render) fn queued_in_view(
    main: &World,
    render: &World,
    camera: Entity,
    mesh: Entity,
) -> bool {
    use bevy::render::sync_world::RenderEntity;
    let camera = main.get::<RenderEntity>(camera).unwrap().id();
    let mesh = main.get::<RenderEntity>(mesh).unwrap().id();
    render.get::<OriginalViewDraws>(camera).is_some_and(|draws| draws.0.contains_key(&mesh))
}

pub(in crate::render) fn compilations_pending(world: &World) -> bool {
    world.resource::<GpuMeshes>().instances.values().any(|instance| {
        instance.pipelines.values().any(|request| matches!(request.pipeline(), Ok(None)))
    })
}

/// The host mesh batcher cannot combine our per-draw programs and bindings.
pub(in crate::render) fn assert_separate_transparent_draws(world: &World) {
    let draw_function =
        world.resource::<DrawFunctions<Transparent3d>>().read().id::<DrawOriginal>();
    for phase in world.resource::<ViewSortedRenderPhases<Transparent3d>>().values() {
        for item in phase.items.values().filter(|item| item.draw_function == draw_function) {
            assert_eq!(item.batch_range.len(), 1, "original transparent meshes were batched");
        }
    }
}

/// Verify a real material reaches the optimized path, independently of pixel
/// agreement: identical images alone would also pass without promotion.
pub(in crate::render) fn assert_local_storage(world: &World, entity: Entity, expected: bool) {
    let mesh = world.get::<OriginalShaderMesh>(entity).expect("original shader mesh");
    let source = mesh.prepared.draw.stages()[1].0;
    assert_eq!(source.contains("var local_words:"), expected);
    eprintln!("Fragment source: {} bytes; local storage: {expected}", source.len());
}

/// Inject a GPU status for the host feedback regression. This deliberately
/// tests readback/fallback isolation, not the compiler's instruction semantics.
pub(in crate::render) fn inject_test_fault(
    world: &mut World,
    main: Entity,
    words: [u32; 4],
) -> Result<()> {
    let entity = world
        .query::<(Entity, &MainEntity)>()
        .iter(world)
        .find_map(|(entity, main_entity)| (main_entity.id() == main).then_some(entity))
        .context("test mesh was not extracted")?;
    let gpu = world.resource::<GpuMeshes>();
    let faults = gpu
        .instances
        .get(&entity)
        .and_then(|instance| instance.faults.as_ref())
        .context("test mesh has no diagnostic buffer")?;
    world.resource::<RenderQueue>().write_buffer(&faults.buffer, 0, bytemuck::cast_slice(&words));
    Ok(())
}

/// Verify scene instances share immutable resources but retain separate faults.
pub(in crate::render) fn shared_instances(world: &World, shared_geometry: bool) {
    let gpu = world.resource::<GpuMeshes>();
    assert_eq!(gpu.instances.len(), 2);
    let mut instances = gpu.instances.values();
    let first = instances.next().unwrap();
    let second = instances.next().unwrap();
    assert_eq!(Arc::ptr_eq(&first.mesh, &second.mesh), shared_geometry);
    assert_eq!(first.pipelines.len(), 1);
    assert_eq!(second.pipelines.len(), 1);
    assert!(Arc::ptr_eq(
        first.pipelines.values().next().unwrap(),
        second.pipelines.values().next().unwrap()
    ));
    assert!(!Arc::ptr_eq(first.faults.as_ref().unwrap(), second.faults.as_ref().unwrap()));
}

/// Repeated placements share the prepared GPU mesh while each draw retains its
/// own diagnostic capture. Validate this across an entire loaded container.
pub(in crate::render) fn assert_instance_resources(
    main: &World,
    render: &World,
    entities: &[Entity],
) {
    let gpu = render.resource::<GpuMeshes>();
    let instances: HashMap<_, _> = gpu
        .instances
        .iter()
        .map(|(entity, instance)| (render.get::<MainEntity>(*entity).unwrap().id(), instance))
        .collect();
    let mut shared = HashMap::new();
    let mut captures = std::collections::HashSet::new();
    let mut count = 0;
    for entity in entities {
        let Some(original) = main.get::<OriginalShaderMesh>(*entity) else { continue };
        let instance = instances.get(entity).expect("original instance was not prepared");
        let key = Arc::as_ptr(&original.prepared);
        if let Some(previous) = shared.insert(key, &instance.mesh) {
            assert!(
                Arc::ptr_eq(previous, &instance.mesh),
                "placement duplicated immutable GPU resources"
            );
        }
        let capture = instance.faults.as_ref().expect("Remax draw has no diagnostic capture");
        assert!(captures.insert(Arc::as_ptr(capture)), "placements share a fault capture");
        count += 1;
    }
    assert!(count > shared.len(), "fixture did not exercise shared resources");
    eprintln!("{count} semantic placements share {} prepared GPU meshes, with separate diagnostic captures", shared.len());
}

/// Rebuild bindings/resources independently, as another loaded model would.
pub(in crate::render) fn independent_instance(original: &OriginalShaderMesh) -> OriginalShaderMesh {
    let source = &original.prepared;
    OriginalShaderMesh {
        diagnostics: Diagnostics::new(source.draw.has_faults()),
        model_bounds: original.model_bounds,
        baked_atlas_lookup: original.baked_atlas_lookup,
        prepared: Arc::new(PreparedMesh {
            draw: MaterialProgram::compile(
                source.draw.bindings.clone(),
                source.compiler,
                source.state.color_routing(),
            )
            .unwrap(),
            compiler: source.compiler,
            vertices: source.vertices.clone(),
            indices: source.indices.clone(),
            images: source
                .images
                .iter()
                .map(|image| match image {
                    PreparedImage::Image(handle) => PreparedImage::Image(handle.clone()),
                    PreparedImage::Cache(kind) => PreparedImage::Cache(*kind),
                })
                .collect(),
            state: source.state,
        }),
    }
}

/// Return a weak pipeline reference so lifetime assertions do not keep it alive.
pub(in crate::render) fn retained_pipeline(world: &World) -> impl Fn() -> bool {
    let gpu = world.resource::<GpuMeshes>();
    let request = gpu.instances.values().next().unwrap().pipelines.values().next().unwrap();
    let retained = Arc::downgrade(request);
    move || retained.strong_count() != 0
}

pub(in crate::render) fn no_resources(world: &World) -> bool {
    let gpu = world.resource::<GpuMeshes>();
    gpu.instances.is_empty()
        && gpu.prepared.is_empty()
        && world.resource::<MaterialPipelines>().is_empty_for_test()
}

/// Alter only the host compilation request. Native bindings/programs are intact.
pub(in crate::render) fn replace_pipeline(world: &mut World, main: Entity, variant: TestVariant) {
    let entity = world
        .query::<(Entity, &MainEntity)>()
        .iter(world)
        .find_map(|(entity, main_entity)| (main_entity.id() == main).then_some(entity))
        .unwrap();
    let device = world.resource::<RenderDevice>().clone();
    world.resource_scope::<MaterialPipelines, _>(|world, mut pipelines| {
        let mut gpu = world.resource_mut::<GpuMeshes>();
        let instance = gpu.instances.get_mut(&entity).unwrap();
        let request = instance.pipelines.values_mut().next().unwrap();
        *request =
            pipelines.variant_for_test(&device, request, instance.mesh.layouts.clone(), variant);
    });
}
