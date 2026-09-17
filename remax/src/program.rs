//! Verified semantic programs and a scalar interpreter for compiler development.
//! The frontend supports integer/predicate code, binary32 arithmetic/conversion,
//! relative branches, logical attributes and supplied interpolation values/planes.
//! Constant reads share bound bytes; texture accesses use an explicit provider.
//! Global-memory reads also use an explicit provider; no host pointer is inferred.
//! Local memory is an invocation-private frame with byte-level availability.
//! AL2P and range-reduction pairs retain opaque provenance. Closed SSY/SYNC
//! regions retain verified contexts. General GPU convergence and raw native
//! intermediate representations remain unresolved.

use std::{collections::BTreeMap, fmt};

pub use crate::maxwell::attribute::InterpolationMode;
use crate::{ir::Function, maxwell::MaxwellCode, resources::ConstantMemory};

pub mod bindings;
pub mod interpolation;
pub mod memory;
pub mod texture;
pub mod wgsl;

/// Optional runtime providers for operations whose results depend on external
/// resources. They are consulted only when the corresponding operation executes.
#[derive(Default)]
pub struct Providers<'a> {
    pub textures: Option<&'a mut dyn texture::Provider>,
    pub memory: Option<&'a mut dyn memory::Provider>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// A per-invocation program without an attribute or raster interface.
    #[default]
    Scalar,
    Vertex,
    Fragment,
}

/// Result after an invocation finishes, including execution following demotion.
/// Discarded fragments have no observable exports. Resource accesses and faults
/// still occur while the demoted invocation runs as a helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Execution {
    Outputs(Vec<Value>),
    Discarded,
}

impl Execution {
    pub fn into_outputs(self) -> Option<Vec<Value>> {
        match self {
            Self::Outputs(values) => Some(values),
            Self::Discarded => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Register {
    Gpr(u8),
    Predicate(u8),
    /// Maxwell's carry bit. Other condition flags and condition-code branch
    /// consumers are not modeled. Incoming carry must be explicitly supplied.
    Carry,
    /// Per-vertex output storage, addressed in bytes. Separate from ALD inputs.
    OutputAttribute(u16),
}

#[derive(Clone, Debug, Default)]
pub struct Signature {
    pub stage: Stage,
    /// Size of the invocation-private frame at local byte offset zero. Contents
    /// start unavailable and never persist between calls. Accesses must fit this
    /// region and be naturally aligned; violations are diagnosed, not a model of
    /// native out-of-bounds behavior. This excludes shared memory and call stacks.
    pub local_memory_bytes: u32,
    /// Finite domain of logical inputs addressable by indirect attribute reads.
    /// Entries are aligned generic attribute byte offsets. Native graphics
    /// derives these from stage headers. Declaration does not promise a value:
    /// availability is checked only when that input is selected and read.
    /// Direct attribute reads are identified separately from the instructions.
    pub indexed_attributes: Vec<u16>,
    /// Registers supplied by the caller. Other incoming contents are unavailable.
    pub inputs: Vec<Register>,
    /// Observable values, returned in this order. Derive these from the stage's
    /// actual output interface; unobserved scratch registers are omitted.
    pub outputs: Vec<Register>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstantAddress {
    pub bank: u8,
    pub byte_offset: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InterpolationLocation {
    Default,
    Centroid,
    /// Exact IPA offset-register bits. A supplied interpolation value must
    /// match every bit. Plane evaluation requires an explicit offset model;
    /// without it the interpreter does not infer a gradient or offset encoding.
    Offset(u32),
}

/// One interpolation input before IPA's multiplier and saturation. Perspective
/// values are screen-linearly interpolated attribute/clip-W numerators, not the
/// final perspective-correct attribute. The provider must also honor the SPH
/// interpolation map and actual invocation/sample state. Flat values retain bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Interpolant {
    pub byte_offset: u16,
    pub mode: InterpolationMode,
    pub location: InterpolationLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Word(u32),
    Predicate(bool),
}

/// NVB197 SET_SHADER_CONTROL bit 2 selects the F32-to-integer NaN result.
/// Callers must provide this input when a conversion executes on NaN.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde_derive::Serialize)]
pub enum F2iNanBehavior {
    #[default]
    Zero,
    /// Destination sign bit only: 0x8000 for 16-bit conversions, 0x80000000
    /// for 32-bit conversions. The narrow NaN result is not sign extended.
    Indefinite,
}

#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub registers: BTreeMap<Register, Value>,
    /// Bound bytes and validity shared by direct ALU reads and indexed LDC.
    pub constants: ConstantMemory,
    /// Current vertex's established input words, keyed by attribute byte offset.
    pub attributes: BTreeMap<u16, u32>,
    /// Values for the exact attribute, mode and location requested on execution.
    pub interpolants: BTreeMap<Interpolant, u32>,
    /// Optional, explicitly modeled affine planes for offset requests absent
    /// from `interpolants`. Does not provide default or centroid interpolation.
    pub offset_interpolation: Option<interpolation::Planes>,
    /// PIXLD.MY_INDEX for this fragment invocation. Per-fragment execution still
    /// needs an established representative index; it must not default to zero.
    /// The provider owns consistency with sample frequency, coverage and layout.
    pub sample_index: Option<u32>,
    pub f2i_nan_behavior: Option<F2iNanBehavior>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatModel {
    /// Reject executed operations that need floating-point emulation.
    #[default]
    Reject,
    /// Use host f32 multiplication and clamp for IPA. This does not establish
    /// SM53 NaN, signed-zero, subnormal or rounding behavior.
    HostF32,
    /// Software IEEE binary32 arithmetic/conversion, including directed rounding
    /// and fused multiply-add. Instruction FTZ is sign-preserving. Saturation
    /// maps NaN and negative values (including -0) to +0 and clips above +1.
    /// APFloat's arithmetic NaN payloads are a model, not verified SM53 results.
    /// Min/max prefers numbers over NaNs and orders -0 below +0; two NaNs
    /// produce 0x7fffffff, measured on SM75 but not established for SM53.
    /// Scaled multiply rounds once after scaling the exact product; its
    /// pre-round result flushing follows SM75 measurements, not SM53 proof.
    /// This does not model MUFU, DNZ or floating condition codes.
    SoftwareIeee,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpecialFunctionModel {
    /// No numeric approximation is authorized for executed MUFU operations.
    #[default]
    Reject,
    /// Host f32 reciprocal, reciprocal square root, square root, log2 and paired
    /// exp2/sin/cos. Instruction input/result flushing and saturation are retained. Host
    /// precision and NaN payloads are not Maxwell's SFU implementation; results
    /// can differ between platforms. RRO/MUFU pairs are approximated as complete
    /// operations (sin/cos take radians); raw reduction encodings remain opaque.
    HostF32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InterpretOptions {
    pub step_budget: u64,
    pub float_model: FloatModel,
    /// Separate from the basic arithmetic model: opting into software IEEE
    /// arithmetic does not implicitly authorize host transcendental functions.
    pub special_functions: SpecialFunctionModel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Lift,
    Verify,
    Interpret,
    Structure,
    Bind,
    Emit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    InvalidInput,
    Unsupported,
    InvalidIr,
    MissingInput,
    UnavailableValue,
    StepLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: Phase,
    /// Byte offset within MaxwellCode, including scheduling words.
    pub pc: Option<usize>,
    pub kind: DiagnosticKind,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(
        phase: Phase,
        pc: Option<usize>,
        kind: DiagnosticKind,
        message: impl Into<String>,
    ) -> Self {
        Self { phase, pc, kind, message: message.into() }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}/{:?}", self.phase, self.kind)?;
        if let Some(pc) = self.pc {
            write!(f, " at {pc:#x}")?;
        }
        write!(f, ": {}", self.message)
    }
}
impl std::error::Error for Diagnostic {}

/// A structurally verified program. Verification establishes SSA and type
/// correctness, not that every runtime path has all its inputs available.
/// The interpreter checks actual reads and observable values on the executed path.
#[derive(Clone, Debug)]
pub struct Program {
    pub(crate) function: Function,
}

impl Program {
    pub fn lift(code: MaxwellCode<'_>, signature: &Signature) -> Result<Self, Diagnostic> {
        let function = crate::maxwell::lift::lift(code, signature)?;
        function.verify()?;
        Ok(Self { function })
    }

    /// Execute one invocation. The budget counts SSA instructions and block transfers;
    /// exhaustion reports an error instead of returning a partial shader result.
    pub fn interpret(&self, inputs: &Inputs, step_budget: u64) -> Result<Execution, Diagnostic> {
        self.interpret_with_options(inputs, InterpretOptions { step_budget, ..Default::default() })
    }

    /// Select any approximate numeric model explicitly for comparison/testing.
    pub fn interpret_with_options(
        &self,
        inputs: &Inputs,
        options: InterpretOptions,
    ) -> Result<Execution, Diagnostic> {
        self.interpret_with_resources(inputs, options, Providers::default())
    }

    /// Execute with an explicit texture provider. This can replay recorded
    /// accesses or use a caller's sampler model; it does not supply implicit
    /// derivatives or establish that the caller's results match hardware.
    pub fn interpret_with_textures(
        &self,
        inputs: &Inputs,
        options: InterpretOptions,
        textures: &mut dyn texture::Provider,
    ) -> Result<Execution, Diagnostic> {
        self.interpret_with_resources(inputs, options, Providers {
            textures: Some(textures),
            memory: None,
        })
    }

    pub fn interpret_with_resources(
        &self,
        inputs: &Inputs,
        options: InterpretOptions,
        providers: Providers<'_>,
    ) -> Result<Execution, Diagnostic> {
        self.function.interpret_with_options(inputs, options, providers)
    }

    /// Inspect block parameters, instructions and original PCs. This is a debug
    /// representation, not a stable interchange or persistence format.
    pub fn dump(&self) -> String { format!("{:#?}", self.function) }
}
