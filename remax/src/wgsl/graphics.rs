//! Graphics linking for the current register-based WGSL backend.
//! Callers resolve execution frequency and supply actual resource properties.

use std::fmt::Write;

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

use super::{lower_graphics_stage, texture::TextureBinding, LoweringOptions, ProgramKernel};
use crate::{
    maxwell::{
        attribute::{InterpolationLocation, InterpolationMode},
        io::PixelInputMode,
        MaxwellCode, ShaderProgramHeader,
    },
    resources::ConstantMemory,
};

/// One bounded graphics stage, after container and hardware-stage validation.
#[derive(Clone, Copy, Debug)]
pub struct GraphicsStage<'a> {
    pub code: MaxwellCode<'a>,
    pub header: &'a ShaderProgramHeader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FragmentFrequency {
    PerFragment,
    PerSample,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphicsOptions {
    pub lowering: LoweringOptions,
    /// Effective invocation frequency, resolved from program and runtime state.
    pub fragment_frequency: FragmentFrequency,
}

/// Actual bound resources and optional constants for attribute specialization.
/// Missing texture declarations cause an error when the program uses them.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphicsBindings<'a> {
    pub vertex_textures: &'a [TextureBinding],
    pub fragment_textures: &'a [TextureBinding],
    /// Vertex then fragment. The output manifests record any assumed bytes.
    pub constants: Option<[&'a ConstantMemory; 2]>,
}

#[derive(Clone, Debug)]
pub struct GraphicsKernel {
    /// Vertex entry mpr_vertex_main; fragment entry mpr_fragment_main.
    /// Group0 storage bindings:0=vertex inputs(256 u32 words per vertex),
    /// 1=vertex constants,2=fragment constants. Constant buffers are compacted
    /// according to each stage's constant_reads (allocate at least one word).
    /// Vertex input indexing uses the draw's @builtin(vertex_index).
    /// Indexed constants use optional group1 bindings from each stage manifest.
    /// Invalid loads set kernel status: the wrapper clips failed vertices and
    /// discards failed fragments. Valid inputs must have zero memory status.
    pub wgsl: String,
    pub vertex: ProgramKernel,
    pub fragment: ProgramKernel,
    pub fragment_frequency: FragmentFrequency,
}

/// Link a vertex/pixel pair with one RGBA output. The vertex program must
/// already produce host clip space (WGSL depth 0..1); no coordinate correction is
/// guessed. Sample-frequency output preserves the current sample's coverage.
/// Fragment-frequency PIXLD.MY_INDEX remains unsupported.
pub fn lower_graphics(
    vertex: GraphicsStage<'_>,
    fragment: GraphicsStage<'_>,
    options: GraphicsOptions,
    bindings: GraphicsBindings<'_>,
) -> Result<GraphicsKernel> {
    let GraphicsBindings { vertex_textures, fragment_textures, constants } = bindings;
    let vertex_header = vertex.header;
    let fragment_header = fragment.header;
    ensure!(
        vertex_header.sph_type == 1
            && vertex_header.shader_type == 1
            && fragment_header.sph_type == 2
            && fragment_header.shader_type == 5,
        "expected vertex and pixel hardware headers"
    );
    ensure!(
        (0..8).all(|plane| {
            vertex_header.vtg_attribute_enabled(0x2c0 + 4 * plane, true) == Some(false)
        }),
        "clip-distance exports require the semantic graphics compiler"
    );
    let io = fragment_header.pixel_io().context("unsupported pixel hardware I/O map")?;
    ensure!(
        io.color_masks == [15, 0, 0, 0, 0, 0, 0, 0] && !io.writes_depth && !io.writes_sample_mask,
        "graphics wrapper requires one complete RGBA color output"
    );
    ensure!(
        io.unknown_word17_high == 0 && io.unknown_word19_bits == 0,
        "unrecognized pixel I/O flags"
    );
    let fragment_frequency = options.fragment_frequency;
    let per_sample = fragment_frequency == FragmentFrequency::PerSample;
    let fragment = lower_graphics_stage(
        fragment.code,
        false,
        options.lowering,
        fragment_textures,
        constants.map(|c| c[1]),
        None,
    )?;
    let mut observed_attributes = vec![0x70, 0x74, 0x78, 0x7c];
    for read in &fragment.interpolant_reads {
        if (0x80..0x280).contains(&read.byte_offset)
            && !observed_attributes.contains(&read.byte_offset)
        {
            observed_attributes.push(read.byte_offset);
        }
    }
    // Some native vertex shaders write undefined scratch values into unused
    // varying components. Only the fragment inputs and clip position are
    // observable in this single-target wrapper; never initialize such scratch.
    // ALD.O of an omitted output is rejected by the stage emitter.
    let vertex = lower_graphics_stage(
        vertex.code,
        true,
        options.lowering,
        vertex_textures,
        constants.map(|c| c[0]),
        Some(&observed_attributes),
    )?;
    ensure!(
        !fragment.uses_sample_index || per_sample,
        "PIXLD.MY_INDEX at fragment frequency requires a representative-sample provider"
    );
    ensure!(
        vertex.input_registers.is_empty()
            && vertex.input_predicates.is_empty()
            && vertex.initial_output_attributes.is_empty()
            && fragment.input_registers.is_empty()
            && fragment.input_predicates.is_empty(),
        "graphics stages depend on incoming state: vertex registers {:?}, predicates {:?}, output attributes {:?}; pixel registers {:?}, predicates {:?}",
        vertex.input_registers, vertex.input_predicates, vertex.initial_output_attributes,
        fragment.input_registers, fragment.input_predicates
    );
    ensure!(
        (0..4).all(|r| fragment.defined_registers_on_exit.contains(&r)),
        "pixel color registers are not assigned on every EXIT"
    );
    for &offset in &vertex.input_attributes {
        ensure!(
            vertex_header.vtg_attribute_enabled(offset, false) == Some(true),
            "vertex input {offset:#x} is absent from the supported hardware input map"
        );
    }
    let require_output = |offset| -> Result<()> {
        ensure!(
            vertex.defined_attributes_on_exit.contains(&offset),
            "vertex output {offset:#x} is not assigned on every EXIT"
        );
        ensure!(
            vertex_header.vtg_attribute_enabled(offset, true) == Some(true),
            "vertex output {offset:#x} is absent from the supported hardware output map"
        );
        Ok(())
    };
    for offset in [0x70, 0x74, 0x78, 0x7c] {
        require_output(offset)?;
    }

    let mut varyings = String::new();
    let mut vertex_values = String::new();
    let mut fragment_values = Vec::new();
    let mut fragment_setup = String::new();
    let mut gradient_values = vec![String::new(); fragment.interpolation_gradient_slots.len()];
    let mut system_planes = Vec::new();
    let mut uses_front_face = false;
    let mut location = 0;
    for (slot, read) in fragment.interpolant_reads.iter().enumerate() {
        let gradient_slot = fragment.interpolation_gradient_slots.iter().position(|&v| v == slot);
        if read.byte_offset == 0x3fc {
            // uam SV_FACE and NAK_ATTR_FRONT_FACE. NAK explicitly excludes
            // this system attribute from the SPH input masks. uam's F32 face
            // conversion is -(raw | 1), mapping all-ones to +1 and zero to -1.
            ensure!(
                read.mode == InterpolationMode::Constant
                    && read.location == InterpolationLocation::Default
                    && gradient_slot.is_none(),
                "front-face input requires ordinary flat interpolation"
            );
            uses_front_face = true;
            fragment_values.push("select(0u,0xffffffffu,mpr_front_facing)".into());
            continue;
        }
        if (0x70..0x80).contains(&read.byte_offset) {
            ensure!(
                read.mode == InterpolationMode::Pass
                    && read.location != InterpolationLocation::Centroid,
                "system interpolation requires direct center or offset PASS inputs"
            );
            ensure!(
                io.system_inputs_ab & (1 << (read.byte_offset / 4)) != 0,
                "system position input is absent from SPH"
            );
            let component = ["x", "y", "z", "w"][usize::from((read.byte_offset - 0x70) / 4)];
            let value = if read.byte_offset < 0x78 {
                if let Some(index) = gradient_slot {
                    gradient_values[index] =
                        if component == "x" { "vec2<f32>(1.0,0.0)" } else { "vec2<f32>(0.0,1.0)" }
                            .into();
                }
                format!("(floor(input.position.{component}) + 0.5)")
            } else if per_sample || gradient_slot.is_some() {
                if !system_planes.contains(&read.byte_offset) {
                    // Builtin depth/reciprocal W are screen-linear, including the
                    // actual viewport depth transform. Recenter a sample value
                    // with its derivatives instead of assuming sample XY=center.
                    writeln!(fragment_setup, "let mpr_gradient_{component}=vec2<f32>(dpdx(input.position.{component}),dpdy(input.position.{component}));")?;
                    let center = if per_sample {
                        format!("input.position.{component} + dot(mpr_gradient_{component}, floor(input.position.xy) + vec2<f32>(0.5) - input.position.xy)")
                    } else {
                        format!("input.position.{component}")
                    };
                    writeln!(fragment_setup, "let mpr_center_{component}={center};")?;
                    system_planes.push(read.byte_offset);
                }
                if let Some(index) = gradient_slot {
                    gradient_values[index] = format!("mpr_gradient_{component}");
                }
                format!("mpr_center_{component}")
            } else {
                format!("input.position.{component}")
            };
            fragment_values.push(format!("bitcast<u32>({value})"));
            continue;
        }
        let header_mode = io
            .inputs
            .iter()
            .find(|v| v.byte_offset == read.byte_offset)
            .context("interpolated input is absent from SPH")?
            .mode;
        ensure!(
            (0x80..0x280).contains(&read.byte_offset),
            "graphics wrapper requires generic varyings"
        );
        require_output(read.byte_offset)?;
        let qualifier = match read.location {
            InterpolationLocation::Default | InterpolationLocation::Offset => "center",
            InterpolationLocation::Centroid => {
                if per_sample {
                    "sample"
                } else {
                    "centroid"
                }
            }
        };
        let value = format!("state.attributes[{}]", read.byte_offset / 4);
        match (header_mode, read.mode) {
            (PixelInputMode::Constant, InterpolationMode::Constant) => {
                writeln!(varyings, "@location({location}) @interpolate(flat) value{slot}:u32,")?;
                writeln!(vertex_values, "output.value{slot}={value};")?;
                fragment_values.push(format!("input.value{slot}"));
            }
            (
                PixelInputMode::Perspective,
                InterpolationMode::Perspective | InterpolationMode::Pass,
            )
            | (PixelInputMode::ScreenLinear, InterpolationMode::Pass) => {
                writeln!(
                    varyings,
                    "@location({location}) @interpolate(linear,{qualifier}) value{slot}:f32,"
                )?;
                let divide = if header_mode == PixelInputMode::Perspective {
                    " / output.position.w"
                } else {
                    ""
                };
                writeln!(vertex_values, "output.value{slot}=bitcast<f32>({value}){divide};")?;
                fragment_values.push(format!("bitcast<u32>(input.value{slot})"));
            }
            _ => anyhow::bail!("unsupported combination of SPH and IPA interpolation modes"),
        }
        if let Some(index) = gradient_slot {
            gradient_values[index] =
                format!("vec2<f32>(dpdx(input.value{slot}),dpdy(input.value{slot}))");
        }
        location += 1;
    }
    if fragment_values.is_empty() {
        fragment_values.push("0u".into());
    }
    let constant_args = |kernel: &ProgramKernel, binding: &str| {
        if kernel.constant_reads.is_empty() {
            "0u".into()
        } else {
            (0..kernel.constant_reads.len())
                .map(|i| format!("{binding}[{i}]"))
                .collect::<Vec<_>>()
                .join(",")
        }
    };
    let vertex_constants = constant_args(&vertex, "mpr_vertex_constants");
    let fragment_constants = constant_args(&fragment, "mpr_fragment_constants");
    let nc_vertex = vertex.constant_reads.len().max(1);
    let nc_fragment = fragment.constant_reads.len().max(1);
    let ni_fragment = fragment_values.len();
    let vertex_memory_arg = if vertex.uses_memory_status { ", &memory_status" } else { "" };
    let fragment_memory_arg = if fragment.uses_memory_status { ", &memory_status" } else { "" };
    let fragment_sample_arg = if fragment.uses_sample_index { ", invocation_sample" } else { "" };
    let fragment_kill_arg = if fragment.uses_fragment_kill { ", &fragment_killed" } else { "" };
    ensure!(
        gradient_values.iter().all(|v| !v.is_empty()),
        "missing interpolation gradient provider"
    );
    let gradient_arg = if gradient_values.is_empty() {
        String::new()
    } else {
        format!(", array<vec2<f32>,{}>({})", gradient_values.len(), gradient_values.join(","))
    };
    let entry_sample_arg =
        if per_sample { ", @builtin(sample_index) invocation_sample:u32" } else { "" };
    let entry_face_arg =
        if uses_front_face { ", @builtin(front_facing) mpr_front_facing:bool" } else { "" };
    let output_declaration = if per_sample {
        "struct MprFragmentOutput { @location(0) color:vec4<f32>, @builtin(sample_mask) mask:u32 };"
    } else {
        ""
    };
    let output_type = if per_sample { "MprFragmentOutput" } else { "@location(0) vec4<f32>" };
    let return_color = if per_sample {
        "return MprFragmentOutput(color,1u << invocation_sample);"
    } else {
        "return color;"
    };
    // Prefix each standalone kernel's private functions/types for one module.
    let mut wgsl = vertex.wgsl.replace("mpr_", "vs_").replace("Mpr", "Vs");
    wgsl.push_str(&fragment.wgsl.replace("mpr_", "ps_").replace("Mpr", "Ps"));
    writeln!(
        wgsl,
        r#"
struct MprVertexInput {{ words:array<u32,256> }};
@group(0) @binding(0) var<storage,read> mpr_vertices:array<MprVertexInput>;
@group(0) @binding(1) var<storage,read> mpr_vertex_constants:array<u32>;
@group(0) @binding(2) var<storage,read> mpr_fragment_constants:array<u32>;
struct MprVaryings {{ @builtin(position) position:vec4<f32>, {varyings} }};
@vertex fn mpr_vertex_main(@builtin(vertex_index) index:u32)->MprVaryings {{
    var initial:VsVertexState;
    var memory_status=0u;
    let state=vs_vertex(initial,mpr_vertices[index].words,array<u32,{nc_vertex}>({vertex_constants}){vertex_memory_arg});
    var output:MprVaryings;
    output.position=vec4<f32>(bitcast<f32>(state.attributes[28]),bitcast<f32>(state.attributes[29]),
        bitcast<f32>(state.attributes[30]),bitcast<f32>(state.attributes[31]));
    if (memory_status!=0u) {{ output.position=vec4<f32>(0.0,0.0,2.0,1.0); }}
    {vertex_values}
    return output;
}}
{output_declaration}
@fragment fn mpr_fragment_main(input:MprVaryings{entry_sample_arg}{entry_face_arg})->{output_type} {{
    var initial:PsFragmentState;
    var memory_status=0u;
    var fragment_killed=0u;
    {fragment_setup}
    let state=ps_fragment(initial,array<u32,{ni_fragment}>({}),array<u32,{nc_fragment}>({fragment_constants}){fragment_sample_arg}{fragment_memory_arg}{gradient_arg}{fragment_kill_arg});
    if (memory_status!=0u || fragment_killed!=0u) {{ discard; }}
    let color=vec4<f32>(bitcast<f32>(state.registers[0]),bitcast<f32>(state.registers[1]),
        bitcast<f32>(state.registers[2]),bitcast<f32>(state.registers[3]));
    {return_color}
}}
"#,
        fragment_values.join(",")
    )?;
    Ok(GraphicsKernel { wgsl, vertex, fragment, fragment_frequency })
}
