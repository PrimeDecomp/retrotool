//! Matched, warmed viewer draws. Durations are serialized host observations,
//! not GPU timestamps or predictions of the window's overlapped frame rate.

use serde::Serialize;

use super::*;
use crate::render::{
    original_shader::{self, Compiler},
    scene_preview::PreviewMeshStatus,
};

#[derive(Serialize)]
struct Measurement {
    compiler: &'static str,
    scope: &'static str,
    original_draws: usize,
    fallback_draws: usize,
    width: u32,
    samples: u32,
    frames: usize,
    update_ms: f64,
    completion_wait_ms: f64,
    serialized_frame_ms: f64,
}

fn median(mut values: Vec<Duration>) -> f64 {
    values.sort_unstable();
    values[values.len() / 2].as_secs_f64() * 1000.
}

fn measure(
    app: &mut App,
    compiler: &'static str,
    scope: &'static str,
    width: u32,
    samples: u32,
) -> Measurement {
    let device = app.world().resource::<bevy::render::renderer::RenderDevice>().clone();
    let drain = || device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mut updates = Vec::new();
    let mut waits = Vec::new();
    let mut totals = Vec::new();
    for frame in 0..40 {
        drain();
        let started = Instant::now();
        app.update();
        let submitted = started.elapsed();
        let waiting = Instant::now();
        drain();
        let wait = waiting.elapsed();
        if frame >= 8 {
            updates.push(submitted);
            waits.push(wait);
            totals.push(started.elapsed());
        }
    }
    let mut query = app.world_mut().query::<(&PreviewMesh, &Visibility)>();
    let preview = app.world().resource::<ShaderPreview>();
    let mut original_draws = 0;
    let mut fallback_draws = 0;
    for (mesh, visibility) in query.iter(app.world()) {
        if *visibility == Visibility::Hidden {
            continue;
        }
        match mesh.status(preview) {
            PreviewMeshStatus::Original => original_draws += 1,
            PreviewMeshStatus::Fallback => fallback_draws += 1,
            other => panic!("benchmark material not ready: {other:?}"),
        }
    }
    Measurement {
        compiler,
        scope,
        original_draws,
        fallback_draws,
        width,
        samples,
        frames: updates.len(),
        update_ms: median(updates),
        completion_wait_ms: median(waits),
        serialized_frame_ms: median(totals),
    }
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn ship_rendering_performance_with_matched_inputs() -> Result<()> {
    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let brdf = PathBuf::from(std::env::var_os("MPR_BRDF_LUT").context("Set MPR_BRDF_LUT")?);
    let output = std::env::var_os("MPR_VIEWER_OUTPUT").map(PathBuf::from);
    // Counterfactual profiling stays test-only. It is not a compiler mode or a
    // correctness claim; compare its captures with an unchanged baseline.
    let variant = std::env::var_os("MPR_BENCH_SHADER_VARIANT").map(PathBuf::from);
    if let Some(path) = &output {
        std::fs::create_dir_all(path)?;
    }
    let mut preview = ShaderPreview::default();
    preview.request_brdf(brdf);
    let mut app = headless_app(preview);
    let server = app.world().resource::<AssetServer>().clone();
    for path in [
        "romfs/Worlds/FrontEnd/CharacterGallery/00-Ship.pak",
        "romfs/Preload/DuplicateData.pak",
        "romfs/Worlds/MP1/universeRoomMP1.pak",
    ] {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &root.join(path))?;
    }
    let handle = server.load::<ModelAsset>(asset_path(0, AssetRef {
        id: uuid::uuid!("c1be5bc1-92dd-4408-a02a-2f9f7ebe7a33"),
        kind: retrolib::format::cmdl::K_FORM_CMDL,
    }));
    advance_until(&mut app, |app| {
        assert!(!server.load_state(&handle).is_failed(), "ship failed to load");
        let Some(model) = app.world().resource::<Assets<ModelAsset>>().get(&handle) else {
            return false;
        };
        let state = model.get_load_state(&server);
        assert!(!state.is_failed(), "ship dependency failed: {state:?}");
        state.is_loaded()
    });
    let built = app.world_mut().resource_scope::<Assets<ModelAsset>, _>(|world, mut models| {
        let mut model = models.get_mut(&handle).unwrap();
        world.resource_scope::<Assets<Image>, _>(|world, mut images| {
            model.build_texture_images(world.resource::<Assets<TextureAsset>>(), &mut images);
        });
        load_model(&model, &mut world.resource_mut::<Assets<Mesh>>())
    })?;
    let mut orbit = ModelCamera::default();
    orbit.init(&built.aabb, true);
    let camera = app.world_mut().spawn((Camera3d::default(), orbit.transform, Msaa::Sample4)).id();
    let entities: Vec<_> = built
        .lod
        .first()
        .context("missing ship LOD0")?
        .meshes
        .iter()
        .map(|index| {
            let mesh = &built.meshes[index];
            let entity = app
                .world_mut()
                .spawn((
                    Mesh3d(mesh.mesh.clone()),
                    PreviewMesh::new(handle.clone(), index, MaterialKey {
                        material_idx: mesh.material_idx,
                        mesh_class: mesh.mesh_class,
                        mesh_two_sided: mesh.two_sided,
                        mesh_mirrored: false,
                    }),
                ))
                .id();
            (index, entity)
        })
        .collect();
    assert_eq!(entities.len(), 5);
    let mut measurements = Vec::new();
    let mut profile_images = std::collections::BTreeMap::new();
    for (label, compiler) in [
        ("legacy", Compiler::Legacy),
        ("rendering", Compiler::Remax(remax::program::wgsl::Profile::Rendering)),
        ("checked", Compiler::Remax(remax::program::wgsl::Profile::Checked)),
    ] {
        if variant.is_some() && label != "rendering" {
            continue;
        }
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(compiler);
        for (width, samples) in [(256, 4), (1024, 4), (2048, 4), (2048, 1)] {
            let target = app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(width, width, TextureFormat::Rgba8UnormSrgb, None));
            app.world_mut().entity_mut(camera).insert((
                RenderTarget::Image(target.clone().into()),
                if samples == 1 { Msaa::Off } else { Msaa::Sample4 },
            ));
            settle_pipelines(&mut app);
            if let Some(variant) = &variant {
                let sources = ["vertex.wgsl", "fragment.wgsl"].map(|name| {
                    std::fs::read_to_string(variant.join(name)).expect("benchmark stage source")
                });
                let hull = entities.iter().find(|(index, _)| *index == 0).unwrap().1;
                original_shader::tests::replace_shader_sources(
                    app.sub_app_mut(RenderApp).world_mut(),
                    hull,
                    samples,
                    sources,
                );
                settle_pipelines(&mut app);
            }
            for &(index, entity) in &entities {
                let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
                assert_eq!(
                    mesh.status(app.world().resource::<ShaderPreview>()),
                    PreviewMeshStatus::Original,
                    "{label} mesh{index}: {}",
                    mesh.description()
                );
                if matches!(compiler, Compiler::Remax(_)) {
                    checked_draw(&mut app, entity);
                }
                if let Some(output) = &output {
                    if width == 256 && variant.is_none() {
                        for (stage, source) in
                            original_shader::tests::shader_sources(app.world(), entity)
                                .iter()
                                .enumerate()
                        {
                            std::fs::write(
                                output.join(format!("{label}-mesh{index}-stage{stage}.wgsl")),
                                source,
                            )?;
                        }
                    }
                }
            }
            for scope in ["complete", "hull"] {
                for &(index, entity) in &entities {
                    app.world_mut().entity_mut(entity).insert(
                        if scope == "complete" || index == 0 {
                            Visibility::Visible
                        } else {
                            Visibility::Hidden
                        },
                    );
                }
                let result = measure(&mut app, label, scope, width, samples);
                eprintln!("{}", serde_json::to_string(&result)?);
                measurements.push(result);
                let image = capture(&mut app, &target);
                if label == "checked" {
                    assert_matching_pixels(
                        "ship checked/rendering",
                        &profile_images[&(width, samples, scope)],
                        &image,
                    );
                } else if label == "rendering" {
                    profile_images.insert((width, samples, scope), image.clone());
                }
                if let Some(output) = &output {
                    image
                        .try_into_dynamic()?
                        .save(output.join(format!("{label}-{scope}-{width}-{samples}x.png")))?;
                    std::fs::write(
                        output.join("measurements.json"),
                        serde_json::to_vec_pretty(&measurements)?,
                    )?;
                }
            }
            for &(_, entity) in &entities {
                app.world_mut().entity_mut(entity).insert(Visibility::Visible);
            }
        }
    }
    Ok(())
}
