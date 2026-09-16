//! SKIN material data and the native 48-byte bone matrix palette binding.
//! Runtime snapshots are separate from the serialized SMDL header. Animation
//! matrices are already in shader palette order; this module does not build a
//! skeleton pose or multiply inverse-bind matrices into a supplied snapshot.

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

use super::{material_features::FeatureVariable, shader_nvn_uniforms::NvnUniformValue};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct SkinningBinding<'a> {
    pub data_id: &'a str,
}

impl FeatureVariable<'_> {
    /// 7100166414 enables only option1. Callback166620 ignores binding flags,
    /// looks up the first Complex ID, and forwards the packed BoneData variable.
    pub fn skinning_binding(&self) -> Result<Option<SkinningBinding<'_>>> {
        if self.definition != "SKIN" || self.definition_option != 1 {
            return Ok(None);
        }
        ensure!(self.semantic == "BoneData", "Invalid SKIN semantic");
        Ok(Some(SkinningBinding {
            data_id: self.data_ids.first().copied().context("SKIN needs a data ID")?,
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinningPalette {
    bytes: Vec<u8>,
}

impl SkinningPalette {
    pub const MATRIX_BYTES: usize = 48;

    /// Uploaded three-row, four-column matrices; raw float words are preserved.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        ensure!(bytes.len() % Self::MATRIX_BYTES == 0, "Truncated skinning matrix");
        ensure!(bytes.len() <= u32::MAX as usize, "Skinning palette exceeds native byte size");
        Ok(Self { bytes })
    }

    /// The original CSkinningMaterialData constructor fills this palette with
    /// identity matrices. The host limits it to one 64-KiB constant buffer.
    pub fn identity(bone_count: u32) -> Result<Self> {
        ensure!(
            bone_count <= 0x10000 / Self::MATRIX_BYTES as u32,
            "Identity palette exceeds host constant-buffer capacity"
        );
        let matrix = [1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        Ok(Self { bytes: matrix.repeat(bone_count as usize) })
    }

    pub fn bone_count(&self) -> usize { self.bytes.len() / Self::MATRIX_BYTES }

    pub fn bytes(&self) -> &[u8] { &self.bytes }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SkinningPaletteSource {
    Fallback,
    Animation,
    Supplied,
}

#[derive(Clone, Debug)]
pub struct SkinningSnapshot {
    pub instanced: bool,
    /// One palette for a normal draw; ordered palettes for an instanced draw.
    pub palettes: Vec<(SkinningPaletteSource, SkinningPalette)>,
}

impl SkinningSnapshot {
    pub fn supplied(palette: SkinningPalette) -> Self {
        Self { instanced: false, palettes: vec![(SkinningPaletteSource::Supplied, palette)] }
    }

    /// Parse a resolved CBaseSkinningMaterialData object and external memory.
    /// Single: animation reference+18 takes precedence over fallback count+20 /
    /// pointer+28, even when the animation has zero bones. Animated count is a
    /// signed i16 at renderData+8; matrices are at renderData+30.
    /// Instanced: pointer range+18/+20 contains CSkinningMaterialData pointers.
    /// Negative counts, overflowing byte sizes and malformed ranges reject as
    /// host validation policy.
    pub fn parse(
        header: &[u8],
        mut memory: impl FnMut(u64, usize) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        Self::parse_impl(header, &mut memory)
    }

    fn parse_impl(
        header: &[u8],
        memory: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let word = |bytes: &[u8], offset| -> Result<u64> {
            Ok(u64::from_le_bytes(
                bytes.get(offset..offset + 8).context("Truncated skinning data")?.try_into()?,
            ))
        };
        let mut palette = |data: &[u8]| -> Result<(SkinningPaletteSource, SkinningPalette)> {
            let animation = word(data, 0x18)?;
            let (source, count, pointer) = if animation != 0 {
                let render = memory(animation, 0x38)?;
                let count = i16::from_le_bytes(
                    render.get(8..10).context("Truncated animation render data")?.try_into()?,
                );
                (SkinningPaletteSource::Animation, i32::from(count), word(&render, 0x30)?)
            } else {
                let count = i32::from_le_bytes(
                    data.get(0x20..0x24).context("Truncated fallback palette")?.try_into()?,
                );
                (SkinningPaletteSource::Fallback, count, word(data, 0x28)?)
            };
            ensure!(count >= 0, "Negative skinning bone count");
            let size = (count as u32)
                .checked_mul(SkinningPalette::MATRIX_BYTES as u32)
                .context("Skinning palette size overflow")? as usize;
            let bytes = if size == 0 {
                Vec::new()
            } else {
                ensure!(pointer != 0, "Null nonempty skinning palette");
                let bytes = memory(pointer, size)?;
                ensure!(bytes.len() == size, "Incomplete skinning palette snapshot");
                bytes
            };
            Ok((source, SkinningPalette::from_bytes(bytes)?))
        };
        let instanced = header.get(0x10).context("Truncated skinning data")? & 1 != 0;
        if !instanced {
            return Ok(Self { instanced, palettes: vec![palette(header)?] });
        }
        // Release the resolver borrow before reading the pointer range.
        let begin = word(header, 0x18)?;
        let end = word(header, 0x20)?;
        let size = end.checked_sub(begin).context("Reversed skinning instance range")?;
        ensure!(size % 8 == 0, "Misaligned skinning instance range");
        let pointers = if size == 0 {
            Vec::new()
        } else {
            ensure!(begin != 0, "Null skinning instance range");
            let bytes = memory(begin, usize::try_from(size)?)?;
            ensure!(bytes.len() as u64 == size, "Incomplete skinning pointer range");
            bytes
        };
        let mut palettes = Vec::with_capacity(pointers.len() / 8);
        for pointer in pointers.chunks_exact(8) {
            let address = u64::from_le_bytes(pointer.try_into()?);
            ensure!(address != 0, "Null skinning instance");
            let data = memory(address, 0x30)?;
            ensure!(
                data.get(0x10).context("Truncated skinning instance")? & 1 == 0,
                "Nested skinning instances are not a native single-palette object"
            );
            let single = Self::parse_impl(&data, memory)?;
            palettes.extend(single.palettes);
        }
        Ok(Self { instanced, palettes })
    }

    /// Native callback uploads each nonempty palette in order. Empty palettes
    /// write nothing and do not reserve space. Offsets and lengths are BYTES.
    pub fn writes(&self) -> Vec<NvnUniformValue<'_>> {
        let mut offset = 0u32;
        self.palettes
            .iter()
            .filter_map(|(_, palette)| {
                let bytes = palette.bytes();
                if bytes.is_empty() {
                    return None;
                }
                let value = NvnUniformValue::Raw { byte_offset: offset, bytes };
                offset = offset.wrapping_add(bytes.len() as u32);
                Some(value)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_animation_takes_precedence_over_nonempty_fallback() -> Result<()> {
        let mut header = [0u8; 0x30];
        header[0x18..0x20].copy_from_slice(&0x1000u64.to_le_bytes());
        header[0x20..0x24].copy_from_slice(&5i32.to_le_bytes());
        header[0x28..0x30].copy_from_slice(&0x2000u64.to_le_bytes());
        let mut reads = 0;
        let snapshot = SkinningSnapshot::parse(&header, |address, size| {
            assert_eq!((address, size), (0x1000, 0x38));
            reads += 1;
            Ok(vec![0; size])
        })?;
        assert_eq!(reads, 1);
        assert_eq!(snapshot.palettes[0].0, SkinningPaletteSource::Animation);
        assert!(snapshot.writes().is_empty());
        Ok(())
    }

    #[test]
    fn invalid_counts_and_ranges_reject_before_reading_payload() {
        let mut header = [0u8; 0x30];
        for count in [-1i32, i32::MAX] {
            header[0x20..0x24].copy_from_slice(&count.to_le_bytes());
            assert!(SkinningSnapshot::parse(&header, |_, _| {
                panic!("Invalid palette must not request external memory")
            })
            .is_err());
        }
        header[0x10] = 1;
        for (begin, end) in [(0x1008u64, 0x1000u64), (0x1000, 0x1001), (0, 8)] {
            header[0x18..0x20].copy_from_slice(&begin.to_le_bytes());
            header[0x20..0x28].copy_from_slice(&end.to_le_bytes());
            assert!(SkinningSnapshot::parse(&header, |_, _| {
                panic!("Invalid range must not request external memory")
            })
            .is_err());
        }
    }

    #[test]
    fn incomplete_matrix_snapshot_rejects() {
        let mut header = [0u8; 0x30];
        header[0x20..0x24].copy_from_slice(&1i32.to_le_bytes());
        header[0x28..0x30].copy_from_slice(&0x2000u64.to_le_bytes());
        assert!(SkinningSnapshot::parse(&header, |address, size| {
            assert_eq!((address, size), (0x2000, 48));
            Ok(vec![0; 47])
        })
        .is_err());
    }
}
