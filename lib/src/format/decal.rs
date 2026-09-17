//! Geometry-decal projection and the static stream-out stage.
//!
//! The native geometry shader rejects whole triangles and emits their original
//! vertices with UVs and six clip distances. The final material's vertex shader
//! exports those distances to the rasterizer; generation does not split polygons.

use anyhow::{ensure, Result};

use super::{retro_math, CTransform4f};

/// Projection axes in the game's coordinate system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionAxis {
    /// Native mode0, used by MCDD geometry decals.
    Y,
    /// Native mode1, also used by shadow-decal setup.
    Z,
}

/// Recovered SDecalStreamOutParams, excluding the unused bounding sphere.
#[derive(Clone, Debug)]
pub struct DecalProjection {
    /// Inward planes, evaluated as dot(normal, world_position) - distance.
    pub clip_planes: [[f32; 4]; 6],
    pub projection_plane: [f32; 4],
    pub origin: [f32; 3],
    pub u_axis: [f32; 3],
    pub v_axis: [f32; 3],
    pub minimum_normal_dot: f32,
}

/// The eleven meaningful floats emitted by the native geometry shader.
#[derive(Clone, Copy, Debug)]
pub struct DecalVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub clip_distances: [f32; 6],
}

impl DecalProjection {
    /// NRenderDecal::initialize_decal_stream_out_cmd (71000ed0a0). The caller
    /// supplies the already scale-separated box, as BuildModConDecals does.
    /// Plane construction follows transformed face winding, including reflections.
    pub fn from_box(
        transform: CTransform4f,
        half_extents: [f32; 3],
        axis: ProjectionAxis,
        angle_radians: f32,
    ) -> Result<Self> {
        let matrix: mint::RowMatrix3x4<f32> = transform.into();
        let rows: [[f32; 4]; 3] = matrix.into();
        ensure!(rows.iter().flatten().all(|v| v.is_finite()), "Nonfinite decal transform");
        ensure!(
            half_extents.iter().all(|v| v.is_finite() && *v >= 0.0) && angle_radians.is_finite(),
            "Invalid decal extents or angle"
        );
        let faces = box_faces(rows, half_extents);
        let (order, projection_face, origin_index, u_index, v_index) = match axis {
            ProjectionAxis::Y => ([2, 0, 3, 1, 4, 5], 5, 0, 3, 1),
            ProjectionAxis::Z => ([4, 3, 5, 2, 0, 1], 0, 1, 2, 0),
        };
        let face = faces[projection_face];
        let origin = face[origin_index];
        Ok(Self {
            clip_planes: order.map(|index| inward_plane(faces[index])),
            projection_plane: inward_plane(face),
            origin,
            u_axis: texture_axis(subtract(face[u_index], origin)),
            v_axis: texture_axis(subtract(face[v_index], origin)),
            minimum_normal_dot: retro_math::fast_cos_r(angle_radians),
        })
    }

    /// Static DecalGeometryStreamOut, SNVN source16. Positions are already in
    /// world space; `mirrored_receiver` is the source model transform's handedness.
    /// Preserves the native vertex order and accepts points exactly on a plane.
    /// Host f32 square root approximates Maxwell RSQ; this is not bit equivalence.
    pub fn project_triangle(
        &self,
        mut positions: [[f32; 3]; 3],
        mirrored_receiver: bool,
    ) -> Option<[DecalVertex; 3]> {
        if mirrored_receiver {
            positions.swap(1, 2);
        }
        let edge_a = subtract(positions[2], positions[1]);
        let edge_b = subtract(positions[0], positions[1]);
        let normal = cross_fused(edge_a, edge_b);
        let inverse_length = dot_fused(normal, normal).sqrt().recip();
        let normal = normal.map(|v| v * inverse_length);
        let projection_normal = plane_normal(self.projection_plane);
        if dot_fused(normal, projection_normal) < self.minimum_normal_dot {
            return None;
        }
        // NVN compares dot products directly with plane.w before subtracting it
        // for the output. Do not turn these into an all-vertices-inside test.
        let dots: [[f32; 6]; 3] = positions
            .map(|position| self.clip_planes.map(|plane| dot_fused(position, plane_normal(plane))));
        if (0..6).any(|plane| dots.iter().all(|vertex| vertex[plane] < self.clip_planes[plane][3]))
        {
            return None;
        }
        Some(std::array::from_fn(|index| {
            let position = positions[index];
            let relative = subtract(position, self.origin);
            DecalVertex {
                position,
                uv: [dot_fused(relative, self.u_axis), dot_fused(relative, self.v_axis)],
                clip_distances: std::array::from_fn(|plane| {
                    dots[index][plane] - self.clip_planes[plane][3]
                }),
            }
        }))
    }

    /// Geometry-stage constant buffer. The unused sphere and trailing padding
    /// are zero; the native POD payload is 180 bytes in a 192-byte allocation.
    pub fn uniform_words(&self) -> [f32; 48] {
        let mut words = [0.0; 48];
        for (index, plane) in self.clip_planes.iter().enumerate() {
            words[4 + index * 4..8 + index * 4].copy_from_slice(plane);
        }
        words[28..32].copy_from_slice(&self.projection_plane);
        words[32..35].copy_from_slice(&self.origin);
        words[35] = 1.0;
        words[36..39].copy_from_slice(&self.u_axis);
        words[40..43].copy_from_slice(&self.v_axis);
        words[44] = self.minimum_normal_dot;
        words
    }
}

fn box_faces(rows: [[f32; 4]; 3], extents: [f32; 3]) -> [[[f32; 3]; 4]; 6] {
    // COBBox::GetFace (7100095704): -Z,+Z,+X,-X,+Y,-Y, outward winding.
    const CORNERS: [[[f32; 3]; 4]; 6] = [
        [[1., 1., -1.], [1., -1., -1.], [-1., -1., -1.], [-1., 1., -1.]],
        [[-1., 1., 1.], [-1., -1., 1.], [1., -1., 1.], [1., 1., 1.]],
        [[1., 1., 1.], [1., -1., 1.], [1., -1., -1.], [1., 1., -1.]],
        [[-1., 1., -1.], [-1., -1., -1.], [-1., -1., 1.], [-1., 1., 1.]],
        [[1., 1., 1.], [1., 1., -1.], [-1., 1., -1.], [-1., 1., 1.]],
        [[1., -1., -1.], [1., -1., 1.], [-1., -1., 1.], [-1., -1., -1.]],
    ];
    CORNERS.map(|face| {
        face.map(|corner| {
            let point = std::array::from_fn(|axis| corner[axis] * extents[axis]);
            rows.map(|[x, y, z, translation]| dot_cpu([x, y, z], point) + translation)
        })
    })
}

fn inward_plane(face: [[f32; 3]; 4]) -> [f32; 4] {
    let a = subtract(face[1], face[0]);
    let b = subtract(face[2], face[0]);
    let normal = retro_math::normalized_or_zero([
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]);
    [-normal[0], -normal[1], -normal[2], -dot_cpu(normal, face[0])]
}

fn texture_axis(edge: [f32; 3]) -> [f32; 3] {
    let squared = dot_cpu(edge, edge);
    // Native uses two reciprocal refinements above this threshold.
    let reciprocal = if squared > 0.001 { squared.recip() } else { 1.0 };
    edge.map(|value| value * reciprocal)
}

fn plane_normal([x, y, z, _]: [f32; 4]) -> [f32; 3] { [x, y, z] }

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { std::array::from_fn(|i| a[i] - b[i]) }

fn dot_cpu(a: [f32; 3], b: [f32; 3]) -> f32 { (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2] }

fn dot_fused(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[2].mul_add(b[2], a[1].mul_add(b[1], a[0] * b[0]))
}

fn cross_fused(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1].mul_add(b[2], -a[2] * b[1]),
        a[2].mul_add(b[0], -a[0] * b[2]),
        a[0].mul_add(b[1], -a[1] * b[0]),
    ]
}

#[cfg(test)]
mod tests;
