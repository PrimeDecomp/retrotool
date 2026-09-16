//! Maxwell float comparison operands. uam GM107 emitFSET/emitFSETP and Mesa NAK
//! SM50 OpFSet/OpFSetP, at the pins in maxwell::alu. In NAK's OpFSet CB
//! branch, the negative-source flag is written at6; uam and SM53 nvdisasm put it
//! at53. Keep that distinction from FSETP, which does use bit6.

use serde_derive::Serialize;

use crate::maxwell::{
    alu::{FloatOperand, PredicateOperand, ValueSource},
    Instruction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FloatComparison {
    False,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    IsNumber,
    IsNan,
    LessUnordered,
    EqualUnordered,
    LessEqualUnordered,
    GreaterUnordered,
    NotEqualUnordered,
    GreaterEqualUnordered,
    True,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PredicateLogic {
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FloatComparisonOperands {
    pub sources: [FloatOperand; 2],
    pub comparison: FloatComparison,
    pub accumulator: PredicateOperand,
    pub logic_code: u8,
    pub logic: Option<PredicateLogic>,
    pub flush_to_zero: bool,
    pub unrecognized_bits: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FloatComparisonInstruction {
    Set {
        destination: u8,
        boolean_float: bool,
        write_condition_code: bool,
        operands: FloatComparisonOperands,
    },
    SetPredicates {
        destination: u8,
        secondary_destination: u8,
        operands: FloatComparisonOperands,
    },
}

impl FloatComparisonInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let normal = word & 0xfff0000000000000;
        let compact = word & 0xfef0000000000000;
        let predicate = matches!(normal, 0x5bb0000000000000 | 0x4bb0000000000000)
            || compact == 0x36b0000000000000;
        let kind = if predicate {
            if normal == 0x5bb0000000000000 {
                0
            } else if normal == 0x4bb0000000000000 {
                1
            } else {
                2
            }
        } else if word >> 56 == 0x58 {
            0
        } else if word >> 56 == 0x48 {
            1
        } else if word & 0xfe00000000000000 == 0x3000000000000000 {
            2
        } else {
            return None;
        };
        let opcode_mask = if predicate {
            if kind == 2 {
                0xfef0000000000000
            } else {
                0xfff0000000000000
            }
        } else if kind == 2 {
            0xfe00000000000000
        } else {
            0xff00000000000000
        };
        let operand_mask = if kind == 0 {
            0xff << 20
        } else {
            (0x7ffff << 20) | if kind == 2 { 1 << 56 } else { 0 }
        };
        let modifier_mask = if predicate { 0xfff << 39 } else { 0x1ffff << 39 };
        let known = opcode_mask | 0xfffff | operand_mask | modifier_mask | (15 << 48);
        let b = match kind {
            0 => ValueSource::Register((word >> 20) as u8),
            1 => ValueSource::Constant {
                buffer: ((word >> 34) & 31) as u8,
                byte_offset: (((word >> 20) & 0x3fff) * 4) as u16 as i16,
            },
            _ => ValueSource::Immediate(
                ((((word >> 20) & 0x7ffff) << 12) | (((word >> 56) & 1) << 31)) as u32,
            ),
        };
        let bit = |b: u8| word & (1 << b) != 0;
        let logic_code = ((word >> 45) & 3) as u8;
        let operands = FloatComparisonOperands {
            sources: [
                FloatOperand {
                    source: ValueSource::Register((word >> 8) as u8),
                    absolute: bit(if predicate { 7 } else { 54 }),
                    negate: bit(43),
                },
                FloatOperand {
                    source: b,
                    absolute: bit(44),
                    negate: bit(if predicate { 6 } else { 53 }),
                },
            ],
            comparison: [
                FloatComparison::False,
                FloatComparison::Less,
                FloatComparison::Equal,
                FloatComparison::LessEqual,
                FloatComparison::Greater,
                FloatComparison::NotEqual,
                FloatComparison::GreaterEqual,
                FloatComparison::IsNumber,
                FloatComparison::IsNan,
                FloatComparison::LessUnordered,
                FloatComparison::EqualUnordered,
                FloatComparison::LessEqualUnordered,
                FloatComparison::GreaterUnordered,
                FloatComparison::NotEqualUnordered,
                FloatComparison::GreaterEqualUnordered,
                FloatComparison::True,
            ][((word >> 48) & 15) as usize],
            accumulator: PredicateOperand { register: ((word >> 39) & 7) as u8, inverted: bit(42) },
            logic_code,
            logic: [PredicateLogic::And, PredicateLogic::Or, PredicateLogic::Xor]
                .get(logic_code as usize)
                .copied(),
            flush_to_zero: bit(if predicate { 47 } else { 55 }),
            unrecognized_bits: word & !known,
        };
        Some(if predicate {
            Self::SetPredicates {
                destination: ((word >> 3) & 7) as u8,
                secondary_destination: (word & 7) as u8,
                operands,
            }
        } else {
            Self::Set {
                destination: word as u8,
                boolean_float: bit(52),
                write_condition_code: bit(47),
                operands,
            }
        })
    }
}

impl Instruction {
    pub fn float_comparison(&self) -> Option<FloatComparisonInstruction> {
        FloatComparisonInstruction::decode(self.word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_negation_does_not_change_fset_destination() {
        // Original: FSET.BF.LT.FTZ.AND R26,R25,-c[4][0x80],PT.
        let FloatComparisonInstruction::Set {
            destination,
            boolean_float,
            write_condition_code,
            operands,
        } = FloatComparisonInstruction::decode(0x48b103900207191a).unwrap()
        else {
            panic!()
        };
        assert_eq!(destination, 26);
        assert!(boolean_float && operands.flush_to_zero && !write_condition_code);
        assert_eq!(operands.sources[1], FloatOperand {
            source: ValueSource::Constant { buffer: 4, byte_offset: 0x80 },
            absolute: false,
            negate: true
        });
        assert_eq!(operands.comparison, FloatComparison::Less);
        assert_eq!(operands.unrecognized_bits, 0);
        // For FSETP, source1 negation lives at6; destination predicates are3..5/0..2.
        let FloatComparisonInstruction::SetPredicates {
            destination,
            secondary_destination,
            operands,
        } = FloatComparisonInstruction::decode(0x5bb1038000170041).unwrap()
        else {
            panic!()
        };
        assert_eq!((destination, secondary_destination), (0, 1));
        assert!(operands.sources[1].negate);
    }
}
