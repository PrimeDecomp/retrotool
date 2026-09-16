//! Reflected NVN constant-buffer writes and their graphics shader locations.
//!
//! Original binders: 71001a7318/7364, cached variants 71001a74c8/7510.
//! Buffer sizes/offsets come from reflection; write sizes come from the caller.
//! Untouched bytes remain unknown unless seeded from an explicit snapshot.

use std::{borrow::Cow, ops::Range};

use anyhow::{ensure, Context, Result};
use remax::resources::{ConstantBank, ConstantMemory, ConstantRead};

use super::{
    material_scene_bindings::{SceneBindingValue, SceneBindingWrite},
    shader_nvn::{ConstantBufferRef, NvnReflection, Uniform},
    shader_nvn_bindings::graphics_uniform_buffer_slot,
};

/// Inputs to the ordinary typed NVN binder methods. Float values are copied
/// bit-for-bit. Matrix4 is the native row-major CMatrix4f input, transposed by
/// BindMatrix4f before upload. Raw offsets and lengths are BYTES, including for
/// the method named BindMem32; array offset addition wraps at 32 bits.
#[derive(Clone, Debug)]
pub enum NvnUniformValue<'a> {
    Int(i32),
    Float(f32),
    Vector2([f32; 2]),
    Vector3([f32; 3]),
    Vector4([f32; 4]),
    Int4([i32; 4]),
    Matrix4([f32; 16]),
    Raw { byte_offset: u32, bytes: &'a [u8] },
}

impl NvnUniformValue<'_> {
    pub fn byte_offset(&self) -> u32 {
        match self {
            Self::Raw { byte_offset, .. } => *byte_offset,
            _ => 0,
        }
    }

    pub fn bytes(&self) -> Cow<'_, [u8]> {
        let bytes = match self {
            Self::Int(v) => v.to_le_bytes().to_vec(),
            Self::Float(v) => v.to_le_bytes().to_vec(),
            Self::Vector2(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Self::Vector3(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Self::Vector4(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Self::Int4(v) => v.iter().flat_map(|v| v.to_le_bytes()).collect(),
            Self::Matrix4(v) => {
                (0..16).flat_map(|i| v[(i % 4) * 4 + i / 4].to_le_bytes()).collect()
            }
            Self::Raw { bytes, .. } => return Cow::Borrowed(bytes),
        };
        Cow::Owned(bytes)
    }
}

impl SceneBindingWrite<'_> {
    /// Preserve the scene callback's exact bytes, including one-byte empty-array
    /// sentinels. Texture writes need a separate resource sink.
    pub fn uniform_value(&self) -> Option<NvnUniformValue<'_>> {
        let bytes: &[u8] = match &self.value {
            SceneBindingValue::Float4(bytes) | SceneBindingValue::Int4(bytes) => bytes,
            SceneBindingValue::Raw(bytes) => bytes,
            SceneBindingValue::Texture { .. } => return None,
        };
        Some(NvnUniformValue::Raw { byte_offset: 0, bytes })
    }
}

/// Host representation of recovered bytes, with explicit validity. The zeroes
/// used for storage allocation are never exposed as known game buffer contents.
#[derive(Clone, Debug)]
pub struct UniformBufferBytes {
    bytes: Vec<u8>,
    known: Vec<bool>,
}

impl UniformBufferBytes {
    pub fn unknown(size: usize) -> Result<Self> {
        ensure!(size <= 0x10000, "Constant buffer exceeds 64 KiB");
        Ok(Self { bytes: vec![0; size], known: vec![false; size] })
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let mut buffer = Self::unknown(bytes.len())?;
        buffer.write(0, bytes)?;
        Ok(buffer)
    }

    pub fn size(&self) -> usize { self.bytes.len() }

    /// Export data together with its validity. This exposes unknown storage
    /// bytes only to the checked constant-memory encoder, not as known values.
    pub fn constant_bank(&self, bank: u8) -> ConstantBank<'_> {
        ConstantBank { bank, bytes: &self.bytes, known: Some(&self.known) }
    }

    pub fn write(&mut self, byte_offset: u32, bytes: &[u8]) -> Result<()> {
        let start = byte_offset as usize;
        let end = start.checked_add(bytes.len()).context("Uniform write span overflow")?;
        let dest = self.bytes.get_mut(start..end).context("Uniform write out of bounds")?;
        dest.copy_from_slice(bytes);
        self.known[start..end].fill(true);
        Ok(())
    }

    pub fn read(&self, span: Range<usize>) -> Result<&[u8]> {
        let bytes = self.bytes.get(span.clone()).context("Uniform read out of bounds")?;
        ensure!(
            self.known[span.clone()].iter().all(|&v| v),
            "Unknown constant-buffer bytes in {span:?}"
        );
        Ok(bytes)
    }

    pub fn known_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = None;
        for (i, known) in self.known.iter().copied().chain([false]).enumerate() {
            if known {
                start.get_or_insert(i);
            } else if let Some(start) = start.take() {
                ranges.push(start..i);
            }
        }
        ranges
    }
}

/// Per-reflection buffer state. The caller supplies the native write mask and
/// resolves shared/global buffers; this does not guess provider readiness.
#[derive(Clone, Debug)]
pub struct NvnUniformBuffers {
    buffers: Vec<UniformBufferBytes>,
    uniforms: Vec<Uniform>,
    graphics_refs: [Vec<ConstantBufferRef>; 3],
}

impl NvnUniformBuffers {
    pub fn new(reflection: &NvnReflection<'_>) -> Result<Self> {
        let buffers = reflection
            .constant_buffers
            .iter()
            .map(|buffer| UniformBufferBytes::unknown(buffer.size as usize))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            buffers,
            uniforms: reflection.uniforms.clone(),
            graphics_refs: std::array::from_fn(|i| reflection.constant_buffer_refs[i].clone()),
        })
    }

    pub fn buffer(&self, index: usize) -> Result<&UniformBufferBytes> {
        self.buffers.get(index).context("Uniform buffer index out of bounds")
    }

    /// Explicit snapshots, global uploads and completed cache data can seed
    /// this buffer without going through the ordinary variable write mask.
    pub fn buffer_mut(&mut self, index: usize) -> Result<&mut UniformBufferBytes> {
        self.buffers.get_mut(index).context("Uniform buffer index out of bounds")
    }

    fn uniform(&self, packed: u16) -> Result<&Uniform> {
        // Host validation, not native skip behavior: the NVN writer itself
        // simply shifts packed>>1, with NO ffff or low-bit check.
        ensure!(
            packed != u16::MAX && packed & 1 == 0,
            "Invalid packed uniform variable {packed:#06x}"
        );
        self.uniforms.get(usize::from(packed >> 1)).context("Uniform index out of bounds")
    }

    /// False means masked off. Preserves every byte outside the write span.
    /// Native ARM64 shifts mask by (buffer_index & 31), including for index>=32.
    /// Host bounds/type checks reject invalid inputs instead of native UB.
    pub fn write(
        &mut self,
        packed: u16,
        write_mask: u32,
        value: &NvnUniformValue<'_>,
    ) -> Result<bool> {
        let uniform = self.uniform(packed)?;
        if write_mask & (1 << (uniform.buffer_index & 31)) == 0 {
            return Ok(false);
        }
        let offset = u32::from(uniform.offset).wrapping_add(value.byte_offset());
        let buffer = usize::from(uniform.buffer_index);
        self.buffer_mut(buffer)?.write(offset, &value.bytes())?;
        Ok(true)
    }

    /// Native cached binder uses one supplied destination, independent of the
    /// reflected buffer index. That index still gates the write mask. The caller
    /// later assigns this completed cache to the appropriate buffer instance.
    pub fn write_cached(
        &self,
        packed: u16,
        write_mask: u32,
        value: &NvnUniformValue<'_>,
        destination: &mut UniformBufferBytes,
    ) -> Result<bool> {
        let uniform = self.uniform(packed)?;
        if write_mask & (1 << (uniform.buffer_index & 31)) == 0 {
            return Ok(false);
        }
        let offset = u32::from(uniform.offset).wrapping_add(value.byte_offset());
        destination.write(offset, &value.bytes())?;
        Ok(true)
    }

    /// Buffer indices for hardware banks, following native PreDrawCallback
    /// binding order. A later binding to the same bank replaces the earlier one.
    /// Graphics stage order is vertex/pixel/geometry; compute is unsupported.
    pub fn graphics_banks(&self, stage: usize) -> Result<[Option<usize>; 32]> {
        let refs = self.graphics_refs.get(stage).context("Invalid NVN graphics stage")?;
        let mut banks = [None; 32];
        for reference in refs {
            let index = reference.buffer_index as usize;
            self.buffer(index)?;
            banks[usize::from(graphics_uniform_buffer_slot(reference.binding))] = Some(index);
        }
        Ok(banks)
    }

    /// Direct shader constants can be used once their individual words are
    /// known, even if other bytes in the same buffer have not been recovered.
    /// Missing/unknown reads return an error, never an invented zero.
    pub fn compact_graphics_reads(&self, stage: usize, reads: &[ConstantRead]) -> Result<Vec<u32>> {
        let banks = self.graphics_banks(stage)?;
        if reads.is_empty() {
            return Ok(vec![0]);
        }
        reads
            .iter()
            .map(|read| {
                let buffer = banks
                    .get(usize::from(read.buffer))
                    .copied()
                    .flatten()
                    .context("Shader constant bank has no reflected buffer")?;
                let start = usize::from(read.byte_offset);
                ensure!(start % 4 == 0, "Unaligned direct constant read");
                Ok(u32::from_le_bytes(self.buffer(buffer)?.read(start..start + 4)?.try_into()?))
            })
            .collect()
    }

    /// Indexed LDC currently needs complete bound buffer bytes. This conservative
    /// bridge rejects partial buffers, including unknown padding. Callers with
    /// incomplete snapshots can still resolve direct reads using the method above.
    /// Driver-owned banks (e.g. texture handles) require separate sources.
    pub fn graphics_memory(&self, stage: usize) -> Result<ConstantMemory> {
        let banks = self.graphics_banks(stage)?;
        let mut data = Vec::new();
        for (bank, buffer) in banks.into_iter().enumerate() {
            if let Some(index) = buffer {
                let buffer = self.buffer(index)?;
                data.push((bank as u8, buffer.read(0..buffer.size())?));
            }
        }
        ConstantMemory::new(&data)
    }

    /// Addressable memory with byte validity, including caller-supplied driver
    /// banks. Unknown LDC bytes set status8 and reject that shader invocation.
    /// Duplicate driver/reflected banks are rejected instead of overriding data.
    pub fn checked_graphics_memory(
        &self,
        stage: usize,
        driver_banks: &[ConstantBank<'_>],
    ) -> Result<ConstantMemory> {
        let banks = self.graphics_banks(stage)?;
        let mut data = driver_banks.to_vec();
        for (bank, buffer) in banks.into_iter().enumerate() {
            if let Some(index) = buffer {
                data.push(self.buffer(index)?.constant_bank(bank as u8));
            }
        }
        ConstantMemory::from_banks(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::shader_nvn::{ConstantBuffer, OffsetAndCount};

    fn reflection() -> NvnReflection<'static> {
        NvnReflection {
            flags: 0,
            ranges: [OffsetAndCount { offset: 0, count: 0 }; 11],
            shader_data: &[],
            constant_buffers: vec![
                ConstantBuffer {
                    name_crc: 0,
                    size: 16,
                    flags: 0,
                    material_flags: 0,
                    material_data_ids: [0; 2]
                };
                2
            ],
            vertex_layout: vec![],
            resources: vec![],
            constant_buffer_refs: [
                vec![ConstantBufferRef { binding: 0, buffer_index: 0 }],
                vec![ConstantBufferRef { binding: 1, buffer_index: 1 }],
                vec![],
                vec![],
            ],
            uniforms: vec![Uniform { buffer_index: 0, offset: 4 }, Uniform {
                buffer_index: 1,
                offset: 0,
            }],
        }
    }

    #[test]
    fn partial_writes_preserve_unknown_bytes_and_stage_locations() -> Result<()> {
        let mut state = NvnUniformBuffers::new(&reflection())?;
        assert!(state.write(0, 1, &NvnUniformValue::Int(0x12345678))?);
        assert_eq!(state.buffer(0)?.known_ranges(), [4..8]);
        assert!(!state.write(0, 0, &NvnUniformValue::Int(0))?);
        let reads = [ConstantRead { buffer: 3, byte_offset: 4 }];
        assert_eq!(state.compact_graphics_reads(0, &reads)?, [0x12345678]);
        assert!(state.compact_graphics_reads(1, &reads).is_err());
        assert!(state
            .compact_graphics_reads(0, &[ConstantRead { buffer: 3, byte_offset: 0 }])
            .is_err());
        assert!(state.graphics_memory(0).is_err());
        state.buffer_mut(0)?.write(0, &[0xaa; 4])?;
        state.buffer_mut(0)?.write(8, &[0xbb; 8])?;
        assert_eq!(state.buffer(0)?.known_ranges(), [0..16]);
        assert_eq!(state.graphics_memory(0)?.compact_reads(&reads)?, [0x12345678]);
        assert!(state.write(u16::MAX, u32::MAX, &NvnUniformValue::Int(0)).is_err());
        assert!(state.write(1, u32::MAX, &NvnUniformValue::Int(0)).is_err());
        assert!(state.write(0, u32::MAX, &NvnUniformValue::Vector4([0.0; 4])).is_err());
        assert_eq!(state.buffer(0)?.read(4..8)?, 0x12345678u32.to_le_bytes());
        Ok(())
    }

    #[test]
    fn cached_destination_is_independent_and_later_stage_binding_wins() -> Result<()> {
        let mut reflection = reflection();
        reflection.constant_buffer_refs[0].push(ConstantBufferRef { binding: 0, buffer_index: 1 });
        let state = NvnUniformBuffers::new(&reflection)?;
        let mut cache = UniformBufferBytes::from_snapshot(&[0xcd; 16])?;
        assert!(!state.write_cached(2, 1, &NvnUniformValue::Int(7), &mut cache)?);
        assert!(state.write_cached(2, 2, &NvnUniformValue::Int(7), &mut cache)?);
        assert_eq!(cache.read(0..4)?, 7i32.to_le_bytes());
        assert_eq!(cache.read(4..16)?, [0xcd; 12]);
        assert!(state.buffer(1)?.known_ranges().is_empty());
        assert_eq!(state.graphics_banks(0)?[3], Some(1));
        assert!(state.graphics_banks(3).is_err());
        Ok(())
    }
}
