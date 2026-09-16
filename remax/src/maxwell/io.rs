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
    /// Bits0..3 select RGBA for each render target; this does not assign GPRs.
    pub color_masks: [u8; 8],
    pub writes_sample_mask: bool,
    pub writes_depth: bool,
    /// Retain the remaining SPHv3 fields without importing later SM revisions.
    pub unknown_word17_high: u16,
    pub unknown_word19_bits: u32,
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

    /// Tests a SPHv3 VTG generic input/output word or a system value at0x60..0x7c.
    /// Other attribute ranges and stages return None instead of false defaults.
    pub fn vtg_attribute_enabled(&self, byte_offset: u16, output: bool) -> Option<bool> {
        if self.version != 3 || self.sph_type != 1 || !byte_offset.is_multiple_of(4) {
            return None;
        }
        let bit = if (0x60..0x80).contains(&byte_offset) {
            (if output { 400 } else { 160 }) + usize::from(byte_offset / 4)
        } else if (0x80..0x280).contains(&byte_offset) {
            (if output { 432 } else { 192 }) + usize::from((byte_offset - 0x80) / 4)
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
