//! Authored WIND sets and the CPU update behind WindAnimData.
//!
//! Set I/O: 710021268c/7100212814. Simulation: 7100212fcc; impulse packing:
//! 71002149ec. Separate f32 operations preserve the ARM64 evaluation order.
//! Finite input/output checks are host validation, not native behavior.

use anyhow::{ensure, Context, Result};

use super::Reader;

#[derive(Clone, Debug)]
pub struct ModelWindData {
    pub sets: Vec<WindSet>,
    /// Render-mesh order. 0xff omits the provider for that mesh.
    pub mesh_sets: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindSet {
    /// Native names and their complete shader interpretation remain unknown.
    /// vector0 also determines the model's bounds padding.
    pub vector0: [f32; 3],
    pub vector1: [f32; 3],
    pub time_scale: f32,
    pub independence: f32,
    pub independent_magnitude: f32,
}

/// Runtime scene values, independent of the authored model sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindEnvironment {
    pub direction: [f32; 3],
    pub magnitude: f32,
    pub impulses: [WindImpulse; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindImpulse {
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub outer_radius: f32,
    pub inner_radius: f32,
    pub amplitude: f32,
    pub phase: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindSnapshot {
    /// Twelve vec4s bound directly to WindAnimData, without the material header.
    pub vectors: [[f32; 4]; 12],
    /// Native padding offered to the renderer, separately from the uniforms.
    /// World models pad local boxes; MCON's owner ignores this value.
    pub bounds_padding: [f32; 3],
}

impl ModelWindData {
    /// Read one MPR WIND body and return the untouched following HEAD bytes.
    pub fn parse(bytes: &[u8]) -> Result<(Self, &[u8])> {
        let mut reader = Reader { remaining: bytes };
        let count = reader.count(36)?;
        let mut sets = Vec::with_capacity(count);
        for _ in 0..count {
            sets.push(WindSet {
                vector0: reader.vector()?,
                vector1: reader.vector()?,
                time_scale: reader.float()?,
                independence: reader.float()?,
                independent_magnitude: reader.float()?,
            });
        }
        let count = reader.count(1)?;
        let mesh_sets = reader.take(count)?.to_vec();
        Ok((Self { sets, mesh_sets }, reader.remaining))
    }

    pub fn mesh_set(&self, mesh: usize) -> Result<Option<&WindSet>> {
        let &index = self.mesh_sets.get(mesh).context("WIND mesh map index out of bounds")?;
        if index == u8::MAX {
            return Ok(None);
        }
        Ok(Some(self.sets.get(usize::from(index)).context("WIND set index out of bounds")?))
    }
}

impl WindEnvironment {
    /// SWindSimVars' state when no external provider is registered. This is a
    /// deliberate preview choice; it does not recover a ROOM's live wind state.
    pub fn native_fallback() -> Self {
        Self {
            direction: [0.0, 0.0, -1.0],
            magnitude: 0.3,
            impulses: [WindImpulse {
                position: [0.0; 3],
                direction: [1.0; 3],
                outer_radius: 0.0,
                inner_radius: 0.0,
                amplitude: 0.0,
                phase: 0.0,
            }; 2],
        }
    }
}

impl WindSet {
    /// Evaluate with the native external-clock rule, phase = time_scale * time.
    /// The engine supplies a scene's modular simulation time. Preview callers
    /// may instead choose their own absolute playback clock explicitly.
    pub fn at_time(&self, environment: &WindEnvironment, seconds: f32) -> Result<WindSnapshot> {
        ensure!(seconds.is_finite(), "Nonfinite wind time");
        self.sample(environment, self.time_scale * seconds)
    }

    /// Update an existing phase when no external clock is registered. The
    /// initial phase is zero; registration performs this update with dt = 0.
    pub fn advance(
        &self,
        environment: &WindEnvironment,
        previous_phase: f32,
        delta_seconds: f32,
    ) -> Result<WindSnapshot> {
        ensure!(previous_phase.is_finite() && delta_seconds.is_finite(), "Nonfinite wind time");
        self.sample(environment, previous_phase + self.time_scale * delta_seconds)
    }

    fn sample(&self, environment: &WindEnvironment, phase: f32) -> Result<WindSnapshot> {
        ensure!(
            self.vector0.iter().chain(&self.vector1).all(|v| v.is_finite())
                && self.time_scale.is_finite()
                && self.independence.is_finite()
                && self.independent_magnitude.is_finite()
                && environment.direction.iter().all(|v| v.is_finite())
                && environment.magnitude.is_finite(),
            "Nonfinite wind input"
        );
        let dependence = 1.0 - self.independence;
        let directed_magnitude = dependence * environment.magnitude;
        let half_magnitude = environment.magnitude * 0.5;
        let local_magnitude =
            half_magnitude + self.independence * (self.independent_magnitude - half_magnitude);
        let axis = environment.direction.map(|v| v.abs() * 0.75 + 0.25);
        let mut vectors = [[0.0; 4]; 12];
        for i in 0..3 {
            vectors[0][i] = environment.direction[i] * directed_magnitude;
            vectors[1][i] = self.vector0[i];
            vectors[2][i] = self.vector1[i];
            vectors[3][i] = self.independence + dependence * axis[i];
        }
        vectors[0][3] = local_magnitude;
        vectors[1][3] = phase;
        vectors[2][3] = 1.0;
        for (index, impulse) in environment.impulses.iter().enumerate() {
            vectors[4 + index * 4..8 + index * 4].copy_from_slice(&impulse.vectors()?);
        }
        // Native compares Z against Y, then the result against X. Keep that
        // order and strict comparisons, including equal signed-zero inputs.
        let mut amplitude = self.vector0[2];
        if amplitude < self.vector0[1] {
            amplitude = self.vector0[1];
        }
        if amplitude < self.vector0[0] {
            amplitude = self.vector0[0];
        }
        let padding_scale = (amplitude * 1.5) * environment.magnitude;
        let bounds_padding = axis.map(|v| v * padding_scale);
        ensure!(
            vectors.iter().flatten().chain(&bounds_padding).all(|v| v.is_finite()),
            "Wind simulation overflow"
        );
        Ok(WindSnapshot { vectors, bounds_padding })
    }
}

impl WindImpulse {
    fn vectors(&self) -> Result<[[f32; 4]; 4]> {
        ensure!(
            self.position.iter().chain(&self.direction).all(|v| v.is_finite())
                && [self.outer_radius, self.inner_radius, self.amplitude, self.phase]
                    .iter()
                    .all(|v| v.is_finite()),
            "Nonfinite wind impulse"
        );
        // SLightDistanceAttenuation::FromValues(type1, inner, outer) repairs
        // reversed/equal radii, then divides each numerator separately.
        let outer = if self.inner_radius < self.outer_radius {
            self.outer_radius
        } else {
            self.inner_radius + 0.001
        };
        let span = outer - self.inner_radius;
        let [px, py, pz] = self.position;
        let [dx, dy, dz] = self.direction;
        Ok([
            [px, py, pz, 0.0],
            [dx, dy, dz, 0.0],
            [1.0 / span, -self.inner_radius / span, self.amplitude, self.phase],
            [0.0, 0.0, -1.0, 1.0],
        ])
    }
}

impl WindSnapshot {
    pub fn phase(&self) -> f32 { self.vectors[1][3] }

    pub fn to_bytes(&self) -> [u8; 0xc0] {
        let mut bytes = [0; 0xc0];
        for (value, out) in self.vectors.iter().flatten().zip(bytes.chunks_exact_mut(4)) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}
