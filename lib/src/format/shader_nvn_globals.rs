//! MPR's global transform and system-value buffers, uploaded at 0x71000f95c4.
//! These are runtime uniforms, not chunks serialized in MTRL files. Transform
//! upload transposes four 4x4 matrices; six 3x4 transforms retain their rows.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

/// `ub_globalSystemValues`, uploaded unchanged by global-buffer flush1
/// (0x71000f95c4). The two unused words retain their constructor zeros.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct NvnGlobalSystemValues {
    pub absolute_modular_time: f32,
    pub inverse_tonemap_exposure: f32,
}

impl Default for NvnGlobalSystemValues {
    fn default() -> Self {
        // SGlobalSystemValues_POD constructor, 0x710017ad24.
        Self { absolute_modular_time: 0.0, inverse_tonemap_exposure: 1.0 }
    }
}

impl NvnGlobalSystemValues {
    pub const BYTE_LEN: usize = 0x10;

    pub fn to_bytes(self) -> [u8; Self::BYTE_LEN] {
        let mut bytes = [0; Self::BYTE_LEN];
        bytes[0..4].copy_from_slice(&self.absolute_modular_time.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.inverse_tonemap_exposure.to_le_bytes());
        bytes
    }
}

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
    /// The bound input starts as identity; Reset clears only the uploaded
    /// 0x220-byte range, leaving that input at cache+0x280 intact.
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
    fn system_values_match_native_constructor_and_setter_offsets() {
        let defaults = NvnGlobalSystemValues::default().to_bytes();
        assert_eq!(defaults, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 0]);
        let snapshot =
            NvnGlobalSystemValues { absolute_modular_time: 1.5, inverse_tonemap_exposure: 0.25 }
                .to_bytes();
        assert_eq!(snapshot, [0, 0, 192, 63, 0, 0, 0, 0, 0, 0, 128, 62, 0, 0, 0, 0]);
    }

    #[test]
    fn uploaded_matrix_layout_round_trips_raw_words() {
        let bytes = (0..NvnGlobalTransforms::BYTE_LEN / 4)
            .flat_map(|i| (0x7f800000u32.wrapping_add(i as u32 * 12347)).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(NvnGlobalTransforms::parse(&bytes).unwrap().to_bytes(), bytes);
    }
}
