pub mod camera;
pub mod grid;
pub mod material_state;
pub mod model;
pub mod original_shader;
pub mod scene;
pub mod scene_lighting;
pub mod scene_preview;

use bevy::{camera::primitives::Aabb, prelude::*};
use retrolib::format::{CAABox, CColor4f, CTransform4f};

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

/// World-aligned bounds of an affine-transformed local box. The absolute basis
/// accounts for every corner, including rotations, reflections and shear.
pub fn transform_aabb(aabb: &Aabb, transform: &bevy::math::Affine3A) -> Aabb {
    let basis = transform.matrix3;
    Aabb {
        center: transform.transform_point3a(aabb.center),
        half_extents: basis.x_axis.abs() * aabb.half_extents.x
            + basis.y_axis.abs() * aabb.half_extents.y
            + basis.z_axis.abs() * aabb.half_extents.z,
    }
}

#[inline]
pub fn convert_color(value: &CColor4f) -> LinearRgba {
    LinearRgba::new(value.r, value.g, value.b, value.a)
}

#[cfg(test)]
mod tests;
