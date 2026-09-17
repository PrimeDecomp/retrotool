//! Host view and sampler properties required when translating texture accesses.

use anyhow::{ensure, Result};
use serde_derive::{Deserialize, Serialize};

use crate::maxwell::texture::TextureDimension;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TextureValueType {
    Float,
    Uint,
    Sint,
    Depth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TextureFilter {
    Nearest,
    Linear,
}

/// Properties of the actual bound view and effective sampler, not the shader.
/// A host must validate these when creating/replacing resources. Unknown view
/// ranges must omit this record rather than assume a single mip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureSampling {
    pub mip_level_count: u32,
    pub min_filter: TextureFilter,
    pub mag_filter: TextureFilter,
    /// One means anisotropy is disabled; hosts with a separate enable flag must
    /// turn it off, rather than enable anisotropy with a maximum of one.
    pub max_anisotropy: u8,
}

impl TextureSampling {
    /// Check view and sampler properties supported by the current backend.
    pub fn validate(self) -> Result<()> {
        ensure!(self.mip_level_count > 0, "texture view must contain at least one mip level");
        ensure!(
            matches!(self.max_anisotropy, 1 | 2 | 4 | 8 | 16),
            "unsupported texture sampling anisotropy"
        );
        Ok(())
    }

    fn fixed_level_zero(self) -> bool {
        self.mip_level_count == 1 && self.min_filter == self.mag_filter && self.max_anisotropy == 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureBinding {
    /// Raw bound descriptor-table word offset, not an NVN API texture slot.
    pub texture_index: u16,
    /// Actual view dimension, independent of the instruction's coordinates.
    /// Filtered 2D and 2D-array accesses can use either view type.
    pub dimension: TextureDimension,
    pub value_type: TextureValueType,
    #[serde(default)]
    pub multisample: bool,
    /// Sampler LOD bias used for implicit sampling. Explicit LOD omits bias.
    #[serde(default)]
    pub lod_bias: f32,
    #[serde(default)]
    pub sampling: Option<TextureSampling>,
}

impl TextureBinding {
    /// For ordinary floating-point sampling, a one-level view with matching
    /// min/mag filters and no anisotropy needs no derivative-dependent choices.
    /// Comparison and integer sampling retain their separate provider rules.
    pub fn implicit_lod_is_fixed(self) -> bool {
        !self.multisample
            && self.value_type == TextureValueType::Float
            && self.sampling.is_some_and(TextureSampling::fixed_level_zero)
    }
}
