//! NVN raster sample locations and the driver constants read by MPR shaders.
//!
//! Recovered from MPR's SDK build 2217a4a3c1f3c883278de72f8b008224ad252a24:
//! sample setter +0x4749f0 and bind helper +0x493980. These are runtime inputs,
//! not bytes stored inside a material archive. The 32-byte constant region is
//! c0[0x620..0x640]. Coordinate nibbles use sixteenths from the upper-left.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

/// Runtime NVN ProgramSetSampleShading modes used by CShaderCore_NVN. The enum
/// names describe recovered behavior; the original SDK names are unavailable.
/// These values are program selection modes, not sample counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[repr(u32)]
pub enum NvnSampleShading {
    /// Preserve the pixel shader's CPU-control byte at0x74a.
    #[default]
    ShaderDefault = 0,
    PerSample = 1,
    PerFragment = 2,
}

pub use remax::wgsl::graphics::FragmentFrequency;

impl NvnSampleShading {
    /// The ordinary texture binder1a6e68 tests the context enum for NONZERO.
    /// This is distinct from the pixel control record's effective frequency:
    /// ShaderDefault uses base samplers even when that shader runs per sample.
    pub fn uses_sampler_variant(self) -> bool { self != Self::ShaderDefault }

    pub fn from_code(code: u32) -> Result<Self> {
        match code {
            0 => Ok(Self::ShaderDefault),
            1 => Ok(Self::PerSample),
            2 => Ok(Self::PerFragment),
            _ => anyhow::bail!("unsupported NVN sample-shading mode {code}"),
        }
    }

    /// Original SDK SetShaders46fbe0 changes the pixel descriptor's sample flag
    /// before building commands. The builder48f130 emits hybrid-AA control0x30
    /// for sample shading or0x01 for fragment shading. This does not specify
    /// PIXLD.MY_INDEX's representative index during fragment-frequency execution.
    pub fn frequency(self, shader_sample_shading: bool) -> FragmentFrequency {
        let per_sample = match self {
            Self::ShaderDefault => shader_sample_shading,
            Self::PerSample => true,
            Self::PerFragment => false,
        };
        if per_sample {
            FragmentFrequency::PerSample
        } else {
            FragmentFrequency::PerFragment
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NvnSampleState {
    pub samples: u8,
    pub raster_samples: u8,
    pub enabled: bool,
    pub custom_locations: Option<[u8; 16]>,
    pub grid: bool,
}

impl NvnSampleState {
    /// Decode the relevant fields from the native 24-byte NVN multisample state.
    /// Alpha-to-coverage, coverage modulation and unrelated flags remain outside
    /// this sample-location provider; this is not a complete render-state decoder.
    pub fn from_runtime_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 24, "truncated NVN multisample state");
        let flags = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        Ok(Self {
            samples: ((flags >> 8) & 31) as u8,
            raster_samples: ((flags >> 15) & 31) as u8,
            enabled: flags & 1 != 0,
            custom_locations: (flags & 8 != 0).then(|| bytes[4..20].try_into().unwrap()),
            grid: flags & 16 != 0,
        })
    }

    pub fn resolve(self) -> Result<NvnSampleLayout> {
        ensure!([0, 1, 2, 4, 8, 16].contains(&self.samples), "unsupported NVN sample count");
        ensure!(
            [0, 1, 2, 4, 8, 16].contains(&self.raster_samples),
            "unsupported NVN raster sample count"
        );
        let count = self.samples.max(self.raster_samples).max(1);
        let mut raster_locations = [0u8; 16];
        if let Some(custom) = self.custom_locations {
            for (i, value) in raster_locations.iter_mut().enumerate() {
                *value = custom[if self.grid { i } else { i % usize::from(count) }];
            }
        } else {
            let pattern: &[u8] = match count {
                1 => &[0x88],
                2 => &[0xcc, 0x44],
                4 => &[0x26, 0x6e, 0xa2, 0xea],
                8 => &[0x59, 0xb7, 0x9d, 0x35, 0xd3, 0x71, 0xfb, 0x1f],
                16 => &[
                    0x99, 0x57, 0xa5, 0x7c, 0x63, 0xda, 0xbd, 0x3b, 0xe6, 0x18, 0x24, 0xc2, 0x80,
                    0x4f, 0xfe, 0x01,
                ],
                _ => unreachable!(),
            };
            for (i, value) in raster_locations.iter_mut().enumerate() {
                *value = pattern[i % pattern.len()];
            }
        }
        let mut constants = NvnSampleConstants::default();
        // The SDK tests the color sample count even when raster_samples is larger.
        if self.enabled && self.samples > 1 {
            constants.locations = raster_locations.map(|value| value ^ 0x88);
            constants.sample_mask = u32::from(count - 1);
            constants.sample_shift = count.trailing_zeros();
            constants.x_mask = u32::from(count <= 4);
            constants.y_mask = match count {
                2 => 3,
                4 | 8 => 1,
                _ => 0,
            };
        }
        Ok(NvnSampleLayout { raster_samples: count, raster_locations, constants })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NvnSampleLayout {
    /// Effective raster pattern size, including target-independent raster samples.
    pub raster_samples: u8,
    /// Hardware raster positions. Custom grid patterns can vary between pixels.
    pub raster_locations: [u8; 16],
    pub constants: NvnSampleConstants,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NvnSampleConstants {
    /// c0[0x620], sixteen bytes. Each nibble is a signed offset in sixteenths.
    pub locations: [u8; 16],
    /// c0[0x630..0x640], four little-endian u32 values.
    pub sample_mask: u32,
    pub sample_shift: u32,
    pub x_mask: u32,
    pub y_mask: u32,
}

impl NvnSampleConstants {
    pub const BYTE_OFFSET: usize = 0x620;

    pub fn to_bytes(self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&self.locations);
        for (i, value) in
            [self.sample_mask, self.sample_shift, self.x_mask, self.y_mask].into_iter().enumerate()
        {
            bytes[16 + i * 4..20 + i * 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// Mirror the original prologue's addressing with defined u32 wrapping.
    /// Returns None for inconsistent externally constructed parameters.
    pub fn sample_byte(self, pixel: [u32; 2], invocation_sample: u32) -> Option<u8> {
        let row = (pixel[1] & self.y_mask).wrapping_shl(self.x_mask & 31);
        let cell = (pixel[0] & self.x_mask).wrapping_add(row);
        let index = cell
            .wrapping_shl(self.sample_shift & 31)
            .wrapping_add(invocation_sample & self.sample_mask);
        self.locations.get(index as usize).copied()
    }

    /// Sample offsets relative to pixel center, in units of 1/16 pixel.
    /// This describes the sample table, not the full IPA offset register format.
    pub fn offset_sixteenths(self, pixel: [u32; 2], sample: u32) -> Option<[i8; 2]> {
        let byte = self.sample_byte(pixel, sample)?;
        Some([((byte << 4) as i8) >> 4, (byte as i8) >> 4])
    }

    /// Raw bits produced by the observed SHL 4/SHL 8/XMAD.PSL.CLO sequence.
    /// Retains the extra bits; interpreting arbitrary IPA offsets is separate.
    pub fn packed_interpolation_offset(self, pixel: [u32; 2], sample: u32) -> Option<u32> {
        let byte = u32::from(self.sample_byte(pixel, sample)?);
        Some((byte << 20) | (byte << 8))
    }
}

/// Quantize finite coordinates exactly as NVN's sample-location setter.
/// This provider accepts finite coordinates only; it rejects NaN and infinity.
pub fn pack_sample_location(location: [f32; 2]) -> Result<u8> {
    ensure!(location.iter().all(|v| v.is_finite()), "sample location must be finite");
    let xy = location.map(|v| (v.clamp(0.0, 15.0 / 16.0) * 16.0 + 0.5) as u8);
    Ok(xy[0] | (xy[1] << 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_game_quarter_pixel_pattern() {
        let mut positions = [0x88; 16];
        positions[..4].copy_from_slice(&[0x44, 0x4c, 0xc4, 0xcc]);
        let layout = NvnSampleState {
            samples: 4,
            raster_samples: 0,
            enabled: true,
            custom_locations: Some(positions),
            grid: false,
        }
        .resolve()
        .unwrap();
        assert_eq!(layout.constants.locations, [
            0xcc, 0xc4, 0x4c, 0x44, 0xcc, 0xc4, 0x4c, 0x44, 0xcc, 0xc4, 0x4c, 0x44, 0xcc, 0xc4,
            0x4c, 0x44
        ]);
        for pixel in [[0, 0], [1, 1], [123, 456]] {
            assert_eq!(layout.constants.offset_sixteenths(pixel, 0), Some([-4, -4]));
            assert_eq!(layout.constants.offset_sixteenths(pixel, 3), Some([4, 4]));
        }
        assert_eq!(layout.constants.packed_interpolation_offset([0, 0], 0), Some(0x0cc0cc00));
    }

    #[test]
    fn raster_sample_count_does_not_enable_shader_sample_offsets() {
        let state = NvnSampleState {
            samples: 1,
            raster_samples: 8,
            enabled: true,
            custom_locations: None,
            grid: false,
        };
        let layout = state.resolve().unwrap();
        assert_eq!(layout.raster_samples, 8);
        assert_eq!(layout.constants, NvnSampleConstants::default());
        assert!(NvnSampleState { samples: 3, ..state }.resolve().is_err());
        assert!(pack_sample_location([f32::NAN, 0.5]).is_err());
    }
}
