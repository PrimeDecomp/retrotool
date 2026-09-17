//! MPR's SNVN shader binary and reflection records (MTRL archive version 22).
//! Offsets in the first five reflection ranges are relative to byte 0x5c;
//! the remaining six ranges are relative to the binding-data blob.

use std::io::{Cursor, Read, Seek};

use anyhow::{ensure, Context, Result};
use binrw::{BinRead, BinReaderExt};
use serde_derive::Serialize;

use crate::util::compression::decompress_zlib_exact;

#[derive(Clone, Copy, Debug, BinRead, Serialize)]
pub struct OffsetAndCount {
    pub offset: u32,
    pub count: u32,
}

impl OffsetAndCount {
    fn slice(self, data: &[u8], stride: usize) -> Result<&[u8]> {
        let start = self.offset as usize;
        let size = (self.count as usize).checked_mul(stride).context("NVN table size overflow")?;
        let end = start.checked_add(size).context("NVN table range overflow")?;
        data.get(start..end).context("NVN table outside its containing blob")
    }

    fn records<T>(self, data: &[u8], stride: usize) -> Result<Vec<T>>
    where T: for<'a> BinRead<Args<'a> = ()> {
        let bytes = self.slice(data, stride)?;
        let mut reader = Cursor::new(bytes);
        let result = (0..self.count).map(|_| read(&mut reader)).collect::<Result<Vec<T>>>()?;
        ensure!(reader.position() == bytes.len() as u64, "NVN record size mismatch");
        Ok(result)
    }
}

fn read<T, R>(reader: &mut R) -> Result<T>
where
    T: for<'a> BinRead<Args<'a> = ()>,
    R: Read + Seek,
{
    Ok(reader.read_le()?)
}

/// NShaderCore::EShader uses 0=vertex, 1=pixel, 2=geometry, 3=compute.
/// Each stage points into both pools. Program lengths are in the vendor control
/// records, not in these MPR stage descriptors; see NvnShader::programs.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct ShaderStage {
    pub kind: u32,
    pub gpu_offset: u32,
    pub cpu_offset: u32,
    pub scratch_memory: [u32; 2],
}

#[derive(Clone, Debug)]
pub struct NvnShader {
    /// Read and discarded by LoadBinaryPools; observed zero, semantics unknown.
    pub unknown: u8,
    pub stages: Vec<ShaderStage>,
    /// Pool uploaded to an NVN GPU buffer; includes headers as well as shader code.
    pub gpu_pool: Vec<u8>,
    /// Pool retained in CPU memory and supplied to nvnProgramSetShaders.
    pub cpu_pool: Vec<u8>,
}

impl NvnShader {
    /// Decode an SShaderSource binary. Does not translate the GPU instructions.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let gpu_size: u32 = read(&mut reader)?;
        let cpu_size: u32 = read(&mut reader)?;
        let unknown = read(&mut reader)?;
        let compressed_size = read(&mut reader)?;
        let compressed = OffsetAndCount { offset: 13, count: compressed_size }.slice(data, 1)?;
        reader.set_position(13 + u64::from(compressed_size));
        let count = read(&mut reader)?;
        // The stage-count word follows a byte-aligned compressed blob.
        let stage_data = &data[reader.position() as usize..];
        let stages: Vec<ShaderStage> =
            OffsetAndCount { offset: 0, count }.records(stage_data, 20)?;
        ensure!(stage_data.len() == stages.len() * 20, "Trailing NVN binary data");
        for stage in &stages {
            ensure!(stage.gpu_offset < gpu_size, "NVN stage GPU offset out of range");
            ensure!(stage.cpu_offset < cpu_size, "NVN stage CPU offset out of range");
        }
        let total =
            (gpu_size as usize).checked_add(cpu_size as usize).context("NVN pool size overflow")?;
        let mut gpu_pool = decompress_zlib_exact(compressed, total)?;
        let cpu_pool = gpu_pool.split_off(gpu_size as usize);
        Ok(Self { unknown, stages, gpu_pool, cpu_pool })
    }
}

/// NShaderCore::SConstantBuffer, 32 bytes.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct ConstantBuffer {
    /// Name hash, using Retro's uncomplemented CRC32 (see shader_name_crc).
    /// Corroborated against 24 named D3D RDEF buffers in exact shader pairs.
    pub name_crc: u32,
    pub size: u32,
    /// Bit 0: uc_globalTransforms; bit 3: ub_globalSystemValues;
    /// bit 4: ub_bakedLightProbeData (build_constant_buffer_flags_from_name).
    pub flags: u32,
    pub material_flags: u32,
    pub material_data_ids: [u64; 2],
}

/// CCRC32::CalculateOnString (0x710019cc7c), seeded with 0xffffffff as in
/// CStringCRC32::CreateFromString (0x7100320f74). Reflected IEEE polynomial,
/// no final XOR, case-sensitive, excluding the terminating NUL.
/// For ordinary shader names this equals !zlib.crc32(name).
pub fn shader_name_crc(name: &str) -> u32 {
    let mut crc = u32::MAX;
    for byte in name.bytes().take_while(|&b| b != 0) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    crc
}

/// SShaderVertexLayoutEntry: engine vertex component and shader input location.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct VertexLayoutEntry {
    pub component: u32,
    pub location: i32,
}

/// NShaderCore::SConstantBufferRef, 8 bytes.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct ConstantBufferRef {
    pub binding: u32,
    pub buffer_index: u32,
}

/// NShaderCore::SUniform, 4 bytes. Size/type come from the feature binder.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct Uniform {
    pub buffer_index: u16,
    pub offset: u16,
}

/// NShaderCore::SResource_NVN, 12 bytes. Binding 0xffff means absent.
#[derive(Clone, Debug, BinRead, Serialize)]
pub struct Resource {
    /// 0 = texture, 1 = storage buffer. Other values retain their raw encoding.
    pub kind: u32,
    /// Vertex, pixel, geometry, compute, in that order.
    pub bindings: [u16; 4],
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
pub enum VariableBinding<'a> {
    Uniform {
        uniform: &'a Uniform,
        /// NVN uniform-buffer slots, in vertex/pixel/geometry/compute order.
        stage_bindings: [Vec<u32>; 4],
    },
    Resource {
        resource: &'a Resource,
    },
}

#[derive(Debug, Serialize)]
pub struct NvnReflection<'a> {
    pub flags: u32,
    /// Original header, including the unresolved fourth range.
    pub ranges: [OffsetAndCount; 11],
    pub constant_buffers: Vec<ConstantBuffer>,
    pub vertex_layout: Vec<VertexLayoutEntry>,
    /// Backend-independent data dispatched to CShader's virtual stream reader.
    #[serde(skip)]
    pub shader_data: &'a [u8],
    /// Per-stage constant-buffer references: vertex, pixel, geometry, compute.
    pub constant_buffer_refs: [Vec<ConstantBufferRef>; 4],
    pub uniforms: Vec<Uniform>,
    pub resources: Vec<Resource>,
}

impl<'a> NvnReflection<'a> {
    /// Resolve a packed CShaderVariable. 0xffff is explicitly unbound; valid
    /// variables use bit 0 to select the uniform or resource table.
    pub fn variable(&self, packed: u16) -> Result<Option<VariableBinding<'_>>> {
        if packed == u16::MAX {
            return Ok(None);
        }
        let index = usize::from(packed >> 1);
        Ok(Some(if packed & 1 == 0 {
            let uniform =
                self.uniforms.get(index).context("NVN variable uniform index out of range")?;
            let stage_bindings = std::array::from_fn(|stage| {
                self.constant_buffer_refs[stage]
                    .iter()
                    .filter(|r| r.buffer_index == u32::from(uniform.buffer_index))
                    .map(|r| r.binding)
                    .collect()
            });
            VariableBinding::Uniform { uniform, stage_bindings }
        } else {
            VariableBinding::Resource {
                resource: self
                    .resources
                    .get(index)
                    .context("NVN variable resource index out of range")?,
            }
        }))
    }

    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let flags = read(&mut reader)?;
        let ranges: [OffsetAndCount; 11] = read(&mut reader)?;
        let base = data.get(0x5c..).context("Truncated NVN reflection header")?;
        let constant_buffers: Vec<ConstantBuffer> = ranges[0].records(base, 32)?;
        let vertex_layout = ranges[1].records(base, 8)?;
        let shader_data = ranges[2].slice(base, 1)?;
        // Range 3 appears to describe 24-byte SDebugData_NVN records, but its
        // interpretation is not established by the runtime loaders. Keep it raw.
        ensure!(ranges[3].offset as usize <= base.len(), "NVN unknown range offset out of bounds");
        let bindings = ranges[4].slice(base, 1)?;
        let constant_buffer_refs: [Vec<ConstantBufferRef>; 4] = [
            ranges[5].records(bindings, 8)?,
            ranges[6].records(bindings, 8)?,
            ranges[7].records(bindings, 8)?,
            ranges[8].records(bindings, 8)?,
        ];
        let uniforms: Vec<Uniform> = ranges[9].records(bindings, 4)?;
        let resources = ranges[10].records(bindings, 12)?;
        for refs in &constant_buffer_refs {
            for reference in refs {
                ensure!(
                    (reference.buffer_index as usize) < constant_buffers.len(),
                    "NVN constant-buffer reference out of range"
                );
            }
        }
        for uniform in &uniforms {
            let buffer = constant_buffers
                .get(uniform.buffer_index as usize)
                .context("NVN uniform buffer index out of range")?;
            ensure!(u32::from(uniform.offset) < buffer.size, "NVN uniform offset out of range");
        }
        Ok(Self {
            flags,
            ranges,
            constant_buffers,
            vertex_layout,
            shader_data,
            constant_buffer_refs,
            uniforms,
            resources,
        })
    }

    /// For CShader_Featured only: count-prefixed packed CShaderVariable values.
    /// 0xffff is invalid; otherwise bit 0 is the type and bits 1..15 are an index.
    /// This payload is dispatched by shader class, so do not assume every class
    /// uses this encoding merely because its backend is SNVN.
    pub fn featured_variables(&self) -> Result<Vec<u16>> {
        let mut reader = Cursor::new(self.shader_data);
        let count: u32 = read(&mut reader)?;
        let variables = OffsetAndCount { offset: 4, count }.records(self.shader_data, 2)?;
        ensure!(
            self.shader_data.len() as u64 == 4 + u64::from(count) * 2,
            "Trailing featured shader data"
        );
        Ok(variables)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn name_crc_matches_reflected_buffers_and_engine_string_termination() {
        assert_eq!(super::shader_name_crc(""), 0xffffffff);
        assert_eq!(super::shader_name_crc("ub_constants"), 0x0da0d6de);
        assert_eq!(super::shader_name_crc("uc_globalTransforms"), 0x630ee0ac);
        assert_eq!(super::shader_name_crc("ub_globalSystemValues"), 0x5b82cdf6);
        assert_eq!(super::shader_name_crc("ub_constants\0suffix"), 0x0da0d6de);
        assert_ne!(super::shader_name_crc("UB_CONSTANTS"), 0x0da0d6de);
    }

    use std::io::Write;

    use flate2::{write::ZlibEncoder, Compression};

    use super::*;

    fn reflection_bytes() -> Vec<u8> {
        let pairs = [
            (0, 1),
            (32, 1),
            (40, 8),
            (48, 0),
            (48, 24),
            (0, 1),
            (8, 0),
            (8, 0),
            (8, 0),
            (8, 1),
            (12, 1),
        ];
        let mut data = 4u32.to_le_bytes().to_vec();
        for (offset, count) in pairs {
            data.extend_from_slice(&u32::to_le_bytes(offset));
            data.extend_from_slice(&u32::to_le_bytes(count));
        }
        // One 32-byte cbuffer; shader input component 5 at location 2.
        for value in [0x12345678u32, 64, 1, 0, 0, 0, 0, 0, 5, 2] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&[0, 0, 1, 0]); // uniform 0, resource 0
        data.extend_from_slice(&7u32.to_le_bytes()); // binding slot 7
        data.extend_from_slice(&0u32.to_le_bytes()); // cbuffer index 0
        data.extend_from_slice(&[0, 0, 16, 0]); // uniform at byte 16
        data.extend_from_slice(&0u32.to_le_bytes()); // resource kind
        for value in [u16::MAX, 3, u16::MAX, u16::MAX] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data
    }

    #[test]
    fn reflection_uses_two_offset_origins_and_resolves_bindings() {
        let bytes = reflection_bytes();
        let r = NvnReflection::parse(&bytes).unwrap();
        assert_eq!(r.constant_buffers[0].size, 64);
        assert_eq!(r.vertex_layout[0].component, 5);
        assert_eq!(r.vertex_layout[0].location, 2);
        assert_eq!(r.constant_buffer_refs[0][0].binding, 7);
        assert_eq!(r.constant_buffer_refs[0][0].buffer_index, 0);
        assert_eq!(r.uniforms[0].offset, 16);
        assert_eq!(r.resources[0].bindings, [u16::MAX, 3, u16::MAX, u16::MAX]);
        assert_eq!(r.featured_variables().unwrap(), [0, 1]);
        match r.variable(0).unwrap().unwrap() {
            VariableBinding::Uniform { uniform, stage_bindings } => {
                assert_eq!(uniform.offset, 16);
                assert_eq!(stage_bindings, [vec![7], vec![], vec![], vec![]]);
            }
            _ => panic!("Uniform variable resolved to a resource"),
        }
        assert!(matches!(r.variable(1).unwrap(), Some(VariableBinding::Resource { .. })));
        assert!(r.variable(u16::MAX).unwrap().is_none());
        assert!(r.variable(2).is_err());
        assert!(r.variable(3).is_err());
        for n in 0..bytes.len() {
            assert!(NvnReflection::parse(&bytes[..n]).is_err(), "accepted length {n}");
        }
    }

    #[test]
    fn reflection_rejects_bad_ranges_and_buffer_indices() {
        let bytes = reflection_bytes();
        // Top-level range, nested range/count, cbuffer reference, uniform index/offset.
        for (offset, word) in [
            (4, u32::MAX),
            (4 + 5 * 8, 72),
            (8 + 9 * 8, u32::MAX),
            (92 + 48 + 4, 1),
            (92 + 48 + 8, 1),
            (92 + 48 + 8, 64 << 16),
        ] {
            let mut bad = bytes.clone();
            bad[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
            assert!(NvnReflection::parse(&bad).is_err(), "accepted bad word at {offset}");
        }
        let mut bad = bytes;
        bad[92 + 40..92 + 44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(NvnReflection::parse(&bad).unwrap().featured_variables().is_err());
    }

    #[test]
    fn binary_requires_complete_zlib_and_valid_stage_offsets() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"GPU!CPU!!").unwrap();
        let compressed = encoder.finish().unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(&5u32.to_le_bytes());
        data.push(0);
        data.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        data.extend_from_slice(&compressed);
        data.extend_from_slice(&1u32.to_le_bytes());
        for value in [3u32, 0, 0, 128, 256] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let shader = NvnShader::parse(&data).unwrap();
        assert_eq!(shader.gpu_pool, b"GPU!");
        assert_eq!(shader.cpu_pool, b"CPU!!");
        assert_eq!(shader.stages[0].kind, 3);
        assert_eq!(shader.stages[0].scratch_memory, [128, 256]);
        for n in 0..data.len() {
            assert!(NvnShader::parse(&data[..n]).is_err(), "accepted length {n}");
        }
        for offset in
            [0, 9, 13 + compressed.len(), 13 + compressed.len() + 8, 13 + compressed.len() + 12]
        {
            let mut bad = data.clone();
            bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(NvnShader::parse(&bad).is_err());
        }
        let mut bad = data.clone();
        bad.remove(12 + compressed.len()); // Missing byte of zlib trailer, keeping envelope parseable.
        bad[9..13].copy_from_slice(&((compressed.len() - 1) as u32).to_le_bytes());
        assert!(NvnShader::parse(&bad).is_err());
        data.push(0);
        assert!(NvnShader::parse(&data).is_err());
    }
}
