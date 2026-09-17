use super::{
    emit::{name, Emitter},
    unsupported, Input,
};
use crate::{
    ir::{InterpolationLocation as IrLocation, ValueId},
    program::{
        interpolation::{Attribute, Component, OffsetModel},
        Diagnostic, Interpolant, InterpolationLocation, InterpolationMode,
    },
};

impl Emitter<'_> {
    pub(super) fn interpolate(
        &mut self,
        byte_offset: u16,
        mode: InterpolationMode,
        location: IrLocation,
        multiplier: Option<ValueId>,
        saturate: bool,
        pc: usize,
    ) -> Result<String, Diagnostic> {
        let value = match location {
            IrLocation::Offset(offset) => {
                self.observe(&self.known(offset), pc);
                self.offset_interpolant(byte_offset, mode, offset, pc)?
            }
            IrLocation::Default | IrLocation::Centroid => {
                let location = if location == IrLocation::Default {
                    InterpolationLocation::Default
                } else {
                    InterpolationLocation::Centroid
                };
                self.read_input(Input::Interpolant(Interpolant { byte_offset, mode, location }), pc)
            }
        };
        let value = if let Some(multiplier) = multiplier {
            self.arithmetic(pc)?;
            format!("bitcast<u32>(bitcast<f32>({value}) * bitcast<f32>({}))", name(multiplier))
        } else {
            value
        };
        Ok(if saturate { format!("remax_saturate({value})") } else { value })
    }

    fn offset_interpolant(
        &mut self,
        byte_offset: u16,
        mode: InterpolationMode,
        offset: ValueId,
        pc: usize,
    ) -> Result<String, Diagnostic> {
        let model = self.options.offset_interpolation.ok_or_else(|| {
            unsupported(pc, "offset interpolation requires an explicit plane/decoder contract")
        })?;
        self.arithmetic(pc)?;
        self.kernel.offset_interpolation = Some(model);
        let mut component = |component| {
            let bits = self.read_input(
                Input::InterpolationPlane { attribute: Attribute { byte_offset, mode }, component },
                pc,
            );
            format!("bitcast<f32>({bits})")
        };
        let center = component(Component::Center);
        let dx = component(Component::Dx);
        let dy = component(Component::Dy);
        let packed = name(offset);
        let (x, y) = match model {
            OffsetModel::SignedSixteenths => (
                format!("(f32(bitcast<i32>({packed} << 20u) >> 28u) * 0.0625)"),
                format!("(f32(bitcast<i32>({packed} << 4u) >> 28u) * 0.0625)"),
            ),
        };
        Ok(format!("bitcast<u32>(({center} + {dx} * {x}) + {dy} * {y})"))
    }
}
