//! Thread-private load/store encodings. Widths and cache spellings checked with
//! nvdisasm 12.9.88 SM53 and uam's GM107 emitter. Cache code 1 is LDL.LU but
//! STL.CG; sharing the global-load cache enum would lose that distinction.

use serde_derive::Serialize;

use crate::maxwell::Instruction;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LocalMemoryType {
    U8,
    I8,
    U16,
    I16,
    B32,
    B64,
    B128,
}

impl LocalMemoryType {
    pub fn byte_len(self) -> u8 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::B32 => 4,
            Self::B64 => 8,
            Self::B128 => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LocalLoadCache {
    Default,
    Lu,
    Ci,
    Cv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LocalStoreCache {
    Default,
    Cg,
    Cs,
    Wt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LocalMemoryKind {
    Load(LocalLoadCache),
    Store(LocalStoreCache),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LocalMemoryInstruction {
    pub kind: LocalMemoryKind,
    /// Destination of LDL; first source register of STL.
    pub register: u8,
    pub address_register: u8,
    /// Signed 24-bit relative displacement, or unsigned absolute offset with RZ.
    pub displacement: i32,
    pub type_code: u8,
    /// Type code 7 is not an established local-memory width.
    pub data_type: Option<LocalMemoryType>,
    pub unrecognized_bits: u64,
}

impl LocalMemoryInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let cache = ((word >> 44) & 3) as usize;
        let kind = match word & 0xfff8000000000000 {
            0xef40000000000000 => LocalMemoryKind::Load(
                [
                    LocalLoadCache::Default,
                    LocalLoadCache::Lu,
                    LocalLoadCache::Ci,
                    LocalLoadCache::Cv,
                ][cache],
            ),
            0xef50000000000000 => LocalMemoryKind::Store(
                [
                    LocalStoreCache::Default,
                    LocalStoreCache::Cg,
                    LocalStoreCache::Cs,
                    LocalStoreCache::Wt,
                ][cache],
            ),
            _ => return None,
        };
        let address_register = (word >> 8) as u8;
        let displacement = ((word >> 20) & 0xffffff) as i32;
        let type_code = ((word >> 48) & 7) as u8;
        Some(Self {
            kind,
            register: word as u8,
            address_register,
            displacement: if address_register == 255 {
                displacement
            } else {
                (displacement << 8) >> 8
            },
            type_code,
            data_type: [
                LocalMemoryType::U8,
                LocalMemoryType::I8,
                LocalMemoryType::U16,
                LocalMemoryType::I16,
                LocalMemoryType::B32,
                LocalMemoryType::B64,
                LocalMemoryType::B128,
            ]
            .get(type_code as usize)
            .copied(),
            unrecognized_bits: word & (3 << 46),
        })
    }
}

impl Instruction {
    pub fn local_memory(&self) -> Option<LocalMemoryInstruction> {
        LocalMemoryInstruction::decode(self.word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_spills_preserve_direction_width_and_cache() {
        let store = LocalMemoryInstruction::decode(0xef5400000047ff09).unwrap();
        assert_eq!(store.kind, LocalMemoryKind::Store(LocalStoreCache::Default));
        assert_eq!((store.register, store.address_register, store.displacement), (9, 255, 4));
        assert_eq!(store.data_type, Some(LocalMemoryType::B32));
        let load = LocalMemoryInstruction::decode(0xef4410000047ff14).unwrap();
        assert_eq!(load.kind, LocalMemoryKind::Load(LocalLoadCache::Lu));
        assert_eq!((load.register, load.displacement, load.unrecognized_bits), (20, 4, 0));
        assert!(LocalMemoryInstruction::decode(0xef4800000007ff00).is_none()); // LDS
        assert_eq!(LocalMemoryInstruction::decode(0xef4700000007ff00).unwrap().data_type, None);
    }

    #[test]
    fn relative_and_absolute_displacement_boundaries_match_sm53_disassembly() {
        for (word, offset) in [
            (0xef4407fffff70408, 0x7fffff),
            (0xef44080000070408, -0x800000),
            (0xef440ffffff70408, -1),
            (0xef440ffffff7ff08, 0xffffff),
        ] {
            assert_eq!(LocalMemoryInstruction::decode(word).unwrap().displacement, offset);
        }
    }
}
