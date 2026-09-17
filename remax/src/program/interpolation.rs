//! Explicit planes for offset interpolation. These are per-invocation inputs,
//! not values computed from neighboring shader invocations inside the kernel.

use std::collections::BTreeMap;

use super::{
    Diagnostic, DiagnosticKind, FloatModel, Interpolant, InterpolationLocation, InterpolationMode,
    Phase,
};

/// Experimental decoding model supported by MPR's sample packing, the forward
/// compilers and SM75 measurements. Arbitrary ignored bits and raw SM53 IPA
/// execution remain unverified; selecting this model is an explicit assumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde_derive::Serialize)]
pub enum OffsetModel {
    /// Signed X/Y nibbles at bits 8..11/24..27, in sixteenths of a pixel
    /// relative to pixel center. Other bits do not affect this model.
    SignedSixteenths,
}

impl OffsetModel {
    pub fn decode(self, packed: u32) -> [f32; 2] {
        match self {
            Self::SignedSixteenths => {
                let x = signed_nibble(packed, 8);
                let y = signed_nibble(packed, 24);
                [x as f32 / 16., y as f32 / 16.]
            }
        }
    }
}

fn signed_nibble(packed: u32, bit_offset: u32) -> i32 {
    let nibble = (packed >> bit_offset) & 0xf;
    if nibble & 0x8 != 0 {
        nibble as i32 - 16
    } else {
        nibble as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Attribute {
    pub byte_offset: u16,
    pub mode: InterpolationMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Center,
    Dx,
    Dy,
}

/// An affine screen-space input in the native raster coordinate system. Center
/// is the value at pixel center even for a per-sample invocation. Gradients are
/// per pixel along the native +X/+Y axes. Recenter and convert axis conventions
/// before supplying this plane; a sample or centroid value is not its center.
///
/// Perspective attributes supply attribute/clip-W, before IPA's multiplier.
/// Reciprocal W is its own plane. Do not differentiate the perspective-correct
/// quotient. A wrapper deriving gradients must do so with valid quad/helper
/// participation, before divergent kernel execution or any early return.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub center: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Plane {
    pub fn component(self, component: Component) -> f32 {
        match component {
            Component::Center => self.center,
            Component::Dx => self.dx,
            Component::Dy => self.dy,
        }
    }
}

/// An explicit fallback for offset requests absent from Inputs::interpolants.
/// Exact supplied requests take precedence. Default and centroid requests never
/// silently use these planes. Numeric evaluation also requires FloatModel.
#[derive(Clone, Debug)]
pub struct Planes {
    model: OffsetModel,
    values: BTreeMap<Attribute, Plane>,
}

impl Planes {
    pub fn new(model: OffsetModel, entries: &[(Attribute, Plane)]) -> Result<Self, Diagnostic> {
        let mut values = BTreeMap::new();
        for &(attribute, plane) in entries {
            attribute.validate()?;
            if values.insert(attribute, plane).is_some() {
                return Err(Diagnostic::new(
                    Phase::Interpret,
                    None,
                    DiagnosticKind::InvalidInput,
                    "duplicate interpolation plane attribute",
                ));
            }
        }
        Ok(Self { model, values })
    }

    pub fn model(&self) -> OffsetModel { self.model }

    pub fn get(&self, attribute: Attribute) -> Option<&Plane> { self.values.get(&attribute) }

    pub(crate) fn evaluate(
        &self,
        input: Interpolant,
        numeric: FloatModel,
        pc: usize,
    ) -> Result<u32, Diagnostic> {
        let error = |kind, message| Diagnostic::new(Phase::Interpret, Some(pc), kind, message);
        let InterpolationLocation::Offset(packed) = input.location else {
            return Err(error(
                DiagnosticKind::MissingInput,
                "interpolation planes do not supply default or centroid requests",
            ));
        };
        let plane =
            self.get(Attribute { byte_offset: input.byte_offset, mode: input.mode }).ok_or_else(
                || error(DiagnosticKind::MissingInput, "interpolation plane is unavailable"),
            )?;
        if numeric == FloatModel::Reject {
            return Err(error(
                DiagnosticKind::Unsupported,
                "offset interpolation requires an explicit floating-point model",
            ));
        }
        Ok(crate::ir::float::interpolate_plane(*plane, self.model.decode(packed), numeric))
    }
}

impl Attribute {
    fn validate(self) -> Result<(), Diagnostic> {
        let message = if self.byte_offset >= 0x400 || !self.byte_offset.is_multiple_of(4) {
            "interpolation plane requires an aligned attribute below 0x400"
        } else if !matches!(self.mode, InterpolationMode::Pass | InterpolationMode::Perspective) {
            "interpolation plane requires PASS or perspective mode"
        } else {
            return Ok(());
        };
        Err(Diagnostic::new(Phase::Interpret, None, DiagnosticKind::InvalidInput, message))
    }
}
