use super::{invalid, unsupported, FragmentFrequency, GenericAttributeDefaults};
use crate::{
    maxwell::{
        io::{PixelInputMode, PixelShaderIo},
        ShaderProgramHeader,
    },
    program::{
        interpolation::Component,
        wgsl::{Input, Kernel},
        Diagnostic, InterpolationLocation, InterpolationMode,
    },
};

/// Validated raster clip planes, independent of fragment varying consumption.
#[derive(Clone, Copy)]
pub(super) struct ClipDistances(u8);

impl ClipDistances {
    pub fn new(header: &ShaderProgramHeader, enabled: Option<u8>) -> Result<Self, Diagnostic> {
        let declared = (0..8).fold(0, |mask, index| {
            if header.vtg_attribute_enabled(0x2c0 + index * 4, true) == Some(true) {
                mask | (1 << index)
            } else {
                mask
            }
        });
        let mask = match enabled {
            Some(mask) => mask,
            None if declared == 0 => 0,
            None => {
                return Err(unsupported("clip-distance outputs require an explicit enable mask"))
            }
        };
        Ok(Self(mask & declared))
    }

    pub fn mask(self) -> u8 { self.0 }

    pub fn count(self) -> u32 { u8::BITS - self.0.leading_zeros() }

    pub fn attributes(self) -> impl Iterator<Item = u16> {
        (0..8).filter(move |index| self.0 & (1 << index) != 0).map(|index| 0x2c0 + index * 4)
    }
}

pub(super) struct Field {
    pub source: VertexValue,
    pub divide_by_clip_w: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VertexValue {
    Attribute(u16),
    Default(u32),
}

impl VertexValue {
    fn resolve(
        offset: u16,
        vertex: &ShaderProgramHeader,
        defaults: Option<GenericAttributeDefaults>,
    ) -> Result<Self, Diagnostic> {
        if vertex.vtg_attribute_enabled(offset, true) == Some(true) {
            Ok(Self::Attribute(offset))
        } else if let Some(defaults) = defaults {
            Ok(Self::Default(defaults.component(offset)))
        } else {
            Err(invalid(format!("vertex output {offset:#x} is absent from SPH")))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Interpolation {
    Flat,
    LinearCenter,
    LinearCentroid,
    LinearSample,
}

impl Interpolation {
    pub fn qualifier(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::LinearCenter => "linear, center",
            Self::LinearCentroid => "linear, centroid",
            Self::LinearSample => "linear, sample",
        }
    }

    pub fn is_flat(self) -> bool { self == Self::Flat }
}

pub(super) struct Location {
    pub interpolation: Interpolation,
    pub fields: Vec<Field>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PositionComponent {
    X,
    Y,
    Depth,
    ReciprocalW,
}

impl PositionComponent {
    pub fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Depth => "z",
            Self::ReciprocalW => "w",
        }
    }

    pub fn is_xy(self) -> bool { matches!(self, Self::X | Self::Y) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RasterValue {
    Varying { location: usize, component: usize },
    Position(PositionComponent),
    FrontFace,
    SampleIndex,
}

pub(super) struct Argument {
    pub value: RasterValue,
    pub plane_component: Option<Component>,
}

impl Argument {
    pub fn needs_plane(&self, frequency: FragmentFrequency) -> bool {
        let recenter_sample = frequency == FragmentFrequency::Sample
            && matches!(self.value, RasterValue::Position(component) if !component.is_xy());
        self.plane_component.is_some() || recenter_sample
    }
}

pub(super) struct Interface {
    pub locations: Vec<Location>,
    pub arguments: Vec<Argument>,
    pub front_face: bool,
}

impl Interface {
    pub fn new(
        kernel: &Kernel,
        io: &PixelShaderIo,
        vertex: &ShaderProgramHeader,
        frequency: FragmentFrequency,
        defaults: Option<GenericAttributeDefaults>,
    ) -> Result<Self, Diagnostic> {
        let mut interface = Self { locations: vec![], arguments: vec![], front_face: false };
        for &input in &kernel.inputs {
            let argument = interface.input(input, io, vertex, frequency, defaults)?;
            interface.arguments.push(argument);
        }
        Ok(interface)
    }

    pub fn attributes(&self) -> impl Iterator<Item = u16> + '_ {
        self.locations.iter().flat_map(|location| {
            location.fields.iter().filter_map(|field| match field.source {
                VertexValue::Attribute(offset) => Some(offset),
                VertexValue::Default(_) => None,
            })
        })
    }

    pub fn needs_vertex_arithmetic(&self) -> bool {
        self.locations
            .iter()
            .any(|location| location.fields.iter().any(|field| field.divide_by_clip_w))
    }

    pub fn needs_fragment_arithmetic(&self, frequency: FragmentFrequency) -> bool {
        self.arguments.iter().any(|argument| argument.needs_plane(frequency))
    }

    fn input(
        &mut self,
        input: Input,
        io: &PixelShaderIo,
        vertex: &ShaderProgramHeader,
        frequency: FragmentFrequency,
        defaults: Option<GenericAttributeDefaults>,
    ) -> Result<Argument, Diagnostic> {
        let (offset, mode, location, plane_component) = match input {
            Input::Interpolant(interpolant) => {
                (interpolant.byte_offset, interpolant.mode, interpolant.location, None)
            }
            Input::InterpolationPlane { attribute, component } => (
                attribute.byte_offset,
                attribute.mode,
                InterpolationLocation::Default,
                Some(component),
            ),
            Input::SampleIndex if frequency == FragmentFrequency::Sample => {
                return Ok(Argument { value: RasterValue::SampleIndex, plane_component: None });
            }
            Input::SampleIndex => {
                return Err(unsupported(
                    "fragment-frequency sample index needs a representative-sample provider",
                ));
            }
            Input::Register(_) | Input::VertexAttribute(_) => {
                return Err(unsupported(
                    "graphics fragment shader depends on incoming register/vertex state",
                ));
            }
        };
        if matches!(location, InterpolationLocation::Offset(_)) {
            return Err(unsupported("exact offset snapshots do not describe a raster interface"));
        }
        let value = match offset {
            0x3fc => {
                if mode != InterpolationMode::Constant
                    || location != InterpolationLocation::Default
                    || plane_component.is_some()
                {
                    return Err(unsupported("front face requires ordinary flat interpolation"));
                }
                self.front_face = true;
                RasterValue::FrontFace
            }
            0x70..0x80 => system_position(offset, mode, location, io)?,
            0x80..0x280 => {
                let header_mode = io
                    .inputs
                    .iter()
                    .find(|input| input.byte_offset == offset)
                    .ok_or_else(|| invalid(format!("pixel input {offset:#x} is absent from SPH")))?
                    .mode;
                let interpolation = interpolation(
                    header_mode,
                    mode,
                    location,
                    plane_component.is_some(),
                    frequency,
                )?;
                let source = VertexValue::resolve(offset, vertex, defaults)?;
                self.varying(source, header_mode, interpolation)
            }
            _ => return Err(unsupported("graphics wrapper requires generic varyings")),
        };
        Ok(Argument { value, plane_component })
    }

    fn varying(
        &mut self,
        source: VertexValue,
        header_mode: PixelInputMode,
        interpolation: Interpolation,
    ) -> RasterValue {
        // Different IPA arithmetic modes can read the same hardware plane.
        // Reuse its varying, while keeping center and centroid/sample separate.
        let divide_by_clip_w = header_mode == PixelInputMode::Perspective;
        for (location, fields) in self.locations.iter().enumerate() {
            if fields.interpolation == interpolation {
                if let Some(component) = fields.fields.iter().position(|field| {
                    field.source == source && field.divide_by_clip_w == divide_by_clip_w
                }) {
                    return RasterValue::Varying { location, component };
                }
            }
        }
        let location = self
            .locations
            .iter()
            .position(|location| {
                location.interpolation == interpolation && location.fields.len() < 4
            })
            .unwrap_or_else(|| {
                self.locations.push(Location { interpolation, fields: vec![] });
                self.locations.len() - 1
            });
        let fields = &mut self.locations[location].fields;
        let component = fields.len();
        fields.push(Field { source, divide_by_clip_w });
        RasterValue::Varying { location, component }
    }
}

fn system_position(
    offset: u16,
    mode: InterpolationMode,
    location: InterpolationLocation,
    io: &PixelShaderIo,
) -> Result<RasterValue, Diagnostic> {
    if mode != InterpolationMode::Pass || location != InterpolationLocation::Default {
        return Err(unsupported("system position requires default/offset PASS interpolation"));
    }
    if io.system_inputs_ab & (1 << (offset / 4)) == 0 {
        return Err(invalid(format!("system input {offset:#x} is absent from SPH")));
    }
    let component = match offset {
        0x70 => PositionComponent::X,
        0x74 => PositionComponent::Y,
        0x78 => PositionComponent::Depth,
        0x7c => PositionComponent::ReciprocalW,
        _ => return Err(invalid("unaligned system position input")),
    };
    Ok(RasterValue::Position(component))
}

fn interpolation(
    header: PixelInputMode,
    mode: InterpolationMode,
    location: InterpolationLocation,
    plane: bool,
    frequency: FragmentFrequency,
) -> Result<Interpolation, Diagnostic> {
    match (header, mode) {
        (PixelInputMode::Constant, InterpolationMode::Constant) if !plane => {
            return Ok(Interpolation::Flat)
        }
        (PixelInputMode::Perspective, InterpolationMode::Perspective | InterpolationMode::Pass)
        | (PixelInputMode::ScreenLinear, InterpolationMode::Pass) => {}
        _ => return Err(unsupported("incompatible SPH and IPA interpolation modes")),
    }
    Ok(match (location, frequency) {
        (InterpolationLocation::Centroid, FragmentFrequency::Sample) => Interpolation::LinearSample,
        (InterpolationLocation::Centroid, FragmentFrequency::Fragment) => {
            Interpolation::LinearCentroid
        }
        _ => Interpolation::LinearCenter,
    })
}
