pub mod camera;
pub mod grid;
pub mod model;

use bevy::{prelude::*, render::primitives::Aabb};
use retrolib::format::{CAABox, CColor4f, CTransform4f};

#[derive(Component)]
pub struct TemporaryLabel;

#[inline]
pub fn convert_aabb(aabb: &CAABox) -> Aabb {
    let min = mint::Vector3::from(aabb.min);
    let max = mint::Vector3::from(aabb.max);
    Aabb::from_min_max(min.into(), max.into())
}

#[inline]
pub fn convert_transform(xf: &CTransform4f) -> Transform {
    let mtx = mint::ColumnMatrix4::from(*xf);
    Transform::from_matrix(mtx.into())
}

#[inline]
pub fn convert_color(value: &CColor4f) -> Color {
    Color::rgba_linear(value.r, value.g, value.b, value.a)
}
