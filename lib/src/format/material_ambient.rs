//! Ambient lighting source consumed by AMBS and the light-gather pass.
//! This is a 0x60-byte runtime POD inside a Complex material object at +0x10.
//! Scene cache construction is separate from serialized model material data.

use anyhow::{ensure, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbientSource {
    bytes: [u8; Self::BYTE_LEN],
}

impl AmbientSource {
    pub const BYTE_LEN: usize = 0x60;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == Self::BYTE_LEN, "Invalid ambient source size");
        Ok(Self { bytes: bytes.try_into()? })
    }

    /// CAmbientSourceMaterialData contains the source after its 16-byte base.
    pub fn from_material(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == 0x70, "Invalid ambient material snapshot size");
        Self::parse(&bytes[0x10..])
    }

    /// Native color constructor710016a440. Directional colors are black with
    /// alpha1, directions are +Y with W0; the flat color preserves all raw bits.
    pub fn flat(color: [f32; 4]) -> Self {
        let mut source = Self { bytes: [0; Self::BYTE_LEN] };
        source.set_vector(0, color);
        for offset in [0x10, 0x40] {
            source.set_vector(offset, [0.0, 1.0, 0.0, 0.0]);
        }
        for offset in [0x20, 0x30, 0x50] {
            source.set_vector(offset, [0.0, 0.0, 0.0, 1.0]);
        }
        source
    }

    pub fn black() -> Self { Self::flat([0.0, 0.0, 0.0, 1.0]) }

    /// Native FullBright710016a57c uses f32 pi for RGB, not unit white.
    pub fn full_bright() -> Self {
        let pi = std::f32::consts::PI;
        Self::flat([pi, pi, pi, 1.0])
    }

    pub fn bytes(&self) -> &[u8; Self::BYTE_LEN] { &self.bytes }

    pub fn flat_bytes(&self) -> &[u8] { &self.bytes[..16] }

    pub fn flat_color(&self) -> [f32; 4] { self.vector(0) }

    /// IsBlack710016a6f0 checks only RGB of gradient0/gradient1/fill. It does
    /// not inspect the flat color, direction vectors or any alpha component.
    pub fn directional_is_black(&self) -> bool {
        [0x20, 0x30, 0x50]
            .into_iter()
            .all(|offset| self.vector(offset)[..3].iter().all(|&component| component == 0.0))
    }

    /// AddFlatAmbient710016a648 adds RGB to flat/gradient0/gradient1 and
    /// sets their alpha to1. It leaves fill light and directions unchanged.
    pub fn add_flat(&mut self, color: [f32; 4]) {
        for offset in [0, 0x20, 0x30] {
            let previous = self.vector(offset);
            self.set_vector(offset, [
                previous[0] + color[0],
                previous[1] + color[1],
                previous[2] + color[2],
                1.0,
            ]);
        }
    }

    /// Rotate710016a768 applies only the rotation part to both directions and
    /// sets their W to0. Input is three rows of the native CTransform4f.
    pub fn rotate(&mut self, rows: [[f32; 4]; 3]) {
        for offset in [0x10, 0x40] {
            let v = self.vector(offset);
            let xyz = rows.map(|row| (row[0] * v[0] + row[1] * v[1]) + row[2] * v[2]);
            self.set_vector(offset, [xyz[0], xyz[1], xyz[2], 0.0]);
        }
    }

    /// BuildAmbientSourceMaterialData71003acf88 after the caller resolves the
    /// pass's scene ambient color and native world-to-view rotation. Disabled
    /// lighting or black directional cache plus exact (0,0,0,1) emits no data.
    /// Finite host f32 operations; no ARM/Maxwell exceptional-value guarantee.
    pub fn for_scene(
        &self,
        enabled: bool,
        scene_color: [f32; 4],
        world_to_view_rotation: [[f32; 4]; 3],
    ) -> Option<Self> {
        if !enabled || (self.directional_is_black() && scene_color == [0.0, 0.0, 0.0, 1.0]) {
            return None;
        }
        let mut source = self.clone();
        source.rotate(world_to_view_rotation);
        source.add_flat(scene_color);
        Some(source)
    }

    fn vector(&self, offset: usize) -> [f32; 4] {
        std::array::from_fn(|i| {
            f32::from_le_bytes(self.bytes[offset + i * 4..offset + i * 4 + 4].try_into().unwrap())
        })
    }

    fn set_vector(&mut self, offset: usize, value: [f32; 4]) {
        for (i, v) in value.into_iter().enumerate() {
            self.bytes[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_scene_gate_ignores_flat_color_and_directional_alpha() {
        let mut source = AmbientSource::full_bright();
        for offset in [0x20, 0x30, 0x50] {
            source.set_vector(offset, [0.0, -0.0, 0.0, -3.0]);
        }
        assert!(source.directional_is_black());
        assert!(source.for_scene(true, [0.0, 0.0, 0.0, 1.0], [[0.0; 4]; 3]).is_none());
        // The native allocation gate also checks the scene color's alpha.
        assert!(source.for_scene(true, [0.0; 4], [[0.0; 4]; 3]).is_some());
        source.set_vector(0x20, [f32::NAN, 0.0, 0.0, 1.0]);
        assert!(!source.directional_is_black());
    }

    #[test]
    fn material_snapshot_retains_raw_color_and_requires_complete_source() -> Result<()> {
        let mut material = vec![0x5a; 0x70];
        let color = [0x80000000u32, 0x7fc12345, 0x3f800000, 0xbf800000];
        for (i, bits) in color.into_iter().enumerate() {
            material[0x10 + i * 4..0x14 + i * 4].copy_from_slice(&bits.to_le_bytes());
        }
        let source = AmbientSource::from_material(&material)?;
        assert_eq!(source.flat_bytes(), &material[0x10..0x20]);
        assert!(AmbientSource::from_material(&material[..0x6f]).is_err());
        Ok(())
    }
}
