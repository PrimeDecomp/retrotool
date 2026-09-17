//! Nonlinear probe coefficients generated from the six baked-lighting volumes.
//! `SLightProbeNonlinear` is an 80-byte runtime BLPD record, not serialized LTPB
//! data. The matching DX reflection and NVN producer/consumer establish its layout.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

/// RGB results from sampling all six volumes at one position and LOD zero.
/// Filtering, sparse residency and the position-to-texture transform belong to
/// the caller. Midpoint/range are unsigned HDR; the remaining samples are UNORM.
#[derive(Debug, Clone, Copy)]
pub struct ProbeSamples {
    pub midpoint_rgb: [f32; 3],
    pub range_rgb: [f32; 3],
    pub exponent_rgb: [f32; 3],
    /// Three sampled RGB vectors, ordered by the light's R, G and B channels.
    pub gradient_directions: [[f32; 3]; 3],
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProbeGradient {
    /// In the consumer's view space. Length is significant; do not normalize.
    pub direction: [f32; 3],
    pub exponent: f32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NonlinearProbe {
    pub midpoint_rgb: [f32; 3],
    pub range_rgb: [f32; 3],
    /// Red, green and blue lobes, respectively.
    pub gradients: [ProbeGradient; 3],
    /// Reflected mUnused0/mUnused1. The producer writes zero; parsing preserves
    /// their bits so a memory record can be re-encoded without losing data.
    pub unused: [f32; 2],
}

impl NonlinearProbe {
    pub const BYTE_LEN: usize = 80;

    /// Decode one exact little-endian record, preserving all float bit patterns.
    /// Numeric validation is separate from parsing a captured GPU buffer.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid nonlinear probe record size");
        let float = |offset| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let rgb = |offset| std::array::from_fn(|i| float(offset + i * 4));
        Ok(Self {
            midpoint_rgb: rgb(0),
            range_rgb: rgb(0x10),
            gradients: std::array::from_fn(|i| ProbeGradient {
                direction: rgb(0x20 + i * 16),
                exponent: float(0x2c + i * 16),
            }),
            unused: [float(0x0c), float(0x1c)],
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::BYTE_LEN] {
        let mut bytes = [0; Self::BYTE_LEN];
        let values = self
            .midpoint_rgb
            .into_iter()
            .chain([self.unused[0]])
            .chain(self.range_rgb)
            .chain([self.unused[1]])
            .chain(self.gradients.iter().flat_map(|g| g.direction.into_iter().chain([g.exponent])));
        for (word, value) in bytes.chunks_exact_mut(4).zip(values) {
            word.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    /// Apply BakedLighting_ProbeTexture's decoding and direction transform.
    /// Supply the linear 3x3 rows of global `uc_bakedToView`; the producer does
    /// not use BPTX's inverse-transpose matrix for this step. Choosing that
    /// global transform from scene state remains the caller's responsibility.
    ///
    /// This follows the shader's fused-operation order for finite host floats.
    /// It does not emulate Maxwell FTZ or texture-decoding precision.
    pub fn from_samples(samples: &ProbeSamples, baked_to_view_rows: [[f32; 3]; 3]) -> Result<Self> {
        ensure!(
            samples
                .midpoint_rgb
                .iter()
                .chain(&samples.range_rgb)
                .all(|v| v.is_finite() && *v >= 0.0)
                && samples
                    .exponent_rgb
                    .iter()
                    .chain(samples.gradient_directions.iter().flatten())
                    .all(|v| (0.0..=1.0).contains(v))
                && baked_to_view_rows.iter().flatten().all(|v| v.is_finite()),
            "Invalid sampled probe or direction transform"
        );
        let gradients = std::array::from_fn(|channel| {
            let direction = samples.gradient_directions[channel].map(|v| v.mul_add(2.0, -1.0));
            ProbeGradient {
                direction: baked_to_view_rows.map(|row| dot(direction, row)),
                exponent: samples.exponent_rgb[channel].mul_add(2.0, 1.0),
            }
        });
        ensure!(
            gradients.iter().flat_map(|g| g.direction).all(f32::is_finite),
            "Nonfinite transformed probe direction"
        );
        Ok(Self {
            midpoint_rgb: samples.midpoint_rgb,
            range_rgb: samples.range_rgb,
            gradients,
            unused: [0.0; 2],
        })
    }

    /// Evaluate the directional irradiance used by ProbeLighting permutation 0.
    /// The supplied view-space normal is normalized here; gradient lengths are
    /// preserved. No final clamp is applied. The ambient-only permutation uses
    /// `midpoint_rgb` directly. Both subsequently multiply irradiance by diffuse
    /// RGB, BLCM RGB and 1/pi; those material factors are not baked into records.
    ///
    /// Accepts finite coefficients, positive exponents and a nonzero normal
    /// whose squared length is representable. Host sqrt/log2/exp2 approximate
    /// Maxwell's special functions and range reduction; this is not bit-exact
    /// shader emulation. Nonfinite normals, dot products and outputs are rejected.
    pub fn irradiance(&self, view_normal: [f32; 3]) -> Result<[f32; 3]> {
        ensure!(
            view_normal
                .iter()
                .chain(&self.midpoint_rgb)
                .chain(&self.range_rgb)
                .all(|v| v.is_finite())
                && self.gradients.iter().all(|g| g.direction.iter().all(|v| v.is_finite())
                    && g.exponent.is_finite()
                    && g.exponent > 0.0),
            "Invalid nonlinear probe evaluation inputs"
        );
        let length_squared = dot(view_normal, view_normal);
        ensure!(length_squared.is_finite() && length_squared > 0.0, "Invalid probe surface normal");
        let inverse_length = length_squared.sqrt().recip();
        let normal = view_normal.map(|v| v * inverse_length);
        let mut result = [0.0; 3];
        for (channel, value) in result.iter_mut().enumerate() {
            let gradient = self.gradients[channel];
            let cosine = dot(normal, gradient.direction);
            ensure!(cosine.is_finite(), "Nonfinite probe directional product");
            let t = cosine.mul_add(0.5, 0.5).clamp(0.0, 1.0);
            // Positive exponent: log2(0) -> -infinity -> exp2 -> zero.
            let power = if t == 0.0 { 0.0 } else { (gradient.exponent * t.log2()).exp2() };
            let weight = power * gradient.exponent.mul_add(0.5, 0.5);
            let lower = self.midpoint_rgb[channel] - self.range_rgb[channel];
            let upper = self.midpoint_rgb[channel] + self.range_rgb[channel];
            *value = (upper - lower).mul_add(weight, lower);
            ensure!(value.is_finite(), "Nonfinite evaluated probe irradiance");
        }
        Ok(result)
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[2].mul_add(b[2], a[1].mul_add(b[1], a[0] * b[0])) }

#[cfg(test)]
mod tests;
