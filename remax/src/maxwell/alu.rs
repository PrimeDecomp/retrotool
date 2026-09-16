//! Maxwell scalar moves and floating-point ALU operands.
//!
//! Encoding references: devkitPro/uam nv50_ir_emit_gm107.cpp at
//! 5a5afc2bae8b55409ab36ba45be63fcb73f68993 and Mesa NAK sm50.rs at
//! 822120d6d5c0109f98c3fbf85ec716c5d1fa00b6. These are decoded operations,
//! not an assertion that host floating-point operations reproduce Maxwell.
//! In particular, rounding, FTZ/DNZ, MUFU and RRO need deliberate lowering.

use serde_derive::Serialize;

use crate::maxwell::Instruction;

/// Sources retain register bits. Their interpretation comes from the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ValueSource {
    /// 255 denotes RZ, including in source positions.
    Register(u8),
    /// Direct ALU addressing: signed 14-bit word offset, followed by 5 bank bits.
    /// Indexed LDC uses a different encoding; see maxwell::memory.
    Constant {
        buffer: u8,
        byte_offset: i16,
    },
    Immediate(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FloatOperand {
    pub source: ValueSource,
    /// Applied before negate.
    pub absolute: bool,
    pub negate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PredicateOperand {
    /// 7 denotes PT.
    pub register: u8,
    pub inverted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FloatRounding {
    NearestEven,
    NegativeInfinity,
    PositiveInfinity,
    Zero,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SpecialFunction {
    Cos,
    Sin,
    Exp2,
    Log2,
    Reciprocal,
    ReciprocalSqrt,
    Reciprocal64High,
    ReciprocalSqrt64High,
    Sqrt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RangeReduction {
    SinCos,
    Exp2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "opcode")]
pub enum AluOperation {
    Move {
        source: ValueSource,
        lane_mask: u8,
    },
    FAdd {
        sources: [FloatOperand; 2],
        rounding: FloatRounding,
        flush_to_zero: bool,
        saturate: bool,
    },
    FMul {
        sources: [FloatOperand; 2],
        rounding: FloatRounding,
        flush_to_zero: bool,
        dnz: bool,
        saturate: bool,
        /// Keep the raw code: 7 is not defined by either reference encoder.
        scale_code: u8,
        scale_exponent: Option<i8>,
    },
    FFma {
        sources: [FloatOperand; 3],
        rounding: FloatRounding,
        flush_to_zero: bool,
        dnz: bool,
        saturate: bool,
    },
    FMinMax {
        sources: [FloatOperand; 2],
        min: PredicateOperand,
        flush_to_zero: bool,
    },
    Mufu {
        source: FloatOperand,
        function_code: u8,
        function: Option<SpecialFunction>,
        saturate: bool,
    },
    Rro {
        source: FloatOperand,
        reduction: RangeReduction,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AluInstruction {
    pub destination: u8,
    pub operation: AluOperation,
    pub write_condition_code: bool,
    /// Set bits not explained by this decoder. Consumers must not silently
    /// execute them as the otherwise recognized operation.
    pub unrecognized_bits: u64,
}

pub(super) struct Fields {
    word: u64,
    pub(super) used: u64,
}

impl Fields {
    pub(super) fn new(word: u64, opcode_mask: u64) -> Self {
        Self { word, used: opcode_mask | 0xf0000 } // common execution predicate
    }

    pub(super) fn get(&mut self, start: u8, width: u8) -> u64 {
        let mask = (1u64 << width) - 1;
        self.used |= mask << start;
        (self.word >> start) & mask
    }

    pub(super) fn bit(&mut self, start: u8) -> bool { self.get(start, 1) != 0 }

    pub(super) fn register(&mut self, start: u8) -> ValueSource {
        ValueSource::Register(self.get(start, 8) as u8)
    }

    pub(super) fn constant(&mut self) -> ValueSource {
        ValueSource::Constant {
            byte_offset: (self.get(20, 14) * 4) as u16 as i16,
            buffer: self.get(34, 5) as u8,
        }
    }

    pub(super) fn float_immediate(&mut self) -> ValueSource {
        ValueSource::Immediate(((self.get(20, 19) << 12) | (self.get(56, 1) << 31)) as u32)
    }

    pub(super) fn immediate32(&mut self) -> ValueSource {
        ValueSource::Immediate(self.get(20, 32) as u32)
    }

    pub(super) fn integer_immediate(&mut self) -> ValueSource {
        ValueSource::Immediate((self.get(20, 19) | (self.get(56, 1) * 0xfff80000)) as u32)
    }

    fn operand(
        &mut self,
        source: ValueSource,
        absolute: Option<u8>,
        negate: Option<u8>,
    ) -> FloatOperand {
        FloatOperand {
            source,
            absolute: absolute.is_some_and(|b| self.bit(b)),
            negate: negate.is_some_and(|b| self.bit(b)),
        }
    }

    pub(super) fn rounding(&mut self, start: u8) -> FloatRounding {
        [
            FloatRounding::NearestEven,
            FloatRounding::NegativeInfinity,
            FloatRounding::PositiveInfinity,
            FloatRounding::Zero,
        ][self.get(start, 2) as usize]
    }
}

impl AluInstruction {
    pub fn decode(word: u64) -> Option<Self> {
        // Compact immediates use bit56 for their sign; it is not an opcode bit.
        let normal = word & 0xfff8000000000000;
        let compact = word & 0xfef8000000000000;
        let fma = word & 0xff80000000000000;
        let compact_fma = word & 0xfe80000000000000;
        let long = word & 0xfc00000000000000;
        let mut f;
        let operation;
        let write_condition_code;
        if normal == 0x5c98000000000000 || normal == 0x4c98000000000000 {
            f = Fields::new(word, 0xfff8000000000000);
            let source = if normal == 0x5c98000000000000 { f.register(20) } else { f.constant() };
            operation = AluOperation::Move { source, lane_mask: f.get(39, 4) as u8 };
            write_condition_code = false;
        } else if word & 0xfff0000000000000 == 0x0100000000000000 {
            f = Fields::new(word, 0xfff0000000000000);
            operation =
                AluOperation::Move { source: f.immediate32(), lane_mask: f.get(12, 4) as u8 };
            write_condition_code = false;
        } else if matches!(normal, 0x5c58000000000000 | 0x4c58000000000000)
            || compact == 0x3858000000000000
            || long == 0x0800000000000000
        {
            let is_long = long == 0x0800000000000000;
            f = Fields::new(
                word,
                if is_long {
                    0xfc00000000000000
                } else if compact == 0x3858000000000000 {
                    0xfef8000000000000
                } else {
                    0xfff8000000000000
                },
            );
            let a = f.register(8);
            let b = if is_long {
                f.immediate32()
            } else if normal == 0x5c58000000000000 {
                f.register(20)
            } else if normal == 0x4c58000000000000 {
                f.constant()
            } else {
                f.float_immediate()
            };
            operation = AluOperation::FAdd {
                sources: [
                    f.operand(
                        a,
                        Some(if is_long { 54 } else { 46 }),
                        Some(if is_long { 56 } else { 48 }),
                    ),
                    f.operand(
                        b,
                        Some(if is_long { 57 } else { 49 }),
                        Some(if is_long { 53 } else { 45 }),
                    ),
                ],
                rounding: if is_long { FloatRounding::NearestEven } else { f.rounding(39) },
                flush_to_zero: f.bit(if is_long { 55 } else { 44 }),
                saturate: !is_long && f.bit(50),
            };
            write_condition_code = f.bit(if is_long { 52 } else { 47 });
        } else if matches!(normal, 0x5c68000000000000 | 0x4c68000000000000)
            || compact == 0x3868000000000000
            || word >> 56 == 0x1e
        {
            let is_long = word >> 56 == 0x1e;
            f = Fields::new(
                word,
                if is_long {
                    0xff00000000000000
                } else if compact == 0x3868000000000000 {
                    0xfef8000000000000
                } else {
                    0xfff8000000000000
                },
            );
            let a = f.register(8);
            let b = if is_long {
                f.immediate32()
            } else if normal == 0x5c68000000000000 {
                f.register(20)
            } else if normal == 0x4c68000000000000 {
                f.constant()
            } else {
                f.float_immediate()
            };
            let scale_code = if is_long { 0 } else { f.get(41, 3) as u8 };
            operation = AluOperation::FMul {
                // Hardware has one sign bit for the product. Canonicalize it on a.
                sources: [f.operand(a, None, (!is_long).then_some(48)), f.operand(b, None, None)],
                rounding: if is_long { FloatRounding::NearestEven } else { f.rounding(39) },
                flush_to_zero: f.bit(if is_long { 53 } else { 44 }),
                dnz: f.bit(if is_long { 54 } else { 45 }),
                saturate: f.bit(if is_long { 55 } else { 50 }),
                scale_code,
                scale_exponent: [0, -1, -2, -3, 3, 2, 1].get(scale_code as usize).copied(),
            };
            write_condition_code = f.bit(if is_long { 52 } else { 47 });
        } else if matches!(fma, 0x5980000000000000 | 0x4980000000000000 | 0x5180000000000000)
            || compact_fma == 0x3280000000000000
            || long == 0x0c00000000000000
        {
            let is_long = long == 0x0c00000000000000;
            f = Fields::new(
                word,
                if is_long {
                    0xfc00000000000000
                } else if compact_fma == 0x3280000000000000 {
                    0xfe80000000000000
                } else {
                    0xff80000000000000
                },
            );
            let a = f.register(8);
            let (b, c) = if is_long {
                (f.immediate32(), f.register(0))
            } else if fma == 0x5180000000000000 {
                (f.register(39), f.constant())
            } else {
                (
                    if fma == 0x5980000000000000 {
                        f.register(20)
                    } else if fma == 0x4980000000000000 {
                        f.constant()
                    } else {
                        f.float_immediate()
                    },
                    f.register(39),
                )
            };
            operation = AluOperation::FFma {
                sources: [
                    f.operand(a, None, Some(if is_long { 56 } else { 48 })),
                    f.operand(b, None, None),
                    f.operand(c, None, Some(if is_long { 57 } else { 49 })),
                ],
                rounding: if is_long { FloatRounding::NearestEven } else { f.rounding(51) },
                flush_to_zero: f.bit(53),
                dnz: f.bit(54),
                saturate: f.bit(if is_long { 55 } else { 50 }),
            };
            write_condition_code = f.bit(if is_long { 52 } else { 47 });
        } else if matches!(normal, 0x5c60000000000000 | 0x4c60000000000000)
            || compact == 0x3860000000000000
        {
            f = Fields::new(
                word,
                if compact == 0x3860000000000000 { 0xfef8000000000000 } else { 0xfff8000000000000 },
            );
            let a = f.register(8);
            let b = if normal == 0x5c60000000000000 {
                f.register(20)
            } else if normal == 0x4c60000000000000 {
                f.constant()
            } else {
                f.float_immediate()
            };
            operation = AluOperation::FMinMax {
                sources: [f.operand(a, Some(46), Some(48)), f.operand(b, Some(49), Some(45))],
                min: PredicateOperand { register: f.get(39, 3) as u8, inverted: f.bit(42) },
                flush_to_zero: f.bit(44),
            };
            write_condition_code = f.bit(47);
        } else if normal == 0x5080000000000000 {
            f = Fields::new(word, 0xfff8000000000000);
            let a = f.register(8);
            let function_code = f.get(20, 4) as u8;
            operation = AluOperation::Mufu {
                source: f.operand(a, Some(46), Some(48)),
                function_code,
                function: [
                    SpecialFunction::Cos,
                    SpecialFunction::Sin,
                    SpecialFunction::Exp2,
                    SpecialFunction::Log2,
                    SpecialFunction::Reciprocal,
                    SpecialFunction::ReciprocalSqrt,
                    SpecialFunction::Reciprocal64High,
                    SpecialFunction::ReciprocalSqrt64High,
                    SpecialFunction::Sqrt,
                ]
                .get(function_code as usize)
                .copied(),
                saturate: f.bit(50),
            };
            write_condition_code = false;
        } else if matches!(normal, 0x5c90000000000000 | 0x4c90000000000000)
            || compact == 0x3890000000000000
        {
            f = Fields::new(
                word,
                if compact == 0x3890000000000000 { 0xfef8000000000000 } else { 0xfff8000000000000 },
            );
            let a = if normal == 0x5c90000000000000 {
                f.register(20)
            } else if normal == 0x4c90000000000000 {
                f.constant()
            } else {
                f.float_immediate()
            };
            operation = AluOperation::Rro {
                source: f.operand(a, Some(49), Some(45)),
                reduction: if f.bit(39) { RangeReduction::Exp2 } else { RangeReduction::SinCos },
            };
            write_condition_code = false;
        } else {
            return None;
        }
        Some(Self {
            destination: f.get(0, 8) as u8,
            operation,
            write_condition_code,
            unrecognized_bits: word & !f.used,
        })
    }
}

impl Instruction {
    pub fn alu(&self) -> Option<AluInstruction> { AluInstruction::decode(self.word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_constant_offset_and_raw_immediate_bits() {
        // nvdisasm: MOV R12,c[0x11][-0x4],0x5.
        let mov = AluInstruction::decode(0x4c9802c7fff8000c).unwrap();
        assert_eq!(mov.operation, AluOperation::Move {
            source: ValueSource::Constant { buffer: 17, byte_offset: -4 },
            lane_mask: 5,
        });
        assert_eq!(mov.unrecognized_bits, 0);
        // MOV32I must preserve a NaN payload as bits, not serialize a float.
        let mov = AluInstruction::decode(0x010000000007f000 | (0x7fc12345u64 << 20)).unwrap();
        assert_eq!(mov.operation, AluOperation::Move {
            source: ValueSource::Immediate(0x7fc12345),
            lane_mask: 15
        });
    }

    #[test]
    fn long_fma_reads_the_old_destination() {
        // nvdisasm: FFMA32I.FTZ.SAT RZ,R0,-QNAN,-RZ.
        // The disassembler omits the NaN payload; the decoder retains it.
        let instruction = AluInstruction::decode(0x0eaffa1a7e0700ff).unwrap();
        let AluOperation::FFma { sources, flush_to_zero, saturate, .. } = instruction.operation
        else {
            panic!()
        };
        assert_eq!(sources[2], FloatOperand {
            source: ValueSource::Register(255),
            absolute: false,
            negate: true
        });
        assert_eq!(sources[1].source, ValueSource::Immediate(0xffa1a7e0));
        assert!(flush_to_zero && saturate);
        assert_eq!(instruction.unrecognized_bits, 0);
    }

    #[test]
    fn unsupported_bits_and_functions_remain_explicit() {
        let word = 0x5c58000000170200;
        assert_eq!(AluInstruction::decode(word).unwrap().unrecognized_bits, 0);
        assert_eq!(AluInstruction::decode(word | (1 << 42)).unwrap().unrecognized_bits, 1 << 42);
        let AluOperation::Mufu { function, function_code, .. } =
            AluInstruction::decode(0x5080000000f70000).unwrap().operation
        else {
            panic!()
        };
        assert_eq!((function, function_code), (None, 15));
        assert!(AluInstruction::decode(0xefd87f800807ff00).is_none()); // ALD
    }
}
