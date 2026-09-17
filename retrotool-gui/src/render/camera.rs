use bevy::{
    camera::{primitives::Aabb, visibility::RenderLayers, ClearColorConfig, Viewport},
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
};
use egui::PointerButton;

use super::grid::GridSettings;

#[derive(Component)]
pub struct SceneCamera;

#[derive(Default)]
pub struct ModelCamera {
    pub transform: Transform,
    pub upside_down: bool,
    pub radius: f32,
    pub origin: Vec3,
    pub projection: Projection,
    entity: Option<Entity>,
    input_frame: Option<u64>,
}

impl ModelCamera {
    pub fn init(&mut self, aabb: &Aabb, center: bool) {
        let radius = (aabb.max() - aabb.min()).max_element() * 1.25;
        if center {
            self.origin = aabb.center.into();
        }
        let mut camera_xf =
            Transform::from_xyz(-radius, 5.0, radius).looking_at(self.origin, Vec3::Y);
        let rot_matrix = Mat3::from_quat(camera_xf.rotation);
        camera_xf.translation = self.origin + rot_matrix.mul_vec3(Vec3::new(0.0, 0.0, radius));
        self.transform = camera_xf;
        self.radius = radius;
    }

    /// Keep one camera per tab. UI passes only replace its requested state;
    /// the next update disables it unless the tab is visible again.
    pub fn show(
        &mut self,
        commands: &mut Commands,
        viewport: Viewport,
        layer: usize,
        environment: Option<EnvironmentMapLight>,
    ) {
        if viewport.physical_size.min_element() == 0 {
            return;
        }
        let id = *self.entity.get_or_insert_with(|| commands.spawn_empty().id());
        let mut entity = commands.entity(id);
        entity.insert((
            SceneCamera,
            Camera3d::default(),
            Camera {
                viewport: Some(viewport),
                order: layer as isize,
                clear_color: ClearColorConfig::None,
                ..default()
            },
            self.transform,
            self.projection.clone(),
            Msaa::Sample4,
            Tonemapping::TonyMcMapface,
            // The custom preview shader uses normalized light values, as the
            // old viewer did. Bevy's physical-light default (80) overexposes it.
            AmbientLight { brightness: 0.05, ..default() },
            GridSettings,
            RenderLayers::layer(layer),
        ));
        if let Some(environment) = environment {
            entity.insert(environment);
        } else {
            entity.remove::<EnvironmentMapLight>();
        }
    }

    pub fn close(&mut self, commands: &mut Commands) {
        if let Some(entity) = self.entity.take() {
            commands.entity(entity).despawn();
        }
    }

    pub fn pointer_ray(&self, rect: &egui::Rect, position: egui::Pos2) -> Option<Ray3d> {
        if !rect.contains(position) || rect.size().min_elem() <= 0.0 {
            return None;
        }
        let local = (position - rect.min) / rect.size();
        let ndc = Vec3::new(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 1.0);
        let view_position = self.projection.get_clip_from_view().inverse().project_point3(ndc);
        let direction = self.transform.rotation * view_position;
        Some(Ray3d::new(self.transform.translation, Dir3::new(direction).ok()?))
    }

    pub fn update(
        &mut self,
        rect: &egui::Rect,
        response: &egui::Response,
        scroll_delta: egui::Vec2,
    ) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        self.projection.update(rect.width(), rect.height());
        let frame = response.ctx.cumulative_frame_nr();
        if self.input_frame == Some(frame) {
            return;
        }
        self.input_frame = Some(frame);
        let mut any = false;
        let mut rotation_move = Vec2::ZERO;
        let mut pan = Vec2::ZERO;
        let scroll = {
            if response.hovered() {
                mint::Vector2::from(scroll_delta).into()
            } else {
                Vec2::ZERO
            }
        };
        if response.drag_started_by(PointerButton::Primary)
            || response.drag_stopped_by(PointerButton::Primary)
        {
            // only check for upside down when orbiting started or ended this frame
            // if the camera is "upside" down, panning horizontally would be inverted, so invert the input to make it correct
            let up = self.transform.rotation * Vec3::Y;
            self.upside_down = up.y <= 0.0;
        }
        if response.dragged_by(PointerButton::Primary) {
            rotation_move = mint::Vector2::from(response.drag_delta()).into();
        } else if response.dragged_by(PointerButton::Middle) {
            pan = mint::Vector2::from(response.drag_delta()).into();
        }
        if rotation_move.length_squared() > 0.0 {
            any = true;
            let delta_x = {
                let delta = rotation_move.x / rect.width() * std::f32::consts::PI * 2.0;
                if self.upside_down {
                    -delta
                } else {
                    delta
                }
            };
            let delta_y = rotation_move.y / rect.height() * std::f32::consts::PI;
            let yaw = Quat::from_rotation_y(-delta_x);
            let pitch = Quat::from_rotation_x(-delta_y);
            self.transform.rotation = yaw * self.transform.rotation; // rotate around global y axis
            self.transform.rotation *= pitch; // rotate around local x axis
        } else if pan.length_squared() > 0.0 {
            any = true;
            if let Projection::Perspective(projection) = &self.projection {
                pan *= Vec2::new(projection.fov * projection.aspect_ratio, projection.fov)
                    / Vec2::from(mint::Vector2::from(rect.size()));
            }
            // translate by local axes
            let right = self.transform.rotation * Vec3::X * -pan.x;
            let up = self.transform.rotation * Vec3::Y * pan.y;
            // make panning proportional to distance away from focus point
            let translation = (right + up) * self.radius;
            self.origin += translation;
        } else if scroll.y.abs() > 0.0 {
            any = true;
            self.radius -= (scroll.y / 50.0/* TODO ? */) * self.radius * 0.2;
            // dont allow zoom to reach zero or you get stuck
            self.radius = f32::max(self.radius, 0.05);
        }
        if any {
            // emulating parent/child to make the yaw/y-axis rotation behave like a turntable
            // parent = x and y rotation
            // child = z-offset
            let rot_matrix = Mat3::from_quat(self.transform.rotation);
            self.transform.translation =
                self.origin + rot_matrix.mul_vec3(Vec3::new(0.0, 0.0, self.radius));
        }
    }
}
