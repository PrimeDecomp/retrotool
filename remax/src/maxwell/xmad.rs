//! Maxwell extended 16-bit multiply/add operands. uam emitXMAD/nv50_ir.h at
//! 5a5afc2 supplies encodings/modes; operands are checked against nvdisasm SM53.
//! NAK uses other integer multiply encodings, so it is not an XMAD encoder oracle.
//! Non-carry semantics are also checked against yuzu's
//! `integer_short_multiply_add.cpp` at 6bb3815b7662b65502ea6a19357bd0354ff27714.

use serde_derive::Serialize;

use crate::maxwell::{
    alu::{Fields, ValueSource},
    Instruction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum XmadAccumulator {
    Full,
    Low,
    High,
    SignCorrection,
    MergeCarry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct XmadInstruction {
    pub destination: u8,
    /// An immediate B operand retains its encoded 16 bits. Apply `signed[1]`
    /// when interpreting it; nvdisasm may print the same literal as negative.
    pub sources: [ValueSource; 3],
    pub high_half: [bool; 2],
    pub signed: [bool; 2],
    pub product_shift_left: bool,
    pub merge: bool,
    pub accumulator_code: u8,
    pub accumulator: Option<XmadAccumulator>,
    pub extended: bool,
    pub write_condition_code: bool,
    pub unrecognized_bits: u64,
}

impl XmadInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        let normal = word & 0xffc0000000000000;
        let buffer_b = word & 0xfe00000000000000 == 0x4e00000000000000;
        let buffer_c = word & 0xff80000000000000 == 0x5100000000000000;
        let form = if buffer_b {
            1
        } else if buffer_c {
            2
        } else if normal == 0x5b00000000000000 {
            0
        } else if normal == 0x3600000000000000 {
            3
        } else {
            return None;
        };
        let mut f = Fields::new(
            word,
            if buffer_b {
                0xfe00000000000000
            } else if buffer_c {
                0xff80000000000000
            } else {
                0xffc0000000000000
            },
        );
        let destination = f.get(0, 8) as u8;
        let a = f.register(8);
        let (b, c) = match form {
            0 => (f.register(20), f.register(39)),
            1 => (f.constant(), f.register(39)),
            2 => {
                let c = f.constant();
                (f.register(39), c)
            }
            _ => (ValueSource::Immediate(f.get(20, 16) as u32), f.register(39)),
        };
        let product_shift_left = form != 2 && f.bit(if form == 1 { 55 } else { 36 });
        let merge = form != 2 && f.bit(if form == 1 { 56 } else { 37 });
        let accumulator_code = f.get(50, if form == 1 || form == 2 { 2 } else { 3 }) as u8;
        let accumulator = [
            XmadAccumulator::Full,
            XmadAccumulator::Low,
            XmadAccumulator::High,
            XmadAccumulator::SignCorrection,
            XmadAccumulator::MergeCarry,
        ]
        .get(accumulator_code as usize)
        .copied();
        let extended = f.bit(if form == 1 || form == 2 { 54 } else { 38 });
        let write_condition_code = f.bit(47);
        let signed = [f.bit(48), f.bit(49)];
        let high_half =
            [f.bit(53), form != 3 && f.bit(if form == 1 || form == 2 { 52 } else { 35 })];
        Some(Self {
            destination,
            sources: [a, b, c],
            high_half,
            signed,
            product_shift_left,
            merge,
            accumulator_code,
            accumulator,
            extended,
            write_condition_code,
            unrecognized_bits: word & !f.used,
        })
    }
}

impl Instruction {
    pub fn xmad(&self) -> Option<XmadInstruction> { XmadInstruction::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_sample_offset_packing_and_signed_literal() {
        // Original: XMAD.PSL.CLO R0, R1, 0x1, R0.
        let pack = XmadInstruction::decode(0x3604001000170100).unwrap();
        assert_eq!(pack.sources, [
            ValueSource::Register(1),
            ValueSource::Immediate(1),
            ValueSource::Register(0)
        ]);
        assert_eq!(pack.accumulator, Some(XmadAccumulator::Low));
        assert!(pack.product_shift_left);
        assert!(!pack.merge);
        assert_eq!(pack.unrecognized_bits, 0);
        // Independently disassembled: XMAD.U16.S16 R4, R5, -0x1, R7.
        let signed = XmadInstruction::decode(0x3602038ffff70504).unwrap();
        assert_eq!(signed.sources[1], ValueSource::Immediate(0xffff));
        assert_eq!(signed.signed, [false, true]);
        assert_eq!(signed.high_half, [false, false]);
        assert_eq!(signed.unrecognized_bits, 0);
    }

    #[test]
    fn constant_accumulator_does_not_overlap_ffma() {
        // Independently disassembled: XMAD R4, R5, R7, c[0x3][0x24].
        let op = XmadInstruction::decode(0x5100038c00970504).unwrap();
        assert_eq!(op.sources[1], ValueSource::Register(7));
        assert_eq!(op.sources[2], ValueSource::Constant { buffer: 3, byte_offset: 0x24 });
        assert_eq!(op.unrecognized_bits, 0);
        assert!(XmadInstruction::decode(0x5180038c00970504).is_none());
        assert!(XmadInstruction::decode(0x36b0038c00970504).is_none());
    }
}
