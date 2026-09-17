//! MCON replay through the same dependency and instance creation path as its tab.

use std::collections::BTreeMap;

use super::*;
use crate::{
    loaders::modcon::ModConAsset,
    render::{original_shader::Compiler, scene_preview::PreviewMeshStatus},
    tabs::{modcon::ModConTab, EditorTab},
};

#[path = "mcon/bounds.rs"]
mod bounds;

#[test]
#[ignore = "requires Metal and MPR_ROOT"]
fn world_model_placements_are_loaded_and_released() -> Result<()> {
    use retrolib::format::{cmdl::K_FORM_WMDL, mcon::K_FORM_MCON};

    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let mut app = headless_app(ShaderPreview::default());
    let server = app.world().resource::<AssetServer>().clone();
    for path in [
        "romfs/Preload/DuplicateData.pak",
        "romfs/Worlds/MP1/universeRoomMP1.pak",
        "romfs/Worlds/MP1/!Intro_Master/!Intro_Master.pak",
        "romfs/Worlds/MP1/!Mines_Master/!Mines_Master.pak",
    ] {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &root.join(path))?;
    }
    for (path, id) in [
        ("!Intro_Master/08e_Intro_ventshaft.pak", "6e68a798-b164-4dcc-bea0-3d43e7cafc06"),
        ("!Mines_Master/00j_Mines_connect.pak", "a85485e0-9686-4c86-b8ad-82188814d79c"),
        ("!Mines_Master/00o_mines_connect.pak", "99f91c92-5e1a-4492-bffd-115a208a390f"),
    ] {
        app.world_mut()
            .resource_mut::<PackageSession>()
            .open(&server, &root.join("romfs/Worlds/MP1").join(path))?;
        let asset = AssetRef { id: Uuid::parse_str(id)?, kind: K_FORM_MCON };
        let handle = server.load::<ModConAsset>(asset_path(0, asset));
        advance_until(&mut app, |app| {
            assert!(!server.load_state(&handle).is_failed(), "MCON failed: {path}");
            let Some(mcon) = app.world().resource::<Assets<ModConAsset>>().get(&handle) else {
                return false;
            };
            mcon.models.iter().chain(mcon.world_models.values()).all(|handle| {
                assert!(!server.load_state(handle).is_failed(), "model failed: {path}");
                let Some(model) = app.world().resource::<Assets<ModelAsset>>().get(handle) else {
                    return false;
                };
                let state = model.get_load_state(&server);
                assert!(!state.is_failed(), "model dependency failed: {state:?}");
                state.is_loaded()
            })
        });
        let mcon = app.world().resource::<Assets<ModConAsset>>().get(&handle).unwrap();
        let visual = mcon.inner.visual_data.as_ref().unwrap();
        assert_eq!(visual.world_instances.len(), 1);
        let placement = &visual.world_instances[0];
        let world_model = mcon.world_models[&placement.model].clone();
        let native: Mat4 = mint::ColumnMatrix4::from(placement.transform).into();
        let expected_instances = visual.transforms.len() + visual.world_instances.len();
        let mut tab = ModConTab::new(asset, handle);
        EditorTab::load(tab.as_mut(), app.world_mut());
        assert_eq!(tab.models.iter().map(|m| m.loaded.len()).sum::<usize>(), expected_instances);
        let model = tab.models.iter().find(|m| m.handle == world_model).unwrap();
        let loaded_asset = app.world().resource::<Assets<ModelAsset>>().get(&world_model).unwrap();
        assert_eq!(loaded_asset.asset_ref.kind, K_FORM_WMDL);
        assert_eq!(model.loaded.len(), 1);
        let entity = model.loaded[0].entity;
        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
            assert!(model.placements[0]
                .transform
                .transform_point(point)
                .abs_diff_eq(native.transform_point3(point), 0.0001));
        }
        let children = app.world().get::<Children>(entity).unwrap().to_vec();
        assert!(!children.is_empty(), "WMDL has no mesh draws: {path}");
        assert!(children.iter().all(|&child| app.world().get::<PreviewMesh>(child).is_some()));
        eprintln!(
            "{path}: {expected_instances} placements, WMDL {}: {} mesh draws",
            loaded_asset.asset_ref.id,
            children.len()
        );
        assert!(EditorTab::close(tab.as_mut(), app.world_mut()));
        assert!(app.world().get_entity(entity).is_err());
        assert!(children.into_iter().all(|child| app.world().get_entity(child).is_err()));
    }
    Ok(())
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn area_mcon_preserves_instances_and_renders_materials() -> Result<()> {
    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let brdf = PathBuf::from(std::env::var_os("MPR_BRDF_LUT").context("Set MPR_BRDF_LUT")?);
    let mut preview = ShaderPreview::default();
    preview.set_compiler(Compiler::Remax(remax::program::wgsl::Profile::Rendering));
    preview.request_brdf(brdf);
    let mut app = headless_app(preview);
    let server = app.world().resource::<AssetServer>().clone();
    for path in [
        "romfs/Worlds/MP1/!Over_Master/00b_over_hall.pak",
        "romfs/Preload/DuplicateData.pak",
        "romfs/Worlds/MP1/universeRoomMP1.pak",
        "romfs/Worlds/MP1/!Over_Master/!Over_Master.pak",
    ] {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &root.join(path))?;
    }
    let asset = AssetRef {
        id: Uuid::parse_str("02030e75-bf4d-46bb-bacd-db134243ed6e")?,
        kind: retrolib::format::mcon::K_FORM_MCON,
    };
    let handle = server.load::<ModConAsset>(asset_path(0, asset));
    advance_until(&mut app, |app| {
        assert!(!server.load_state(&handle).is_failed(), "MCON failed to load");
        let Some(mcon) = app.world().resource::<Assets<ModConAsset>>().get(&handle) else {
            return false;
        };
        mcon.models.iter().all(|handle| {
            assert!(!server.load_state(handle).is_failed(), "MCON model failed to load");
            let Some(model) = app.world().resource::<Assets<ModelAsset>>().get(handle) else {
                return false;
            };
            let state = model.get_load_state(&server);
            assert!(!state.is_failed(), "MCON model dependency failed: {state:?}");
            state.is_loaded()
        })
    });
    let mcon = app.world().resource::<Assets<ModConAsset>>().get(&handle).unwrap();
    let visual = mcon.inner.visual_data.as_ref().unwrap();
    assert_eq!(visual.models.len(), 16);
    assert_eq!(visual.transforms.len(), 316);
    assert!(
        visual.instances()?.all(|instance| instance.color.to_array() == [1.; 4]),
        "this preview test does not yet bind MCON color modulation"
    );
    let mut native_transforms = BTreeMap::<Uuid, Vec<Mat4>>::new();
    for instance in visual.instances()? {
        let matrix = mint::ColumnMatrix4::from(*instance.transform);
        native_transforms
            .entry(visual.models[instance.model_index])
            .or_default()
            .push(matrix.into());
    }
    let mut tab = ModConTab::new(asset, handle);
    EditorTab::load(tab.as_mut(), app.world_mut());
    assert_eq!(tab.models.len(), 16);
    assert_eq!(tab.models.iter().map(|model| model.loaded.len()).sum::<usize>(), 316);

    let mut draws = Vec::new();
    let mut world_min = Vec3::splat(f32::INFINITY);
    let mut world_max = Vec3::splat(f32::NEG_INFINITY);
    let mut placements = Vec::new();
    for model in &tab.models {
        let asset = app.world().resource::<Assets<ModelAsset>>().get(&model.handle).unwrap();
        for (instance, (loaded, placement)) in
            model.loaded.iter().zip(&model.placements).enumerate()
        {
            let transform = &placement.transform;
            assert_eq!(app.world().get::<Transform>(loaded.entity), Some(transform));
            let native = native_transforms[&asset.asset_ref.id][instance];
            placements.push((loaded.entity, native));
            // Compare against stored affine data, not the decomposition used to
            // create the entity. This catches lost shear or reflected axes.
            for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
                assert!(
                    transform
                        .transform_point(point)
                        .abs_diff_eq(native.transform_point3(point), 0.0001),
                    "MCON transform changed for {} instance {instance}",
                    asset.asset_ref.id
                );
            }
            for corner in 0..8 {
                let point = Vec3::new(
                    if corner & 1 == 0 { model.aabb.min().x } else { model.aabb.max().x },
                    if corner & 2 == 0 { model.aabb.min().y } else { model.aabb.max().y },
                    if corner & 4 == 0 { model.aabb.min().z } else { model.aabb.max().z },
                );
                let world = native.transform_point3(point);
                world_min = world_min.min(world);
                world_max = world_max.max(world);
            }
            let children = app.world().get::<Children>(loaded.entity).unwrap();
            draws.extend(
                children
                    .iter()
                    .enumerate()
                    .map(|(mesh, entity)| (entity, asset.asset_ref.id, instance, mesh)),
            );
        }
    }
    assert!(tab.camera.origin.abs_diff_eq((world_min + world_max) * 0.5, 0.0001));
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_target_texture(
        512,
        512,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            tab.camera.transform,
            Msaa::Sample4,
            RenderTarget::Image(target.clone().into()),
        ))
        .id();
    settle_pipelines(&mut app);
    for (entity, native) in placements {
        let world = app.world().get::<GlobalTransform>(entity).unwrap();
        for point in [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z] {
            assert!(world
                .transform_point(point)
                .abs_diff_eq(native.transform_point3(point), 0.0001));
        }
    }
    let empty = capture(&mut app, &target);
    for model in &tab.models {
        for loaded in &model.loaded {
            app.world_mut().entity_mut(loaded.entity).insert(Visibility::Visible);
        }
    }
    let mut rendering = None;
    let mut material_failures = Vec::new();
    for (label, profile) in [
        ("rendering", remax::program::wgsl::Profile::Rendering),
        ("checked", remax::program::wgsl::Profile::Checked),
    ] {
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(Compiler::Remax(profile));
        settle_pipelines(&mut app);
        // Every instance is inside the framing bounds. Wait for actual draws,
        // not just successful CPU translation or pipeline creation.
        advance_until(&mut app, |app| {
            draws.iter().all(|&(entity, _, _, _)| {
                let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
                mesh.status(app.world().resource::<ShaderPreview>()) != PreviewMeshStatus::Preparing
            })
        });
        let mut summaries = BTreeMap::new();
        let mut failures = Vec::new();
        for &(entity, model, instance, mesh_index) in &draws {
            let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
            let status = mesh.status(app.world().resource::<ShaderPreview>());
            summaries.entry((model, mesh_index)).or_insert_with(|| (status, mesh.description()));
            if status != PreviewMeshStatus::Original {
                failures.push(format!(
                    "{model} instance {instance} mesh {mesh_index}: {}",
                    mesh.description()
                ));
            }
        }
        for ((model, mesh), (status, description)) in summaries {
            eprintln!("MCON {label} {model} LOD0 mesh {mesh}: {status:?}\n{description}");
        }
        eprintln!(
            "MCON {label}: {} instances, {} mesh draws, {} failures",
            316,
            draws.len(),
            failures.len()
        );
        let image = capture(&mut app, &target);
        assert_ne!(image.data, empty.data, "area produced no image");
        if let Some(output) = std::env::var_os("MPR_VIEWER_OUTPUT") {
            let output = PathBuf::from(output);
            std::fs::create_dir_all(&output)?;
            image
                .clone()
                .try_into_dynamic()?
                .save(output.join(format!("mcon-{}-{label}.png", asset.id)))?;
        }
        material_failures.extend(failures.into_iter().map(|failure| format!("{label}: {failure}")));
        let entities = draws.iter().map(|&(entity, _, _, _)| entity).collect::<Vec<_>>();
        super::super::original_shader::tests::assert_instance_resources(
            app.world(),
            app.sub_app(RenderApp).world(),
            &entities,
        );
        if let Some(rendering) = &rendering {
            assert_matching_pixels("MCON checked/rendering", rendering, &image);
        } else {
            rendering = Some(image);
        }
    }
    verify_wind_playback(&mut app, &tab, camera, &target)?;
    assert!(EditorTab::close(tab.as_mut(), app.world_mut()));
    app.world_mut().despawn(camera);
    advance_until(&mut app, |app| {
        super::super::original_shader::tests::no_resources(app.sub_app(RenderApp).world())
    });
    for &(entity, _, _, _) in &draws {
        assert!(app.world().get_entity(entity).is_err(), "closing MCON retained a child mesh");
    }
    assert!(
        material_failures.is_empty(),
        "MCON material failures:\n{}",
        material_failures.join("\n")
    );
    Ok(())
}

fn verify_wind_playback(
    app: &mut App,
    tab: &ModConTab,
    camera: Entity,
    target: &Handle<Image>,
) -> Result<()> {
    let grass_id = Uuid::parse_str("c80f28dd-dab6-45ff-a3f6-0b7dd8026292")?;
    let mut grass = None;
    for model in &tab.models {
        let id =
            app.world().resource::<Assets<ModelAsset>>().get(&model.handle).unwrap().asset_ref.id;
        for loaded in &model.loaded {
            app.world_mut().entity_mut(loaded.entity).insert(Visibility::Hidden);
        }
        if id == grass_id {
            grass = Some(model);
        }
    }
    let grass = grass.context("grass model missing from area")?;
    let entity = grass.loaded[0].entity;
    let transform = app.world().get::<GlobalTransform>(entity).unwrap().affine();
    let bounds = super::super::transform_aabb(&grass.aabb, &transform);
    let mut view = super::super::camera::ModelCamera::default();
    view.init(&bounds, true);
    app.world_mut().entity_mut(camera).insert(view.transform);
    settle_pipelines(app);
    let empty = capture(app, target);
    app.world_mut().entity_mut(entity).insert(Visibility::Visible);

    let mut previous_profile = None;
    for profile in
        [remax::program::wgsl::Profile::Rendering, remax::program::wgsl::Profile::Checked]
    {
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(Compiler::Remax(profile));
        let mut snapshots = Vec::new();
        for seconds in [0.0, 1.0, 0.0] {
            app.world_mut().resource_mut::<ShaderPreview>().set_animation_time(seconds)?;
            settle_pipelines(app);
            let mesh = app.world().get::<Children>(entity).unwrap()[0];
            let info = app
                .world()
                .get::<super::super::original_shader::OriginalShaderMesh>(mesh)
                .context("grass fell back during animation")?
                .info();
            let completed = info.diagnostics.completed();
            let image = capture(app, target);
            advance_until(app, |_| {
                info.diagnostics.completed() > completed || info.diagnostics.failure().is_some()
            });
            assert_eq!(info.diagnostics.failure(), None);
            assert_ne!(image.data, empty.data, "grass produced no pixels");
            snapshots.push(image);
        }
        assert_ne!(
            snapshots[0].data, snapshots[1].data,
            "wind clock did not change the rendered grass"
        );
        assert_eq!(
            snapshots[0].data, snapshots[2].data,
            "seeking wind back to zero changed its image"
        );
        if let Some(previous) = previous_profile {
            for (previous, current) in std::iter::zip(previous, &snapshots) {
                assert_matching_pixels("WIND checked/rendering", &previous, current);
            }
        }
        previous_profile = Some(snapshots);
    }
    eprintln!("WIND: both profiles animate one isolated grass placement and reproduce its image after seeking");
    bounds::verify_grass_culling(app, entity, camera, target, &grass.aabb)?;
    Ok(())
}
