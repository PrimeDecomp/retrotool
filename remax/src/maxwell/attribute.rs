//! Maxwell attribute and interpolation operands. References: devkitPro/uam
//! GM107 emitter5a5afc2bae8b55409ab36ba45be63fcb73f68993; Mesa NAK SM50
//! encoder822120d6d5c0109f98c3fbf85ec716c5d1fa00b6; nvdisasm12.9.88 SM53.
//! Field decoding does not implement physical, patch or interpolation semantics.

use serde_derive::Serialize;

use crate::maxwell::{alu::PredicateOperand, Instruction};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AttributeAccess {
    /// First destination (ALD) or source (AST) register.255 is RZ.
    pub data_register: u8,
    /// Byte-address register;255 means the direct attribute address is used.
    pub address_register: u8,
    pub byte_offset: u16,
    /// 255 denotes the current vertex; other values need stage-specific handling.
    pub vertex_register: u8,
    pub components: u8,
    pub patch: bool,
    /// ALD selects output attributes. AST retains this bit, but nvdisasm prints
    /// both values identically; uam emits0, NAK emits1. Original MPR AST uses0.
    pub output_flag: bool,
    pub unrecognized_bits: u64,
}

impl AttributeAccess {
    pub fn physical(self) -> bool { !self.patch && self.address_register != 255 }

    /// The disassembler rejects a physical address with a nonzero immediate.
    pub fn physical_immediate_conflict(self) -> bool { self.physical() && self.byte_offset != 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AttributeAddress {
    pub destination: u8,
    pub predicate_destination: u8,
    pub address_register: u8,
    /// Signed 11-bit byte displacement in nvdisasm; ALD/AST use unsigned 10 bits.
    pub byte_offset: i16,
    pub components: u8,
    pub output: bool,
    pub unrecognized_bits: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Hash)]
pub enum InterpolationMode {
    Pass,
    Perspective,
    Constant,
    State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum InterpolationLocation {
    Default,
    Centroid,
    Offset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Interpolation {
    pub destination: u8,
    /// Trailing input predicate, distinct from the instruction execution guard.
    /// nvdisasm prints P0..!PT; its effect on interpolation remains unverified.
    pub predicate: PredicateOperand,
    pub indexed: bool,
    pub address_register: u8,
    /// Retained even in IDX mode, where nvdisasm omits this field.
    pub byte_offset: u16,
    pub multiplier_register: u8,
    /// Retained even if the selected location does not consume an offset.
    pub offset_register: u8,
    pub mode: InterpolationMode,
    pub location_code: u8,
    /// Code 3 prints INVALID in nvdisasm; never assume a default location.
    pub location: Option<InterpolationLocation>,
    pub saturate: bool,
    pub unrecognized_bits: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AttributeInstruction {
    Load(AttributeAccess),
    Store(AttributeAccess),
    Address(AttributeAddress),
    Interpolate(Interpolation),
}

impl AttributeInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let opcode = word & 0xfff8000000000000;
        if opcode == 0xefd8000000000000 || opcode == 0xeff0000000000000 {
            let known = 0xfff8000000000000u64
                | 0xfffff
                | (0x3ff << 20)
                | (3 << 31)
                | (0xff << 39)
                | (3 << 47);
            let access = AttributeAccess {
                data_register: word as u8,
                address_register: (word >> 8) as u8,
                byte_offset: ((word >> 20) & 0x3ff) as u16,
                patch: word & (1 << 31) != 0,
                output_flag: word & (1 << 32) != 0,
                vertex_register: (word >> 39) as u8,
                components: ((word >> 47) & 3) as u8 + 1,
                unrecognized_bits: word & !known,
            };
            Some(if opcode == 0xefd8000000000000 {
                Self::Load(access)
            } else {
                Self::Store(access)
            })
        } else if opcode == 0xefa0000000000000 {
            let known =
                0xfff8000000000000u64 | 0xfffff | (0x7ff << 20) | (1 << 32) | (7 << 44) | (3 << 47);
            Some(Self::Address(AttributeAddress {
                destination: word as u8,
                predicate_destination: ((word >> 44) & 7) as u8,
                address_register: (word >> 8) as u8,
                byte_offset: ((((word >> 20) & 0x7ff) as i16) << 5) >> 5,
                components: ((word >> 47) & 3) as u8 + 1,
                output: word & (1 << 32) != 0,
                unrecognized_bits: word & !known,
            }))
        } else if word >> 56 == 0xe0 {
            let known = 0xff00000000000000u64
                | 0xfffff
                | (0xff << 20)
                | (0x3ff << 28)
                | (1 << 38)
                | (0xff << 39)
                | (15 << 47)
                | (1 << 51)
                | (15 << 52);
            let location_code = ((word >> 52) & 3) as u8;
            Some(Self::Interpolate(Interpolation {
                destination: word as u8,
                predicate: PredicateOperand {
                    register: ((word >> 47) & 7) as u8,
                    inverted: word & (1 << 50) != 0,
                },
                indexed: word & (1 << 38) != 0,
                address_register: (word >> 8) as u8,
                byte_offset: ((word >> 28) & 0x3ff) as u16,
                multiplier_register: (word >> 20) as u8,
                offset_register: (word >> 39) as u8,
                mode: [
                    InterpolationMode::Pass,
                    InterpolationMode::Perspective,
                    InterpolationMode::Constant,
                    InterpolationMode::State,
                ][((word >> 54) & 3) as usize],
                location_code,
                location: [
                    InterpolationLocation::Default,
                    InterpolationLocation::Centroid,
                    InterpolationLocation::Offset,
                ]
                .get(location_code as usize)
                .copied(),
                saturate: word & (1 << 51) != 0,
                unrecognized_bits: word & !known,
            }))
        } else {
            None
        }
    }
}

impl Instruction {
    pub fn attribute(&self) -> Option<AttributeInstruction> {
        AttributeInstruction::decode(self.word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_load_output_alias_and_physical_conflict() {
        let AttributeInstruction::Load(v) =
            AttributeInstruction::decode(0xefd87f800807ff00).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (v.data_register, v.byte_offset, v.address_register, v.vertex_register, v.components),
            (0, 0x80, 255, 255, 1)
        );
        assert!(!v.physical());
        let AttributeInstruction::Store(v) =
            AttributeInstruction::decode(0xeff0ff810907ff00).unwrap()
        else {
            panic!()
        };
        assert!(v.output_flag);
        assert_eq!((v.byte_offset, v.components), (0x90, 2));
        let AttributeInstruction::Load(v) =
            AttributeInstruction::decode(0xefd87f8000470403).unwrap()
        else {
            panic!()
        };
        assert!(v.physical_immediate_conflict());
    }

    #[test]
    fn signed_address_conversion_and_invalid_interpolation_location() {
        let AttributeInstruction::Address(v) =
            AttributeInstruction::decode(0xefa000007fc70403).unwrap()
        else {
            panic!()
        };
        assert_eq!(v.byte_offset, -4);
        assert_eq!((v.destination, v.predicate_destination, v.address_register), (3, 0, 4));
        let AttributeInstruction::Interpolate(v) =
            AttributeInstruction::decode(0xe030034800570403).unwrap()
        else {
            panic!()
        };
        assert!(v.indexed);
        assert_eq!(v.location, None);
        assert_eq!(
            (v.destination, v.address_register, v.multiplier_register, v.offset_register),
            (3, 4, 5, 6)
        );
        assert_eq!(v.unrecognized_bits, 0);
    }

    #[test]
    fn interpolation_trailing_predicate_is_an_input_with_inversion() {
        // All 16 predicates, direct/indexed, four modes and three locations
        // independently checked with nvdisasm12.9.88 -b SM53 -hex.
        for indexed in [false, true] {
            for mode in 0..4u64 {
                for location in 0..3u64 {
                    for predicate in 0..16u64 {
                        let address = if indexed { 4u64 } else { 255 };
                        let word = 0xe000000000000000
                            | 3
                            | (address << 8)
                            | (7 << 16)
                            | (5 << 20)
                            | (0xa0 << 28)
                            | ((indexed as u64) << 38)
                            | (6 << 39)
                            | (predicate << 47)
                            | (location << 52)
                            | (mode << 54);
                        let Some(AttributeInstruction::Interpolate(ipa)) =
                            AttributeInstruction::decode(word)
                        else {
                            panic!("IPA not decoded: {word:016x}");
                        };
                        assert_eq!(ipa.predicate.register, (predicate & 7) as u8);
                        assert_eq!(ipa.predicate.inverted, predicate & 8 != 0);
                        assert_eq!(ipa.indexed, indexed);
                        assert_eq!(ipa.unrecognized_bits, 0);
                    }
                }
            }
        }
    }
}
