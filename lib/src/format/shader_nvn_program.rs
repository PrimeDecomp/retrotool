//! NVN control-pool fields and bounded Maxwell program extraction.
//!
//! MPR forwards these records unchanged to nvnProgramSetShaders (0x71001a9370).
//! Field offsets are established for the control signature below from the
//! original SDK, corpus checks and SM53 disassembly, not by substituting DKSH.

use anyhow::{ensure, Context, Result};
use remax::maxwell::{MaxwellCode, ShaderProgramHeader};
use serde_derive::Serialize;

use crate::format::shader_nvn::{NvnShader, ShaderStage};

fn word(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data.get(offset..offset + 4).context("Truncated NVN control record")?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

#[derive(Clone, Debug, Serialize)]
pub struct ProgramControl {
    /// Raw prefix, including the currently supported version/layout signature.
    pub header: [u32; 9],
    /// Bytes starting at program_offset: graphics SPH + code, or compute code.
    pub program_size: u32,
    /// Exact compiler constant bytes after code_allocation_size, excluding padding.
    /// SDK getter4706a0 -> descriptor+4 -> stageInfo+14. Graphics macro12 binds
    /// the allocation's GPU address to physical constant bank1 (method value0x11).
    pub constant_data_size: u32,
    /// Aligned allocation containing prefix, hardware header, and code.
    pub code_allocation_size: u32,
    /// Entire GPU record, including data following the code allocation.
    pub gpu_record_size: u32,
    /// 0x30 for graphics (points to SPH), 0x100 for compute (points to code).
    pub program_offset: u32,
    /// Matches/bounds the registers used by all disassembled startup programs.
    pub register_count: u32,
    /// Vendor stage at0x714: vertex0, pixel1, geometry2, compute5 in MPR.
    /// This numbering differs from both the MPR envelope and the hardware SPH.
    pub vendor_stage: u32,
    /// Pixel-only flag at0x74a; other stages use a different control union.
    /// The SDK getter4708b4 returns this byte, and470fa8 converts nonzero to true.
    /// NVN ProgramSetSampleShading may override it when building the program.
    pub fragment_sample_shading: Option<bool>,
}

impl ProgramControl {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() >= 0x880, "Truncated NVN control record");
        let mut header = [0; 9];
        for (i, v) in header.iter_mut().enumerate() {
            *v = word(data, i * 4)?;
        }
        ensure!(
            header[..6] == [0x98761234, 1, 16, 0x120, 11, 0x878] && header[8] == 0x880,
            "Unsupported NVN control record layout"
        );
        let vendor_stage = word(data, 0x714)?;
        Ok(Self {
            header,
            program_size: word(data, 0x6f8)?,
            constant_data_size: word(data, 0x6fc)?,
            code_allocation_size: word(data, 0x700)?,
            gpu_record_size: word(data, 0x704)?,
            program_offset: word(data, 0x708)?,
            register_count: word(data, 0x70c)?,
            vendor_stage,
            fragment_sample_shading: (vendor_stage == 1).then(|| data[0x74a] != 0),
        })
    }
}

#[derive(Debug)]
pub struct NvnProgram<'a> {
    pub stage: &'a ShaderStage,
    pub control: ProgramControl,
    pub header: Option<ShaderProgramHeader>,
    /// Absolute byte offset in this shader's GPU pool.
    pub code_offset: usize,
    pub code: MaxwellCode<'a>,
    /// Absolute byte offset in this shader's GPU pool, after the code allocation.
    pub constant_data_offset: usize,
    /// Compiler constants, bounded by constant_data_size. Graphics binds these
    /// at bank1 separately for each stage. Allocation padding is not readable.
    pub constant_data: &'a [u8],
    /// Entire tail after the code allocation, including constants and padding.
    pub additional_gpu_data: &'a [u8],
}

impl NvnShader {
    /// Decode vendor program records separately from the portable MPR envelope.
    /// Unknown NVN control revisions return an error rather than guessed ranges.
    pub fn programs(&self) -> Result<Vec<NvnProgram<'_>>> {
        self.stages
            .iter()
            .map(|stage| {
                ensure!(stage.kind <= 3, "Unsupported MPR shader stage {}", stage.kind);
                let control_data = self
                    .cpu_pool
                    .get(stage.cpu_offset as usize..)
                    .context("NVN CPU program offset out of range")?;
                let control = ProgramControl::parse(control_data)?;
                ensure!(
                    control.vendor_stage == [0, 1, 2, 5][stage.kind as usize],
                    "NVN envelope/control stage mismatch"
                );
                let start = stage.gpu_offset as usize;
                let gpu_size = control.gpu_record_size as usize;
                let end = start.checked_add(gpu_size).context("NVN GPU program size overflow")?;
                let data = self.gpu_pool.get(start..end).context("NVN GPU program out of range")?;
                let compute = stage.kind == 3;
                ensure!(
                    word(data, 0)? == if compute { 0x12345679 } else { 0x12345678 },
                    "Invalid NVN GPU program magic"
                );
                let program_offset = control.program_offset as usize;
                ensure!(
                    program_offset == if compute { 0x100 } else { 0x30 },
                    "Unexpected NVN program offset"
                );
                let program_end = program_offset
                    .checked_add(control.program_size as usize)
                    .context("NVN program size overflow")?;
                let allocation = control.code_allocation_size as usize;
                ensure!(
                    program_end <= allocation && allocation <= data.len(),
                    "NVN code exceeds allocation"
                );
                let additional_gpu_data = &data[allocation..];
                let constant_data = additional_gpu_data
                    .get(..control.constant_data_size as usize)
                    .context("NVN compiler constants exceed GPU record")?;
                let program =
                    data.get(program_offset..program_end).context("NVN program out of range")?;
                let header = if compute {
                    None
                } else {
                    let header = ShaderProgramHeader::parse(program)?;
                    let expected_type = [1, 5, 4][stage.kind as usize];
                    ensure!(
                        header.shader_type == expected_type
                            && header.sph_type == if stage.kind == 1 { 2 } else { 1 },
                        "NVN stage/hardware header mismatch"
                    );
                    Some(header)
                };
                let header_size = if header.is_some() { 80 } else { 0 };
                let code = MaxwellCode::parse(&program[header_size..])?;
                Ok(NvnProgram {
                    stage,
                    control,
                    header,
                    code_offset: start + program_offset + header_size,
                    code,
                    constant_data_offset: start + allocation,
                    constant_data,
                    additional_gpu_data,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn sample(kind: u32) -> NvnShader {
        let compute = kind == 3;
        // Deliberately unaligned pool offsets exercise packed little-endian reads.
        let cpu_base = 3;
        let gpu_base = 5;
        let mut cpu_pool = vec![0; cpu_base + 0x880];
        let cpu = &mut cpu_pool[cpu_base..];
        for (i, v) in [0x98761234, 1, 16, 0x120, 11, 0x878, 0, 0, 0x880].iter().enumerate() {
            put(cpu, i * 4, *v);
        }
        let (program_offset, program_size, allocation) =
            if compute { (256, 32, 512) } else { (48, 112, 256) };
        for (offset, value) in [
            (0x6f8, program_size),
            (0x700, allocation),
            (0x704, 768),
            (0x708, program_offset),
            (0x70c, 8),
            (0x714, [0, 1, 2, 5][kind as usize]),
        ] {
            put(cpu, offset, value);
        }
        // Nonzero allocation padding and constants must never become instructions.
        let mut gpu_pool = vec![0xff; gpu_base + 768];
        let gpu = &mut gpu_pool[gpu_base..];
        put(gpu, 0, if compute { 0x12345679 } else { 0x12345678 });
        if !compute {
            gpu[48..128].fill(0);
            let shader_type = [1, 5, 4][kind as usize];
            let sph_type = if kind == 1 { 2 } else { 1 };
            put(gpu, 48, sph_type | (3 << 5) | (shader_type << 10) | (1 << 15));
            put(gpu, 52, 0x12000080);
            put(gpu, 56, 0x34000100);
            put(gpu, 60, 0x07000200);
            put(gpu, 64, 0x56078009);
        }
        let code_start = if compute { 256 } else { 128 };
        gpu[code_start..code_start + 32].fill(0);
        for i in 0..3 {
            gpu[code_start + 8 + i * 8..code_start + 16 + i * 8]
                .copy_from_slice(&0x50b0000000070f00u64.to_le_bytes());
        }
        NvnShader {
            unknown: 0,
            gpu_pool,
            cpu_pool,
            stages: vec![ShaderStage {
                kind,
                gpu_offset: gpu_base as u32,
                cpu_offset: cpu_base as u32,
                scratch_memory: [0, 0],
            }],
        }
    }

    #[test]
    fn programs_use_control_bounds_and_distinguish_compute_headers() {
        for kind in 0..4 {
            let shader = sample(kind);
            let programs = shader.programs().unwrap();
            let p = &programs[0];
            assert_eq!(p.code.bytes.len(), 32);
            assert_eq!(p.code.instructions().count(), 3);
            assert!(p.additional_gpu_data.iter().all(|&b| b == 0xff));
            assert_eq!(p.control.register_count, 8);
            if kind == 3 {
                assert!(p.header.is_none());
                assert_eq!(p.code_offset, 261);
                assert_eq!(p.additional_gpu_data.len(), 256);
            } else {
                assert_eq!(p.code_offset, 133);
                assert_eq!(p.additional_gpu_data.len(), 512);
                let h = p.header.as_ref().unwrap();
                assert!(h.kills_pixels);
                assert_eq!((h.version, h.local_memory_low, h.per_patch_attributes), (3, 128, 0x12));
                assert_eq!((h.local_memory_high, h.threads_per_input_primitive), (256, 0x34));
                assert_eq!((h.local_memory_crs, h.output_topology), (512, 7));
                assert_eq!(
                    (h.max_output_vertices, h.store_request_start, h.store_request_end),
                    (9, 0x78, 0x56)
                );
            }
        }
    }

    #[test]
    fn rejects_unsupported_control_layouts_and_out_of_bounds_programs() {
        for (offset, value) in [
            (0, 0),
            (4, 2),
            (0x10, 12),
            (0x6f8, u32::MAX),
            (0x6f8, 79),
            (0x6fc, u32::MAX),
            (0x6fc, 513),
            (0x700, 127),
            (0x704, u32::MAX),
            (0x708, 49),
            (0x714, 1),
        ] {
            let mut shader = sample(0);
            put(&mut shader.cpu_pool, 3 + offset, value);
            assert!(shader.programs().is_err(), "accepted {offset:#x}={value:#x}");
        }
        let mut shader = sample(0);
        shader.cpu_pool.pop();
        assert!(shader.programs().is_err());
        let mut shader = sample(0);
        shader.gpu_pool.pop();
        assert!(shader.programs().is_err());
        let mut shader = sample(0);
        shader.gpu_pool[5 + 128 + 7] = 0x80;
        assert!(shader.programs().is_err());
        let mut shader = sample(0);
        shader.stages[0].kind = 1; // Vertex header contradicts pixel descriptor.
        assert!(shader.programs().is_err());
    }

    #[test]
    fn compiler_constants_exclude_allocation_padding() {
        for kind in 0..4 {
            let mut shader = sample(kind);
            for size in [0, 1, 4, 23, 24, 256] {
                put(&mut shader.cpu_pool, 3 + 0x6fc, size);
                let programs = shader.programs().unwrap();
                let p = &programs[0];
                assert_eq!(p.constant_data.len(), size as usize);
                assert_eq!(p.constant_data_offset, 5 + p.control.code_allocation_size as usize);
                assert_eq!(p.constant_data, &p.additional_gpu_data[..size as usize]);
                assert!(p.additional_gpu_data.len() >= 256);
                assert_eq!(p.code.instructions().count(), 3);
            }
        }
    }

    #[test]
    fn frequency_flag_is_a_pixel_control_union_member() {
        for kind in 0..4 {
            let mut shader = sample(kind);
            for flag in [0, 1, 0x80, 0xff] {
                shader.cpu_pool[3 + 0x74a] = flag;
                let programs = shader.programs().unwrap();
                let control = &programs[0].control;
                assert_eq!(control.vendor_stage, [0, 1, 2, 5][kind as usize]);
                assert_eq!(control.fragment_sample_shading, (kind == 1).then_some(flag != 0));
            }
        }
    }
}
