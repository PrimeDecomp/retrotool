//! SPHv3 graphics linking for the semantic compiler. The two stage modules
//! remain separate; no rewriting of generated identifiers is needed.
//!
//! Callers provide host clip-space transforms, native-compatible raster axes,
//! established sample state and resources. A draw is valid only after checking
//! its fault buffer. Diagnostic clipping/discard does not repair a faulty draw.

use std::collections::BTreeSet;

use super::{
    io::{PixelInputMode, PixelOutputRegisters, PixelShaderIo},
    MaxwellCode, ShaderProgramHeader,
};
use crate::{
    program::{
        wgsl::{self, texture, FaultScope, Input, Kernel},
        Diagnostic, DiagnosticKind, Phase, Program, Register, Signature, Stage,
    },
    resources::ConstantMemoryBinding,
};

mod emit;
mod interface;
use interface::ClipDistances;

#[derive(Clone, Copy, Debug)]
pub struct StageSource<'a> {
    pub code: MaxwellCode<'a>,
    pub header: &'a ShaderProgramHeader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentFrequency {
    Fragment,
    Sample,
}

/// Routing of native pixel color exports, resolved from the draw's blend state.
/// Two exports alone do not distinguish two attachments from dual-source blending.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorOutputRouting {
    /// Export N writes attachment N. Gaps in the native output map are retained.
    #[default]
    Attachments,
    /// Exports 0 and 1 supply the primary and secondary colors for attachment 0.
    /// Requires exactly two complete RGBA exports and host dual-source blending
    /// support (`dual_source_blending` in WGSL). The secondary color is a blend
    /// operand, not a second attachment.
    DualSource,
}

/// SET_ATTRIBUTE_DEFAULT's generic-vector state. These values supply components
/// absent from the preceding stage's output map, not declared outputs that the
/// shader fails to write. System attributes have separate hardware defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericAttributeDefaults {
    /// All four components are floating-point zero.
    Zero,
    /// XYZ are floating-point zero; W is floating-point one.
    ZeroOne,
}

impl GenericAttributeDefaults {
    fn component(self, offset: u16) -> u32 {
        if self == Self::ZeroOne && offset % 16 == 12 {
            1f32.to_bits()
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BufferBinding {
    pub group: u32,
    pub binding: u32,
}

/// How the graphics wrapper transfers faults from helpers to covered fragments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuadFaultMethod {
    /// Broadcast integer status/PC pairs with WGSL quad operations. Requires
    /// subgroup support and a backend that keeps helpers active in these calls.
    /// WGSL does not guarantee helper participation: validate the device/backend
    /// with the native quad tests before selecting this method. Native tests cover
    /// Apple M5 Pro with wgpu 29's Metal backend; other backends remain unestablished.
    QuadBroadcast,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub vertex: wgsl::Options,
    pub fragment: wgsl::Options,
    /// Resolved from the program and runtime override, not from the sample count.
    pub frequency: FragmentFrequency,
    pub color_routing: ColorOutputRouting,
    /// Native default state for generic components absent from the vertex SPH.
    /// None requires every consumed component to have a vertex output declaration.
    pub generic_attribute_defaults: Option<GenericAttributeDefaults>,
    /// Native clip-plane enable state. Bit N enables output attribute 0x2c0 +
    /// 4*N when declared in the vertex SPH. None requires a shader with no clip
    /// declarations; Some(0) explicitly disables clipping. The intersection
    /// with the SPH map determines whether host clip-distance support is needed.
    pub clip_distance_mask: Option<u8>,
    pub vertex_inputs: BufferBinding,
    /// Four atomic u32 words: vertex status / minimum fault PC, then fragment
    /// status / minimum fault PC. Initialize to [0, MAX, 0, MAX] before drawing
    /// and inspect after completion. Nonzero status invalidates the whole draw.
    /// A host may accumulate several draws without resetting these words; a
    /// failure then invalidates every draw contributing to that capture. Do not
    /// reset or drop unchecked faults when throttling asynchronous readbacks.
    /// Requires storage writes/atomics from the vertex and fragment stages.
    pub faults: BufferBinding,
    /// Required for implicit-derivative kernels. None rejects those kernels;
    /// an invocation's own atomics cannot report faults exclusive to a helper.
    pub quad_faults: Option<QuadFaultMethod>,
}

impl Options {
    /// Separate constant buffers and two wrapper buffers in group zero. Numeric
    /// approximations remain disabled; choose texture/global bindings separately.
    pub fn new(frequency: FragmentFrequency) -> Self {
        Self {
            vertex: wgsl::Options {
                constant_memory: ConstantMemoryBinding { group: 0, binding: 1 },
                ..Default::default()
            },
            fragment: wgsl::Options {
                constant_memory: ConstantMemoryBinding { group: 0, binding: 2 },
                ..Default::default()
            },
            frequency,
            color_routing: ColorOutputRouting::Attachments,
            generic_attribute_defaults: None,
            clip_distance_mask: None,
            vertex_inputs: BufferBinding { group: 0, binding: 0 },
            faults: BufferBinding { group: 0, binding: 3 },
            quad_faults: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StageModule {
    /// Compile each stage as a separate shader module.
    pub source: String,
    pub entry_point: &'static str,
    /// Callable kernel's resource and numeric contract. The wrapper additionally
    /// needs the graphics buffers and host interpolation described by Options.
    pub kernel: Kernel,
}

#[derive(Clone, Debug)]
pub struct GraphicsProgram {
    pub vertex: StageModule,
    pub fragment: StageModule,
    /// Native pixel exports, before `color_routing` maps them to attachments.
    /// Colors use vec4<f32>; bind corresponding float render targets. At sample frequency
    /// the wrapper also exports a sample mask, intersected with any native mask.
    pub fragment_outputs: PixelOutputRegisters,
    pub color_routing: ColorOutputRouting,
    /// Each vertex record is words[N] followed by validity[N], with
    /// N=max(attributes.len(),1). Only zero/one validity words are allowed.
    /// Index with the draw's vertex_index, including its base-vertex offset.
    pub attributes: Vec<u16>,
    pub vertex_inputs: BufferBinding,
    pub faults: BufferBinding,
    /// Number of generic vec4 locations. Clip distances consume additional
    /// inter-stage components according to the host's limits.
    pub varying_locations: u32,
    /// Enabled native clip planes. Nonzero requires WGSL `clip_distances`.
    /// Sparse plane indices retain their original positions in the output array;
    /// disabled holes are positive. This array is additional to varying_locations.
    pub clip_distance_mask: u8,
    pub frequency: FragmentFrequency,
    /// Actual method used by the fragment wrapper; None for invocation scope.
    pub quad_faults: Option<QuadFaultMethod>,
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(Phase::Emit, None, DiagnosticKind::InvalidInput, message)
}
fn unsupported(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(Phase::Emit, None, DiagnosticKind::Unsupported, message)
}

/// Lift and link a vertex/pixel pair with complete RGBA color targets and optional
/// depth/sample-mask and vertex clip-distance exports. Empty color output sets
/// are supported. Clip distances require explicit enable state. The caller
/// must preserve native target routing, depth conventions and render state;
/// broadcast is not modeled. Dual-source routing must be selected explicitly
/// and requires host support; otherwise each export writes its own attachment.
/// Resource bindings must be distinct across stages. Implicit-derivative kernels
/// require an established quad fault method so helpers can invalidate the draw.
pub fn translate(
    vertex: StageSource<'_>,
    fragment: StageSource<'_>,
    options: Options,
    vertex_textures: &[texture::Binding],
    fragment_textures: &[texture::Binding],
) -> Result<GraphicsProgram, Diagnostic> {
    let io = validate_headers(vertex.header, fragment.header, options.color_routing)?;
    let clip_distances = ClipDistances::new(vertex.header, options.clip_distance_mask)?;
    let fragment_outputs = io.output_registers();
    let fragment_program = Program::lift(fragment.code, &Signature {
        local_memory_bytes: local_frame(fragment.header)?,
        // Complete AL2P/IPA recovery currently establishes perspective inputs.
        // Flat/state-selected cases need the native predicate's semantics.
        // A runtime selection outside this domain faults instead of receiving
        // a fabricated varying or a different interpolation mode.
        indexed_attributes: io
            .inputs
            .iter()
            .filter(|input| {
                input.mode == PixelInputMode::Perspective
                    && (0x80..0x280).contains(&input.byte_offset)
                    && (options.generic_attribute_defaults.is_some()
                        || vertex.header.vtg_attribute_enabled(input.byte_offset, true)
                            == Some(true))
            })
            .map(|input| input.byte_offset)
            .collect(),
        stage: Stage::Fragment,
        inputs: vec![],
        outputs: fragment_outputs.registers().map(Register::Gpr).collect(),
    })?;
    let fragment_kernel =
        fragment_program.emit_wgsl_with_textures(options.fragment, fragment_textures)?;
    let quad_faults = if fragment_kernel.fault_scope == FaultScope::Quad {
        Some(options.quad_faults.ok_or_else(|| {
            unsupported(
                "graphics linking needs helper-aware quad fault reporting for implicit derivatives",
            )
        })?)
    } else {
        None
    };
    let interface = interface::Interface::new(
        &fragment_kernel,
        &io,
        vertex.header,
        options.frequency,
        options.generic_attribute_defaults,
    )?;
    if interface.needs_vertex_arithmetic() && !options.vertex.approximate_arithmetic {
        return Err(unsupported(
            "perspective varying transport requires vertex arithmetic permission",
        ));
    }
    if interface.needs_fragment_arithmetic(options.frequency)
        && !options.fragment.approximate_arithmetic
    {
        return Err(unsupported(
            "interpolation planes/recentering require fragment arithmetic permission",
        ));
    }
    let signature = vertex_signature(vertex.header, &interface, clip_distances)?;
    let vertex_program = Program::lift(vertex.code, &signature)?;
    let vertex_kernel = vertex_program.emit_wgsl_with_textures(options.vertex, vertex_textures)?;
    let attributes = vertex_attributes(&vertex_kernel, vertex.header)?;
    check_bindings(&vertex_kernel, &fragment_kernel, options)?;
    let vertex_source = emit::vertex(&vertex_kernel, &interface, clip_distances, options);
    let fragment_source = emit::fragment(&fragment_kernel, &interface, &fragment_outputs, options);
    Ok(GraphicsProgram {
        vertex: StageModule {
            source: vertex_source,
            entry_point: "remax_vertex_main",
            kernel: vertex_kernel,
        },
        fragment: StageModule {
            source: fragment_source,
            entry_point: "remax_fragment_main",
            kernel: fragment_kernel,
        },
        fragment_outputs,
        color_routing: options.color_routing,
        attributes,
        vertex_inputs: options.vertex_inputs,
        faults: options.faults,
        varying_locations: interface.locations.len() as u32,
        clip_distance_mask: clip_distances.mask(),
        frequency: options.frequency,
        quad_faults,
    })
}

fn validate_headers(
    vertex: &ShaderProgramHeader,
    fragment: &ShaderProgramHeader,
    color_routing: ColorOutputRouting,
) -> Result<PixelShaderIo, Diagnostic> {
    if vertex.version != 3 || vertex.sph_type != 1 || vertex.shader_type != 1 {
        return Err(unsupported("graphics linking requires an SPHv3 vertex header"));
    }
    let io = fragment.pixel_io().ok_or_else(|| unsupported("expected SPHv3 pixel header"))?;
    if io.unknown_word17_high != 0 || io.unknown_word19_bits != 0 {
        return Err(unsupported("graphics linking requires known pixel IO flags"));
    }
    if io.color_masks.iter().any(|&mask| mask != 0 && mask != 15) {
        return Err(unsupported("graphics linking requires complete RGBA color outputs"));
    }
    if color_routing == ColorOutputRouting::DualSource
        && io.color_masks != [15, 15, 0, 0, 0, 0, 0, 0]
    {
        return Err(invalid("dual-source blending requires only RGBA exports 0 and 1"));
    }
    if !fragment.mrt_enable && io.color_masks.iter().filter(|&&mask| mask != 0).count() > 1 {
        return Err(unsupported(
            "multiple color outputs with disabled SPH MRT routing require broadcast handling",
        ));
    }
    Ok(io)
}

fn vertex_signature(
    header: &ShaderProgramHeader,
    interface: &interface::Interface,
    clip_distances: ClipDistances,
) -> Result<Signature, Diagnostic> {
    let outputs: BTreeSet<_> = [0x70, 0x74, 0x78, 0x7c]
        .into_iter()
        .chain(interface.attributes())
        .chain(clip_distances.attributes())
        .collect();
    for &offset in &outputs {
        if header.vtg_attribute_enabled(offset, true) != Some(true) {
            return Err(invalid(format!("vertex output {offset:#x} is absent from SPH")));
        }
    }
    Ok(Signature {
        local_memory_bytes: local_frame(header)?,
        indexed_attributes: (0x80..0x280)
            .step_by(4)
            .filter(|&offset| header.vtg_attribute_enabled(offset, false) == Some(true))
            .collect(),
        stage: Stage::Vertex,
        inputs: vec![],
        outputs: outputs.into_iter().map(Register::OutputAttribute).collect(),
    })
}

fn local_frame(header: &ShaderProgramHeader) -> Result<u32, Diagnostic> {
    if header.local_memory_high != 0 {
        return Err(unsupported("high local-memory regions require an established address layout"));
    }
    if header.local_memory_low != 0 && !header.load_or_store {
        return Err(unsupported("local frame declared with the SPH load/store path disabled"));
    }
    Ok(header.local_memory_low)
}

fn vertex_attributes(
    kernel: &Kernel,
    header: &ShaderProgramHeader,
) -> Result<Vec<u16>, Diagnostic> {
    kernel
        .inputs
        .iter()
        .map(|input| {
            let Input::VertexAttribute(offset) = *input else {
                return Err(unsupported(
                    "graphics vertex shader depends on incoming register/raster state",
                ));
            };
            if header.vtg_attribute_enabled(offset, false) != Some(true) {
                return Err(invalid(format!("vertex input {offset:#x} is absent from SPH")));
            }
            Ok(offset)
        })
        .collect()
}

fn check_bindings(vertex: &Kernel, fragment: &Kernel, options: Options) -> Result<(), Diagnostic> {
    let mut used = BTreeSet::new();
    let mut add = |group, binding| {
        if used.insert((group, binding)) {
            Ok(())
        } else {
            Err(invalid(format!("overlapping graphics binding {group}:{binding}")))
        }
    };
    add(options.vertex_inputs.group, options.vertex_inputs.binding)?;
    add(options.faults.group, options.faults.binding)?;
    for kernel in [vertex, fragment] {
        for (group, binding) in stage_bindings(kernel) {
            add(group, binding)?;
        }
    }
    Ok(())
}

fn stage_bindings(kernel: &Kernel) -> BTreeSet<(u32, u32)> {
    // Emission already checks compatibility within a stage, where descriptors
    // may deliberately share a sampler or texture view.
    let mut bindings = BTreeSet::new();
    if let Some(binding) = kernel.constant_memory {
        bindings.insert((binding.group, binding.binding));
    }
    if let Some(binding) = kernel.global_memory {
        bindings.insert((binding.group, binding.binding));
    }
    for resource in &kernel.textures {
        let binding = resource.binding;
        bindings.insert((binding.group, binding.texture_binding));
        if resource.uses_sampler {
            let sampler = binding.sampler_binding.expect("validated sampler binding");
            bindings.insert((binding.group, sampler));
        }
    }
    bindings
}
