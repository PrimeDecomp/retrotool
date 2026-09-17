use std::{collections::BTreeSet, fmt::Write};

use super::{
    interface::{Argument, ClipDistances, Interface, PositionComponent, RasterValue, VertexValue},
    ColorOutputRouting, FragmentFrequency, Options, QuadFaultMethod,
};
use crate::{
    maxwell::io::PixelOutputRegisters,
    program::{
        interpolation::Component,
        wgsl::{FaultScope, Kernel},
        Register,
    },
};

mod output;

const COMPONENT_NAMES: [&str; 4] = ["x", "y", "z", "w"];

pub(super) fn vertex(
    kernel: &Kernel,
    interface: &Interface,
    clip_distances: ClipDistances,
    options: Options,
) -> String {
    let mut source = declarations(kernel, interface, options, clip_distances.count());
    write_vertex_inputs(&mut source, kernel.inputs.len().max(1), options);
    source.push_str(
        r#"
@vertex
fn remax_vertex_main(@builtin(vertex_index) index: u32) -> RemaxVaryings {
    var initial: RemaxVertexRecord;
    if (index < arrayLength(&remax_vertices)) {
        initial = remax_vertices[index];
    }

    let result = remax_main(initial.words, initial.valid);
    var output: RemaxVaryings;
    if (result.status != 0u) {
        atomicOr(&remax_draw_faults[0], result.status);
        atomicMin(&remax_draw_faults[1], result.pc);
        output.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return output;
    }
"#,
    );
    write_vertex_outputs(&mut source, kernel, interface);
    for index in 0..clip_distances.count() {
        let value = if clip_distances.mask() & (1 << index) != 0 {
            let word = vertex_output_word(kernel, 0x2c0 + index as u16 * 4);
            format!("bitcast<f32>({word})")
        } else {
            // WGSL enables the whole array. Keep holes neutral instead of
            // observing disabled native exports or their validity state.
            "1.0".into()
        };
        writeln!(source, "    output.clip_distances[{index}] = {value};").unwrap();
    }
    source.push_str("    return output;\n}\n");
    source
}

pub(super) fn fragment(
    kernel: &Kernel,
    interface: &Interface,
    outputs: &PixelOutputRegisters,
    options: Options,
) -> String {
    let mut source = declarations(kernel, interface, options, 0);
    if options.color_routing == ColorOutputRouting::DualSource {
        source.insert_str(0, "enable dual_source_blending;\n");
    }
    if kernel.fault_scope == FaultScope::Quad {
        match options.quad_faults.expect("validated quad fault method") {
            QuadFaultMethod::QuadBroadcast => source.push_str(include_str!("emit/faults.wgsl")),
        }
    }
    output::declaration(&mut source, outputs, options.frequency, options.color_routing);
    write_fragment_signature(&mut source, interface, outputs, options.frequency);

    let planes = fragment_planes(interface, options.frequency);
    write_fragment_planes(&mut source, &planes, options.frequency);
    write_fragment_arguments(&mut source, interface, &planes);
    source.push_str("    let result = remax_main(words, valid);\n");
    let fault = if kernel.fault_scope == FaultScope::Quad {
        "remax_quad_fault(result.status, result.pc)"
    } else {
        "vec2<u32>(result.status, result.pc)"
    };
    writeln!(source, "    let fault = {fault};").unwrap();
    source.push_str(
        r#"
    if (fault.x != 0u) {
        atomicOr(&remax_draw_faults[2], fault.x);
        atomicMin(&remax_draw_faults[3], fault.y);
    }
    if (fault.x != 0u || result.discarded != 0u) {
        discard;
    }

"#,
    );
    output::result(&mut source, kernel, outputs, options.frequency);
    source.push_str("}\n");
    source
}

fn declarations(
    kernel: &Kernel,
    interface: &Interface,
    options: Options,
    clip_distance_count: u32,
) -> String {
    let mut source = kernel.source.clone();
    if clip_distance_count != 0 {
        source.insert_str(0, "enable clip_distances;\n");
    }
    let binding = options.faults;
    writeln!(
        source,
        "@group({}) @binding({}) var<storage, read_write> remax_draw_faults: array<atomic<u32>, 4>;",
        binding.group, binding.binding,
    )
    .unwrap();
    source.push_str("\nstruct RemaxVaryings {\n    @builtin(position) position: vec4<f32>,\n");
    if clip_distance_count != 0 {
        writeln!(
            source,
            "    @builtin(clip_distances) clip_distances: array<f32, {clip_distance_count}>,"
        )
        .unwrap();
    }
    for (location, field) in interface.locations.iter().enumerate() {
        let qualifier = field.interpolation.qualifier();
        let scalar_type = if field.interpolation.is_flat() { "u32" } else { "f32" };
        writeln!(
            source,
            "    @location({location}) @interpolate({qualifier}) v{location}: vec4<{scalar_type}>,",
        )
        .unwrap();
    }
    source.push_str("}\n");
    source
}

fn write_vertex_inputs(source: &mut String, input_count: usize, options: Options) {
    let binding = options.vertex_inputs;
    writeln!(
        source,
        r#"
struct RemaxVertexRecord {{
    words: array<u32, {input_count}>,
    valid: array<u32, {input_count}>,
}}
@group({}) @binding({})
var<storage, read> remax_vertices: array<RemaxVertexRecord>;
"#,
        binding.group, binding.binding,
    )
    .unwrap();
}

fn vertex_output_word(kernel: &Kernel, attribute: u16) -> String {
    let slot = kernel
        .outputs
        .iter()
        .position(|&register| register == Register::OutputAttribute(attribute))
        .expect("linker includes every required output in the vertex signature");
    format!("result.words[{slot}]")
}

fn write_vertex_outputs(source: &mut String, kernel: &Kernel, interface: &Interface) {
    let position = [0x70, 0x74, 0x78, 0x7c]
        .map(|attribute| format!("bitcast<f32>({})", vertex_output_word(kernel, attribute)));
    writeln!(source, "    output.position = vec4<f32>({});", position.join(", ")).unwrap();

    for (location, fields) in interface.locations.iter().enumerate() {
        for (component, field) in fields.fields.iter().enumerate() {
            let word = match field.source {
                VertexValue::Attribute(offset) => vertex_output_word(kernel, offset),
                VertexValue::Default(bits) => format!("{bits}u"),
            };
            let expression = match (fields.interpolation.is_flat(), field.divide_by_clip_w) {
                (true, _) => word,
                (false, true) => format!("bitcast<f32>({word}) / output.position.w"),
                (false, false) => format!("bitcast<f32>({word})"),
            };
            let component = COMPONENT_NAMES[component];
            writeln!(source, "    output.v{location}.{component} = {expression};").unwrap();
        }
    }
}

fn write_fragment_signature(
    source: &mut String,
    interface: &Interface,
    outputs: &PixelOutputRegisters,
    frequency: FragmentFrequency,
) {
    source.push_str("\n@fragment\nfn remax_fragment_main(\n    input: RemaxVaryings,\n");
    if frequency == FragmentFrequency::Sample {
        source.push_str("    @builtin(sample_index) sample_index: u32,\n");
    }
    if interface.front_face {
        source.push_str("    @builtin(front_facing) front_face: bool,\n");
    }
    if output::has_result(outputs, frequency) {
        source.push_str(") -> RemaxFragmentOutput {\n");
    } else {
        source.push_str(") {\n");
    }
}

fn raster_value(value: RasterValue) -> String {
    match value {
        RasterValue::Varying { location, component } => {
            format!("input.v{location}.{}", COMPONENT_NAMES[component])
        }
        RasterValue::Position(component) => format!("input.position.{}", component.name()),
        RasterValue::FrontFace => "select(0u, 0xffffffffu, front_face)".into(),
        RasterValue::SampleIndex => "sample_index".into(),
    }
}

fn fragment_planes(interface: &Interface, frequency: FragmentFrequency) -> BTreeSet<RasterValue> {
    interface
        .arguments
        .iter()
        .filter(|argument| argument.needs_plane(frequency))
        .map(|argument| argument.value)
        .collect()
}

fn write_fragment_planes(
    source: &mut String,
    planes: &BTreeSet<RasterValue>,
    frequency: FragmentFrequency,
) {
    for (index, &value) in planes.iter().enumerate() {
        let expression = raster_value(value);
        let gradient = match value {
            RasterValue::Position(PositionComponent::X) => "vec2<f32>(1.0, 0.0)".into(),
            RasterValue::Position(PositionComponent::Y) => "vec2<f32>(0.0, 1.0)".into(),
            _ => format!("vec2<f32>(dpdx({expression}), dpdy({expression}))"),
        };
        writeln!(source, "    let gradient{index} = {gradient};").unwrap();
        let center = match value {
            RasterValue::Position(component) if component.is_xy() => {
                format!("floor({expression}) + 0.5")
            }
            RasterValue::Position(_) if frequency == FragmentFrequency::Sample => {
                format!("{expression} + dot(gradient{index}, floor(input.position.xy) + vec2<f32>(0.5) - input.position.xy)")
            }
            _ => expression,
        };
        writeln!(source, "    let center{index} = {center};").unwrap();
    }
}

fn fragment_argument(argument: &Argument, planes: &BTreeSet<RasterValue>) -> String {
    if let Some(index) = planes.iter().position(|&value| value == argument.value) {
        return match argument.plane_component {
            Some(Component::Dx) => format!("gradient{index}.x"),
            Some(Component::Dy) => format!("gradient{index}.y"),
            _ => format!("center{index}"),
        };
    }
    let expression = raster_value(argument.value);
    if matches!(argument.value, RasterValue::Position(component) if component.is_xy()) {
        format!("floor({expression}) + 0.5")
    } else {
        expression
    }
}

fn write_fragment_arguments(
    source: &mut String,
    interface: &Interface,
    planes: &BTreeSet<RasterValue>,
) {
    let input_count = interface.arguments.len().max(1);
    writeln!(source, "    var words: array<u32, {input_count}>;").unwrap();
    writeln!(source, "    var valid: array<u32, {input_count}>;").unwrap();
    for (slot, argument) in interface.arguments.iter().enumerate() {
        let expression = fragment_argument(argument, planes);
        let preserves_bits = match argument.value {
            RasterValue::FrontFace | RasterValue::SampleIndex => true,
            RasterValue::Varying { location, .. } => {
                interface.locations[location].interpolation.is_flat()
            }
            RasterValue::Position(_) => false,
        };
        let word = if preserves_bits { expression } else { format!("bitcast<u32>({expression})") };
        writeln!(source, "    words[{slot}] = {word};").unwrap();
        writeln!(source, "    valid[{slot}] = 1u;").unwrap();
    }
}
