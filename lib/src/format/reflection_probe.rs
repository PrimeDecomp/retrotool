//! MPR REFL v3 resources and runtime reflection uniforms. The REFL loader is
//! 0x710021b940; runtime REFP upload is 0x71001712e8. These are distinct layouts.

use anyhow::{ensure, Result};
use serde_derive::Serialize;
use uuid::Uuid;
use zerocopy::LittleEndian;

use super::{chunk::ChunkDescriptor, rfrm::FormDescriptor, FourCC};

#[derive(Debug, Clone, Serialize)]
pub struct ReflectionProbeData {
    /// Read and discarded by the original loader; zero in the examined assets.
    pub unknown: u32,
    pub texture: Uuid,
    pub intensity: f32,
}

impl ReflectionProbeData {
    /// Parse the version3 REFL form and its DATA chunk; return any asset footer.
    pub fn slice(bytes: &[u8]) -> Result<(Self, &[u8])> {
        let (form, data, remain) = FormDescriptor::<LittleEndian>::slice(bytes)?;
        ensure!(form.id == FourCC(*b"REFL"), "Expected REFL form");
        ensure!(
            form.reader_version.get() == 3 && form.writer_version.get() == 3,
            "Unsupported REFL version"
        );
        let (chunk, data, extra) = ChunkDescriptor::<LittleEndian>::slice(data)?;
        ensure!(chunk.id == FourCC(*b"DATA"), "Expected REFL DATA chunk");
        ensure!(data.len() == 24 && extra.is_empty(), "Unexpected REFL DATA layout");
        Ok((
            Self {
                unknown: u32::from_le_bytes(data[..4].try_into().unwrap()),
                texture: Uuid::from_bytes_le(data[4..20].try_into().unwrap()),
                intensity: read_float(data, 20),
            },
            remain,
        ))
    }
}

/// SReflectionProbeDataPOD, named SReflectionProbeParams in D3D reflection.
/// CReflectionProbeMaterialData stores the BRDF token at+0x10 and this POD at
/// +0x20. REFP is a complex material value; REFL is the separate cube texture.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReflectionProbeParams {
    pub max_valid_mip_index: f32,
    pub min_intensity: f32,
    pub max_intensity: f32,
    pub probe_intensity: f32,
}

impl Default for ReflectionProbeParams {
    /// Original POD constructor 0x7100219858. A resolved cube replaces x with
    /// float(int(texture_token.miprange.max)-1), not log2(texture width).
    fn default() -> Self {
        Self {
            max_valid_mip_index: 0.0,
            min_intensity: 0.0,
            max_intensity: 1.0,
            probe_intensity: 1.0,
        }
    }
}

impl ReflectionProbeParams {
    pub const BYTE_LEN: usize = 16;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid reflection probe parameter size");
        Ok(Self {
            max_valid_mip_index: read_float(bytes, 0),
            min_intensity: read_float(bytes, 4),
            max_intensity: read_float(bytes, 8),
            probe_intensity: read_float(bytes, 12),
        })
    }

    /// Source66 SingleVine/LayeredVine equation. Other baked-lighting feature
    /// combinations can supply different intensity inputs and need tracing.
    /// Maxwell reciprocal approximation, FTZ and NaN behavior are not emulated.
    pub fn vine_intensity(&self) -> f32 {
        self.min_intensity
            + (self.probe_intensity - self.min_intensity)
                * self.max_intensity.recip().clamp(0.0, 1.0)
    }

    /// Uses raw METL.g, BEFORE the separate 0.02 roughness floor for the BRDF.
    pub fn cube_lod(&self, raw_roughness: f32) -> f32 {
        (1.0 - raw_roughness) * -self.max_valid_mip_index + self.max_valid_mip_index
    }

    /// Combine already sampled cube RGB and BRDF RG for the two source66 vines.
    /// Occlusion is METL.r; F0 is the recovered, face-adjusted surface value.
    pub fn vine_specular(
        &self,
        cube: [f32; 3],
        brdf: [f32; 2],
        f0: [f32; 3],
        occlusion: f32,
    ) -> [f32; 3] {
        let scale = occlusion * self.vine_intensity();
        std::array::from_fn(|i| cube[i] * scale * (f0[i] * brdf[0] + brdf[1]))
    }
}

/// Original CGraphicsUtilCache BRDF table: 16x8 RG8_UNORM, one mip, linear
/// filtering and clamp-to-edge. The game table is supplied by the caller.
/// MPR's examined main.image contains256 bytes at relative offset0x1d0dbfc;
/// that executable offset is build-specific and is not a general extractor.
pub struct ReflectionBrdfLut<'a> {
    pub rg8: &'a [u8; 256],
}

impl<'a> ReflectionBrdfLut<'a> {
    pub const BYTE_LEN: usize = Self::WIDTH * Self::HEIGHT * 2;
    pub const HEIGHT: usize = 8;
    pub const WIDTH: usize = 16;

    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid reflection BRDF table size");
        Ok(Self { rg8: bytes.try_into().unwrap() })
    }

    /// Normalized texture coordinates (NoV, max(raw roughness,0.02)). Texel
    /// centers follow normalized linear sampling; hardware filter precision
    /// and Maxwell texture-unit rounding are not emulated.
    pub fn sample(&self, uv: [f32; 2]) -> Result<[f32; 2]> {
        ensure!(uv.into_iter().all(f32::is_finite), "Nonfinite BRDF coordinates");
        let xy = [uv[0].clamp(0.0, 1.0) * 16.0 - 0.5, uv[1].clamp(0.0, 1.0) * 8.0 - 0.5];
        let low = xy.map(|v| v.floor() as i32);
        let w = [xy[0] - low[0] as f32, xy[1] - low[1] as f32];
        let texel = |x: i32, y: i32, c: usize| {
            self.rg8[(y.clamp(0, 7) as usize * 16 + x.clamp(0, 15) as usize) * 2 + c] as f32 / 255.0
        };
        Ok(std::array::from_fn(|c| {
            let a = texel(low[0], low[1], c);
            let b = texel(low[0] + 1, low[1], c);
            let d = texel(low[0], low[1] + 1, c);
            let e = texel(low[0] + 1, low[1] + 1, c);
            let top = a + (b - a) * w[0];
            top + (d + (e - d) * w[0] - top) * w[1]
        }))
    }
}

fn read_float(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflection_layouts_and_edges() {
        assert!(ReflectionProbeParams::parse(&[0; 15]).is_err());
        assert!(ReflectionBrdfLut::parse(&[0; 255]).is_err());
        let mut bytes = [0u8; 256];
        bytes[..2].copy_from_slice(&[255, 128]);
        bytes[254..].copy_from_slice(&[64, 255]);
        let lut = ReflectionBrdfLut::parse(&bytes).unwrap();
        assert_eq!(lut.sample([-1.0, -1.0]).unwrap(), [1.0, 128.0 / 255.0]);
        assert_eq!(lut.sample([2.0, 2.0]).unwrap(), [64.0 / 255.0, 1.0]);
        assert!(lut.sample([f32::NAN, 0.0]).is_err());
        let p = ReflectionProbeParams {
            max_valid_mip_index: 8.0,
            min_intensity: 0.25,
            max_intensity: 4.0,
            probe_intensity: 2.0,
        };
        assert_eq!(p.vine_intensity(), 0.6875);
        assert_eq!(p.cube_lod(0.0), 0.0);
        assert_eq!(p.cube_lod(0.5), 4.0);
    }
}
