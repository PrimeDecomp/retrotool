//! Maxwell F2F, F2I and I2F conversions. Types, source selection, rounding and
//! unknown bits are preserved. References: uam's conversion encoders (5a5afc2),
//! Mesa NAK sm50.rs (822120d6), and nvdisasm 12.9.88 SM53.

use serde_derive::Serialize;

use crate::maxwell::{
    alu::{Fields, FloatRounding, ValueSource},
    Instruction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ConversionOperation {
    FloatToFloat,
    FloatToInteger,
    IntegerToFloat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConversionInstruction {
    pub operation: ConversionOperation,
    pub destination: u8,
    pub source: ValueSource,
    /// Encoded sizes are 8 << code; unsupported float sizes remain visible.
    pub destination_bits: u8,
    pub source_bits: u8,
    /// Signedness of the integer type, whether source or destination.
    pub signed: bool,
    pub absolute: bool,
    pub negate: bool,
    pub rounding: FloatRounding,
    /// F2F rounds to an integral floating value when bit42 is set. This does
    /// not convert the destination to an integer bit representation.
    pub integer_rounding: bool,
    pub saturate: bool,
    pub flush_to_zero: bool,
    /// Source byte/half selection (raw code); F2I has one high-half bit.
    pub selector: u8,
    pub write_condition_code: bool,
    pub unrecognized_bits: u64,
}

impl ConversionInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let normal = word & 0xfff8000000000000;
        let compact = word & 0xfef8000000000000;
        let (base, form) = if word >> 60 == 5 {
            (normal, 0)
        } else if word >> 60 == 4 {
            (normal + 0x1000000000000000, 1)
        } else if word >> 60 == 3 {
            (compact + 0x2400000000000000, 2)
        } else {
            return None;
        };
        let operation = match base {
            0x5ca8000000000000 => ConversionOperation::FloatToFloat,
            0x5cb0000000000000 => ConversionOperation::FloatToInteger,
            0x5cb8000000000000 => ConversionOperation::IntegerToFloat,
            _ => return None,
        };
        let f2i = operation == ConversionOperation::FloatToInteger;
        let f2f = operation == ConversionOperation::FloatToFloat;
        let mut f =
            Fields::new(word, if form == 2 { 0xfef8000000000000 } else { 0xfff8000000000000 });
        let destination = f.get(0, 8) as u8;
        let destination_bits = 8 << f.get(8, 2);
        let source_bits = 8 << f.get(10, 2);
        let source = match form {
            0 => f.register(20),
            1 => f.constant(),
            _ => {
                if f2i || f2f {
                    f.float_immediate()
                } else {
                    f.integer_immediate()
                }
            }
        };
        let signed = !f2f && f.bit(if f2i { 12 } else { 13 });
        let rounding = f.rounding(39);
        let selector = f.get(41, if f2i || f2f { 1 } else { 2 }) as u8;
        let integer_rounding = f2f && f.bit(42);
        let saturate = f2f && f.bit(50);
        let flush_to_zero = (f2i || f2f) && f.bit(44);
        let negate = f.bit(45);
        let write_condition_code = f.bit(47);
        let absolute = f.bit(49);
        Some(Self {
            operation,
            destination,
            source,
            destination_bits,
            source_bits,
            signed,
            rounding,
            integer_rounding,
            saturate,
            selector,
            flush_to_zero,
            negate,
            absolute,
            write_condition_code,
            unrecognized_bits: word & !f.used,
        })
    }
}

impl Instruction {
    pub fn conversion(&self) -> Option<ConversionInstruction> {
        ConversionInstruction::decode(self.word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disassembled_f2f_forms_distinguish_integral_rounding_and_saturation() {
        let c = ConversionInstruction::decode(0x4ca8149002570a01).unwrap();
        assert_eq!(c.operation, ConversionOperation::FloatToFloat);
        assert_eq!((c.source_bits, c.destination_bits), (32, 32));
        assert_eq!(c.source, ValueSource::Constant { buffer: 4, byte_offset: 0x94 });
        assert_eq!(c.rounding, FloatRounding::NegativeInfinity);
        assert!(c.flush_to_zero && c.integer_rounding);
        assert!(!c.saturate && !c.signed);
        assert_eq!(c.unrecognized_bits, 0);
        let c = ConversionInstruction::decode(0x5cae240000170a00).unwrap();
        assert!(c.absolute && c.negate && c.saturate && c.integer_rounding);
        assert_eq!(c.unrecognized_bits, 0);
        let c = ConversionInstruction::decode(0x38a8043fc0070a00).unwrap();
        assert_eq!(c.source, ValueSource::Immediate(1.5f32.to_bits()));
        assert_eq!(c.unrecognized_bits, 0);
        assert!(!ConversionInstruction::decode(0x5ca8018000170a00).unwrap().integer_rounding);
    }

    #[test]
    fn original_conversions_preserve_sizes_rounding_and_buffer() {
        let c = ConversionInstruction::decode(0x4cb0118c00271a07).unwrap();
        assert_eq!(c.operation, ConversionOperation::FloatToInteger);
        assert_eq!(c.source, ValueSource::Constant { buffer: 3, byte_offset: 8 });
        assert_eq!((c.source_bits, c.destination_bits, c.signed), (32, 32, true));
        assert_eq!(c.rounding, FloatRounding::Zero);
        assert!(c.flush_to_zero);
        assert_eq!(c.unrecognized_bits, 0);
        let c = ConversionInstruction::decode(0x5cb8000000070606).unwrap();
        assert_eq!(c.operation, ConversionOperation::IntegerToFloat);
        assert_eq!((c.source_bits, c.destination_bits, c.signed), (16, 32, false));
    }
}
