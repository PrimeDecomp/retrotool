//! MPR runtime light uniforms. SCachedLight packing: 0x710016b5c8.
//! These records are assembled from CLight; they are not serialized MTRL data.
//! D3D reflection calls the five vectors SLightSource::{mColor,mPosition,
//! mNegSpotDirection,mDistanceNormalization,mDistanceAttnCoeffs}. Names below
//! also describe packed w-components recovered from the engine and NVN shaders.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CachedLight {
    /// 0x00: intensity-scaled RGB; w blends material roughness toward1.
    pub color_roughness: [f32; 4],
    /// 0x10: transformed position; w is angular attenuation scale.
    pub position_cone_scale: [f32; 4],
    /// 0x20: NEGATED rotated light direction; w is angular attenuation bias.
    pub direction_cone_bias: [f32; 4],
    /// 0x30: distance scale/bias; z/w are zero in the runtime packer.
    pub distance_scale_bias: [f32; 4],
    /// 0x40: cubic coefficients a,b,c,d, evaluated ((a*t+b)*t+c)*t+d.
    pub distance_polynomial: [f32; 4],
}

impl CachedLight {
    pub const BYTE_LEN: usize = 0x50;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid cached light size");
        let row = |offset| std::array::from_fn(|i| read_float(bytes, offset + i * 4));
        Ok(Self {
            color_roughness: row(0),
            position_cone_scale: row(0x10),
            direction_cone_bias: row(0x20),
            distance_scale_bias: row(0x30),
            distance_polynomial: row(0x40),
        })
    }

    /// Original scalar equation (0x71001739a0), without an inverse-square term.
    pub fn distance_attenuation(&self, distance: f32) -> f32 {
        let t =
            (distance * self.distance_scale_bias[0] + self.distance_scale_bias[1]).clamp(0.0, 1.0);
        let [a, b, c, d] = self.distance_polynomial;
        ((a * t + b) * t + c) * t + d
    }

    /// cosine is dot(fragment-to-light unit vector, packed negated direction).
    /// The source66 shader uses this clamped value directly, without squaring.
    pub fn angle_attenuation(&self, cosine: f32) -> f32 {
        (cosine * self.position_cone_scale[3] + self.direction_cone_bias[3]).clamp(0.0, 1.0)
    }
}

/// A reflected CLPP uniform payload with three prefix vectors and an array of
/// unshadowed lights. Source66 allocates32 records (0xa30 total bytes). Parse
/// the exact reflected buffer span; shadow arrays may require separate layouts.
#[derive(Debug, Clone, Serialize)]
pub struct ClusteredLightBuffer {
    /// Reciprocal tile width/height in pixels, then grid width/height.
    pub cluster_size: [f32; 4],
    /// Reciprocal depth exponent, depth scale, camera near plane, depth range.
    /// SetupClusterContextBase currently uses exponent1 and slices/range.
    pub cluster_depth: [f32; 4],
    /// Grid width*height, grid width, mask of lights with node flag bit0 clear,
    /// zero. The two source66 PS programs do not apply that flag mask.
    pub cluster_index: [u32; 4],
    pub lights: Vec<CachedLight>,
}

impl ClusteredLightBuffer {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 0x30 && (bytes.len() - 0x30) % CachedLight::BYTE_LEN == 0,
            "Invalid clustered light buffer span"
        );
        Ok(Self {
            cluster_size: std::array::from_fn(|i| read_float(bytes, i * 4)),
            cluster_depth: std::array::from_fn(|i| read_float(bytes, 0x10 + i * 4)),
            cluster_index: std::array::from_fn(|i| {
                u32::from_le_bytes(bytes[0x20 + i * 4..0x24 + i * 4].try_into().unwrap())
            }),
            lights: bytes[0x30..]
                .chunks_exact(CachedLight::BYTE_LEN)
                .map(CachedLight::parse)
                .collect::<Result<_>>()?,
        })
    }

    /// Source66 TLDS coordinates: screen tiles flattened into texture X, depth
    /// slice in texture Y. Supply sample-adjusted pixel coordinates and native
    /// view-space Z (negative in front of the camera), not normalized UV/depth.
    ///
    /// No clamp to the texture extent is performed by the original shader.
    /// In particular, the far endpoint can produce Y==slice_count. The caller
    /// must handle texture access separately. Finite arithmetic is reproduced;
    /// Maxwell MUFU/FTZ/rounding and exact boundary equivalence are not emulated.
    pub fn mask_coordinate(&self, pixel: [f32; 2], view_z: f32) -> Result<[i32; 2]> {
        let [power, scale, near, range] = self.cluster_depth;
        ensure!(
            pixel
                .iter()
                .chain(self.cluster_size[..2].iter())
                .chain(self.cluster_depth.iter())
                .copied()
                .chain([view_z])
                .all(f32::is_finite)
                && self.cluster_size[0] > 0.0
                && self.cluster_size[1] > 0.0
                && power > 0.0
                && scale >= 0.0
                && range >= 0.0,
            "Invalid clustered mask parameters"
        );
        let distance = (-view_z - near).clamp(0.0, range);
        // log2(0) -> -infinity -> exp2 -> 0 in the original dependency chain.
        let depth = if distance == 0.0 { 0.0 } else { (distance.log2() * power).exp2() * scale };
        let tile = [pixel[0] * self.cluster_size[0], pixel[1] * self.cluster_size[1]];
        let checked_integer = |value: f32| -> Result<i32> {
            ensure!(
                value.is_finite() && (-2147483648.0..2147483648.0).contains(&value),
                "Cluster coordinate exceeds signed32-bit conversion range"
            );
            Ok(value as i32)
        };
        let x = checked_integer(tile[0])?;
        let y = checked_integer(tile[1])?;
        Ok([x.wrapping_add(y.wrapping_mul(self.cluster_index[1] as i32)), checked_integer(depth)?])
    }
}

/// Runtime raster culling uniforms for ClusteredCullRaster, technique x.
/// Permutation0 is 0x790 bytes; permutation1 adds eight secondary spheres and
/// is 0x810 bytes. NVN sources75/76 and D3D PS177/179 agree on these layouts.
#[derive(Debug, Clone, Serialize)]
pub struct ClusteredCullBuffer {
    /// Light counts0/1, grid width*height, grid width.
    pub params: [u32; 4],
    pub spheres0: Vec<[f32; 4]>,
    pub spheres1: Vec<[f32; 4]>,
    pub planes_x: Vec<[f32; 4]>,
    pub planes_y: Vec<[f32; 4]>,
    pub planes_z: Vec<[f32; 4]>,
}

impl ClusteredCullBuffer {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(matches!(bytes.len(), 0x790 | 0x810), "Unknown clustered raster cull layout");
        let secondary = if bytes.len() == 0x810 { 8 } else { 0 };
        let mut cursor = 0x10;
        let mut vectors = |count| {
            let out = (0..count)
                .map(|i| std::array::from_fn(|k| read_float(bytes, cursor + i * 16 + k * 4)))
                .collect();
            cursor += count * 16;
            out
        };
        let params = std::array::from_fn(|i| {
            u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        });
        ensure!(
            params[0] <= 32 && params[1] <= secondary as u32,
            "Cull light count exceeds reflected array"
        );
        Ok(Self {
            params,
            spheres0: vectors(32),
            spheres1: vectors(secondary),
            planes_x: vectors(35),
            planes_y: vectors(20),
            planes_z: vectors(33),
        })
    }

    /// Evaluate the original raster shader for an in-range mask texel. The
    /// caller supplies only rows belonging to the actual render-target height;
    /// reflected plane capacity alone does not identify initialized slices.
    /// Tangency is asymmetric: >= for the low plane, < for the high plane.
    pub fn mask_at_texel(&self, texel: [u32; 2]) -> Result<[u32; 2]> {
        ensure!(self.params[3] != 0 && texel[0] < self.params[2], "Invalid cluster mask texel");
        let cell = [texel[0] % self.params[3], texel[0] / self.params[3], texel[1]];
        let arrays = [&self.planes_x, &self.planes_y, &self.planes_z];
        let mut planes = [[0.0; 4]; 6];
        for axis in 0..3 {
            let i = cell[axis] as usize;
            ensure!(
                i < arrays[axis].len().saturating_sub(1),
                "Cull plane index outside reflected array"
            );
            planes[axis * 2] = arrays[axis][i];
            planes[axis * 2 + 1] = arrays[axis][i + 1];
        }
        let mut result = [0u32; 2];
        for (channel, spheres) in [&self.spheres0, &self.spheres1].into_iter().enumerate() {
            ensure!(
                self.params[channel] as usize <= spheres.len() && self.params[channel] <= 32,
                "Cull light count exceeds sphere array"
            );
            for (i, sphere) in spheres[..self.params[channel] as usize].iter().enumerate() {
                if cluster_sphere_visible(*sphere, planes) {
                    result[channel] |= 1 << i;
                }
            }
        }
        Ok(result)
    }
}

/// Both raster-cull shaders use these six sphere/plane comparisons. Sphere
/// xyz and plane xyz/w must share the original scene coordinate space.
pub fn cluster_sphere_visible(sphere: [f32; 4], planes: [[f32; 4]; 6]) -> bool {
    planes.chunks_exact(2).all(|pair| {
        let dot = |p: [f32; 4]| p[0] * sphere[0] + p[1] * sphere[1] + p[2] * sphere[2];
        dot(pair[0]) >= pair[0][3] - sphere[3] && dot(pair[1]) < pair[1][3] + sphere[3]
    })
}

fn read_float(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_cull_preserves_asymmetric_plane_tangency() {
        // Both original PS177/179 and NVN75/76 use >= low and < high.
        let cell = [
            [1., 0., 0., 0.],
            [1., 0., 0., 1.],
            [0., 1., 0., 0.],
            [0., 1., 0., 1.],
            [0., 0., 1., 0.],
            [0., 0., 1., 1.],
        ];
        assert!(cluster_sphere_visible([0., 0.5, 0.5, 0.], cell));
        assert!(!cluster_sphere_visible([1., 0.5, 0.5, 0.], cell));
        assert!(cluster_sphere_visible([-0.5, 0.5, 0.5, 0.5], cell));
        assert!(!cluster_sphere_visible([1.5, 0.5, 0.5, 0.5], cell));
    }
}
