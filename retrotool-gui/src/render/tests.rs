//! Opt-in replay through Bevy's actual asset, material and render schedules.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::{
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{CachedPipelineState, PipelineCache, TextureFormat},
        view::window::screenshot::{Screenshot, ScreenshotCaptured},
        RenderApp,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};
use uuid::Uuid;

use super::{
    camera::ModelCamera,
    convert_aabb,
    grid::GridPlugin,
    model::load_model,
    original_shader::OriginalShaderPlugin,
    scene_preview::{PreviewMesh, ShaderPreview, ShaderPreviewPlugin},
};
use crate::{
    loaders::{
        model::{MaterialKey, ModelAsset},
        package::{asset_path, PackageSession, PackageSourcePlugin},
        texture::TextureAsset,
        RetroAssetPlugin,
    },
    material::CustomMaterial,
    AssetRef,
};

mod gallery;
mod mcon;
mod performance;
mod room;

#[test]
fn transformed_bounds_enclose_all_corners() {
    use bevy::{camera::primitives::Aabb, math::Affine3A};
    let local = Aabb::from_min_max(Vec3::new(-2., -1., -3.), Vec3::new(1., 4., 2.));
    let rotation = Affine3A::from_scale_rotation_translation(
        Vec3::new(-2., 0.5, 3.),
        Quat::from_euler(EulerRot::XYZ, 0.4, 0.7, -0.8),
        Vec3::new(8., -3., 11.),
    );
    let shear = Affine3A::from_mat3_translation(
        Mat3::from_cols(Vec3::X, Vec3::new(0.7, 1., 0.), Vec3::new(0.2, 0.3, 1.)),
        Vec3::ZERO,
    );
    for transform in [rotation, rotation * shear] {
        let actual = super::transform_aabb(&local, &transform);
        let corners = (0..8).map(|bits| {
            let point = Vec3::new(
                if bits & 1 == 0 { local.min().x } else { local.max().x },
                if bits & 2 == 0 { local.min().y } else { local.max().y },
                if bits & 4 == 0 { local.min().z } else { local.max().z },
            );
            transform.transform_point3(point)
        });
        let expected = Aabb::enclosing(corners).unwrap();
        assert!(actual.min().abs_diff_eq(expected.min(), 0.00001));
        assert!(actual.max().abs_diff_eq(expected.max(), 0.00001));
    }
}

#[derive(Resource, Default)]
struct Capture(Option<Image>);

#[test]
fn bundled_environment_maps_decode_as_linear_cubemaps() {
    use bevy::{
        asset::RenderAssetUsages,
        image::{CompressedImageFormats, ImageSampler, ImageType},
        render::render_resource::TextureViewDimension,
    };

    for (name, width, mips) in [
        ("papermill_diffuse_rgb9e5_zstd.ktx2", 32, 1),
        ("papermill_specular_rgb9e5_zstd.ktx2", 512, 9),
    ] {
        let bytes = std::fs::read(format!("{}/assets/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let image = Image::from_buffer(
            &bytes,
            ImageType::Extension("ktx2"),
            CompressedImageFormats::NONE,
            false,
            ImageSampler::default(),
            RenderAssetUsages::default(),
        )
        .unwrap();
        assert_eq!(image.texture_descriptor.format, TextureFormat::Rgb9e5Ufloat);
        assert_eq!(image.texture_descriptor.size.width, width);
        assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 6);
        assert_eq!(image.texture_descriptor.mip_level_count, mips);
        assert_eq!(
            image.texture_view_descriptor.unwrap().dimension,
            Some(TextureViewDimension::Cube)
        );
    }
}

fn advance_until(app: &mut App, ready: impl Fn(&App) -> bool) -> (usize, Duration) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut frames = 0;
    let mut slowest = Duration::ZERO;
    while !ready(app) {
        assert!(Instant::now() < deadline, "viewer replay did not settle");
        let start = Instant::now();
        app.update();
        slowest = slowest.max(start.elapsed());
        frames += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    (frames, slowest)
}

fn settle_pipelines(app: &mut App) {
    // Allow extraction, visibility and the first material queue to run before
    // observing pipeline readiness. Compilation can finish on worker threads.
    for _ in 0..8 {
        app.update();
    }
    let start = Instant::now();
    let (frames, slowest) = advance_until(app, |app| {
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        cache.pipelines().all(|pipeline| matches!(pipeline.state, CachedPipelineState::Ok(_)))
            && !super::original_shader::tests::compilations_pending(app.sub_app(RenderApp).world())
    });
    eprintln!("Pipeline wait: {:?}, {frames} frames; slowest update {slowest:?}", start.elapsed());
    for _ in 0..4 {
        app.update();
    }
}

fn capture(app: &mut App, target: &Handle<Image>) -> Image {
    app.world_mut().resource_mut::<Capture>().0 = None;
    app.world_mut().spawn(Screenshot::image(target.clone())).observe(
        |event: On<ScreenshotCaptured>, mut capture: ResMut<Capture>| {
            capture.0 = Some(event.image.clone());
        },
    );
    advance_until(app, |app| app.world().resource::<Capture>().0.is_some());
    app.world_mut().resource_mut::<Capture>().0.take().unwrap()
}

fn checked_draw(app: &mut App, entity: Entity) {
    let info =
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).unwrap().info();
    advance_until(app, |_| {
        info.diagnostics.completed() > 0 || info.diagnostics.failure().is_some()
    });
    assert_eq!(info.diagnostics.failure(), None);
}

fn headless_app(preview: ShaderPreview) -> App {
    let mut app = App::new();
    app.add_plugins(PackageSourcePlugin)
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>()
                .disable::<PipelinedRenderingPlugin>(),
        )
        .add_plugins((
            RetroAssetPlugin,
            MaterialPlugin::<CustomMaterial>::default(),
            OriginalShaderPlugin,
            ShaderPreviewPlugin,
            GridPlugin,
        ))
        .insert_resource(preview)
        .insert_resource(ClearColor(Color::BLACK))
        .init_resource::<Capture>()
        // No swapchain provides frame-latency backpressure in this harness.
        // Drain the previous frame before submitting another, so expensive
        // materials cannot accumulate thousands of frames ahead of readback.
        .add_systems(Last, |device: Res<bevy::render::renderer::RenderDevice>| {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(30)),
                })
                .expect("headless frame did not finish on the GPU");
        });
    app.finish();
    app.cleanup();
    app
}

#[test]
#[ignore = "requires Metal, MPR_PAKS and MPR_BRDF_LUT"]
fn viewer_renders_original_and_preview_materials_after_upgrade() -> Result<()> {
    let paths = std::env::var_os("MPR_PAKS").context("Set MPR_PAKS")?;
    let brdf = PathBuf::from(std::env::var_os("MPR_BRDF_LUT").context("Set MPR_BRDF_LUT")?);
    let mut preview = ShaderPreview::default();
    preview.request_brdf(brdf);
    let mut app = headless_app(preview);
    let server = app.world().resource::<AssetServer>().clone();
    for path in std::env::split_paths(&paths) {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &path)?;
    }
    let handle = server.load::<ModelAsset>(asset_path(0, AssetRef {
        id: Uuid::parse_str("2a3bbc26-c384-43ab-b9ba-a16b8560ed5f")?,
        kind: retrolib::format::cmdl::K_FORM_CMDL,
    }));
    advance_until(&mut app, |app| {
        assert!(!server.load_state(&handle).is_failed(), "model failed");
        app.world()
            .resource::<Assets<ModelAsset>>()
            .get(&handle)
            .is_some_and(|model| model.get_load_state(&server).is_loaded())
    });
    let built = app.world_mut().resource_scope::<Assets<ModelAsset>, _>(|world, mut models| {
        let mut model = models.get_mut(&handle).unwrap();
        world.resource_scope::<Assets<Image>, _>(|world, mut images| {
            model.build_texture_images(world.resource::<Assets<TextureAsset>>(), &mut images);
        });
        load_model(&model, &mut world.resource_mut::<Assets<Mesh>>())
    })?;
    let mut orbit = ModelCamera::default();
    orbit.init(
        &convert_aabb(
            &app.world().resource::<Assets<ModelAsset>>().get(&handle).unwrap().inner.head.bounds,
        ),
        true,
    );
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_target_texture(
        256,
        256,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    app.world_mut().spawn((
        Camera3d::default(),
        super::grid::GridSettings,
        AmbientLight { brightness: 0.05, ..default() },
        EnvironmentMapLight {
            diffuse_map: server.load("papermill_diffuse_rgb9e5_zstd.ktx2"),
            specular_map: server.load("papermill_specular_rgb9e5_zstd.ktx2"),
            intensity: 1.0,
            ..default()
        },
        orbit.transform,
        Msaa::Sample4,
        RenderTarget::Image(target.clone().into()),
    ));
    settle_pipelines(&mut app);
    let empty = capture(&mut app, &target);
    let mesh = &built.meshes[0];
    let entity = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.mesh.clone()),
            PreviewMesh::new(handle.clone(), 0, MaterialKey {
                material_idx: mesh.material_idx,
                mesh_class: mesh.mesh_class,
                mesh_two_sided: mesh.two_sided,
                mesh_mirrored: false,
            }),
        ))
        .id();
    settle_pipelines(&mut app);
    assert!(
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).is_some(),
        "original shader fell back"
    );
    let original = capture(&mut app, &target);
    assert!(app.world().get::<bevy::camera::visibility::NoFrustumCulling>(entity).is_some());
    assert_ne!(original.data, empty.data, "original shader did not draw");
    app.world_mut().resource_mut::<ShaderPreview>().set_enabled(false);
    settle_pipelines(&mut app);
    assert!(app.world().get::<MeshMaterial3d<CustomMaterial>>(entity).is_some());
    assert!(app.world().get::<bevy::camera::visibility::NoFrustumCulling>(entity).is_none());
    let fallback = capture(&mut app, &target);
    assert_ne!(fallback.data, empty.data, "preview material did not draw");
    app.world_mut().resource_mut::<ShaderPreview>().set_enabled(true);
    settle_pipelines(&mut app);
    let restored = capture(&mut app, &target);
    assert_eq!(original.data, restored.data, "toggling renderer changed the original draw");
    for profile in
        [remax::program::wgsl::Profile::Rendering, remax::program::wgsl::Profile::Checked]
    {
        let compiler = super::original_shader::Compiler::Remax(profile);
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(compiler);
        settle_pipelines(&mut app);
        let info = app
            .world()
            .get::<super::original_shader::OriginalShaderMesh>(entity)
            .unwrap_or_else(|| {
                panic!(
                    "semantic compiler fell back: {}",
                    app.world().get::<PreviewMesh>(entity).unwrap().description()
                )
            })
            .info();
        assert_eq!(info.compiler, compiler);
        advance_until(&mut app, |_| {
            info.diagnostics.completed() > 0 || info.diagnostics.failure().is_some()
        });
        assert_eq!(info.diagnostics.failure(), None);
        let semantic = capture(&mut app, &target);
        let difference = original
            .data
            .as_ref()
            .unwrap()
            .iter()
            .zip(semantic.data.as_ref().unwrap())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(difference <= 1, "{profile:?} pixel difference: {difference}");
        eprintln!(
            "Bevy {profile:?}: max RGBA8 difference {difference}, {} completed GPU checks",
            info.diagnostics.completed()
        );
        assert_eq!(info.diagnostics.failure(), None);
    }
    let previous =
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).unwrap().info();
    super::original_shader::tests::inject_test_fault(
        app.sub_app_mut(RenderApp).world_mut(),
        entity,
        [0, u32::MAX, 8, 0x778],
    )?;
    advance_until(&mut app, |app| {
        app.world().get::<MeshMaterial3d<CustomMaterial>>(entity).is_some()
    });
    assert!(previous.diagnostics.failure().unwrap().contains("0x778"));
    assert!(app.world().get::<PreviewMesh>(entity).unwrap().description().contains("0x778"));
    assert!(app.world().get::<super::original_shader::OriginalShaderMesh>(entity).is_none());
    assert!(app.world().get::<bevy::camera::visibility::NoFrustumCulling>(entity).is_none());
    app.world_mut().resource_mut::<ShaderPreview>().set_compiler(
        super::original_shader::Compiler::Remax(remax::program::wgsl::Profile::Rendering),
    );
    settle_pipelines(&mut app);
    let replacement =
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).unwrap().info();
    advance_until(&mut app, |_| {
        replacement.diagnostics.completed() > 0 || replacement.diagnostics.failure().is_some()
    });
    assert_eq!(replacement.diagnostics.failure(), None);
    assert!(
        previous.diagnostics.failure().is_some(),
        "replacement reset the old diagnostic handle"
    );
    // A second scene instance shares geometry, textures and its pipeline request,
    // while keeping an independent GPU status accumulator.
    let original_mesh =
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).unwrap().instance();
    let duplicate = app.world_mut().spawn((Mesh3d(mesh.mesh.clone()), original_mesh)).id();
    settle_pipelines(&mut app);
    checked_draw(&mut app, duplicate);
    super::original_shader::tests::shared_instances(app.sub_app(RenderApp).world(), true);
    app.world_mut().despawn(duplicate);
    settle_pipelines(&mut app);
    let independent = super::original_shader::tests::independent_instance(
        app.world().get::<super::original_shader::OriginalShaderMesh>(entity).unwrap(),
    );
    let duplicate = app.world_mut().spawn((Mesh3d(mesh.mesh.clone()), independent)).id();
    settle_pipelines(&mut app);
    checked_draw(&mut app, duplicate);
    super::original_shader::tests::shared_instances(app.sub_app(RenderApp).world(), false);
    assert_eq!(capture(&mut app, &target).data, original.data);
    app.world_mut().despawn(duplicate);
    settle_pipelines(&mut app);
    let retained = super::original_shader::tests::retained_pipeline(app.sub_app(RenderApp).world());
    app.world_mut().entity_mut(entity).insert(Visibility::Hidden);
    for _ in 0..8 {
        app.update();
    }
    assert!(retained(), "hiding discarded the compiled pipeline");
    app.world_mut().entity_mut(entity).insert(Visibility::Visible);
    settle_pipelines(&mut app);
    assert_eq!(capture(&mut app, &target).data, original.data);

    use super::original_shader::tests::{replace_pipeline, TestVariant};
    replace_pipeline(app.sub_app_mut(RenderApp).world_mut(), entity, TestVariant::FreshNames);
    settle_pipelines(&mut app);
    assert_eq!(
        capture(&mut app, &target).data,
        original.data,
        "fresh worker compilation changed pixels"
    );

    // Real wgpu module/pipeline validation errors must reach the same fallback
    // path as execution faults, without invoking the uncaptured-error handler.
    for (variant, reason) in [
        (TestVariant::InvalidShader, "parsing error"),
        (TestVariant::MissingEntryPoint, "Original material pipeline"),
    ] {
        replace_pipeline(app.sub_app_mut(RenderApp).world_mut(), entity, variant);
        advance_until(&mut app, |app| {
            app.world().get::<MeshMaterial3d<CustomMaterial>>(entity).is_some()
        });
        assert!(app
            .world()
            .get::<PreviewMesh>(entity)
            .unwrap()
            .description()
            .contains("Original shader compilation:"));
        let description = app.world().get::<PreviewMesh>(entity).unwrap().description();
        assert!(description.contains(reason), "unexpected compilation failure: {description}");
        app.world_mut().resource_mut::<ShaderPreview>().set_enabled(false);
        app.update();
        app.world_mut().resource_mut::<ShaderPreview>().set_enabled(true);
        settle_pipelines(&mut app);
        assert!(app.world().get::<super::original_shader::OriginalShaderMesh>(entity).is_some());
    }
    // Drop a draw with fresh compilation pending. Queued work can be cancelled;
    // a driver call already running may finish but must not retain scene owners.
    replace_pipeline(app.sub_app_mut(RenderApp).world_mut(), entity, TestVariant::FreshNames);
    let retained = super::original_shader::tests::retained_pipeline(app.sub_app(RenderApp).world());
    app.world_mut().despawn(entity);
    advance_until(&mut app, |app| {
        super::original_shader::tests::no_resources(app.sub_app(RenderApp).world())
    });
    assert!(!retained(), "closed draw retained its pipeline");
    Ok(())
}

fn assert_matching_pixels(label: &str, expected: &Image, actual: &Image) {
    assert_eq!(expected.texture_descriptor.size, actual.texture_descriptor.size);
    assert_eq!(expected.texture_descriptor.format, actual.texture_descriptor.format);
    let expected = expected.data.as_ref().unwrap();
    let actual = actual.data.as_ref().unwrap();
    assert_eq!(expected.len(), actual.len());
    let difference = expected.iter().zip(actual).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    eprintln!("{label}: maximum RGBA8 difference {difference}");
    assert!(difference <= 1, "{label}: rendered images differ");
}
