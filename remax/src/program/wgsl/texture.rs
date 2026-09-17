//! Target resource legalization. View types, channel mappings and sampler state
//! come from the caller, independently of register masks or arithmetic uses.
//! Filtered 2D/2D-array instructions share addressing: a 2D view has one layer,
//! a 2D instruction selects layer zero, and array layers clamp to the view range.
//! This does not authorize changing storage or applying the same rule to loads.
//! Host filtering is not a bit-exact Maxwell texture-unit model.

mod emit;
mod plan;

pub(super) use emit::declarations;
pub(super) use plan::plan;

use crate::program::{Diagnostic, DiagnosticKind, Phase};
pub use crate::resources::texture::{
    TextureBinding, TextureFilter, TextureSampling, TextureValueType,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthComponent {
    Depth,
    Zero,
    One,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding {
    /// The host view/sampler must satisfy these properties throughout the draw.
    pub view: TextureBinding,
    pub group: u32,
    pub texture_binding: u32,
    pub sampler_binding: Option<u32>,
    /// Required for ordinary depth reads, which WGSL returns as one scalar.
    /// Describe the native descriptor's logical RGBA mapping; do not infer it
    /// from an output mask. Depth-comparison results have their own expansion.
    pub depth_components: Option<[DepthComponent; 4]>,
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub binding: Binding,
    pub uses_sampler: bool,
    pub comparison_sampler: bool,
    pub uses_implicit_lod: bool,
    pub uses_fixed_implicit_lod: bool,
    /// Some original accesses permit omitting helper results (NODEP). The host
    /// may compute them; this does not recover suppressed native helper bits.
    pub helper_results_optional: bool,
}

fn invalid(pc: Option<usize>, message: &str) -> Diagnostic {
    Diagnostic::new(Phase::Emit, pc, DiagnosticKind::InvalidInput, message)
}
