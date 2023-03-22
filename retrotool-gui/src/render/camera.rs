use bevy::{prelude::*, render::primitives::Aabb};
use egui::PointerButton;

#[derive(Default)]
pub struct ModelCamera {
    pub transform: Transform,
    pub upside_down: bool,
    pub radius: f32,
    pub origin: Vec3,
    pub projection: Projection,
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

    pub fn update(
        &mut self,
        rect: &egui::Rect,
        response: &egui::Response,
        scroll_delta: egui::Vec2,
    ) {
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
            || response.drag_released_by(PointerButton::Primary)
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
