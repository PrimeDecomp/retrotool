//! Maxwell indexed constant loads. Verified against devkitPro/uam emitLDC
//! (5a5afc2bae8b55409ab36ba45be63fcb73f68993), Mesa NAK OpLdc
//! (822120d6d5c0109f98c3fbf85ec716c5d1fa00b6), and nvdisasm SM53.

use serde_derive::Serialize;

use crate::maxwell::Instruction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LdcType {
    U8,
    I8,
    U16,
    I16,
    B32,
    B64,
}

impl LdcType {
    pub fn byte_len(self) -> u8 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::B32 => 4,
            Self::B64 => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LdcMode {
    Indexed,
    IndexedLinear,
    IndexedSegmented,
    IndexedSegmentedLinear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConstantLoad {
    pub destination: u8,
    /// 255 is RZ. This is a byte-address register, not a constant-buffer slot.
    pub address_register: u8,
    pub buffer: u8,
    /// Signed16-bit byte displacement, confirmed at both sign boundaries.
    pub displacement: i16,
    pub mode: LdcMode,
    pub type_code: u8,
    /// Codes6/7 remain unsupported. The upstream shared memory-type encoder
    /// labels6 B128, but nvdisasm SM53 reports INVALIDSIZE6 for LDC.
    pub data_type: Option<LdcType>,
    pub unrecognized_bits: u64,
}

impl ConstantLoad {
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xfff8000000000000 != 0xef90000000000000 {
            return None;
        }
        let type_code = ((word >> 48) & 7) as u8;
        Some(Self {
            destination: word as u8,
            address_register: (word >> 8) as u8,
            buffer: ((word >> 36) & 31) as u8,
            displacement: (word >> 20) as u16 as i16,
            mode: [
                LdcMode::Indexed,
                LdcMode::IndexedLinear,
                LdcMode::IndexedSegmented,
                LdcMode::IndexedSegmentedLinear,
            ][((word >> 44) & 3) as usize],
            type_code,
            data_type: [
                LdcType::U8,
                LdcType::I8,
                LdcType::U16,
                LdcType::I16,
                LdcType::B32,
                LdcType::B64,
            ]
            .get(type_code as usize)
            .copied(),
            unrecognized_bits: word
                & !(0xfff8000000000000
                    | 0xf0000
                    | 0xffff
                    | (0x1fffff << 20)
                    | (3 << 44)
                    | (7 << 48)),
        })
    }
}

impl Instruction {
    pub fn constant_load(&self) -> Option<ConstantLoad> { ConstantLoad::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_array_load_and_signed_driver_offset() {
        let light = ConstantLoad::decode(0xef9500400407291a).unwrap();
        assert_eq!(
            (light.destination, light.address_register, light.buffer, light.displacement),
            (26, 41, 4, 0x40)
        );
        assert_eq!(light.data_type, Some(LdcType::B64));
        assert_eq!(light.mode, LdcMode::Indexed);
        // Independently disassembled boundary: LDC.U8.ISL R8,c[3][R4+-1].
        let negative = ConstantLoad::decode(0xef90303ffff70408).unwrap();
        assert_eq!(negative.displacement, -1);
        assert_eq!(negative.mode, LdcMode::IndexedSegmentedLinear);
        assert_eq!(negative.data_type.unwrap().byte_len(), 1);
        assert_eq!(ConstantLoad::decode(0xef96003000070408).unwrap().data_type, None);
        assert!(ConstantLoad::decode(0xd830012ff0770606).is_none()); // TEXS
    }
}
