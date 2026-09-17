//! Maxwell SPHv3 stage I/O maps. References: NVIDIA's Shader Program Header
//! specification; uam source/nv_shader_header.h and nv_attributes.h at
//! 5a5afc2bae8b55409ab36ba45be63fcb73f68993; Mesa NAK sph.rs at
//! 822120d6d5c0109f98c3fbf85ec716c5d1fa00b6. The hardware header supplies
//! interpolation setup independently of the IPA instruction's operation mode.

use serde_derive::Serialize;

use crate::maxwell::ShaderProgramHeader;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PixelInputMode {
    Unused,
    Constant,
    Perspective,
    ScreenLinear,
}

impl PixelInputMode {
    fn from_code(code: u32) -> Self {
        [Self::Unused, Self::Constant, Self::Perspective, Self::ScreenLinear][code as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PixelInput {
    pub byte_offset: u16,
    pub mode: PixelInputMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PixelShaderIo {
    /// Raw system A/B mask; position XYZW occupies bits28..31.
    pub system_inputs_ab: u32,
    /// System attributes0x2c0..0x2fc, one bit per word.
    pub system_inputs_c: u16,
    /// Enabled generic, front-color and fixed-function texture attributes.
    pub inputs: Vec<PixelInput>,
    /// Bits0..3 select RGBA for each render target. See `output_registers` for
    /// the compacted GPR assignment; target indices are not register groups.
    pub color_masks: [u8; 8],
    pub writes_sample_mask: bool,
    pub writes_depth: bool,
    /// Retain the remaining SPHv3 fields without importing later SM revisions.
    pub unknown_word17_high: u16,
    pub unknown_word19_bits: u32,
}

/// GPRs exported when a pixel shader exits. Each present target reserves four
/// registers, including masked components. Absent targets reserve none. The
/// sample-mask slot follows the colors; depth is one register after that slot,
/// even when the shader does not export a sample mask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PixelOutputRegisters {
    /// Target index, then RGBA component. Only exported components are present.
    pub colors: [[Option<u8>; 4]; 8],
    pub sample_mask: Option<u8>,
    pub depth: Option<u8>,
}

impl PixelOutputRegisters {
    /// Observable registers in target/component order, followed by mask/depth.
    /// Unexported components and the reserved mask slot are excluded.
    pub fn registers(&self) -> impl Iterator<Item = u8> + '_ {
        self.colors.iter().flatten().copied().flatten().chain(self.sample_mask).chain(self.depth)
    }
}

impl PixelShaderIo {
    /// Maxwell output allocation, corroborated by uam's
    /// `nvc0_fp_assign_output_slots`, NAK's `copy_fs_outputs_nv` lowering and MPR's
    /// original MRT/depth programs. Routing to bound targets is separate state.
    pub fn output_registers(&self) -> PixelOutputRegisters {
        let mut next_register = 0;
        let colors = self.color_masks.map(|mask| {
            if mask == 0 {
                return [None; 4];
            }
            let base = next_register;
            next_register += 4;
            std::array::from_fn(|component| {
                (mask & (1 << component) != 0).then_some(base + component as u8)
            })
        });
        PixelOutputRegisters {
            colors,
            sample_mask: self.writes_sample_mask.then_some(next_register),
            depth: self.writes_depth.then_some(next_register + 1),
        }
    }
}

impl ShaderProgramHeader {
    /// SPHv3 pixel I/O only. Unknown versions/stages must not inherit this map.
    pub fn pixel_io(&self) -> Option<PixelShaderIo> {
        if self.version != 3 || self.sph_type != 2 || self.shader_type != 5 {
            return None;
        }
        let mut inputs = Vec::new();
        // Generic128 words; front diffuse/specular8; fixed-function texture40.
        for (address, first_bit, count) in [(0x80, 192, 128), (0x280, 448, 8), (0x300, 480, 40)] {
            for i in 0..count {
                let bit = first_bit + i * 2;
                let mode = PixelInputMode::from_code((self.words[bit / 32] >> (bit % 32)) & 3);
                if mode != PixelInputMode::Unused {
                    inputs.push(PixelInput { byte_offset: (address + i * 4) as u16, mode });
                }
            }
        }
        Some(PixelShaderIo {
            system_inputs_ab: self.words[5],
            system_inputs_c: (self.words[14] >> 16) as u16,
            inputs,
            color_masks: std::array::from_fn(|i| ((self.words[18] >> (i * 4)) & 15) as u8),
            writes_sample_mask: self.words[19] & 1 != 0,
            writes_depth: self.words[19] & 2 != 0,
            unknown_word17_high: (self.words[17] >> 16) as u16,
            unknown_word19_bits: self.words[19] & !3,
        })
    }

    /// Tests a SPHv3 VTG generic word, a system value at 0x60..0x7c, or one of
    /// the eight clip-distance words at 0x2c0..0x2dc.
    /// Other attribute ranges and stages return None instead of false defaults.
    pub fn vtg_attribute_enabled(&self, byte_offset: u16, output: bool) -> Option<bool> {
        if self.version != 3 || self.sph_type != 1 || !byte_offset.is_multiple_of(4) {
            return None;
        }
        let bit = if (0x60..0x80).contains(&byte_offset) {
            (if output { 400 } else { 160 }) + usize::from(byte_offset / 4)
        } else if (0x80..0x280).contains(&byte_offset) {
            (if output { 432 } else { 192 }) + usize::from((byte_offset - 0x80) / 4)
        } else if (0x2c0..0x2e0).contains(&byte_offset) {
            (if output { 576 } else { 336 }) + usize::from((byte_offset - 0x2c0) / 4)
        } else {
            return None;
        };
        Some((self.words[bit / 32] >> (bit % 32)) & 1 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_distance_maps_keep_inputs_outputs_and_adjacent_system_bits_separate() {
        let mut words = [0u32; 20];
        words[0] = 1 | (3 << 5) | (1 << 10);
        words[10] = (0x81 << 16) | (1 << 24);
        words[18] = 0x42 | (1 << 8);
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let mut header = ShaderProgramHeader::parse(&bytes).unwrap();
        for plane in 0..8 {
            let offset = 0x2c0 + 4 * plane;
            assert_eq!(header.vtg_attribute_enabled(offset, false), Some(plane == 0 || plane == 7));
            assert_eq!(header.vtg_attribute_enabled(offset, true), Some(plane == 1 || plane == 6));
        }
        for offset in [0x2bc, 0x2c1, 0x2dd, 0x2e0] {
            assert_eq!(header.vtg_attribute_enabled(offset, true), None);
        }
        header.sph_type = 2;
        header.shader_type = 5;
        assert_eq!(header.vtg_attribute_enabled(0x2c0, true), None);
        header.sph_type = 1;
        header.shader_type = 1;
        header.version = 4;
        assert_eq!(header.vtg_attribute_enabled(0x2c0, true), None);
    }

    #[test]
    fn pixel_exports_reserve_vectors_and_keep_the_depth_gap() {
        let mut header = ShaderProgramHeader::parse(&[0; 80]).unwrap();
        header.version = 3;
        header.sph_type = 2;
        header.shader_type = 5;
        // Sparse targets and sparse components still reserve whole vectors.
        header.words[18] = (0x5 << 4) | (0x8 << 16) | (0xf << 28);
        header.words[19] = 2;
        let outputs = header.pixel_io().unwrap().output_registers();
        assert_eq!(outputs.colors[0], [None; 4]);
        assert_eq!(outputs.colors[1], [Some(0), None, Some(2), None]);
        assert_eq!(outputs.colors[4], [None, None, None, Some(7)]);
        assert_eq!(outputs.colors[7], [Some(8), Some(9), Some(10), Some(11)]);
        assert_eq!(outputs.depth, Some(13));
        assert_eq!(outputs.sample_mask, None);
        assert_eq!(outputs.registers().collect::<Vec<_>>(), [0, 2, 7, 8, 9, 10, 11, 13]);

        for colors in [0, 0xf, 0xff, 0xffff_ffff] {
            for flags in 0..4 {
                header.words[18] = colors;
                header.words[19] = flags;
                let outputs = header.pixel_io().unwrap().output_registers();
                let color_words = colors.count_ones() as u8;
                assert_eq!(outputs.sample_mask, (flags & 1 != 0).then_some(color_words));
                assert_eq!(outputs.depth, (flags & 2 != 0).then_some(color_words + 1));
            }
        }
    }

    #[test]
    fn stage_maps_distinguish_two_bit_inputs_and_unaligned_vtg_outputs() {
        let mut words = [0u32; 20];
        words[0] = 2 | (3 << 5) | (5 << 10);
        words[5] = 1 << 31;
        words[6] = 0xe4; // unused, constant, perspective, screen-linear
        words[13] = 3 << 30; // final generic component0x27c
        words[14] = 2 | (0x8001 << 16);
        words[17] = (1 << 14) | (0x1234 << 16);
        words[18] = 0xf0000003;
        words[19] = 0x80000003;
        let data: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let mut header = ShaderProgramHeader::parse(&data).unwrap();
        let io = header.pixel_io().unwrap();
        assert_eq!(io.inputs.iter().map(|v| v.byte_offset).collect::<Vec<_>>(), [
            0x84, 0x88, 0x8c, 0x27c, 0x280, 0x39c
        ]);
        assert_eq!(io.inputs[1].mode, PixelInputMode::Perspective);
        assert_eq!(io.color_masks, [3, 0, 0, 0, 0, 0, 0, 15]);
        assert_eq!(io.system_inputs_c, 0x8001);
        assert!(io.writes_depth && io.writes_sample_mask);
        assert_eq!(io.unknown_word17_high, 0x1234);
        assert_eq!(io.unknown_word19_bits, 0x80000000);
        assert_eq!(header.vtg_attribute_enabled(0x80, true), None);
        header.sph_type = 1;
        header.shader_type = 1;
        header.words[13] = (1 << 12) | (1 << 16);
        assert_eq!(header.vtg_attribute_enabled(0x70, true), Some(true));
        assert_eq!(header.vtg_attribute_enabled(0x74, true), Some(false));
        assert_eq!(header.vtg_attribute_enabled(0x80, true), Some(true));
        assert_eq!(header.vtg_attribute_enabled(0x84, true), Some(false));
        assert!(header.pixel_io().is_none());
    }
}
