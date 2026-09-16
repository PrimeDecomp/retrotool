//! Runtime NVN texture views and Maxwell texture image control (TIC) records.
//!
//! These records are constructed by the driver, not stored in TXTR HEAD chunks.
//! MPR SDK build 2217a4a3c1f3c883278de72f8b008224ad252a24: view setters
//! +0x475cf0/+0x475f60, texture initialization +0x476bb0, descriptor writer
//! +0x493bf0 and pool registration +0x4773a0. Hardware field names follow NVIDIA
//! clb097tex.h and Mesa NIL descriptor.rs at 822120d6d5c0109f98c3fbf85ec716c5d1fa00b6.
//! Parsing preserves unknown bits; only the verified block-linear layout is
//! interpreted. Descriptor construction does not establish GPU sampling behavior
//! for an instruction whose dimension differs from the descriptor's dimension.

use anyhow::{ensure, Result};
use serde_derive::Serialize;

use super::txtr::{ETextureFormat, ETextureType};

impl ETextureType {
    /// MPR engine table at 0x7101d0d468, used by BuildFromParams/BuildTextureView.
    pub fn nvn_target(self) -> u8 { [0, 1, 2, 8, 3, 4, 5, 6, 9][self as usize] }
}

impl ETextureFormat {
    /// Exact MPR table at 0x7101d0d0f4 (8-byte entries, first word is NVNformat).
    /// None maps to zero. This is a mapping, not a capability check: the bundled
    /// SDK rejects RGB8's 0x19 and Rg16Snorm's anomalous 0x1f during initialization.
    pub fn nvn_format(self) -> u32 {
        const FORMATS: [u32; 85] = [
            0x01, 0x02, 0x03, 0x04, 0x06, 0x07, 0x08, 0x09, 0x05, 0x0b, 0x0c, 0x19, 0x25, 0x38,
            0x29, 0x2e, 0x32, 0x32, 0x35, 0x34, 0x42, 0x46, 0x43, 0x47, 0x44, 0x48, 0x49, 0x4a,
            0x4b, 0x4c, 0x3f, 0x0a, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x1f, 0x14, 0x15, 0x3d,
            0x3e, 0x17, 0x18, 0x16, 0x2a, 0x2b, 0x2c, 0x2d, 0x2f, 0x30, 0x00, 0x79, 0x7a, 0x7b,
            0x7c, 0x7d, 0x7e, 0x7f, 0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x50, 0x4f, 0x4d,
            0x4e,
        ];
        FORMATS[self as usize]
    }
}

/// Native ranges store a base and a count. A zero count means the remaining
/// levels/layers, not an empty view. Packed values retain setter truncation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NvnTextureViewRange {
    pub base: u16,
    pub count: u16,
}

/// The 16-byte NVNtextureView. Unset fields inherit from the backing texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NvnTextureView {
    pub raw: [u8; 16],
}

impl NvnTextureView {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == 16, "NVN texture view must contain exactly 16 bytes");
        Ok(Self { raw: bytes.try_into().unwrap() })
    }

    pub fn levels(self) -> Option<NvnTextureViewRange> {
        (self.raw[7] & 1 != 0).then_some(NvnTextureViewRange {
            base: u16::from(self.raw[4]),
            count: u16::from(self.raw[5]),
        })
    }

    pub fn layers(self) -> Option<NvnTextureViewRange> {
        (self.raw[7] & 2 != 0).then_some(NvnTextureViewRange {
            base: u16::from_le_bytes(self.raw[0..2].try_into().unwrap()),
            count: u16::from_le_bytes(self.raw[2..4].try_into().unwrap()),
        })
    }

    pub fn format(self) -> Option<u32> {
        (self.raw[7] & 4 != 0).then(|| u32::from_le_bytes(self.raw[8..12].try_into().unwrap()))
    }

    /// NVN swizzles: zero0, one1, R2, G3, B4, A5. These are not TIC source codes.
    pub fn swizzle(self) -> Option<[u8; 4]> {
        (self.raw[7] & 8 != 0).then(|| self.raw[12..16].try_into().unwrap())
    }

    /// Zero selects depth; one selects stencil. Other packed values are retained.
    pub fn depth_stencil_mode(self) -> Option<u8> {
        (self.raw[7] & 16 != 0).then_some(self.raw[7] >> 6)
    }

    /// Native NVN target, not ETextureType or the TIC texture_type field.
    pub fn target(self) -> Option<u8> { (self.raw[7] & 32 != 0).then_some(self.raw[6]) }
}

/// A 32-byte Maxwell TIC, as written to an NVN texture descriptor pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MaxwellTextureDescriptor {
    pub words: [u32; 8],
}

/// Verified fields of the NVB097 SELECT_BLOCKLINEAR layout (header version 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BlockLinearTextureDescriptor {
    pub component_sizes: u8,
    /// R/G/B/A: SNORM1, UNORM2, SINT3, UINT4, forced-half SNORM5/UNORM6, FLOAT7.
    /// Depth/stencil can mix types; do not infer the sampled type from R alone.
    pub component_types: [u8; 4],
    /// X/Y/Z/W: zero0, R2, G3, B4, A5, integer-one6, float-one7.
    pub sources: [u8; 4],
    /// Already includes the selected base layer's byte offset.
    pub gpu_address: u64,
    /// 1D0, 2D1, 3D2, cube3, 1D-array4, 2D-array5, buffer6, rectangle7, cube-array8.
    pub texture_type: u8,
    pub srgb: bool,
    pub width: u32,
    pub height: u32,
    /// Raw DEPTH_MINUS_ONE + 1. Array views use layer count, cube arrays use cube
    /// count, and a 2D view of array backing can retain its original backing depth.
    pub depth: u16,
    pub max_mip_level: u8,
    pub view_min_mip_level: u8,
    pub view_max_mip_level: u8,
    /// Hardware layout code, not a sample count (e.g. NVN 2x/4x/8x -> 5/2/4).
    pub multisample_mode: u8,
    pub normalized_coordinates: bool,
    pub block_height_log2: u8,
    pub block_depth_log2: u8,
    pub tile_width_log2: u8,
    /// NVN pool registration stores base-layer bits in fields NVIDIA reserves.
    /// Only meaningful with NVN provenance; do not add it to gpu_address again.
    pub nvn_registered_base_layer: u16,
}

impl MaxwellTextureDescriptor {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == 32, "Maxwell texture descriptor must contain exactly 32 bytes");
        Ok(Self {
            words: std::array::from_fn(|i| {
                u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
            }),
        })
    }

    pub fn header_version(self) -> u8 { ((self.words[2] >> 21) & 7) as u8 }

    pub fn block_linear(self) -> Result<BlockLinearTextureDescriptor> {
        ensure!(self.header_version() == 3, "Unsupported Maxwell texture header version");
        let w = self.words;
        Ok(BlockLinearTextureDescriptor {
            component_sizes: (w[0] & 127) as u8,
            component_types: std::array::from_fn(|i| ((w[0] >> (7 + 3 * i)) & 7) as u8),
            sources: std::array::from_fn(|i| ((w[0] >> (19 + 3 * i)) & 7) as u8),
            gpu_address: u64::from(w[1] & 0xfffffe00) | (u64::from(w[2] & 0xffff) << 32),
            texture_type: ((w[4] >> 23) & 15) as u8,
            srgb: w[4] & (1 << 22) != 0,
            width: (w[4] & 0xffff) + 1,
            height: (w[5] & 0xffff) + 1,
            depth: (((w[5] >> 16) & 0x3fff) + 1) as u16,
            max_mip_level: (w[3] >> 28) as u8,
            view_min_mip_level: (w[7] & 15) as u8,
            view_max_mip_level: ((w[7] >> 4) & 15) as u8,
            multisample_mode: ((w[7] >> 8) & 15) as u8,
            normalized_coordinates: w[5] & (1 << 31) != 0,
            block_height_log2: ((w[3] >> 3) & 7) as u8,
            block_depth_log2: ((w[3] >> 6) & 7) as u8,
            tile_width_log2: ((w[3] >> 10) & 7) as u8,
            nvn_registered_base_layer: (((w[4] >> 16) & 7)
                | (((w[2] >> 16) & 31) << 3)
                | (((w[2] >> 29) & 7) << 8)) as u16,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_sdk_depth_descriptor_keeps_mixed_types_and_sources() {
        // Native SDK output: 64x32 D24S8, five mips, GPU address 0x123400000.
        let desc = MaxwellTextureDescriptor {
            words: [
                0x701a4a29, 0x23400000, 0x00600001, 0x40060010, 0xe080003f, 0x8000001f, 0x01000000,
                0x40,
            ],
        };
        let bytes = desc.words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        assert_eq!(MaxwellTextureDescriptor::parse(&bytes).unwrap(), desc);
        let fields = desc.block_linear().unwrap();
        assert_eq!(fields.component_types, [4, 2, 2, 2]);
        assert_eq!(fields.sources, [3, 0, 0, 7]); // Depth comes from G; R is stencil.
        assert_eq!(fields.gpu_address, 0x123400000);
        assert_eq!((fields.width, fields.height, fields.depth), (64, 32, 1));
        assert_eq!(fields.max_mip_level, 4);
    }

    #[test]
    fn native_array_registration_metadata_does_not_change_address() {
        // SDK view at layer 256 of a 2048-layer array. Native stride 0x3000.
        let desc = MaxwellTextureDescriptor {
            words: [
                0x58d24908, 0x23700000, 0x20600001, 0x40060010, 0xe280003f, 0x8000001f, 0x01000000,
                0x40,
            ],
        };
        let fields = desc.block_linear().unwrap();
        assert_eq!(fields.texture_type, 5);
        assert_eq!(fields.depth, 1);
        assert_eq!(fields.nvn_registered_base_layer, 256);
        assert_eq!(fields.gpu_address, 0x123700000);
        assert_eq!(fields.component_types, [2; 4]);
        assert_eq!(fields.sources, [2, 3, 4, 5]);
        let view =
            NvnTextureView::parse(&[0, 1, 1, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(view.layers(), Some(NvnTextureViewRange { base: 256, count: 1 }));
        assert_eq!(view.levels(), None);
        assert_eq!(view.format(), None);
        assert_eq!(view.target(), None);
        assert_eq!(view.swizzle(), None);
        assert_eq!(view.depth_stencil_mode(), None);
    }

    #[test]
    fn view_flags_gate_values_and_unknown_descriptor_layouts_stay_raw() {
        let view =
            NvnTextureView::parse(&[0xff, 0xff, 0, 0, 2, 0, 4, 0x7d, 0x35, 0, 0, 0, 2, 3, 4, 5])
                .unwrap();
        assert_eq!(view.layers(), None); // Nonzero raw bytes, validity unset.
        assert_eq!(view.levels(), Some(NvnTextureViewRange { base: 2, count: 0 }));
        assert_eq!(view.target(), Some(4));
        assert_eq!(view.format(), Some(0x35));
        assert_eq!(view.swizzle(), Some([2, 3, 4, 5]));
        assert_eq!(view.depth_stencil_mode(), Some(1));
        for length in [0, 15, 17] {
            assert!(NvnTextureView::parse(&vec![0; length]).is_err());
        }
        for length in [0, 31, 33] {
            assert!(MaxwellTextureDescriptor::parse(&vec![0; length]).is_err());
        }
        for version in [0, 1, 2, 4, 5, 6, 7] {
            let mut desc = MaxwellTextureDescriptor { words: [0xffffffff; 8] };
            desc.words[2] = (desc.words[2] & !(7 << 21)) | (version << 21);
            assert_eq!(desc.header_version(), version as u8);
            assert!(desc.block_linear().is_err());
        }
    }
}
