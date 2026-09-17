//! Maxwell hardware shader headers and instruction framing.
//!
//! References: NVIDIA's Shader Program Header specification; devkitPro/uam
//! nv_shader_header.h and nv50_ir_emit_gm107.cpp (5a5afc2bae8b55409ab36ba45be63fcb73f68993);
//! Mesa NAK sm50.rs (822120d6d5c0109f98c3fbf85ec716c5d1fa00b6).
//! This module decodes framing and control fields. TEXS/TLDS operand decoding is in
//! maxwell::texture; indexed constant loads are in maxwell::memory.
//! Scalar moves and float operations are in maxwell::alu. Other instruction
//! families remain raw; recognizing an instruction does not imply host support.

pub mod alu;
pub mod attribute;
pub mod compare;
pub mod control;
pub mod convert;
mod decode;
pub mod graphics;
pub mod integer;
pub mod integer_compare;
pub mod io;
pub(crate) mod lift;
pub mod memory;
pub mod raster;
pub mod texture;
pub mod xmad;

use anyhow::{ensure, Context, Result};
pub use decode::Operation;
use serde_derive::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct ShaderProgramHeader {
    /// Preserve all 80 hardware bytes, including stage-specific attribute maps.
    pub words: [u32; 20],
    /// 1 = VTG, 2 = pixel. NVIDIA's field table and uam agree on these values.
    pub sph_type: u8,
    pub version: u8,
    /// 1 = vertex, 2/3 = tessellation, 4 = geometry, 5 = pixel.
    pub shader_type: u8,
    pub mrt_enable: bool,
    pub kills_pixels: bool,
    pub global_store: bool,
    pub sass_version: u8,
    pub load_or_store: bool,
    pub fp64: bool,
    pub stream_out_mask: u8,
    pub local_memory_low: u32,
    pub local_memory_high: u32,
    pub local_memory_crs: u32,
    pub per_patch_attributes: u8,
    pub threads_per_input_primitive: u8,
    pub output_topology: u8,
    pub max_output_vertices: u16,
    pub store_request_start: u8,
    pub store_request_end: u8,
}

impl ShaderProgramHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let bytes = data.get(..80).context("Truncated Maxwell shader program header")?;
        let words: [u32; 20] = std::array::from_fn(|i| {
            u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        });
        let w = words[0];
        Ok(Self {
            words,
            sph_type: (w & 31) as u8,
            version: ((w >> 5) & 31) as u8,
            shader_type: ((w >> 10) & 15) as u8,
            mrt_enable: w & (1 << 14) != 0,
            kills_pixels: w & (1 << 15) != 0,
            global_store: w & (1 << 16) != 0,
            sass_version: ((w >> 17) & 15) as u8,
            load_or_store: w & (1 << 26) != 0,
            fp64: w & (1 << 27) != 0,
            stream_out_mask: (w >> 28) as u8,
            local_memory_low: words[1] & 0xffffff,
            local_memory_high: words[2] & 0xffffff,
            local_memory_crs: words[3] & 0xffffff,
            per_patch_attributes: (words[1] >> 24) as u8,
            threads_per_input_primitive: (words[2] >> 24) as u8,
            output_topology: ((words[3] >> 24) & 15) as u8,
            max_output_vertices: (words[4] & 0xfff) as u16,
            store_request_start: ((words[4] >> 12) & 255) as u8,
            store_request_end: (words[4] >> 24) as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Schedule {
    pub raw: u32,
    pub stall: u8,
    pub yield_flag: bool,
    /// 7 encodes no barrier; all other values are preserved.
    pub write_barrier: Option<u8>,
    pub read_barrier: Option<u8>,
    pub wait_barrier_mask: u8,
    pub reuse_mask: u8,
}

impl Schedule {
    fn from_raw(raw: u32) -> Self {
        let write = ((raw >> 5) & 7) as u8;
        let read = ((raw >> 8) & 7) as u8;
        Self {
            raw,
            stall: (raw & 15) as u8,
            yield_flag: raw & 16 != 0,
            write_barrier: (write != 7).then_some(write),
            read_barrier: (read != 7).then_some(read),
            wait_barrier_mask: ((raw >> 11) & 63) as u8,
            reuse_mask: ((raw >> 17) & 15) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Instruction {
    /// Physical byte offset from the code start, including scheduling words.
    pub offset: usize,
    pub word: u64,
    pub schedule: Schedule,
    /// Common instruction predicate; 7 is PT. Full operand decoding is separate.
    pub predicate: u8,
    pub predicate_inverted: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MaxwellCode<'a> {
    pub bytes: &'a [u8],
}

impl<'a> MaxwellCode<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            bytes.len().is_multiple_of(32),
            "Maxwell code contains a partial instruction group"
        );
        for group in bytes.chunks_exact(32) {
            ensure!(group[7] & 0x80 == 0, "Maxwell scheduling word has its reserved high bit set");
        }
        Ok(Self { bytes })
    }

    /// Each 32-byte group has one scheduling word and three 64-bit instructions.
    pub fn instructions(&self) -> impl Iterator<Item = Instruction> + '_ {
        self.bytes.chunks_exact(32).enumerate().flat_map(|(group_index, group)| {
            let schedule = u64::from_le_bytes(group[..8].try_into().unwrap());
            (0..3).map(move |slot| {
                let offset = 8 * (slot + 1);
                let word = u64::from_le_bytes(group[offset..offset + 8].try_into().unwrap());
                Instruction {
                    offset: group_index * 32 + offset,
                    word,
                    schedule: Schedule::from_raw(((schedule >> (slot * 21)) & 0x1fffff) as u32),
                    predicate: ((word >> 16) & 7) as u8,
                    predicate_inverted: word & (1 << 19) != 0,
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduling_words_do_not_become_instructions() {
        let a: u32 = 5 | 16 | (2 << 5) | (7 << 8) | (0x15 << 11) | (9 << 17);
        let b: u32 = 3 | (7 << 5) | (4 << 8);
        let c: u32 = 0x7e0;
        let words = [
            u64::from(a) | (u64::from(b) << 21) | (u64::from(c) << 42),
            0x50b0000000070f00, // NOP, PT
            0xe3000000000b000f, // Predicated EXIT, !P3
            0xe2400fffff87000f, // Padding BRA
        ];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let code = MaxwellCode::parse(&bytes).unwrap();
        let instructions: Vec<_> = code.instructions().collect();
        assert_eq!(instructions.iter().map(|i| i.offset).collect::<Vec<_>>(), [8, 16, 24]);
        assert_eq!(instructions[0].predicate, 7);
        assert_eq!(instructions[1].predicate, 3);
        assert!(instructions[1].predicate_inverted);
        let s = instructions[0].schedule;
        assert_eq!(
            (s.stall, s.yield_flag, s.write_barrier, s.read_barrier),
            (5, true, Some(2), None)
        );
        assert_eq!((s.wait_barrier_mask, s.reuse_mask), (0x15, 9));
        assert_eq!(instructions[1].schedule.read_barrier, Some(4));
        assert!(MaxwellCode::parse(&bytes[..31]).is_err());
        let mut bad = bytes;
        bad[7] |= 0x80;
        assert!(MaxwellCode::parse(&bad).is_err());
    }
}
