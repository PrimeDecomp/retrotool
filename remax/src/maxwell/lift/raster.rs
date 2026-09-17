use super::{emit, unsupported};
use crate::{
    ir::{ssa::Builder, BlockId, Operation, Type},
    maxwell::raster::{PixelLoad, PixelValue},
    program::{Diagnostic, Register, Stage},
};

pub(super) fn load(
    b: &mut Builder,
    block: BlockId,
    pc: usize,
    load: PixelLoad,
) -> Result<(), Diagnostic> {
    if b.function.stage != Stage::Fragment {
        return Err(unsupported(pc, "PIXLD requires a fragment invocation interface"));
    }
    if load.unrecognized_bits != 0
        || load.address_register != 255
        || load.predicate_destination != 7
        || load.value != Some(PixelValue::MyIndex)
    {
        return Err(unsupported(
            pc,
            "PIXLD mode, addressing or predicate result needs additional raster semantics",
        ));
    }
    let value = emit(b, block, pc, Type::Bits32, Operation::ReadSampleIndex);
    b.write(block, Register::Gpr(load.destination), value);
    Ok(())
}
