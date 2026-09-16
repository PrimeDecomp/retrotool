//! Maxwell raster-value loads. Encodings follow uam emitPIXLD and NAK OpPixLd
//! at 5a5afc2 and 822120d6 respectively. Only MY_INDEX occurs in the current corpus.

use serde_derive::Serialize;

use crate::maxwell::{alu::Fields, Instruction};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PixelValue {
    Count,
    CoverageMask,
    Covered,
    Offset,
    CentroidOffset,
    MyIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PixelLoad {
    pub destination: u8,
    pub address_register: u8,
    pub predicate_destination: u8,
    pub value_code: u8,
    /// Modes 6/7 have no assigned meaning. Address displacement fields, when
    /// present, remain in `unrecognized_bits` rather than being discarded.
    pub value: Option<PixelValue>,
    pub unrecognized_bits: u64,
}

impl PixelLoad {
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xfff8000000000000 != 0xefe8000000000000 {
            return None;
        }
        let mut f = Fields::new(word, 0xfff8000000000000);
        let destination = f.get(0, 8) as u8;
        let address_register = f.get(8, 8) as u8;
        let value_code = f.get(31, 3) as u8;
        let predicate_destination = f.get(45, 3) as u8;
        let value = [
            PixelValue::Count,
            PixelValue::CoverageMask,
            PixelValue::Covered,
            PixelValue::Offset,
            PixelValue::CentroidOffset,
            PixelValue::MyIndex,
        ]
        .get(value_code as usize)
        .copied();
        Some(Self {
            destination,
            address_register,
            predicate_destination,
            value_code,
            value,
            unrecognized_bits: word & !f.used,
        })
    }
}

impl Instruction {
    pub fn pixel_load(&self) -> Option<PixelLoad> { PixelLoad::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_sample_index_and_unknown_fields() {
        // Original: PIXLD.MY_INDEX R0, [RZ].
        let op = PixelLoad::decode(0xefe8e0028007ff00).unwrap();
        assert_eq!((op.destination, op.address_register, op.predicate_destination), (0, 255, 7));
        assert_eq!(op.value, Some(PixelValue::MyIndex));
        assert_eq!(op.unrecognized_bits, 0);
        // nvdisasm names mode 7 INVALID1. The decoder preserves the raw code.
        let reserved = PixelLoad::decode(0xefe8e0038007ff04).unwrap();
        assert_eq!((reserved.value_code, reserved.value), (7, None));
        let extra = PixelLoad::decode(0xefe8e0028017ff00).unwrap();
        assert_ne!(extra.unrecognized_bits, 0);
        assert!(PixelLoad::decode(0xef9000028007ff00).is_none()); // LDC family
    }
}
