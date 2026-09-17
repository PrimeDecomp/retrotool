//! Local-asset acceptance cases. Fallbacks are reported per mesh, never counted
//! as successful original-shader draws.

use std::path::Path;

use super::*;
use crate::{
    loaders::package::SharedPackageInfo,
    render::{original_shader::Compiler, scene_preview::PreviewMeshStatus},
};

const GALLERIES: [&str; 3] = ["02-VariaSuit", "20-Sheegoth", "30-PhazonElite"];

#[test]
#[ignore = "requires Metal, MPR_ROOT and MPR_BRDF_LUT; optionally MPR_VIEWER_OUTPUT"]
fn gallery_models_render_with_reported_coverage() -> Result<()> {
    let root = PathBuf::from(std::env::var_os("MPR_ROOT").context("Set MPR_ROOT")?);
    let brdf = PathBuf::from(std::env::var_os("MPR_BRDF_LUT").context("Set MPR_BRDF_LUT")?);
    let output = std::env::var_os("MPR_VIEWER_OUTPUT").map(PathBuf::from);
    if let Some(path) = &output {
        std::fs::create_dir_all(path)?;
    }
    let mut preview = ShaderPreview::default();
    preview.request_brdf(brdf);
    let mut app = headless_app(preview);
    let server = app.world().resource::<AssetServer>().clone();
    for name in GALLERIES {
        let path = root.join(format!("romfs/Worlds/FrontEnd/CharacterGallery/{name}.pak"));
        app.world_mut().resource_mut::<PackageSession>().open(&server, &path)?;
    }
    for path in ["romfs/Preload/DuplicateData.pak", "romfs/Worlds/MP1/universeRoomMP1.pak"] {
        app.world_mut().resource_mut::<PackageSession>().open(&server, &root.join(path))?;
    }
    advance_until(&mut app, |app| {
        app.world().resource::<SharedPackageInfo>().indexes().len() == GALLERIES.len() + 2
    });
    let packages = app.world().resource::<SharedPackageInfo>().indexes();
    for name in GALLERIES {
        let package = packages.iter().find(|package| package.name == name).unwrap();
        let models: Vec<_> = package
            .entries
            .iter()
            .filter(|entry| entry.kind == retrolib::format::cmdl::K_FORM_CMDL)
            .collect();
        assert!(!models.is_empty(), "{name} has no CMDL assets");
        eprintln!("{name}: {} CMDL assets", models.len());
        for model in models {
            render_model(
                &mut app,
                name,
                AssetRef { id: model.id, kind: model.kind },
                output.as_deref(),
            )?;
        }
    }
    Ok(())
}

fn render_model(
    app: &mut App,
    gallery: &str,
    asset: AssetRef,
    output: Option<&Path>,
) -> Result<()> {
    let server = app.world().resource::<AssetServer>().clone();
    let handle = server.load::<ModelAsset>(asset_path(0, asset));
    advance_until(app, |app| {
        assert!(!server.load_state(&handle).is_failed(), "{} failed to load", asset.id);
        let Some(model) = app.world().resource::<Assets<ModelAsset>>().get(&handle) else {
            return false;
        };
        let state = model.get_load_state(&server);
        assert!(!state.is_failed(), "{} dependency failed: {state:?}", asset.id);
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
    // Leave room for the full silhouette in the square capture, including the
    // tall crystals and wide arms that exceed the viewer's initial framing.
    orbit.transform.translation = orbit.origin + (orbit.transform.translation - orbit.origin) * 1.5;
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
        ))
        .id();
    settle_pipelines(app);
    let empty = capture(app, &target);
    let visible = &built.lod.first().context("Model has no LOD")?.meshes;
    let entities: Vec<_> = visible
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
    eprintln!("{gallery} {}: {}/{} meshes at LOD0", asset.id, entities.len(), built.meshes.len());
    let mut rendering_image: Option<Image> = None;
    let mut legacy_common: Option<Image> = None;
    for (label, compiler) in [
        ("legacy", Compiler::Legacy),
        ("rendering", Compiler::Remax(remax::program::wgsl::Profile::Rendering)),
        ("checked", Compiler::Remax(remax::program::wgsl::Profile::Checked)),
    ] {
        app.world_mut().resource_mut::<ShaderPreview>().set_compiler(compiler);
        settle_pipelines(app);
        super::super::original_shader::tests::assert_separate_transparent_draws(
            app.sub_app(RenderApp).world(),
        );
        let deadline = Instant::now() + Duration::from_secs(50);
        advance_until(app, |app| {
            let preview = app.world().resource::<ShaderPreview>();
            if Instant::now() >= deadline {
                let pending: Vec<_> = entities
                    .iter()
                    .filter_map(|&(index, entity)| {
                        let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
                        (mesh.status(preview) == PreviewMeshStatus::Preparing)
                            .then(|| format!("mesh {index}: {}", mesh.description()))
                    })
                    .collect();
                panic!("{gallery} {label} did not finish draws: {}", pending.join("\n"));
            }
            entities.iter().all(|&(_, entity)| {
                app.world().get::<PreviewMesh>(entity).unwrap().status(preview)
                    != PreviewMeshStatus::Preparing
            })
        });
        let mut original = 0;
        for &(index, entity) in &entities {
            let mesh = app.world().get::<PreviewMesh>(entity).unwrap();
            let status = mesh.status(app.world().resource::<ShaderPreview>());
            let material = &built.materials[built.meshes[index].material_idx].name;
            eprintln!(
                "{gallery} {label} mesh {index} {material}: {:?}\n{}",
                status,
                mesh.description()
            );
            if let Some(reason) = expected_fallback(gallery, index, compiler) {
                assert_eq!(status, PreviewMeshStatus::Fallback, "{gallery} mesh {index}");
                assert!(
                    mesh.description().contains(reason),
                    "unexpected fallback: {}",
                    mesh.description()
                );
            } else {
                assert_eq!(
                    status,
                    PreviewMeshStatus::Original,
                    "{gallery} mesh {index}: {}",
                    mesh.description()
                );
            }
            original += usize::from(status == PreviewMeshStatus::Original);
            if gallery == "20-Sheegoth" && index == 0 {
                if let Compiler::Remax(profile) = compiler {
                    super::super::original_shader::tests::assert_local_storage(
                        app.world(),
                        entity,
                        profile == remax::program::wgsl::Profile::Checked,
                    );
                }
            }
        }
        eprintln!("{gallery} {label}: {original}/{} original shader draws", entities.len());
        let image = capture(app, &target);
        assert_ne!(image.data, empty.data, "{gallery} {label} produced no image");
        if label == "checked" {
            assert_matching_pixels(
                &format!("{gallery} checked/rendering"),
                rendering_image.as_ref().unwrap(),
                &image,
            );
        } else if label == "rendering" {
            rendering_image = Some(image.clone());
        }
        if gallery == "02-VariaSuit" && matches!(compiler, Compiler::Remax(_)) {
            let glass = entities.iter().find(|(index, _)| *index == 10).unwrap().1;
            app.world_mut().entity_mut(glass).insert(Visibility::Hidden);
            settle_pipelines(app);
            let without_glass = capture(app, &target);
            assert_ne!(
                image.data, without_glass.data,
                "glass draw did not contribute to the image"
            );
            app.world_mut().entity_mut(glass).insert(Visibility::Visible);
            settle_pipelines(app);
        }
        if let Some(output) = output {
            image
                .try_into_dynamic()?
                .save(output.join(format!("{gallery}-{}-{label}.png", asset.id)))?;
        }
        // Compare the same supported geometry against the migration baseline.
        // A compiler's fallback meshes must not contaminate that comparison.
        for &(index, entity) in &entities {
            let common = expected_fallback(gallery, index, Compiler::Legacy).is_none()
                && expected_fallback(
                    gallery,
                    index,
                    Compiler::Remax(remax::program::wgsl::Profile::Rendering),
                )
                .is_none();
            app.world_mut().entity_mut(entity).insert(if common {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
        settle_pipelines(app);
        let common = capture(app, &target);
        if let Some(legacy) = &legacy_common {
            assert_matching_pixels(
                &format!("{gallery} legacy/{label} shared meshes"),
                legacy,
                &common,
            );
        } else {
            legacy_common = Some(common);
        }
        for &(_, entity) in &entities {
            app.world_mut().entity_mut(entity).insert(Visibility::Visible);
        }
    }
    for (_, entity) in entities {
        app.world_mut().despawn(entity);
    }
    app.world_mut().despawn(camera);
    settle_pipelines(app);
    Ok(())
}

// Retain the observed limits explicitly. A new unexpected fallback must fail the
// replay even if the approximate material still produces a plausible image.
fn expected_fallback(gallery: &str, mesh: usize, compiler: Compiler) -> Option<&'static str> {
    match (gallery, mesh, compiler) {
        ("02-VariaSuit", 10, Compiler::Legacy) => Some("Dual-source blending requires Remax"),
        ("20-Sheegoth", 0, Compiler::Legacy) => Some("word 0xef5400000007ff0c"),
        ("20-Sheegoth", 2 | 4, Compiler::Legacy) => {
            Some("vertex output 0xc0 is not assigned on every EXIT")
        }
        ("30-PhazonElite", 1, Compiler::Legacy) => Some("F2F requires the semantic compiler"),
        _ => None,
    }
}
