//! WGSL emission from semantic SSA. The result is a callable kernel plus its
//! input/buffer contract; the caller supplies the graphics or compute entry point.
//! Offset interpolation uses explicitly supplied planes and a selected decoder.
//! The Maxwell graphics adapter builds entry points from SPHv3 interfaces.
//! General GPU execution analysis remains pending.

use super::{
    interpolation, Diagnostic, DiagnosticKind, F2iNanBehavior, Interpolant, Phase, Program,
    Register, Stage,
};
use crate::resources::ConstantMemoryBinding;

mod attribute;
mod budget;
mod constant;
mod control;
mod emit;
mod float;
mod instruction;
mod interpolate;
mod local;
pub mod memory;
mod plan;
pub mod texture;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Register(Register),
    VertexAttribute(u16),
    Interpolant(Interpolant),
    /// Raw f32 bits for a plane component. Supply it in the native raster frame
    /// described by interpolation::Plane, honoring Kernel::offset_interpolation.
    InterpolationPlane {
        attribute: interpolation::Attribute,
        component: interpolation::Component,
    },
    SampleIndex,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub profile: Profile,
    /// Budget counts SSA instructions and block transfers, as in the interpreter.
    /// A whole-function bound within this budget removes the runtime counter;
    /// Checked only uses acyclic bounds. Rendering also proves supported bitset
    /// loops and charges finite prefixes together. Unproven checkpoints retain
    /// their execution order and exhaustion PCs.
    pub step_budget: u32,
    /// Permit WGSL binary32 arithmetic. This does not promise IEEE rounding,
    /// fusion, NaN payloads, subnormal preservation or Maxwell equivalence.
    pub approximate_arithmetic: bool,
    /// Independently permit WGSL reciprocal, inverseSqrt, sqrt, log2 and paired
    /// exp2/sin/cos. These approximate the SFU; they do not reproduce its bits.
    pub approximate_special_functions: bool,
    /// Explicitly select the offset decoder for affine interpolation planes.
    /// Also requires approximate_arithmetic. The wrapper supplies established
    /// planes; the emitted kernel does not compute implicit derivatives.
    pub offset_interpolation: Option<interpolation::OffsetModel>,
    /// If absent, an executed F2I on NaN produces a host fault.
    pub f2i_nan_behavior: Option<F2iNanBehavior>,
    pub constant_memory: ConstantMemoryBinding,
    /// Enable global reads from a validated immutable memory snapshot. An absent
    /// binding is an emission error for programs with global-memory operations.
    pub global_memory: Option<memory::Binding>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            profile: Profile::Checked,
            step_budget: 65_536,
            approximate_arithmetic: false,
            approximate_special_functions: false,
            offset_interpolation: None,
            f2i_nan_behavior: None,
            constant_memory: ConstantMemoryBinding { group: 1, binding: 0 },
            global_memory: None,
        }
    }
}

/// Where established obligations are checked. Both profiles preserve guest
/// control flow, value availability, numeric policy and residual diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Diagnose executed resource accesses in the shader, including guarded
    /// reads whose bytes need not be bound when their path is not taken.
    #[default]
    Checked,
    /// Use validated constant-memory snapshots and move fixed-address checks
    /// to the binding contract. Dynamic address/byte-validity checks and other
    /// unproven obligations remain in the shader, so fault reporting is still
    /// required. Proven local words are promoted to SSA across control flow;
    /// redundant input flushing is removed, identical dominating expressions
    /// are reused, linear blocks are merged and supported natural loops are
    /// structured, retaining execution checkpoints and ordered resource reads.
    /// Finite prefixes share budget accounting even before an unbounded tail;
    /// bounded bitset loops can discharge the whole execution counter.
    /// Neither uniform values nor packing are specialized.
    Rendering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlFlow {
    /// Lexical branches and, in Rendering, supported natural loops. This
    /// describes scalar structure and does not promise GPU reconvergence.
    Regions,
    /// A non-repeating entry prefix followed by a scalar state dispatcher.
    /// Cross-lane operations are supported only in the entry prefix.
    ScalarDispatcher,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultScope {
    Invocation,
    /// Implicit derivatives couple neighboring results. The caller must reject
    /// the entire quad if any participating invocation (including a helper)
    /// faults. A local status of zero does not validate the neighboring inputs.
    /// Faults are deferred through the finite entry prefix, preserving all lanes
    /// for sampling. The remaining invocation-local tail may exit on a fault;
    /// the caller still reports across the quad after the kernel returns.
    Quad,
}

#[derive(Clone, Debug)]
pub struct Kernel {
    pub stage: Stage,
    /// Defines remax_main(words, valid) -> RemaxResult. Both input arrays have
    /// max(inputs.len(),1) words; validity entries are zero or one. Predicates
    /// use zero/one words. Result.words has max(outputs.len(),1) entries.
    /// Result.status records local execution faults; validity of returned words
    /// also depends on fault_scope. Result.pc identifies the first faulting PC.
    /// Result.discarded (zero/one u32) suppresses fragment exports, not fault
    /// reporting. The kernel retains helper execution and defers host discard
    /// until the graphics wrapper has reported faults. Current guest effects
    /// are local stores and demotion; adding external stores, atomics or helper
    /// queries requires revisiting this lowering before supporting them.
    /// Status bits: 1 absent bank, 2 range, 4 alignment, 8 unknown buffer bytes,
    /// 16 step limit, 32 unavailable value, 64 missing F2I NaN policy,
    /// 128 texture range/view mismatch, 256 invalid floating sample operand,
    /// 512 unmapped global address/span, 1024 unknown global-memory bytes,
    /// 2048 address/span outside the invocation's local frame.
    pub source: String,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Register>,
    pub constant_memory: Option<ConstantMemoryBinding>,
    /// All potential constant-buffer accesses, including guarded/dynamic reads.
    pub constant_requirements: super::bindings::ConstantRequirements,
    /// Preconditions used to remove shader checks. Upload constant memory only
    /// through this contract's `bind` method, and preserve those words throughout
    /// execution. Empty in the checked profile. A successful bind does not
    /// discharge dynamic accesses or other shader faults. Rendering also relies
    /// on the snapshot's packed-layout invariant, including for empty contracts.
    pub constant_preconditions: super::bindings::ConstantRequirements,
    pub global_memory: Option<memory::Binding>,
    pub textures: Vec<texture::Resource>,
    pub fault_scope: FaultScope,
    /// Call from uniform fragment control flow with valid interpolation and
    /// quad context. The kernel has no general Maxwell reconvergence model.
    pub requires_uniform_call: bool,
    pub control_flow: ControlFlow,
    pub step_budget: u32,
    /// Actual numeric concessions used by this kernel, not just allowed options.
    pub approximate_arithmetic: bool,
    pub approximate_special_functions: bool,
    /// Actual offset model used; None when no offset interpolation is emitted.
    pub offset_interpolation: Option<interpolation::OffsetModel>,
    /// A specialization of NVN conversion state; preserve it when binding.
    pub f2i_nan_behavior: Option<F2iNanBehavior>,
}

impl Program {
    pub fn emit_wgsl(&self, options: Options) -> Result<Kernel, Diagnostic> {
        self.emit_wgsl_with_textures(options, &[])
    }

    pub fn emit_wgsl_with_textures(
        &self,
        options: Options,
        textures: &[texture::Binding],
    ) -> Result<Kernel, Diagnostic> {
        emit::emit(&self.function, options, textures)
    }
}

pub(super) fn unsupported(pc: usize, message: &str) -> Diagnostic {
    Diagnostic::new(Phase::Emit, Some(pc), DiagnosticKind::Unsupported, message)
}
