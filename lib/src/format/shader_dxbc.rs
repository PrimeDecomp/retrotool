//! DXBC container, SM5.0 RDEF, signatures and tokenized instruction framing.
//!
//! Format references: Microsoft's d3d12TokenizedProgramFormat.hpp and
//! DxilContainer.h; Wine/vkd3d's RDEF reader and DXBC signature reader.
//! Independently implemented from the serialized fields. Original chunks are
//! retained; checksum validation and instruction operand decoding are not done.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

fn range(data: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    let end = offset.checked_add(size).context("DXBC range overflow")?;
    data.get(offset..end).context("DXBC range outside containing data")
}

fn table(data: &[u8], offset: u32, count: u32, stride: usize) -> Result<&[u8]> {
    let size = (count as usize).checked_mul(stride).context("DXBC table size overflow")?;
    range(data, offset as usize, size)
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(range(data, offset, 4)?.try_into()?))
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(range(data, offset, 2)?.try_into()?))
}

fn string_at(data: &[u8], offset: u32) -> Result<&str> {
    let tail = data.get(offset as usize..).context("DXBC string offset out of range")?;
    let end = tail.iter().position(|&b| b == 0).context("Unterminated DXBC string")?;
    Ok(std::str::from_utf8(&tail[..end])?)
}

#[derive(Debug, Serialize)]
pub struct DxbcChunk<'a> {
    pub tag: [u8; 4],
    /// Container-relative offset of the chunk header (tag + byte length).
    pub offset: u32,
    pub size: u32,
    #[serde(skip)]
    pub data: &'a [u8],
}

#[derive(Debug, Serialize)]
pub struct Dxbc<'a> {
    /// Preserved, not recomputed or verified by this parser.
    pub checksum: [u8; 16],
    pub chunks: Vec<DxbcChunk<'a>>,
    pub reflection: Option<DxbcReflection<'a>>,
    pub signatures: Vec<DxbcSignature<'a>>,
    pub program: Option<TokenizedProgram>,
}

impl<'a> Dxbc<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        ensure!(range(data, 0, 4)? == b"DXBC", "Invalid DXBC magic");
        let checksum = range(data, 4, 16)?.try_into()?;
        ensure!(u32_at(data, 20)? == 1, "Unsupported DXBC container version");
        ensure!(u32_at(data, 24)? as usize == data.len(), "DXBC container size mismatch");
        let offsets = table(data, 32, u32_at(data, 28)?, 4)?;
        let table_end = 32 + offsets.len();
        let mut chunks = Vec::new();
        let mut spans = Vec::new();
        for offset in offsets.chunks_exact(4) {
            let offset = u32_at(offset, 0)?;
            ensure!(offset as usize >= table_end && offset % 4 == 0, "Invalid DXBC chunk offset");
            let header = range(data, offset as usize, 8)?;
            let start = offset as usize + 8;
            let bytes = range(data, start, u32_at(header, 4)? as usize)?;
            spans.push((offset as usize, start + bytes.len()));
            chunks.push(DxbcChunk {
                tag: header[..4].try_into()?,
                offset,
                size: bytes.len() as u32,
                data: bytes,
            });
        }
        spans.sort_unstable();
        ensure!(spans.windows(2).all(|s| s[0].1 <= s[1].0), "Overlapping DXBC chunks");
        let mut reflection = None;
        let mut signatures = Vec::new();
        let mut program = None;
        for chunk in &chunks {
            match &chunk.tag {
                b"RDEF" => {
                    ensure!(reflection.is_none(), "Duplicate RDEF chunk");
                    reflection = Some(DxbcReflection::parse(chunk.data)?);
                }
                b"SHEX" | b"SHDR" => {
                    ensure!(program.is_none(), "Duplicate DXBC program chunk");
                    program = Some(TokenizedProgram::parse(chunk.data)?);
                }
                b"ISGN" | b"OSGN" | b"OSG5" | b"PCSG" | b"ISG1" | b"OSG1" | b"PSG1" => {
                    signatures.push(DxbcSignature::parse(chunk.tag, chunk.data)?);
                }
                _ => {} // STAT, debug data and other chunks remain available verbatim.
            }
        }
        if let (Some(r), Some(p)) = (&reflection, &program) {
            ensure!(
                r.stage == p.stage && r.major == p.major && r.minor == p.minor,
                "DXBC reflection/program stage or version mismatch"
            );
        }
        Ok(Self { checksum, chunks, reflection, signatures, program })
    }
}

/// D3D ordering differs from MPR's NShaderCore::EShader (compute is 5 here).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum D3dStage {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
}

#[derive(Debug, Serialize)]
pub struct DxbcReflection<'a> {
    pub stage: D3dStage,
    pub major: u8,
    pub minor: u8,
    pub compile_flags: u32,
    pub creator: &'a str,
    pub interface_slot_count: u32,
    pub buffers: Vec<DxbcBuffer<'a>>,
    pub bindings: Vec<DxbcBinding<'a>>,
    /// RDEF-relative offsets identify shared types. Members refer to this table;
    /// keeping a graph prevents repeated expansion of shared or cyclic records.
    pub types: BTreeMap<u32, DxbcType<'a>>,
}

#[derive(Debug, Serialize)]
pub struct DxbcBuffer<'a> {
    pub name: &'a str,
    pub size: u32,
    pub flags: u32,
    /// D3D_CBUFFER_TYPE.
    pub kind: u32,
    pub variables: Vec<DxbcVariable<'a>>,
}

#[derive(Debug, Serialize)]
pub struct DxbcVariable<'a> {
    pub name: &'a str,
    pub offset: u32,
    pub size: u32,
    /// D3D_SHADER_VARIABLE_FLAGS; bit 1 marks variables used by this program.
    pub flags: u32,
    pub type_offset: u32,
    pub default_value: Option<&'a [u8]>,
    pub start_texture: u32,
    pub texture_size: u32,
    pub start_sampler: u32,
    pub sampler_size: u32,
}

#[derive(Debug, Serialize)]
pub struct DxbcBinding<'a> {
    pub name: &'a str,
    /// D3D_SHADER_INPUT_TYPE (0=cbuffer, 2=texture, 3=sampler).
    pub kind: u32,
    pub return_type: u32,
    pub dimension: u32,
    pub sample_count: u32,
    pub bind_point: u32,
    pub bind_count: u32,
    pub flags: u32,
}

#[derive(Debug, Serialize)]
pub struct DxbcType<'a> {
    pub name: Option<&'a str>,
    /// D3D_SHADER_VARIABLE_CLASS / D3D_SHADER_VARIABLE_TYPE.
    pub class: u16,
    pub base_type: u16,
    pub rows: u16,
    pub columns: u16,
    /// Zero means a non-array type.
    pub elements: u16,
    pub members: Vec<DxbcTypeMember<'a>>,
    pub unknown: [u32; 4],
}

#[derive(Debug, Serialize)]
pub struct DxbcTypeMember<'a> {
    pub name: &'a str,
    pub type_offset: u32,
    /// Byte offset within the structure, not within RDEF.
    pub offset: u32,
}

impl<'a> DxbcReflection<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let target = u32_at(data, 16)?;
        // All startup programs use SM5.0. Gate the record sizes instead of
        // interpreting SM4, SM5.1 or DXIL reflection with the wrong strides.
        ensure!(target & 0xffff == 0x0500, "Unsupported RDEF shader version");
        let stage = match target >> 16 {
            0xffff => D3dStage::Pixel,
            0xfffe => D3dStage::Vertex,
            0x4753 => D3dStage::Geometry,
            0x4853 => D3dStage::Hull,
            0x4453 => D3dStage::Domain,
            0x4353 => D3dStage::Compute,
            _ => anyhow::bail!("Unsupported RDEF shader stage"),
        };
        ensure!(range(data, 28, 4)? == b"RD11", "Missing SM5 RD11 header");
        for (i, size) in [60, 24, 32, 40, 36, 12].into_iter().enumerate() {
            ensure!(u32_at(data, 32 + i * 4)? == size, "Unsupported RDEF record size");
        }
        let interface_slot_count = u32_at(data, 56)?;
        let creator = string_at(data, u32_at(data, 24)?)?;
        let mut buffers = Vec::new();
        let mut pending_types = BTreeSet::new();
        for b in table(data, u32_at(data, 4)?, u32_at(data, 0)?, 24)?.chunks_exact(24) {
            let size = u32_at(b, 12)?;
            let mut variables = Vec::new();
            for v in table(data, u32_at(b, 8)?, u32_at(b, 4)?, 40)?.chunks_exact(40) {
                let offset = u32_at(v, 4)?;
                let variable_size = u32_at(v, 8)?;
                ensure!(
                    u64::from(offset) + u64::from(variable_size) <= u64::from(size),
                    "RDEF variable outside its buffer"
                );
                let type_offset = u32_at(v, 16)?;
                pending_types.insert(type_offset);
                let default_offset = u32_at(v, 20)?;
                variables.push(DxbcVariable {
                    name: string_at(data, u32_at(v, 0)?)?,
                    offset,
                    size: variable_size,
                    flags: u32_at(v, 12)?,
                    type_offset,
                    default_value: if default_offset == 0 {
                        None
                    } else {
                        Some(range(data, default_offset as usize, variable_size as usize)?)
                    },
                    start_texture: u32_at(v, 24)?,
                    texture_size: u32_at(v, 28)?,
                    start_sampler: u32_at(v, 32)?,
                    sampler_size: u32_at(v, 36)?,
                });
            }
            buffers.push(DxbcBuffer {
                name: string_at(data, u32_at(b, 0)?)?,
                size,
                flags: u32_at(b, 16)?,
                kind: u32_at(b, 20)?,
                variables,
            });
        }
        let mut bindings = Vec::new();
        for b in table(data, u32_at(data, 12)?, u32_at(data, 8)?, 32)?.chunks_exact(32) {
            bindings.push(DxbcBinding {
                name: string_at(data, u32_at(b, 0)?)?,
                kind: u32_at(b, 4)?,
                return_type: u32_at(b, 8)?,
                dimension: u32_at(b, 12)?,
                sample_count: u32_at(b, 16)?,
                bind_point: u32_at(b, 20)?,
                bind_count: u32_at(b, 24)?,
                flags: u32_at(b, 28)?,
            });
        }
        let mut types = BTreeMap::new();
        while let Some(offset) = pending_types.pop_first() {
            if types.contains_key(&offset) {
                continue;
            }
            let t = range(data, offset as usize, 36)?;
            let mut members = Vec::new();
            for m in table(data, u32_at(t, 12)?, u32::from(u16_at(t, 10)?), 12)?.chunks_exact(12) {
                let type_offset = u32_at(m, 4)?;
                pending_types.insert(type_offset);
                members.push(DxbcTypeMember {
                    name: string_at(data, u32_at(m, 0)?)?,
                    type_offset,
                    offset: u32_at(m, 8)?,
                });
            }
            let name_offset = u32_at(t, 32)?;
            types.insert(offset, DxbcType {
                name: if name_offset == 0 { None } else { Some(string_at(data, name_offset)?) },
                class: u16_at(t, 0)?,
                base_type: u16_at(t, 2)?,
                rows: u16_at(t, 4)?,
                columns: u16_at(t, 6)?,
                elements: u16_at(t, 8)?,
                members,
                unknown: [u32_at(t, 16)?, u32_at(t, 20)?, u32_at(t, 24)?, u32_at(t, 28)?],
            });
        }
        Ok(Self {
            stage,
            major: 5,
            minor: 0,
            compile_flags: u32_at(data, 20)?,
            creator,
            interface_slot_count,
            buffers,
            bindings,
            types,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct DxbcSignature<'a> {
    pub tag: [u8; 4],
    pub header_size: u32,
    pub elements: Vec<DxbcSignatureElement<'a>>,
}

#[derive(Debug, Serialize)]
pub struct DxbcSignatureElement<'a> {
    pub stream: u32,
    pub semantic: &'a str,
    pub semantic_index: u32,
    pub system_value: u32,
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    /// Raw read/write mask: input components always read, or output components
    /// never written. Do not treat this as the same "used" mask for both.
    pub read_write_mask: u8,
    pub reserved: u16,
    pub min_precision: Option<u32>,
}

impl<'a> DxbcSignature<'a> {
    pub fn parse(tag: [u8; 4], data: &'a [u8]) -> Result<Self> {
        let (stream, precision) = match &tag {
            b"ISGN" | b"OSGN" | b"PCSG" => (false, false),
            b"OSG5" => (true, false),
            b"ISG1" | b"OSG1" | b"PSG1" => (true, true),
            _ => anyhow::bail!("Unsupported DXBC signature tag"),
        };
        let header_size = u32_at(data, 4)?;
        ensure!(header_size >= 8 && header_size % 4 == 0, "Invalid DXBC signature header size");
        let stride = 24 + usize::from(stream) * 4 + usize::from(precision) * 4;
        let mut elements = Vec::new();
        for e in table(data, header_size, u32_at(data, 0)?, stride)?.chunks_exact(stride) {
            let skip = usize::from(stream) * 4;
            elements.push(DxbcSignatureElement {
                stream: if stream { u32_at(e, 0)? } else { 0 },
                semantic: string_at(data, u32_at(e, skip)?)?,
                semantic_index: u32_at(e, skip + 4)?,
                system_value: u32_at(e, skip + 8)?,
                component_type: u32_at(e, skip + 12)?,
                register: u32_at(e, skip + 16)?,
                mask: e[skip + 20],
                read_write_mask: e[skip + 21],
                reserved: u16_at(e, skip + 22)?,
                min_precision: if precision { Some(u32_at(e, skip + 24)?) } else { None },
            });
        }
        Ok(Self { tag, header_size, elements })
    }
}

#[derive(Debug, Serialize)]
pub struct TokenizedProgram {
    pub stage: D3dStage,
    pub major: u8,
    pub minor: u8,
    pub token_count: u32,
    pub instructions: Vec<DxbcInstruction>,
}

#[derive(Debug, Serialize)]
pub struct DxbcInstruction {
    /// Byte offset relative to the SHDR/SHEX payload, including its 8-byte header.
    pub offset: u32,
    pub opcode: u16,
    /// All original tokens including opcode, extensions and operands. Operand
    /// decoding is intentionally separate from this instruction-boundary parser.
    pub words: Vec<u32>,
    pub extended_opcodes: Vec<u32>,
    pub custom_data_class: Option<u32>,
}

impl TokenizedProgram {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let version = u32_at(data, 0)?;
        let major = ((version >> 4) & 15) as u8;
        let minor = (version & 15) as u8;
        ensure!(matches!((major, minor), (4, 0 | 1) | (5, 0)), "Unsupported DXBC program version");
        let stage = match version >> 16 {
            0 => D3dStage::Pixel,
            1 => D3dStage::Vertex,
            2 => D3dStage::Geometry,
            3 => D3dStage::Hull,
            4 => D3dStage::Domain,
            5 => D3dStage::Compute,
            _ => anyhow::bail!("Unsupported DXBC program stage"),
        };
        let token_count = u32_at(data, 4)?;
        ensure!(
            token_count >= 2 && u64::from(token_count) * 4 == data.len() as u64,
            "DXBC program token count mismatch"
        );
        let mut offset = 8;
        let mut instructions = Vec::new();
        while offset < data.len() {
            let token = u32_at(data, offset)?;
            let opcode = (token & 0x7ff) as u16;
            let custom = opcode == 53; // D3D10_SB_OPCODE_CUSTOMDATA.
            let length = if custom { u32_at(data, offset + 4)? } else { (token >> 24) & 127 };
            ensure!(length >= if custom { 2 } else { 1 }, "Invalid DXBC instruction length");
            let bytes = table(data, offset as u32, length, 4)?;
            let words = bytes.chunks_exact(4).map(|b| u32_at(b, 0)).collect::<Result<Vec<_>>>()?;
            let mut extended_opcodes = Vec::new();
            if !custom {
                let mut extended = token & 0x80000000 != 0;
                let mut index = 1;
                while extended {
                    let word = *words.get(index).context("Truncated DXBC extended opcode")?;
                    extended_opcodes.push(word);
                    extended = word & 0x80000000 != 0;
                    index += 1;
                }
            }
            instructions.push(DxbcInstruction {
                offset: offset as u32,
                opcode,
                words,
                extended_opcodes,
                custom_data_class: custom.then_some(token >> 11),
            });
            offset += bytes.len();
        }
        Ok(Self { stage, major, minor, token_count, instructions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() }

    fn put(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_string(data: &mut Vec<u8>, field: usize, value: &str) {
        let offset = data.len() as u32;
        put(data, field, offset);
        data.extend(value.as_bytes());
        data.push(0);
    }

    fn container(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut data = vec![0; 32 + chunks.len() * 4];
        data[..4].copy_from_slice(b"DXBC");
        put(&mut data, 20, 1);
        put(&mut data, 28, chunks.len() as u32);
        for (index, (tag, bytes)) in chunks.iter().enumerate() {
            while data.len() % 4 != 0 {
                data.push(0);
            }
            let offset = data.len() as u32;
            put(&mut data, 32 + index * 4, offset);
            data.extend(tag);
            data.extend((bytes.len() as u32).to_le_bytes());
            data.extend(bytes);
        }
        let size = data.len() as u32;
        put(&mut data, 24, size);
        data
    }

    fn reflection() -> Vec<u8> {
        let mut d = vec![0; 252];
        for (offset, value) in [
            (0, 1),
            (4, 60),
            (8, 1),
            (12, 84),
            (16, 0xffff0500),
            (28, u32::from_le_bytes(*b"RD11")),
            (32, 60),
            (36, 24),
            (40, 32),
            (44, 40),
            (48, 36),
            (52, 12),
            // Buffer at 60, binding at 84, variable at 116.
            (64, 1),
            (68, 116),
            (72, 32),
            (108, 1),
            (124, 32),
            (128, 2),
            (132, 156),
            // A structure with two members sharing the float4 type at 192.
            (156, 5),
            (160, 1 | (8 << 16)),
            (164, 2 << 16),
            (168, 228),
            (192, 1 | (3 << 16)),
            (196, 1 | (4 << 16)),
            (232, 192),
            (244, 192),
            (248, 16),
        ] {
            put(&mut d, offset, value);
        }
        for (field, text) in [
            (24, "test compiler"),
            (60, "Buffer"),
            (84, "Buffer"),
            (116, "pair"),
            (188, "Pair"),
            (224, "float4"),
            (228, "a"),
            (240, "b"),
        ] {
            put_string(&mut d, field, text);
        }
        let default_offset = d.len() as u32;
        put(&mut d, 136, default_offset);
        d.extend([0x55; 32]);
        d
    }

    #[test]
    fn rdef_preserves_shared_nested_types_and_checks_references() {
        let data = reflection();
        let r = DxbcReflection::parse(&data).unwrap();
        assert_eq!(r.creator, "test compiler");
        assert_eq!(r.stage, D3dStage::Pixel);
        assert_eq!(r.buffers[0].name, "Buffer");
        assert_eq!(r.bindings[0].name, "Buffer");
        assert_eq!(r.buffers[0].variables[0].name, "pair");
        assert_eq!(r.buffers[0].variables[0].default_value.unwrap(), &[0x55; 32]);
        assert_eq!(r.types.len(), 2);
        assert_eq!(r.types[&156].members[1].name, "b");
        assert_eq!(r.types[&156].members[1].offset, 16);
        assert_eq!(r.types[&156].members[1].type_offset, 192);
        assert_eq!(r.types[&192].columns, 4);
        for (offset, value) in [
            (0, u32::MAX),
            (4, u32::MAX),
            (16, 0xffff0501),
            (44, 24),
            (72, 31),
            (132, u32::MAX),
            (136, u32::MAX),
            (232, u32::MAX),
            (240, data.len() as u32),
        ] {
            let mut invalid = data.clone();
            put(&mut invalid, offset, value);
            assert!(DxbcReflection::parse(&invalid).is_err(), "field {offset}");
        }
        // Even a corrupt self-reference cannot cause recursive expansion.
        let mut cyclic = data.clone();
        put(&mut cyclic, 232, 156);
        assert_eq!(DxbcReflection::parse(&cyclic).unwrap().types.len(), 2);
        let mut unaligned = vec![0];
        unaligned.extend(&data);
        assert!(DxbcReflection::parse(&unaligned[1..]).is_ok());
    }

    #[test]
    fn container_rejects_bad_ranges_and_preserves_unknown_chunks() {
        let data =
            container(&[(*b"JUNK", vec![1, 2, 3]), (*b"SHEX", words(&[0x50, 3, 0x0100003e]))]);
        let parsed = Dxbc::parse(&data).unwrap();
        assert_eq!(parsed.chunks[0].data, &[1, 2, 3]);
        assert_eq!(parsed.program.unwrap().stage, D3dStage::Pixel);
        for end in 0..data.len() {
            assert!(Dxbc::parse(&data[..end]).is_err());
        }
        for (offset, value) in
            [(20, 2), (28, u32::MAX), (32, 4), (32, 41), (36, 40), (44, u32::MAX)]
        {
            let mut invalid = data.clone();
            put(&mut invalid, offset, value);
            assert!(Dxbc::parse(&invalid).is_err(), "field {offset}");
        }
        let mismatched = container(&[(*b"RDEF", reflection()), (*b"SHEX", words(&[0x10050, 2]))]);
        assert!(Dxbc::parse(&mismatched).is_err());
    }

    #[test]
    fn geometry_output_signature_keeps_stream_and_raw_write_mask() {
        let mut data = words(&[1, 8, 2, 36, 1, 0, 3, 5, 0x010f]);
        data.extend(b"COLOR\0");
        let s = DxbcSignature::parse(*b"OSG5", &data).unwrap();
        assert_eq!(s.elements[0].stream, 2);
        assert_eq!(s.elements[0].semantic, "COLOR");
        assert_eq!(s.elements[0].register, 5);
        assert_eq!(s.elements[0].mask, 15);
        assert_eq!(s.elements[0].read_write_mask, 1);
        for (offset, value) in [(0, u32::MAX), (4, 7), (4, 44), (12, u32::MAX)] {
            let mut invalid = data.clone();
            put(&mut invalid, offset, value);
            assert!(DxbcSignature::parse(*b"OSG5", &invalid).is_err());
        }
        data.pop();
        assert!(DxbcSignature::parse(*b"OSG5", &data).is_err());
    }

    #[test]
    fn token_framing_handles_extensions_and_custom_data() {
        let data = words(&[0x50, 9, 0x82000036, 0, 53 | (3 << 11), 4, 123, 456, 0x0100003e]);
        let p = TokenizedProgram::parse(&data).unwrap();
        assert_eq!(p.instructions.len(), 3);
        assert_eq!(p.instructions[0].extended_opcodes, [0]);
        assert_eq!(p.instructions[1].custom_data_class, Some(3));
        assert_eq!(p.instructions[1].words[2..], [123, 456]);
        assert_eq!(p.instructions[2].offset, 32);
        for (offset, value) in
            [(4, 8), (8, 0), (8, 0x7f000036), (12, 0x80000000), (20, 1), (20, u32::MAX)]
        {
            let mut invalid = data.clone();
            put(&mut invalid, offset, value);
            assert!(TokenizedProgram::parse(&invalid).is_err());
        }
    }
}
