//! ROOM dependency loading, tab visibility and original material draws.

use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::{
    loaders::{modcon::ModConAsset, room::RoomAsset},
    render::{
        camera::SceneCamera,
        original_shader::{Compiler, OriginalShaderMesh},
        scene_preview::PreviewMeshStatus,
    },
    tabs::{room::RoomTab, EditorTab, TabState},
};

fn show_tab(app: &mut App, tab: &mut RoomTab, context: &egui::Context) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(512., 512.))),
        ..default()
    };
    let output = context.run_ui(input, |ui| {
        EditorTab::ui(tab, app.world_mut(), ui, &mut TabState::default());
    });
    output.drop_without_applying_deltas();
    // The application clears the shared target before its tab cameras. This
    // headless replay has only the tab camera, so it must perform that clear.
    for mut camera in
        app.world_mut().query_filtered::<&mut Camera, With<SceneCamera>>().iter_mut(app.world_mut())
    {
        camera.clear_color = bevy::camera::ClearColorConfig::Custom(Color::BLACK);
    }
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_loads_geometry_renders_and_releases_draws() -> Result<()> {
    replay_room(RoomFixture {
        path: "!Over_Master/00b_over_hall.pak",
        id: "74850a80-959c-44c6-817d-4686fcd91b7c",
        containers: 1,
    })
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_intro_world_models() -> Result<()> {
    replay_room(RoomFixture {
        path: "!Intro_Master/08e_Intro_ventshaft.pak",
        id: "08a88b1c-ba6a-42aa-8621-7d507dd1ba20",
        containers: 3,
    })
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_ruins_courtyard() -> Result<()> {
    replay_room(RoomFixture {
        path: "!RuinsWorld/8_courtyard.pak",
        id: "8f6016fa-446b-4df7-812a-b3032adc6209",
        containers: 1,
    })
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_ice_observatory() -> Result<()> {
    replay_room(RoomFixture {
        path: "!IceWorld/11_Ice_Observatory.pak",
        id: "a750163a-5c4d-495e-b4c4-c43a5d084670",
        containers: 1,
    })
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_lava_fiery_shores() -> Result<()> {
    replay_room(RoomFixture {
        path: "!Lava_Master/12_Over_fieryshores.pak",
        id: "5898a308-f7ee-4eab-a94e-661d4042f800",
        containers: 7,
    })
}

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn room_mines_world_models() -> Result<()> {
    replay_room(RoomFixture {
        path: "!Mines_Master/00j_Mines_connect.pak",
        id: "5f46b758-a6d7-45eb-b517-18baf91667ed",
        containers: 2,
    })
}

struct RoomFixture {
    path: &'static str,
    id: &'static str,
    containers: usize,
}

// Keep the remaining compiler boundary explicit, as in the gallery replay.
// Any new fallback fails the test. These are integration targets, not accepted
// substitutes for the original materials; remove each case when supported.
fn expected_fallback(label: &str, baked_atlas: bool) -> Option<&'static str> {
    match (label, baked_atlas) {
        ("model 5bc70441-b91d-44b6-a6db-ea11494ecf28, mesh child 0", true) => {
            Some("implicit sampling after branching or in a loop needs GPU execution analysis")
        }
        _ => None,
    }
}

fn replay_room(fixture: RoomFixture) -> Result<()> {
    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let brdf = PathBuf::from(std::env::var_os("MPR_BRDF_LUT").context("Set MPR_BRDF_LUT")?);
    let mut preview = ShaderPreview::default();
    preview.set_compiler(Compiler::Remax(remax::program::wgsl::Profile::Rendering));
    preview.request_brdf(brdf);
    let mut app = headless_app(preview);
    let server = app.world().resource::<AssetServer>().clone();
    let package = PathBuf::from("romfs/Worlds/MP1").join(fixture.path);
    let zone = package.parent().unwrap();
    for path in [
        package.clone(),
        PathBuf::from("romfs/Preload/DuplicateData.pak"),
        PathBuf::from("romfs/Worlds/MP1/universeRoomMP1.pak"),
        zone.join(zone.file_name().unwrap()).with_extension("pak"),
    ] {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &root.join(path))?;
    }
    let asset =
        AssetRef { id: Uuid::parse_str(fixture.id)?, kind: retrolib::format::room::K_FORM_ROOM };
    let handle = server.load::<RoomAsset>(asset_path(0, asset));
    advance_until(&mut app, |app| {
        assert!(!server.load_state(&handle).is_failed(), "ROOM failed to load");
        let Some(room) = app.world().resource::<Assets<RoomAsset>>().get(&handle) else {
            return false;
        };
        room.modcons.iter().all(|geometry| {
            assert!(!server.load_state(&geometry.handle).is_failed(), "ROOM MCON failed to load");
            let Some(container) =
                app.world().resource::<Assets<ModConAsset>>().get(&geometry.handle)
            else {
                return false;
            };
            container.models.iter().chain(container.world_models.values()).all(|handle| {
                assert!(!server.load_state(handle).is_failed(), "ROOM model failed to load");
                let Some(model) = app.world().resource::<Assets<ModelAsset>>().get(handle) else {
                    return false;
                };
                let state = model.get_load_state(&server);
                assert!(!state.is_failed(), "ROOM material dependency failed: {state:?}");
                state.is_loaded()
            })
        })
    });
    let mut tab = RoomTab::new(asset, handle);
    EditorTab::load(tab.as_mut(), app.world_mut());
    assert_eq!(tab.geometry.len(), fixture.containers);
    let roots: Vec<_> = tab
        .geometry
        .iter()
        .flat_map(|g| &g.models)
        .flat_map(|m| &m.loaded)
        .map(|i| i.entity)
        .collect();
    assert!(!roots.is_empty(), "ROOM contains no placements");
    let draws: Vec<_> =
        roots.iter().flat_map(|&root| app.world().get::<Children>(root).unwrap().iter()).collect();
    assert!(!draws.is_empty(), "ROOM contains no mesh draws");
    // Read expected associations from the source table, independently of the
    // viewer's regrouping of placements by model resource.
    let mut atlas_lookups = HashMap::new();
    let mut draw_labels = HashMap::new();
    let containers = app.world().resource::<Assets<ModConAsset>>();
    let models = app.world().resource::<Assets<ModelAsset>>();
    for geometry in &tab.geometry {
        let visual =
            containers.get(&geometry.source.handle).unwrap().inner.visual_data.as_ref().unwrap();
        for group in &geometry.models {
            let id = models.get(&group.handle).unwrap().asset_ref.id;
            let source = visual
                .instances()?
                .filter(|instance| visual.models[instance.model_index] == id)
                .map(|instance| instance.atlas_lookup.copied())
                .chain(
                    visual
                        .world_instances
                        .iter()
                        .enumerate()
                        .filter(|(_, instance)| instance.model == id)
                        .map(|(index, _)| visual.world_atlas_lookups.get(index).copied()),
                )
                .collect::<Vec<_>>();
            assert_eq!(group.loaded.len(), source.len(), "placement associations: {id}");
            for (loaded, source) in group.loaded.iter().zip(source) {
                for (index, child) in
                    app.world().get::<Children>(loaded.entity).unwrap().iter().enumerate()
                {
                    draw_labels.insert(child, format!("model {id}, mesh child {index}"));
                    atlas_lookups.insert(child, source.filter(|lookup| lookup.is_valid()));
                }
            }
        }
    }
    assert_eq!(atlas_lookups.len(), draws.len());
    let context = egui::Context::default();
    tab.visible_layers.fill(false);
    show_tab(&mut app, &mut tab, &context);
    let camera =
        app.world_mut().query_filtered::<Entity, With<SceneCamera>>().single(app.world())?;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_target_texture(
        512,
        512,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    app.world_mut().entity_mut(camera).insert(RenderTarget::Image(target.clone().into()));
    settle_pipelines(&mut app);
    let empty = capture(&mut app, &target);
    tab.visible_layers.fill(true);
    show_tab(&mut app, &mut tab, &context);
    let mut reference = None;
    for (label, profile) in [
        ("rendering", remax::program::wgsl::Profile::Rendering),
        ("checked", remax::program::wgsl::Profile::Checked),
    ] {
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(Compiler::Remax(profile));
        settle_pipelines(&mut app);
        advance_until(&mut app, |app| {
            draws.iter().all(|&entity| {
                let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
                mesh.status(app.world().resource::<ShaderPreview>()) != PreviewMeshStatus::Preparing
            })
        });
        let mut fallbacks = BTreeMap::new();
        let mut known_fallbacks = 0;
        for &entity in &draws {
            let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
            let status = mesh.status(app.world().resource::<ShaderPreview>());
            if let Some(reason) = expected_fallback(&draw_labels[&entity], true) {
                assert_eq!(status, PreviewMeshStatus::Fallback, "{}", draw_labels[&entity]);
                assert!(mesh.description().contains(reason), "{}", mesh.description());
                known_fallbacks += 1;
            } else if status != PreviewMeshStatus::Original {
                *fallbacks
                    .entry(format!("{}: {}", draw_labels[&entity], mesh.description()))
                    .or_insert(0) += 1;
            }
        }
        anyhow::ensure!(fallbacks.is_empty(), "{} {label} fallbacks: {fallbacks:#?}", fixture.path);
        for &entity in &draws {
            let Some(original) = app.world().get::<OriginalShaderMesh>(entity) else { continue };
            if let Some(lookup) = atlas_lookups[&entity] {
                super::super::original_shader::tests::assert_atlas_draw_input(original, lookup);
            } else {
                assert!(original.baked_atlas_lookup.is_none());
            }
        }
        let baked = draws
            .iter()
            .filter(|&&entity| {
                app.world()
                    .get::<OriginalShaderMesh>(entity)
                    .is_some_and(|mesh| mesh.info().baked_atlas)
            })
            .count();
        assert!(baked > 0, "ROOM did not select any baked variants");
        eprintln!("{} {label}: {baked} baked-atlas draws", fixture.path);
        let image = capture(&mut app, &target);
        assert_ne!(image.data, empty.data, "ROOM contains no rendered geometry");
        if let Some(output) = std::env::var_os("MPR_VIEWER_OUTPUT") {
            let output = PathBuf::from(output);
            std::fs::create_dir_all(&output)?;
            image
                .clone()
                .try_into_dynamic()?
                .save(output.join(format!("{}-{label}.png", fixture.id)))?;
        }
        if let Some(reference) = &reference {
            assert_matching_pixels("ROOM checked/rendering", reference, &image);
        }
        reference = Some(image);
        super::super::original_shader::tests::assert_instance_resources(
            app.world(),
            app.sub_app(RenderApp).world(),
            &draws,
        );
        eprintln!(
            "{} {label}: {} placements, {} original mesh draws, {known_fallbacks} known fallbacks",
            fixture.path,
            roots.len(),
            draws.len() - known_fallbacks
        );
    }
    app.world_mut()
        .resource_mut::<ShaderPreview>()
        .set_compiler(Compiler::Remax(remax::program::wgsl::Profile::Rendering));
    tab.use_baked_atlas = false;
    EditorTab::load(tab.as_mut(), app.world_mut());
    show_tab(&mut app, &mut tab, &context);
    settle_pipelines(&mut app);
    let preview = capture(&mut app, &target);
    assert_ne!(
        preview.data,
        reference.as_ref().unwrap().data,
        "Lightmap toggle did not change lighting"
    );
    for &entity in &draws {
        if let Some(mesh) = app.world().get::<OriginalShaderMesh>(entity) {
            assert!(!mesh.info().baked_atlas && mesh.baked_atlas_lookup.is_none());
        } else {
            let preview = app.world().get::<PreviewMesh>(entity).unwrap();
            anyhow::ensure!(
                expected_fallback(&draw_labels[&entity], false).is_some_and(|reason| {
                    preview.status(app.world().resource::<ShaderPreview>())
                        == PreviewMeshStatus::Fallback
                        && preview.description().contains(reason)
                }),
                "Lightmap disabled, {}: {}",
                draw_labels[&entity],
                preview.description()
            );
        }
    }
    let preview_draws = draws
        .iter()
        .filter(|&&entity| app.world().get::<OriginalShaderMesh>(entity).is_some())
        .count();
    eprintln!("{} preview lighting: {preview_draws} original mesh draws", fixture.path);
    tab.use_baked_atlas = true;
    EditorTab::load(tab.as_mut(), app.world_mut());
    show_tab(&mut app, &mut tab, &context);
    settle_pipelines(&mut app);
    assert_matching_pixels(
        "ROOM restored lightmap",
        reference.as_ref().unwrap(),
        &capture(&mut app, &target),
    );
    tab.visible_layers.fill(false);
    show_tab(&mut app, &mut tab, &context);
    assert_matching_pixels("ROOM hidden layers", &empty, &capture(&mut app, &target));
    assert!(EditorTab::close(tab.as_mut(), app.world_mut()));
    advance_until(&mut app, |app| {
        super::super::original_shader::tests::no_resources(app.sub_app(RenderApp).world())
    });
    for entity in roots.into_iter().chain(draws).chain([camera]) {
        assert!(app.world().get_entity(entity).is_err(), "ROOM retained an entity after close");
    }
    Ok(())
}
