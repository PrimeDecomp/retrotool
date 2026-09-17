use std::fmt::Write;

use crate::{
    maxwell::{
        graphics::{ColorOutputRouting, FragmentFrequency},
        io::PixelOutputRegisters,
    },
    program::{wgsl::Kernel, Register},
};

pub(super) fn has_result(outputs: &PixelOutputRegisters, frequency: FragmentFrequency) -> bool {
    outputs.registers().next().is_some() || frequency == FragmentFrequency::Sample
}

pub(super) fn declaration(
    source: &mut String,
    outputs: &PixelOutputRegisters,
    frequency: FragmentFrequency,
    color_routing: ColorOutputRouting,
) {
    if !has_result(outputs, frequency) {
        return;
    }
    source.push_str("\nstruct RemaxFragmentOutput {\n");
    for (target, registers) in outputs.colors.iter().enumerate() {
        if registers.iter().any(Option::is_some) {
            match color_routing {
                ColorOutputRouting::Attachments => {
                    writeln!(source, "    @location({target}) color{target}: vec4<f32>,").unwrap();
                }
                ColorOutputRouting::DualSource => {
                    writeln!(
                        source,
                        "    @location(0) @blend_src({target}) color{target}: vec4<f32>,"
                    )
                    .unwrap();
                }
            }
        }
    }
    if outputs.depth.is_some() {
        source.push_str("    @builtin(frag_depth) depth: f32,\n");
    }
    if outputs.sample_mask.is_some() || frequency == FragmentFrequency::Sample {
        source.push_str("    @builtin(sample_mask) sample_mask: u32,\n");
    }
    source.push_str("}\n");
}

fn register_word(kernel: &Kernel, register: u8) -> String {
    let slot = kernel
        .outputs
        .iter()
        .position(|&output| output == Register::Gpr(register))
        .expect("fragment signature includes each exported register");
    format!("result.words[{slot}]")
}

pub(super) fn result(
    source: &mut String,
    kernel: &Kernel,
    outputs: &PixelOutputRegisters,
    frequency: FragmentFrequency,
) {
    if !has_result(outputs, frequency) {
        source.push_str("    return;\n");
        return;
    }
    source.push_str("    var output: RemaxFragmentOutput;\n");
    for (target, registers) in outputs.colors.iter().enumerate() {
        if registers.iter().all(Option::is_none) {
            continue;
        }
        let components = registers.map(|register| {
            let register = register.expect("validated complete RGBA output");
            format!("bitcast<f32>({})", register_word(kernel, register))
        });
        writeln!(source, "    output.color{target} = vec4<f32>({});", components.join(", "))
            .unwrap();
    }
    if let Some(register) = outputs.depth {
        let word = register_word(kernel, register);
        writeln!(source, "    output.depth = bitcast<f32>({word});").unwrap();
    }
    let mask = match (outputs.sample_mask, frequency) {
        (Some(register), FragmentFrequency::Fragment) => Some(register_word(kernel, register)),
        (Some(register), FragmentFrequency::Sample) => {
            Some(format!("{} & (1u << sample_index)", register_word(kernel, register)))
        }
        (None, FragmentFrequency::Sample) => Some("1u << sample_index".into()),
        (None, FragmentFrequency::Fragment) => None,
    };
    if let Some(mask) = mask {
        // Make sample-frequency execution affect only the current sample.
        // A native sample-mask export may suppress it, but cannot enable peers.
        writeln!(source, "    output.sample_mask = {mask};").unwrap();
    }
    source.push_str("    return output;\n");
}
