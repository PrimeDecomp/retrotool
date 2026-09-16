//! Shared decoded operation interface. Instruction retains the raw word,
//! physical offset, execution predicate and schedule even for unknown operations.

use serde_derive::Serialize;

use crate::maxwell::{
    alu::AluInstruction,
    attribute::AttributeInstruction,
    compare::FloatComparisonInstruction,
    control::{ControlInstruction, DependencyBarrier, FragmentKill},
    convert::ConversionInstruction,
    integer::IntegerInstruction,
    integer_compare::IntegerComparisonInstruction,
    memory::ConstantLoad,
    raster::PixelLoad,
    texture::{Texs, Tlds},
    xmad::XmadInstruction,
    Instruction,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Operation {
    Alu(AluInstruction),
    Attribute(AttributeInstruction),
    FloatComparison(FloatComparisonInstruction),
    Conversion(ConversionInstruction),
    Control(ControlInstruction),
    DependencyBarrier(DependencyBarrier),
    FragmentKill(FragmentKill),
    Integer(IntegerInstruction),
    IntegerComparison(IntegerComparisonInstruction),
    PixelLoad(PixelLoad),
    Xmad(XmadInstruction),
    ConstantLoad(ConstantLoad),
    TextureSample(Texs),
    TextureLoad(Tlds),
    Nop,
    Exit,
    Unknown,
}

impl Instruction {
    pub fn operation(&self) -> Operation {
        if let Some(v) = self.alu() {
            Operation::Alu(v)
        } else if let Some(v) = self.attribute() {
            Operation::Attribute(v)
        } else if let Some(v) = self.float_comparison() {
            Operation::FloatComparison(v)
        } else if let Some(v) = DependencyBarrier::decode(self.word) {
            Operation::DependencyBarrier(v)
        } else if let Some(v) = FragmentKill::decode(self.word) {
            Operation::FragmentKill(v)
        } else if let Some(v) = self.control_flow() {
            Operation::Control(v)
        } else if let Some(v) = self.conversion() {
            Operation::Conversion(v)
        } else if let Some(v) = self.integer_comparison() {
            Operation::IntegerComparison(v)
        } else if let Some(v) = self.integer() {
            Operation::Integer(v)
        } else if let Some(v) = self.pixel_load() {
            Operation::PixelLoad(v)
        } else if let Some(v) = self.xmad() {
            Operation::Xmad(v)
        } else if let Some(v) = self.constant_load() {
            Operation::ConstantLoad(v)
        } else if let Some(v) = self.texture_sample() {
            Operation::TextureSample(v)
        } else if let Some(v) = self.texture_load() {
            Operation::TextureLoad(v)
        } else {
            match self.word & !0xf0000 {
                0x50b0000000000f00 => Operation::Nop,
                0xe30000000000000f => Operation::Exit,
                _ => Operation::Unknown,
            }
        }
    }
}
