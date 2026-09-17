//! Frustum-edge replay against an uncropped draw and an independent box oracle.

use anyhow::ensure;
use bevy::camera::{
    primitives::{Aabb, MeshAabb},
    visibility::{NoFrustumCulling, RenderLayers},
    ScalingMode,
};

use super::*;
use crate::render::original_shader::{tests as renderer, OriginalShaderMesh};

struct GrassCulling {
    mesh: Entity,
    camera: Entity,
    centered_camera: Entity,
    target: Handle<Image>,
    authored: Aabb,
}

impl GrassCulling {
    fn queued(&self, app: &App, camera: Entity) -> bool {
        renderer::queued_in_view(app.world(), app.sub_app(RenderApp).world(), camera, self.mesh)
    }

    /// Compare one frame; report motion excluded by the native owner's bounds.
    fn compare_frame(&self, app: &mut App, world_bounds: &Aabb, centered: bool) -> Result<bool> {
        renderer::override_model_bounds(app.world_mut(), self.mesh, self.authored);
        let actual = capture(app, &self.target);
        ensure!(self.queued(app, self.centered_camera), "another camera lost its draw");
        let transform = app.world().get::<GlobalTransform>(self.camera).unwrap().to_matrix();
        let clip =
            app.world().get::<Camera>(self.camera).unwrap().clip_from_view() * transform.inverse();
        let expected_visible = box_intersects_clip(world_bounds, clip);
        ensure!(self.queued(app, self.camera) == expected_visible, "native box visibility differs");

        let uncropped = Aabb::from_min_max(Vec3::splat(-100_000.), Vec3::splat(100_000.));
        renderer::override_model_bounds(app.world_mut(), self.mesh, uncropped);
        let reference = capture(app, &self.target);
        ensure!(self.queued(app, self.camera), "uncropped reference was not queued");
        if expected_visible {
            assert_matching_pixels("authored bounds / uncropped draw", &actual, &reference);
            if centered {
                ensure!(lit_pixels(&actual) > 100, "vacuous center image");
            }
        } else {
            ensure!(lit_pixels(&actual) == 0, "culled draw produced pixels");
        }
        let info = app.world().get::<OriginalShaderMesh>(self.mesh).unwrap().info();
        ensure!(info.diagnostics.failure().is_none(), "grass GPU fault");
        renderer::override_model_bounds(app.world_mut(), self.mesh, self.authored);
        Ok(!expected_visible && lit_pixels(&reference) != 0)
    }

    /// This edge is inside the native world box but outside the old mesh OBB.
    fn verify_old_culling_rejects(&self, app: &mut App) -> Result<()> {
        let mesh = &app.world().get::<Mesh3d>(self.mesh).unwrap().0;
        let geometry = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .unwrap()
            .compute_aabb()
            .context("grass geometry bounds")?;
        app.world_mut().entity_mut(self.mesh).insert(geometry).remove::<NoFrustumCulling>();
        capture(app, &self.target);
        ensure!(!self.queued(app, self.camera), "old culling did not exercise the regression");
        app.world_mut().entity_mut(self.mesh).insert(NoFrustumCulling);
        capture(app, &self.target);
        ensure!(self.queued(app, self.camera), "authored culling did not restore the draw");
        Ok(())
    }

    fn verify_visibility_gates(&self, app: &mut App, placement: Entity) -> Result<()> {
        let centered = *app.world().get::<Transform>(self.centered_camera).unwrap();
        app.world_mut().entity_mut(self.camera).insert(centered);
        app.world_mut().entity_mut(self.centered_camera).insert(RenderLayers::layer(5));
        capture(app, &self.target);
        ensure!(
            self.queued(app, self.camera) && !self.queued(app, self.centered_camera),
            "render layers were not respected per view"
        );
        app.world_mut().entity_mut(self.centered_camera).remove::<RenderLayers>();
        app.world_mut().entity_mut(placement).insert(Visibility::Hidden);
        capture(app, &self.target);
        ensure!(
            !self.queued(app, self.camera) && !self.queued(app, self.centered_camera),
            "hidden placement remained queued"
        );
        app.world_mut().entity_mut(placement).insert(Visibility::Visible);
        capture(app, &self.target);
        ensure!(
            self.queued(app, self.camera) && self.queued(app, self.centered_camera),
            "showing placement did not restore both views"
        );
        Ok(())
    }
}

pub(super) fn verify_grass_culling(
    app: &mut App,
    placement: Entity,
    camera: Entity,
    target: &Handle<Image>,
    authored: &Aabb,
) -> Result<()> {
    let mesh = app.world().get::<Children>(placement).unwrap()[0];
    let original_transform = *app.world().get::<Transform>(placement).unwrap();
    let original_camera = *app.world().get::<Transform>(camera).unwrap();
    let original_projection = app.world().get::<Projection>(camera).unwrap().clone();
    let mut reflected = original_transform;
    reflected.rotation = Quat::from_euler(EulerRot::XYZ, 0.3, 0.7, -0.4) * reflected.rotation;
    reflected.scale *= Vec3::new(-1.4, 0.7, 1.2);
    let second_target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(128, 128, TextureFormat::Rgba8UnormSrgb, None));
    let centered_camera = app
        .world_mut()
        .spawn((Camera3d::default(), Msaa::Sample4, RenderTarget::Image(second_target.into())))
        .id();
    let replay =
        GrassCulling { mesh, camera, centered_camera, target: target.clone(), authored: *authored };
    let mut cases = 0;
    let mut clipped_motion = 0;
    for (pose, transform) in [("authored", original_transform), ("reflected", reflected)] {
        app.world_mut().entity_mut(placement).insert(transform);
        // Enumerate corners independently of the renderer's absolute-basis transform.
        let points = corners(authored).map(|p| transform.transform_point(p));
        let world_bounds = Aabb::enclosing(points).unwrap();
        let center = Vec3::from(world_bounds.center);
        let span = world_bounds.half_extents.max_element() * 2.;
        let view = Transform::from_translation(center + Vec3::new(1.4, 1., 1.8) * span)
            .looking_at(center, Vec3::Y);
        let right = *view.right();
        let world_radius = Vec3::from(world_bounds.half_extents).dot(right.abs());
        let geometry_radius =
            points.iter().map(|p| (*p - center).dot(right).abs()).fold(0.0f32, f32::max);
        ensure!(
            world_radius - geometry_radius > span * 0.01,
            "fixture does not distinguish world and local boxes"
        );
        let half_width = span;
        let projection = Projection::Orthographic(OrthographicProjection {
            near: 0.,
            far: span * 20.,
            scaling_mode: ScalingMode::FixedVertical { viewport_height: half_width * 2. },
            ..OrthographicProjection::default_3d()
        });
        app.world_mut().entity_mut(camera).insert(projection.clone());
        app.world_mut().entity_mut(centered_camera).insert((view, projection));
        let edge = half_width + (world_radius + geometry_radius) * 0.5;
        let outside = half_width + world_radius + span * 0.05;
        for profile in
            [remax::program::wgsl::Profile::Rendering, remax::program::wgsl::Profile::Checked]
        {
            app.world_mut().resource_mut::<ShaderPreview>().set_compiler(Compiler::Remax(profile));
            for (position, pan) in
                [("center", 0.), ("left", edge), ("right", -edge), ("outside", outside)]
            {
                let mut panned = view;
                panned.translation += right * pan;
                app.world_mut().entity_mut(camera).insert(panned);
                for time in [0., 1.] {
                    app.world_mut().resource_mut::<ShaderPreview>().set_animation_time(time)?;
                    settle_pipelines(app);
                    clipped_motion += usize::from(
                        replay
                            .compare_frame(app, &world_bounds, position == "center")
                            .with_context(|| {
                                format!("culling {pose}/{profile:?}/{position}/{time}")
                            })?,
                    );
                    if pose == "authored" && position == "left" && time == 0. {
                        replay.verify_old_culling_rejects(app)?;
                    }
                    cases += 1;
                }
            }
        }
    }
    replay.verify_visibility_gates(app, placement)?;
    app.world_mut().despawn(centered_camera);
    app.world_mut().entity_mut(placement).insert(original_transform);
    app.world_mut().entity_mut(camera).insert((original_camera, original_projection));
    eprintln!("Authored culling: {cases} cases, two simultaneous views; {clipped_motion} outside-box motion cases clipped by native owner policy");
    Ok(())
}

fn lit_pixels(image: &Image) -> usize {
    image
        .data
        .as_ref()
        .unwrap()
        .chunks_exact(4)
        .filter(|pixel| pixel[..3].iter().any(|&v| v != 0))
        .count()
}

fn corners(bounds: &Aabb) -> [Vec3; 8] {
    std::array::from_fn(|bits| {
        Vec3::new(
            if bits & 1 == 0 { bounds.min().x } else { bounds.max().x },
            if bits & 2 == 0 { bounds.min().y } else { bounds.max().y },
            if bits & 4 == 0 { bounds.min().z } else { bounds.max().z },
        )
    })
}

fn box_intersects_clip(bounds: &Aabb, clip: Mat4) -> bool {
    let points = corners(bounds).map(|p| clip * p.extend(1.));
    // Reversed depth; retain the viewer's side/near-plane policy, omitting far.
    let tests: [fn(Vec4) -> f32; 5] =
        [|p| p.x + p.w, |p| p.w - p.x, |p| p.y + p.w, |p| p.w - p.y, |p| p.w - p.z];
    tests.iter().all(|test| points.iter().any(|&p| test(p) > 0.))
}
