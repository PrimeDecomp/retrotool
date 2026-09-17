//! Bound TEX uses contiguous coordinate and result registers, unlike TEXS.
//! Field and operand order agree with uam's emitTEX and Eden's texture_fetch.

use serde_derive::Serialize;

use super::{TextureDimension, TextureLod, TextureOperands};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tex {
    pub destination: u8,
    pub sources: [u8; 2],
    pub component_mask: u8,
    pub dimension_code: u8,
    pub dimension: Option<TextureDimension>,
    pub lod_code: u8,
    pub lod: Option<TextureLod>,
    /// Bound descriptor offset in 32-bit words, as for TEXS.
    pub texture_index: u16,
    pub nodep: bool,
    pub depth_compare: bool,
    pub offset: bool,
    pub lod_clamp: bool,
    pub non_divergent: bool,
    /// Residency-result predicate; seven suppresses this result.
    pub sparse_predicate: u8,
}

impl Tex {
    pub fn decode(word: u64) -> Option<Self> {
        if word >> 59 != 0b11000 {
            return None;
        }
        let dimension_code = ((word >> 28) & 7) as u8;
        let dimension = match dimension_code {
            0 => Some(TextureDimension::D1),
            2 => Some(TextureDimension::D2),
            3 => Some(TextureDimension::D2Array),
            4 => Some(TextureDimension::D3),
            6 => Some(TextureDimension::Cube),
            _ => None,
        };
        let lod_code = ((word >> 55) & 7) as u8;
        let lod = match lod_code {
            0 => Some(TextureLod::Implicit),
            1 => Some(TextureLod::Zero),
            3 => Some(TextureLod::Explicit),
            _ => None,
        };
        Some(Self {
            destination: word as u8,
            sources: [(word >> 8) as u8, (word >> 20) as u8],
            component_mask: ((word >> 31) & 15) as u8,
            dimension_code,
            dimension,
            lod_code,
            lod,
            texture_index: ((word >> 36) & 0x1fff) as u16,
            nodep: word & (1 << 49) != 0,
            depth_compare: word & (1 << 50) != 0,
            offset: word & (1 << 54) != 0,
            lod_clamp: word & (1 << 58) != 0,
            non_divergent: word & (1 << 35) != 0,
            sparse_predicate: ((word >> 51) & 7) as u8,
        })
    }

    /// Enabled channels pack consecutively, even for masks such as R/B that
    /// TEXS cannot express in one result group. Never wrap a register span.
    pub fn component_registers(&self) -> Option<[Option<u8>; 4]> {
        let mut result = [None; 4];
        let mut offset = 0;
        for (component, register) in result.iter_mut().enumerate() {
            if self.component_mask & (1 << component) != 0 {
                let index = self.destination.checked_add(offset)?;
                *register = (index != 255).then_some(index);
                offset += 1;
            }
        }
        Some(result)
    }

    /// Coordinate group: optional U16 layer followed by coordinates. Metadata
    /// group: optional LOD then comparison. Offset/clamp forms stay unresolved.
    pub fn operand_registers(&self) -> Option<TextureOperands> {
        if self.offset || self.lod_clamp {
            return None;
        }
        let dimension = self.dimension?;
        let coordinate_count = match dimension {
            TextureDimension::D1 => 1,
            TextureDimension::D2 | TextureDimension::D2Array => 2,
            TextureDimension::D3 | TextureDimension::Cube => 3,
        };
        let layer_count = u8::from(dimension == TextureDimension::D2Array);
        let mut coordinates = [None; 3];
        for index in 0..coordinate_count {
            coordinates[index as usize] = Some(self.sources[0].checked_add(layer_count + index)?);
        }
        let lod_count = u8::from(self.lod? == TextureLod::Explicit);
        let comparison =
            if self.depth_compare { Some(self.sources[1].checked_add(lod_count)?) } else { None };
        Some(TextureOperands {
            coordinates,
            layer: (layer_count != 0).then_some(self.sources[0]),
            lod: (lod_count != 0).then_some(self.sources[1]),
            comparison,
            texel_offset: None,
            sample_index: None,
            source_widths: [
                coordinate_count + layer_count,
                lod_count + u8::from(self.depth_compare),
            ],
        })
    }
}
