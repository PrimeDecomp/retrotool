//! MPR sampler construction and overrides, recovered from the NVN renderer.
//!
//! These are runtime parameters, not an additional on-disk structure. TXTR
//! supplies the defaults; each material texture use can override them.

use anyhow::{ensure, Result};

use super::{cmdl::STextureUsageInfo, txtr::STextureSamplerData};

/// Binding paths have different rules for choosing a generated sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSamplerBinding {
    Regular {
        sample_shading: bool,
    },
    /// The texture token's cached handle follows its streaming LOD variant.
    BindlessDefault,
    /// An explicitly supplied nondefault sampler always uses its base ID.
    BindlessCustom,
}

/// SBuildTextureSamplerParams (48 bytes in the engine). Integer -1 means
/// unspecified. Retain raw enum values until the backend resolves them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureSamplerParams {
    pub filter: i32,
    pub mip_filter: i32,
    pub wrap: [i32; 3],
    pub compare: i32,
    pub lod_min: f32,
    pub lod_max: f32,
    pub lod_bias: f32,
    /// -1 keeps the NVN builder default RGBA (0,0,0,0); 0/1/2 explicitly select
    /// transparent black, opaque black, or opaque white respectively.
    pub border: i32,
    pub anisotropy: i32,
    /// Bit 0 requests an extra sampler with LOD bias -1 for sample shading.
    pub flags: u8,
}

impl TextureSamplerParams {
    /// SBuildTextureSamplerParams::Simple, 0x7100118970.
    pub fn simple(filter: i32, wrap: [i32; 3], border: i32) -> Self {
        Self {
            filter,
            mip_filter: if (-1..=1).contains(&filter) { filter } else { -1 },
            wrap,
            compare: -1,
            lod_min: 0.0,
            lod_max: 16.0,
            lod_bias: 0.0,
            border,
            anisotropy: 0,
            flags: 0,
        }
    }

    /// NTextureFormat::BuildSamplerParams, 0x71001d4490. The stream reader
    /// zero-extends all six enum bytes, including the 0xff anisotropy value.
    pub fn from_header(header: &STextureSamplerData) -> Self {
        let mut params = Self::simple(
            header.filter as i32,
            [header.wrap_x as i32, header.wrap_y as i32, header.wrap_z as i32],
            -1,
        );
        params.mip_filter = header.mip_filter as i32;
        params.anisotropy = header.aniso as i32;
        params.flags = ((header.flags >> 1) & 1) as u8;
        params
    }

    /// CMaterialTextureTokenDataStorage::BuildMergedSampler, 0x7100160ac8.
    /// Runtime usage also contains anisotropy and border overrides; the wire
    /// constructor sets both to -1. Texcoord selection does not affect sampling.
    pub fn with_usage(mut self, usage: &STextureUsageInfo) -> Self {
        let mut overrides =
            Self::simple(usage.filter, [usage.wrap_x, usage.wrap_y, usage.wrap_z], -1);
        overrides.anisotropy = -1;
        self.merge_override(&overrides);
        self
    }

    /// MergeInOverrideParams, 0x71001189a4. Floats always replace the old
    /// values, integer -1 inherits, and flags accumulate.
    pub fn merge_override(&mut self, overrides: &Self) {
        fn merge(dst: &mut i32, src: i32) {
            if src != -1 {
                *dst = src;
            }
        }
        merge(&mut self.filter, overrides.filter);
        merge(&mut self.mip_filter, overrides.mip_filter);
        for (dst, src) in self.wrap.iter_mut().zip(overrides.wrap) {
            merge(dst, src);
        }
        merge(&mut self.compare, overrides.compare);
        merge(&mut self.border, overrides.border);
        merge(&mut self.anisotropy, overrides.anisotropy);
        self.lod_min = overrides.lod_min;
        self.lod_max = overrides.lod_max;
        self.lod_bias = overrides.lod_bias;
        self.flags |= overrides.flags;
    }

    /// NVN min/mag enum values emitted by 0x710010dc50. Unknown filter values
    /// select nearest/nearest-mipmap-nearest, even with mip_filter == 1.
    pub fn nvn_filters(&self) -> [u32; 2] {
        match self.filter {
            0 => [if self.mip_filter == 1 { 4 } else { 2 }, 0],
            1 => [if self.mip_filter == 1 { 5 } else { 3 }, 1],
            _ => [2, 0],
        }
    }

    /// NVN wrap enum values. Engine wrap 5 (Clamp) and unknown values fall
    /// through to Repeat in MPR's NVN backend.
    pub fn nvn_wrap_modes(&self) -> [u32; 3] {
        self.wrap.map(|w| match w {
            0 => 7, // ClampToEdge
            1 => 1, // Repeat
            2 => 6, // MirroredRepeat
            3 => 2, // MirrorClamp
            4 => 5, // ClampToBorder
            _ => 1,
        })
    }

    pub fn max_anisotropy(&self) -> u8 {
        match self.anisotropy {
            1 => 2,
            2 => 4,
            3 => 8,
            4 => 16,
            _ => 1,
        }
    }

    /// Additional sampler used when sample shading is enabled. The binder
    /// otherwise reuses the base sampler ID if flag bit 0 is absent.
    pub fn sample_shading_variant(&self) -> Self {
        let mut params = *self;
        if self.flags & 1 != 0 {
            params.lod_bias = -1.0;
        }
        params
    }

    /// The four additional minimum-LOD variants made at 0x710010d80c.
    /// A texture token can select one of these in preference to sample shading.
    pub fn min_lod_variants(&self) -> [Self; 4] {
        [0.25, 0.5, 0.75, 1.0].map(|offset| {
            let mut params = *self;
            params.lod_min += offset;
            if (params.lod_min - params.lod_min.trunc()).abs() >= 1e-5 {
                params.mip_filter = 1;
            }
            params
        })
    }

    /// Resolve the binder's sampler choice (0x71001a6e68 / 0x71001a6f58).
    /// `streaming_variant` is texture-token byte +0x15c: 0..3, or 0xff for
    /// inactive. NotifyStreamingBlendLodLevel (0x7100117250) refreshes the
    /// cached default bindless handle with the same streaming choice.
    pub fn for_binding(
        &self,
        binding: TextureSamplerBinding,
        streaming_variant: u8,
    ) -> Result<Self> {
        ensure!(
            streaming_variant < 4 || streaming_variant == 0xff,
            "Invalid texture streaming sampler variant {streaming_variant}"
        );
        if binding == TextureSamplerBinding::BindlessCustom {
            return Ok(*self);
        }
        if streaming_variant != 0xff {
            return Ok(self.min_lod_variants()[usize::from(streaming_variant)]);
        }
        match binding {
            TextureSamplerBinding::Regular { sample_shading: true } => {
                Ok(self.sample_shading_variant())
            }
            _ => Ok(*self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::txtr::{
        ETextureAnisotropicRatio, ETextureFilter, ETextureMipFilter, ETextureWrap,
    };

    #[test]
    fn usage_inherits_header_and_explicit_filter_also_changes_mips() {
        let header = STextureSamplerData {
            flags: 2,
            filter: ETextureFilter::Linear,
            mip_filter: ETextureMipFilter::Nearest,
            wrap_x: ETextureWrap::ClampToEdge,
            wrap_y: ETextureWrap::MirroredRepeat,
            wrap_z: ETextureWrap::Clamp,
            aniso: ETextureAnisotropicRatio::Ratio16,
        };
        let base = TextureSamplerParams::from_header(&header);
        let mut usage =
            STextureUsageInfo { tex_coord: 3, filter: -1, wrap_x: -1, wrap_y: -1, wrap_z: -1 };
        assert_eq!(base.with_usage(&usage), base);
        usage.filter = 1;
        usage.wrap_y = 1;
        let merged = base.with_usage(&usage);
        assert_eq!(merged.nvn_filters(), [5, 1]);
        assert_eq!(merged.nvn_wrap_modes(), [7, 1, 1]);
        assert_eq!(merged.max_anisotropy(), 16);
        assert_eq!(merged.flags, 1);
        assert_eq!(merged.sample_shading_variant().lod_bias, -1.0);
        assert_eq!(base.min_lod_variants().map(|p| p.mip_filter), [1, 1, 1, 0]);
    }

    #[test]
    fn streaming_overrides_sample_shading_except_for_custom_bindless_samplers() {
        let mut base = TextureSamplerParams::simple(1, [1; 3], -1);
        base.flags = 1;
        let regular = TextureSamplerBinding::Regular { sample_shading: true };
        assert_eq!(base.for_binding(regular, 0xff).unwrap().lod_bias, -1.0);
        let streaming = base.for_binding(regular, 2).unwrap();
        assert_eq!((streaming.lod_min, streaming.lod_bias), (0.75, 0.0));
        assert_eq!(base.for_binding(TextureSamplerBinding::BindlessDefault, 2).unwrap(), streaming);
        assert_eq!(base.for_binding(TextureSamplerBinding::BindlessDefault, 0xff).unwrap(), base);
        assert_eq!(base.for_binding(TextureSamplerBinding::BindlessCustom, 2).unwrap(), base);
        assert!(base.for_binding(regular, 4).is_err());
    }
}
