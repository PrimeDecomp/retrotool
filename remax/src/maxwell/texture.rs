//! Maxwell TEX/TEXS/TLDS operands, from devkitPro/uam's GM107 encoder (commit
//! 5a5afc2bae8b55409ab36ba45be63fcb73f68993). Mesa NAK sm50.rs provides the
//! corresponding ordinary texture instruction family; it does not emit TEXS/TLDS.
//! Other instruction families remain raw, including half-precision result forms.

use serde_derive::{Deserialize, Serialize};

use crate::maxwell::Instruction;

mod contiguous;
pub use contiguous::Tex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Hash)]
pub enum TextureDimension {
    D1,
    D2,
    D2Array,
    D3,
    Cube,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TextureLod {
    Implicit,
    Zero,
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TexsMode {
    pub dimension: TextureDimension,
    pub lod: TextureLod,
    pub depth_compare: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TldsMode {
    pub dimension: TextureDimension,
    pub lod: TextureLod,
    pub offset: bool,
    pub multisample: bool,
}

/// Register words consumed by a texture instruction. uam's
/// NVC0LoweringPass::handleTEX orders layer/coordinates/LOD/comparison/offset,
/// and RegAlloc::handleScalarTexGM107 condenses that list into two groups.
/// TLDS array mode uniquely keeps the layer alone in the first group.
///
/// TEXS coordinates, LOD and comparison are f32 bits; TLDS coordinates, LOD and
/// sample index are integer words. Array layers have already been converted to
/// unsigned16 by the shader. TLDS offsets pack signed X/Y nibbles at bits0/4.
/// R255 is RZ. Unused operands are None, independently of the encoded group base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TextureOperands {
    pub coordinates: [Option<u8>; 3],
    pub layer: Option<u8>,
    pub lod: Option<u8>,
    pub comparison: Option<u8>,
    pub texel_offset: Option<u8>,
    pub sample_index: Option<u8>,
    pub source_widths: [u8; 2],
}

// Select words from [source0, source0+1, source1, source1+1]. A used register
// group must not run beyond RZ; no wraparound is inferred for such encodings.
#[allow(clippy::too_many_arguments)] // The distinct ISA operand roles.
fn operand_layout(
    sources: [u8; 2],
    widths: [u8; 2],
    coordinates: &[usize],
    layer: Option<usize>,
    lod: Option<usize>,
    comparison: Option<usize>,
    texel_offset: Option<usize>,
    sample_index: Option<usize>,
) -> Option<TextureOperands> {
    let mut words = [None; 4];
    for group in 0..2 {
        for component in 0..widths[group] {
            words[group * 2 + usize::from(component)] =
                Some(sources[group].checked_add(component)?);
        }
    }
    Some(TextureOperands {
        coordinates: std::array::from_fn(|i| coordinates.get(i).and_then(|&slot| words[slot])),
        layer: layer.and_then(|slot| words[slot]),
        lod: lod.and_then(|slot| words[slot]),
        comparison: comparison.and_then(|slot| words[slot]),
        texel_offset: texel_offset.and_then(|slot| words[slot]),
        sample_index: sample_index.and_then(|slot| words[slot]),
        source_widths: widths,
    })
}

/// Integer texel fetch with split register groups. Unlike TEXS, coordinates
/// are integer texels and the fetch has no filtering or depth comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tlds {
    pub mode_code: u8,
    pub mode: Option<TldsMode>,
    pub mask_code: u8,
    pub component_mask: Option<u8>,
    /// Hardware order; nvdisasm prints destination1 first. 255 is RZ.
    pub destinations: [u8; 2],
    /// Group bases; operand_registers resolves coordinate/layer/LOD/offset/sample
    /// words for the supported modes, retaining the original bases here.
    pub sources: [u8; 2],
    /// Bound descriptor-table offset in 32-bit words, not an API texture slot.
    pub texture_index: u16,
    pub nodep: bool,
}

impl Tlds {
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xfe00000000000000 != 0xda00000000000000 {
            return None;
        }
        let mode_code = ((word >> 53) & 15) as u8;
        use TextureDimension::*;
        use TextureLod::*;
        // Only modes supported by uam getTLDSTarget and the independent
        // SM53 disassembly oracle are interpreted. Preserve all other codes.
        let mode = match mode_code {
            0 => Some((D1, Zero, false, false)),
            1 => Some((D1, Explicit, false, false)),
            2 => Some((D2, Zero, false, false)),
            4 => Some((D2, Zero, true, false)),
            5 => Some((D2, Explicit, false, false)),
            6 => Some((D2, Zero, false, true)),
            7 => Some((D3, Zero, false, false)),
            8 => Some((D2Array, Zero, false, false)),
            12 => Some((D2, Explicit, true, false)),
            _ => None,
        }
        .map(|(dimension, lod, offset, multisample)| TldsMode {
            dimension,
            lod,
            offset,
            multisample,
        });
        let destinations = [word as u8, (word >> 28) as u8];
        let mask_code = ((word >> 50) & 7) as u8;
        Some(Self {
            mode_code,
            mode,
            mask_code,
            component_mask: component_mask(destinations[1], mask_code),
            destinations,
            sources: [(word >> 8) as u8, (word >> 20) as u8],
            texture_index: ((word >> 36) & 0x1fff) as u16,
            nodep: word & (1 << 49) != 0,
        })
    }

    pub fn component_registers(&self) -> Option<[Option<u8>; 4]> {
        component_registers(self.component_mask?, self.destinations)
    }

    pub fn operand_registers(&self) -> Option<TextureOperands> {
        self.mode?;
        let (widths, coords, layer, lod, offset, sample): (_, &[usize], _, _, _, _) =
            match self.mode_code {
                0 => ([1, 0], &[0], None, None, None, None),
                1 => ([1, 1], &[0], None, Some(2), None, None),
                2 => ([1, 1], &[0, 2], None, None, None, None),
                4 => ([2, 1], &[0, 1], None, None, Some(2), None),
                5 => ([2, 1], &[0, 1], None, Some(2), None, None),
                6 => ([2, 1], &[0, 1], None, None, None, Some(2)),
                7 => ([2, 1], &[0, 1, 2], None, None, None, None),
                8 => ([1, 2], &[2, 3], Some(0), None, None, None),
                12 => ([2, 2], &[0, 1], None, Some(2), Some(3), None),
                _ => return None,
            };
        operand_layout(self.sources, widths, coords, layer, lod, None, offset, sample)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Texs {
    /// Raw encoding retained when mode/mask uses an unsupported combination.
    pub mode_code: u8,
    pub mode: Option<TexsMode>,
    pub mask_code: u8,
    /// RGBA bit mask; the encoding depends on whether destination1 is present.
    pub component_mask: Option<u8>,
    /// In hardware/IR order, NOT nvdisasm's printed order. Each group holds up
    /// to two 32-bit values. nvdisasm prints destination1 before destination0.
    /// 255 is RZ and does not receive a result.
    pub destinations: [u8; 2],
    /// Register group bases. Coordinate/layer/LOD packing depends on mode.
    pub sources: [u8; 2],
    /// Bound descriptor offset in 32-bit words, not an NVN API binding slot.
    pub texture_index: u16,
    /// Disassembler NODEP bit; uam calls this tex.liveOnly.
    pub nodep: bool,
}

impl Texs {
    /// Recognize the 32-bit-result TEXS family; preserve unrecognized mode/mask.
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xfe00000000000000 != 0xd800000000000000 {
            return None;
        }
        let mode_code = ((word >> 53) & 15) as u8;
        use TextureDimension::*;
        use TextureLod::*;
        let mode = match mode_code {
            0 => Some((D1, Zero, false)),
            1 => Some((D2, Implicit, false)),
            2 => Some((D2, Zero, false)),
            3 => Some((D2, Explicit, false)),
            4 => Some((D2, Implicit, true)),
            5 => Some((D2, Explicit, true)),
            6 => Some((D2, Zero, true)),
            7 => Some((D2Array, Implicit, false)),
            8 => Some((D2Array, Zero, false)),
            9 => Some((D2Array, Zero, true)),
            10 => Some((D3, Implicit, false)),
            11 => Some((D3, Zero, false)),
            12 => Some((Cube, Implicit, false)),
            13 => Some((Cube, Explicit, false)),
            _ => None,
        }
        .map(|(dimension, lod, depth_compare)| TexsMode { dimension, lod, depth_compare });
        let destinations = [word as u8, (word >> 28) as u8];
        let mask_code = ((word >> 50) & 7) as u8;
        let component_mask = component_mask(destinations[1], mask_code);
        Some(Self {
            mode_code,
            mode,
            mask_code,
            component_mask,
            destinations,
            sources: [(word >> 8) as u8, (word >> 20) as u8],
            texture_index: ((word >> 36) & 0x1fff) as u16,
            nodep: word & (1 << 49) != 0,
        })
    }

    /// Map logical R/G/B/A to result registers, skipping masked channels and RZ.
    /// E.g. mask RBA packs R/B into destination0/0+1 and A into destination1.
    pub fn component_registers(&self) -> Option<[Option<u8>; 4]> {
        component_registers(self.component_mask?, self.destinations)
    }

    pub fn operand_registers(&self) -> Option<TextureOperands> {
        self.mode?;
        let (widths, coords, layer, lod, comparison): (_, &[usize], _, _, _) = match self.mode_code
        {
            0 => ([1, 0], &[0], None, None, None),
            1 | 2 => ([1, 1], &[0, 2], None, None, None),
            3 => ([2, 1], &[0, 1], None, Some(2), None),
            4 | 6 => ([2, 1], &[0, 1], None, None, Some(2)),
            5 => ([2, 2], &[0, 1], None, Some(2), Some(3)),
            7 | 8 => ([2, 1], &[1, 2], Some(0), None, None),
            9 => ([2, 2], &[1, 2], Some(0), None, Some(3)),
            10..=12 => ([2, 1], &[0, 1, 2], None, None, None),
            13 => ([2, 2], &[0, 1, 2], None, Some(3), None),
            _ => return None,
        };
        operand_layout(self.sources, widths, coords, layer, lod, comparison, None, None)
    }
}

fn component_mask(destination1: u8, mask_code: u8) -> Option<u8> {
    if destination1 == 255 {
        Some([1, 2, 4, 8, 3, 9, 10, 12][mask_code as usize])
    } else {
        [7, 11, 13, 14, 15].get(mask_code as usize).copied()
    }
}

fn component_registers(mask: u8, destinations: [u8; 2]) -> Option<[Option<u8>; 4]> {
    let mut registers = [None; 4];
    let mut output = 0;
    for (component, register) in registers.iter_mut().enumerate() {
        if mask & (1 << component) != 0 {
            let base = destinations[output / 2];
            *register = base.checked_add((output % 2) as u8).filter(|&v| v != 255);
            output += 1;
        }
    }
    Some(registers)
}

impl Instruction {
    pub fn texture_sample_contiguous(&self) -> Option<Tex> { Tex::decode(self.word) }

    pub fn texture_sample(&self) -> Option<Texs> { Texs::decode(self.word) }

    pub fn texture_load(&self) -> Option<Tlds> { Tlds::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cluster_mask_fetch_has_separate_integer_xy_sources() {
        // Original SingleVine: TLDS.LZ.P RZ,R40,R22,R40,0xa,2D,R.
        let t = Tlds::decode(0xda4000aff2871628).unwrap();
        assert_eq!(t.destinations, [40, 255]);
        assert_eq!(t.sources, [22, 40]);
        assert_eq!(t.texture_index, 10);
        assert_eq!(t.component_registers(), Some([Some(40), None, None, None]));
        assert_eq!(t.mode.unwrap(), TldsMode {
            dimension: TextureDimension::D2,
            lod: TextureLod::Zero,
            offset: false,
            multisample: false,
        });
        assert!(Texs::decode(0xda4000aff2871628).is_none());
        assert!(Tlds::decode(0xd830012ff0770606).is_none());
    }
    #[test]
    fn rgba_destinations_are_reversed_in_disassembler_text() {
        // Source66 overlay: TEXS.T R6,R10,R6,R17,0xa,ARRAY_2D,RGBA.
        let t = Texs::decode(0xd8f000a06117060a).unwrap();
        assert_eq!(t.destinations, [10, 6]);
        assert_eq!(t.sources, [6, 17]);
        assert_eq!(t.texture_index, 10);
        assert_eq!(t.component_mask, Some(15));
        assert_eq!(t.component_registers(), Some([Some(10), Some(11), Some(6), Some(7)]));
        assert_eq!(t.mode.unwrap().dimension, TextureDimension::D2Array);
        // Two-result normal sample, not RGBA despite the same raw mask code4.
        let t = Texs::decode(0xd830012ff0770606).unwrap();
        assert_eq!(t.component_mask, Some(3));
        assert_eq!(t.component_registers(), Some([Some(6), Some(7), None, None]));
    }
    #[test]
    fn compiler_array_layouts_and_comparison_lod_have_distinct_groups() {
        // Pinned uam: texture(t,c.xyz), source0=[u16(c.z),c.x], source1=c.y.
        let a = Texs::decode(0xd8f01a4020270000).unwrap().operand_registers().unwrap();
        assert_eq!(a.coordinates, [Some(1), Some(2), None]);
        assert_eq!(a.layer, Some(0));
        assert_eq!(a.source_widths, [2, 1]);
        // texelFetch(t,i.xyz,0): source0=layer, source1=[x,y].
        let a = Tlds::decode(0xdb101a4020070200).unwrap().operand_registers().unwrap();
        assert_eq!(a.coordinates, [Some(0), Some(1), None]);
        assert_eq!(a.layer, Some(2));
        assert_eq!(a.source_widths, [1, 2]);
        // textureLod(shadow,c.xyz,e.x): source0=[x,y], source1=[LOD,reference].
        let a = Texs::decode(0xd8a01a4ff0070203).unwrap().operand_registers().unwrap();
        assert_eq!(a.coordinates, [Some(2), Some(3), None]);
        assert_eq!(a.lod, Some(0));
        assert_eq!(a.comparison, Some(1));
    }
    #[test]
    fn input_groups_preserve_rz_and_reject_register_wraparound() {
        let word = 0xd8f01a4020270000u64;
        let a = Texs::decode((word & !0xff00) | 254 << 8).unwrap();
        assert_eq!(a.operand_registers().unwrap().coordinates[0], Some(255));
        assert!(Texs::decode(word | 255 << 8).unwrap().operand_registers().is_none());
        // A one-word RZ group is valid; an unused second base stays unused.
        let a = Texs::decode(0xd80000000007ffff).unwrap().operand_registers().unwrap();
        assert_eq!(a.coordinates, [Some(255), None, None]);
        assert_eq!(a.source_widths, [1, 0]);
    }
    #[test]
    fn masks_pack_selected_channels_and_unknown_encodings_stay_unknown() {
        let word = 0xd800000000070000u64 | (4 << 28) | (2 << 50);
        let t = Texs::decode(word).unwrap(); // Three-result mask RBA.
        assert_eq!(t.component_registers(), Some([Some(0), None, Some(1), Some(4)]));
        assert!(Texs::decode(word | (7 << 50)).unwrap().component_mask.is_none());
        assert!(Texs::decode(word | (15 << 53)).unwrap().mode.is_none());
        assert!(Texs::decode(0x50b0000000070f00).is_none());
        assert!(Texs::decode(0xd000000000070000).is_none());
    }
}
