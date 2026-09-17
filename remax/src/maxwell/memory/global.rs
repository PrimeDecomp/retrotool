//! Global-load encoding checked against uam emitLDG (5a5afc2) and nvdisasm
//! 12.9.88 SM53. Recognition does not supply an address space or memory model.

use serde_derive::Serialize;

use crate::maxwell::Instruction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum GlobalLoadType {
    U8,
    I8,
    U16,
    I16,
    B32,
    B64,
    B128,
    /// Native .U.128 spelling. Keep its extra modifier distinct from .128;
    /// its execution semantics have not been independently established.
    B128U,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum GlobalLoadCache {
    Default,
    Cg,
    Ci,
    Cv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GlobalLoad {
    pub destination: u8,
    /// RZ (255) encodes an absolute address with an unsigned displacement.
    pub address_register: u8,
    /// .E selects a register pair for a 64-bit address. Retained even for RZ.
    pub extended_address: bool,
    /// Signed 24-bit byte displacement for register addressing; unsigned
    /// 24-bit absolute byte address for RZ. Both fit this signed host field.
    pub displacement: i32,
    pub data_type: GlobalLoadType,
    pub cache: GlobalLoadCache,
    pub unrecognized_bits: u64,
}

impl GlobalLoad {
    pub fn decode(word: u64) -> Option<Self> {
        if word & 0xfff8000000000000 != 0xeed0000000000000 {
            return None;
        }
        let address_register = (word >> 8) as u8;
        let displacement = ((word >> 20) & 0xffffff) as i32;
        Some(Self {
            destination: word as u8,
            address_register,
            extended_address: word & (1 << 45) != 0,
            displacement: if address_register == 255 {
                displacement
            } else {
                (displacement << 8) >> 8
            },
            data_type: [
                GlobalLoadType::U8,
                GlobalLoadType::I8,
                GlobalLoadType::U16,
                GlobalLoadType::I16,
                GlobalLoadType::B32,
                GlobalLoadType::B64,
                GlobalLoadType::B128,
                GlobalLoadType::B128U,
            ][((word >> 48) & 7) as usize],
            cache: [
                GlobalLoadCache::Default,
                GlobalLoadCache::Cg,
                GlobalLoadCache::Ci,
                GlobalLoadCache::Cv,
            ][((word >> 46) & 3) as usize],
            unrecognized_bits: word & (1 << 44),
        })
    }
}

impl Instruction {
    pub fn global_load(&self) -> Option<GlobalLoad> { GlobalLoad::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_read_preserves_native_width_and_cache_modifiers() {
        let i = GlobalLoad::decode(0xeed7a00000070604).unwrap();
        assert_eq!(i.destination, 4);
        assert_eq!(i.address_register, 6);
        assert!(i.extended_address);
        assert_eq!(i.displacement, 0);
        assert_eq!(i.data_type, GlobalLoadType::B128U);
        assert_eq!(i.cache, GlobalLoadCache::Ci);
        assert_eq!(i.unrecognized_bits, 0);
        assert_eq!(GlobalLoad::decode(0xeed6200000070408).unwrap().data_type, GlobalLoadType::B128);
        assert_eq!(GlobalLoad::decode(0xeed4e00000070408).unwrap().cache, GlobalLoadCache::Cv);
        assert!(GlobalLoad::decode(0xef9500400407291a).is_none()); // LDC
    }

    #[test]
    fn disassembled_displacement_boundaries_and_absolute_address() {
        for (word, expected) in [
            (0xeed427fffff70408, 0x7fffff),
            (0xeed4280000070408, -0x800000),
            (0xeed42ffffff70408, -1),
            (0xeed00ffffff7ff08, 0xffffff),
            (0xeed02ffffff7ff08, 0xffffff),
        ] {
            let i = GlobalLoad::decode(word).unwrap();
            assert_eq!(i.displacement, expected);
            assert_eq!(i.unrecognized_bits, 0);
            assert_eq!(GlobalLoad::decode(word | (1 << 44)).unwrap().unrecognized_bits, 1 << 44);
        }
    }
}
