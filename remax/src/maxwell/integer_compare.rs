//! Maxwell ISET/ISETP. uam emitISET/ISETP and NAK sm50 OpISetP at the
//! established 5a5afc2 / 822120d6 pins. Extended carry state is decoded separately.
use serde_derive::Serialize;

use crate::maxwell::{
    alu::{Fields, PredicateOperand, ValueSource},
    compare::PredicateLogic,
    Instruction,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum IntegerComparison {
    False,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    True,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct IntegerComparisonOperands {
    pub sources: [ValueSource; 2],
    pub comparison: IntegerComparison,
    pub signed: bool,
    pub accumulator: PredicateOperand,
    pub logic: Option<PredicateLogic>,
    pub logic_code: u8,
    pub extended: bool,
    pub unrecognized_bits: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum IntegerComparisonInstruction {
    Set {
        destination: u8,
        boolean_float: bool,
        write_condition_code: bool,
        operands: IntegerComparisonOperands,
    },
    SetPredicates {
        destination: u8,
        secondary_destination: u8,
        operands: IntegerComparisonOperands,
    },
}
impl IntegerComparisonInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let normal = word & 0xfff0000000000000;
        let compact = word & 0xfef0000000000000;
        let (base, form) = match word >> 60 {
            5 => (normal, 0),
            4 => (normal + 0x1000000000000000, 1),
            3 => (compact + 0x2500000000000000, 2),
            _ => return None,
        };
        let predicate = match base {
            0x5b50000000000000 => false,
            0x5b60000000000000 => true,
            _ => return None,
        };
        let mut f =
            Fields::new(word, if form == 2 { 0xfef0000000000000 } else { 0xfff0000000000000 });
        let destination = if predicate { f.get(3, 3) as u8 } else { f.get(0, 8) as u8 };
        let secondary_destination = if predicate { f.get(0, 3) as u8 } else { 7 };
        let a = f.register(8);
        let b = match form {
            0 => f.register(20),
            1 => f.constant(),
            _ => f.integer_immediate(),
        };
        let accumulator = PredicateOperand { register: f.get(39, 3) as u8, inverted: f.bit(42) };
        let extended = f.bit(43);
        let boolean_float = !predicate && f.bit(44);
        let logic_code = f.get(45, 2) as u8;
        let write_condition_code = !predicate && f.bit(47);
        let signed = f.bit(48);
        let comparison = [
            IntegerComparison::False,
            IntegerComparison::Less,
            IntegerComparison::Equal,
            IntegerComparison::LessEqual,
            IntegerComparison::Greater,
            IntegerComparison::NotEqual,
            IntegerComparison::GreaterEqual,
            IntegerComparison::True,
        ][f.get(49, 3) as usize];
        let logic = match logic_code {
            0 => Some(PredicateLogic::And),
            1 => Some(PredicateLogic::Or),
            2 => Some(PredicateLogic::Xor),
            _ => None,
        };
        let operands = IntegerComparisonOperands {
            sources: [a, b],
            comparison,
            signed,
            accumulator,
            logic,
            logic_code,
            extended,
            unrecognized_bits: word & !f.used,
        };
        Some(if predicate {
            Self::SetPredicates { destination, secondary_destination, operands }
        } else {
            Self::Set { destination, boolean_float, write_condition_code, operands }
        })
    }
}
impl Instruction {
    pub fn integer_comparison(&self) -> Option<IntegerComparisonInstruction> {
        IntegerComparisonInstruction::decode(self.word)
    }
}
