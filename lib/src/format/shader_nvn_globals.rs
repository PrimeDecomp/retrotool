//! MPR's uploaded `uc_globalTransforms` buffer, populated at 0x71000f95c4.
//! This is runtime uniform data, not a chunk serialized in MTRL files.
//! Four 4x4 matrices are transposed for upload; six 3x4 transforms retain rows.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct NvnGlobalTransforms {
    /// 0x000: projection * model_to_view, four column vectors.
    pub model_to_clip_columns: [[f32; 4]; 4],
    /// 0x040: inverse transpose of model_to_view's 3x3, with zero row.w.
    pub model_view_inverse_transpose_rows: [[f32; 4]; 3],
    /// 0x070: native_world_to_view * model_to_world, three row vectors.
    pub model_to_view_rows: [[f32; 4]; 3],
    /// 0x0a0: the bound model transform.
    pub model_to_world_rows: [[f32; 4]; 3],
    /// 0x0d0: inverse of the bound model transform.
    pub world_to_model_rows: [[f32; 4]; 3],
    /// 0x100: the native camera transform, after the engine's axis conversion.
    pub world_to_view_rows: [[f32; 4]; 3],
    /// 0x130: world_to_view * the transform supplied to BindWorldToBaked.
    /// UpdateBakedToViewTransforms names this result; the setter's input-space
    /// naming still requires caller analysis.
    pub baked_to_view_rows: [[f32; 4]; 3],
    /// 0x160: the bound projection, four column vectors.
    pub projection_columns: [[f32; 4]; 4],
    /// 0x1a0: inverse of the bound projection, four column vectors.
    pub inverse_projection_columns: [[f32; 4]; 4],
    /// 0x1e0: the matrix supplied to BindCustom0, four column vectors.
    pub custom0_columns: [[f32; 4]; 4],
}

impl NvnGlobalTransforms {
    pub const BYTE_LEN: usize = 0x220;

    /// Serialize explicitly supplied runtime matrices in their uploaded layout.
    /// This does not compute camera transforms or choose a projection convention.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::BYTE_LEN);
        for vectors in [
            self.model_to_clip_columns.as_slice(),
            self.model_view_inverse_transpose_rows.as_slice(),
            self.model_to_view_rows.as_slice(),
            self.model_to_world_rows.as_slice(),
            self.world_to_model_rows.as_slice(),
            self.world_to_view_rows.as_slice(),
            self.baked_to_view_rows.as_slice(),
            self.projection_columns.as_slice(),
            self.inverse_projection_columns.as_slice(),
            self.custom0_columns.as_slice(),
        ] {
            bytes.extend(vectors.iter().flatten().flat_map(|v| v.to_le_bytes()));
        }
        bytes
    }

    /// Decode an exact buffer payload in its uploaded little-endian layout.
    /// NaNs/infinities are preserved, including those from singular transforms.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid NVN global transform buffer size");
        let vectors = |offset: usize| {
            std::array::from_fn(|i| {
                f32::from_le_bytes(bytes[offset + i * 4..offset + i * 4 + 4].try_into().unwrap())
            })
        };
        Ok(Self {
            model_to_clip_columns: std::array::from_fn(|i| vectors(i * 16)),
            model_view_inverse_transpose_rows: std::array::from_fn(|i| vectors(0x40 + i * 16)),
            model_to_view_rows: std::array::from_fn(|i| vectors(0x70 + i * 16)),
            model_to_world_rows: std::array::from_fn(|i| vectors(0xa0 + i * 16)),
            world_to_model_rows: std::array::from_fn(|i| vectors(0xd0 + i * 16)),
            world_to_view_rows: std::array::from_fn(|i| vectors(0x100 + i * 16)),
            baked_to_view_rows: std::array::from_fn(|i| vectors(0x130 + i * 16)),
            projection_columns: std::array::from_fn(|i| vectors(0x160 + i * 16)),
            inverse_projection_columns: std::array::from_fn(|i| vectors(0x1a0 + i * 16)),
            custom0_columns: std::array::from_fn(|i| vectors(0x1e0 + i * 16)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uploaded_matrix_layout_round_trips_raw_words() {
        let bytes = (0..NvnGlobalTransforms::BYTE_LEN / 4)
            .flat_map(|i| (0x7f800000u32.wrapping_add(i as u32 * 12347)).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(NvnGlobalTransforms::parse(&bytes).unwrap().to_bytes(), bytes);
    }
}
