//! Maxwell integer add, logic and shifts. Operand encodings follow uam's
//! nv50_ir_emit_gm107.cpp (5a5afc2) and NAK sm50.rs (822120d6).
//! Carry, saturation and predicate flags are retained even when not executable.

use serde_derive::Serialize;

use crate::maxwell::{
    alu::{Fields, ValueSource},
    Instruction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum LogicOperation {
    And,
    Or,
    Xor,
    PassB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "opcode")]
pub enum IntegerOperation {
    BitFieldExtract {
        signed: bool,
        reverse: bool,
    },
    FindLeading {
        signed: bool,
        shift_amount: bool,
        invert: bool,
    },
    Add {
        /// Both negate bits encode IADD.PO, not two independent negations.
        negate: [bool; 2],
        saturate: bool,
    },
    Logic {
        operation: LogicOperation,
        invert: [bool; 2],
        /// 0/1 reserved predicate modes; 2=Z, 3=NZ. Long form has no predicate.
        predicate_mode: u8,
        predicate_destination: u8,
    },
    Shift {
        right: bool,
        signed: bool,
        wrap: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct IntegerInstruction {
    pub destination: u8,
    pub sources: [ValueSource; 2],
    pub operation: IntegerOperation,
    pub extended: bool,
    pub write_condition_code: bool,
    pub unrecognized_bits: u64,
}

impl IntegerInstruction {
    fn decode_bits(word: u64) -> Option<Self> {
        let normal = word & 0xfff8000000000000;
        let compact = word & 0xfef8000000000000;
        let (base, form) = match word >> 60 {
            5 => (normal, 0),
            4 => (normal + 0x1000000000000000, 1),
            3 => (compact + 0x2400000000000000, 2),
            _ => return None,
        };
        let extract = match base {
            0x5c00000000000000 => true,
            0x5c30000000000000 => false,
            _ => return None,
        };
        let mut f =
            Fields::new(word, if form == 2 { 0xfef8000000000000 } else { 0xfff8000000000000 });
        let destination = f.get(0, 8) as u8;
        let a = if extract { f.register(8) } else { ValueSource::Register(255) };
        let b = match form {
            0 => f.register(20),
            1 => f.constant(),
            _ => f.integer_immediate(),
        };
        let operation = if extract {
            IntegerOperation::BitFieldExtract { signed: f.bit(48), reverse: f.bit(40) }
        } else {
            IntegerOperation::FindLeading {
                signed: f.bit(48),
                shift_amount: f.bit(41),
                invert: f.bit(40),
            }
        };
        let write_condition_code = f.bit(47);
        Some(Self {
            destination,
            sources: [a, b],
            operation,
            extended: false,
            write_condition_code,
            unrecognized_bits: word & !f.used,
        })
    }

    pub fn decode(word: u64) -> Option<Self> {
        if let Some(i) = Self::decode_bits(word) {
            return Some(i);
        }
        let normal = word & 0xfff8000000000000;
        let compact = word & 0xfef8000000000000;
        let long = word & 0xfc00000000000000;
        // IADD32I bit56 is negate, but bit57 distinguishes FMUL32I.
        let long_add = word & 0xfe00000000000000 == 0x1c00000000000000;
        let (family, form) = if long_add || long == 0x0400000000000000 {
            (if long_add { 0 } else { 1 }, 3)
        } else {
            let (base, form) = if word >> 60 == 5 {
                (normal, 0)
            } else if word >> 60 == 4 {
                (normal + 0x1000000000000000, 1)
            } else if word >> 60 == 3 {
                (compact + 0x2400000000000000, 2)
            } else {
                return None;
            };
            (
                match base {
                    0x5c10000000000000 => 0,
                    0x5c40000000000000 => 1,
                    0x5c48000000000000 => 2,
                    0x5c28000000000000 => 3,
                    _ => return None,
                },
                form,
            )
        };
        let mut f = Fields::new(word, match form {
            3 => {
                if long_add {
                    0xfe00000000000000
                } else {
                    0xfc00000000000000
                }
            }
            2 => 0xfef8000000000000,
            _ => 0xfff8000000000000,
        });
        let destination = f.get(0, 8) as u8;
        let a = f.register(8);
        let b = match form {
            0 => f.register(20),
            1 => f.constant(),
            2 => f.integer_immediate(),
            _ => f.immediate32(),
        };
        let write_condition_code = f.bit(if form == 3 { 52 } else { 47 });
        let extended = f.bit(if form == 3 {
            if family == 0 {
                53
            } else {
                57
            }
        } else if family == 3 {
            44
        } else {
            43
        });
        let operation = match family {
            0 => IntegerOperation::Add {
                negate: [f.bit(if form == 3 { 56 } else { 49 }), form != 3 && f.bit(48)],
                saturate: f.bit(if form == 3 { 54 } else { 50 }),
            },
            1 => IntegerOperation::Logic {
                operation: [
                    LogicOperation::And,
                    LogicOperation::Or,
                    LogicOperation::Xor,
                    LogicOperation::PassB,
                ][f.get(if form == 3 { 53 } else { 41 }, 2) as usize],
                invert: [
                    f.bit(if form == 3 { 55 } else { 39 }),
                    f.bit(if form == 3 { 56 } else { 40 }),
                ],
                predicate_mode: if form == 3 { 0 } else { f.get(44, 2) as u8 },
                predicate_destination: if form == 3 { 7 } else { f.get(48, 3) as u8 },
            },
            _ => IntegerOperation::Shift {
                right: family == 3,
                signed: family == 3 && f.bit(48),
                wrap: f.bit(39),
            },
        };
        Some(Self {
            destination,
            sources: [a, b],
            operation,
            extended,
            write_condition_code,
            unrecognized_bits: word & !f.used,
        })
    }
}

impl Instruction {
    pub fn integer(&self) -> Option<IntegerInstruction> { IntegerInstruction::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_add_does_not_claim_float_multiply_immediate() {
        assert!(IntegerInstruction::decode(0x1e2358637bd70000).is_none());
        let i = IntegerInstruction::decode(0x1c0fffffffd70808).unwrap();
        assert_eq!(i.sources, [ValueSource::Register(8), ValueSource::Immediate(0xfffffffd)]);
        assert_eq!(i.unrecognized_bits, 0);
    }

    #[test]
    fn original_logic_predicate_and_carry_flags_remain_visible() {
        let i = IntegerInstruction::decode(0x38403000001700ff).unwrap();
        assert!(matches!(i.operation, IntegerOperation::Logic {
            predicate_destination: 0,
            predicate_mode: 3,
            ..
        }));
        assert_eq!(i.destination, 255);
        assert_eq!(i.unrecognized_bits, 0);
        let add = IntegerInstruction::decode(0x4c10800004470000).unwrap();
        assert!(add.write_condition_code);
        let add = IntegerInstruction::decode(0x4c1008000457ff01).unwrap();
        assert!(add.extended);
    }
}
