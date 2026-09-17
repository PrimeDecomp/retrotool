use std::collections::BTreeMap;

use super::{invalid, Binding, DepthComponent, Resource, TextureValueType};
use crate::{
    ir::texture::{FetchLevel, Kind, Operation, SampleLevel},
    maxwell::texture::TextureDimension,
    program::{
        wgsl::{
            emit::{name, Emitter},
            FaultScope,
        },
        Diagnostic,
    },
};

fn texture_name(binding: &Binding) -> String {
    format!("remax_texture_{}_{}", binding.group, binding.texture_binding)
}
fn sampler_name(binding: &Binding) -> String {
    format!("remax_sampler_{}_{}", binding.group, binding.sampler_binding.unwrap())
}

pub(in crate::program::wgsl) fn declarations(
    resources: &[Resource],
    buffer_bindings: &[(u32, u32)],
) -> Result<String, Diagnostic> {
    let mut declarations = BTreeMap::<(u32, u32), String>::new();
    for &binding in buffer_bindings {
        if declarations.insert(binding, String::new()).is_some() {
            return Err(invalid(None, "incompatible buffers share a WGSL binding"));
        }
    }
    let mut declare = |key, text: String| -> Result<(), Diagnostic> {
        if let Some(old) = declarations.get(&key) {
            if old != &text {
                return Err(invalid(None, "incompatible resources share a WGSL binding"));
            }
        } else {
            declarations.insert(key, text);
        }
        Ok(())
    };
    for resource in resources {
        let binding = &resource.binding;
        let dimension = match binding.view.dimension {
            TextureDimension::D1 => "1d",
            TextureDimension::D2 => "2d",
            TextureDimension::D2Array => "2d_array",
            TextureDimension::D3 => "3d",
            TextureDimension::Cube => "cube",
        };
        let class = if binding.view.multisample { "multisampled_" } else { "" };
        let shader_type = match binding.view.value_type {
            TextureValueType::Depth => format!("texture_depth_{class}{dimension}"),
            value_type => {
                let scalar = match value_type {
                    TextureValueType::Float => "f32",
                    TextureValueType::Uint => "u32",
                    TextureValueType::Sint => "i32",
                    TextureValueType::Depth => unreachable!(),
                };
                format!("texture_{class}{dimension}<{scalar}>")
            }
        };
        declare(
            (binding.group, binding.texture_binding),
            format!(
                "@group({}) @binding({}) var {}: {shader_type};",
                binding.group,
                binding.texture_binding,
                texture_name(binding)
            ),
        )?;
        if resource.uses_sampler {
            declare(
                (binding.group, binding.sampler_binding.unwrap()),
                format!(
                    "@group({}) @binding({}) var {}: {};",
                    binding.group,
                    binding.sampler_binding.unwrap(),
                    sampler_name(binding),
                    if resource.comparison_sampler { "sampler_comparison" } else { "sampler" }
                ),
            )?;
        }
    }
    Ok(declarations.values().cloned().collect::<Vec<_>>().join("\n"))
}

fn sample_bits(value: crate::ir::ValueId, recover: bool) -> String {
    let value = name(value);
    if recover {
        format!("select(0u, {value}, remax_finite({value}))")
    } else {
        value
    }
}

fn coordinates(operation: &Operation, shader_type: &str, recover: bool) -> String {
    let words = operation.coordinates.iter().map(|&v| sample_bits(v, recover)).collect::<Vec<_>>();
    if words.len() == 1 {
        format!("bitcast<{shader_type}>({})", words[0])
    } else {
        format!(
            "bitcast<vec{}<{shader_type}>>(vec{}<u32>({}))",
            words.len(),
            words.len(),
            words.join(", ")
        )
    }
}
fn depth_result(binding: &Binding, scalar: &str) -> String {
    let components = binding.depth_components.expect("validated depth mapping").map(|c| match c {
        DepthComponent::Depth => format!("bitcast<u32>({scalar})"),
        DepthComponent::Zero => "0u".into(),
        DepthComponent::One => "1065353216u".into(),
    });
    format!("vec4<u32>({})", components.join(", "))
}

impl Emitter<'_> {
    pub(in crate::program::wgsl) fn texture(&mut self, operation: &Operation, pc: usize) -> String {
        let resource = self
            .kernel
            .textures
            .iter()
            .find(|resource| resource.binding.view.texture_index == operation.descriptor)
            .expect("planned texture")
            .clone();
        let binding = &resource.binding;
        let texture = texture_name(binding);
        if let Some(properties) = binding.view.sampling {
            if !binding.view.multisample {
                self.fault(
                    &format!("u32(textureNumLevels({texture})) != {}u", properties.mip_level_count),
                    "128u",
                    pc,
                );
            }
        }
        match operation.kind {
            Kind::Sample { .. } => self.sample_texture(operation, binding, pc),
            Kind::Load { .. } => self.load_texture(operation, binding, pc),
        }
    }

    fn sample_texture(&mut self, operation: &Operation, binding: &Binding, pc: usize) -> String {
        let Kind::Sample { level, comparison } = operation.kind else {
            unreachable!("sample operation dispatched by texture")
        };
        let texture = texture_name(binding);
        let recover = self.kernel.fault_scope == FaultScope::Quad;
        let finite = operation
            .coordinates
            .iter()
            .copied()
            .chain(comparison)
            .chain(match level {
                SampleLevel::Explicit(v) => Some(v),
                _ => None,
            })
            .map(|v| format!("remax_finite({})", name(v)))
            .collect::<Vec<_>>()
            .join(" && ");
        self.fault(&format!("!({finite})"), "256u", pc);
        if operation.dimension == TextureDimension::Cube {
            self.fault(
                &operation
                    .coordinates
                    .iter()
                    .map(|&v| format!("({} & 0x7fffffffu) == 0u", name(v)))
                    .collect::<Vec<_>>()
                    .join(" && "),
                "256u",
                pc,
            );
        }
        // Faulting lanes must still participate, but invalid operands
        // must not reach host texture operations. These substitutions
        // do not make the affected quad's result valid.
        self.line(&format!("var coords = {};", coordinates(operation, "f32", recover)));
        if recover && operation.dimension == TextureDimension::Cube {
            self.line("if (all(coords == vec3<f32>(0.0))) {");
            self.line("    coords.z = 1.0;");
            self.line("}");
        }
        let mut arguments = vec![texture.clone(), sampler_name(binding), "coords".into()];
        if binding.view.dimension == TextureDimension::D2Array {
            // Filtered sampling clamps the unsigned layer to the view range;
            // it is not a bounds fault. A non-array instruction selects zero.
            // A D2 host view has only one layer, so it needs no layer argument.
            let layer = operation.layer.map_or_else(|| "0u".into(), name);
            arguments.push(format!("i32(min({layer}, u32(textureNumLayers({texture})) - 1u))"));
        }
        if let Some(reference) = comparison {
            arguments.push(format!("bitcast<f32>({})", sample_bits(reference, recover)));
        }
        let function = match (comparison.is_some(), level) {
            (true, SampleLevel::Implicit) => "textureSampleCompare",
            (true, SampleLevel::Explicit(_)) => "textureSampleCompareLevel",
            (false, SampleLevel::Implicit) if binding.view.implicit_lod_is_fixed() => {
                arguments.push("0.0".into());
                "textureSampleLevel"
            }
            (false, SampleLevel::Implicit)
                if binding.view.value_type == TextureValueType::Depth =>
            {
                "textureSample"
            }
            (false, SampleLevel::Implicit) => {
                arguments.push(format!("{:e}", binding.view.lod_bias));
                "textureSampleBias"
            }
            (false, SampleLevel::Explicit(v)) => {
                arguments.push(if binding.view.value_type == TextureValueType::Depth {
                    "0i".into()
                } else {
                    format!("bitcast<f32>({})", sample_bits(v, recover))
                });
                "textureSampleLevel"
            }
        };
        self.line(&format!("let texel = {function}({});", arguments.join(", ")));
        if comparison.is_some() {
            let bits = "bitcast<u32>(texel)";
            format!("vec4<u32>({bits}, {bits}, {bits}, 1065353216u)")
        } else if binding.view.value_type == TextureValueType::Depth {
            depth_result(binding, "texel")
        } else {
            "bitcast<vec4<u32>>(texel)".into()
        }
    }

    fn load_texture(&mut self, operation: &Operation, binding: &Binding, pc: usize) -> String {
        let Kind::Load { level, offset } = operation.kind else {
            unreachable!("load operation dispatched by texture")
        };
        let texture = texture_name(binding);
        let coordinate_expression = coordinates(operation, "i32", false);
        let coordinate_expression = if let Some([x, y]) = offset {
            let offset = format!("vec2<u32>({}, {})", name(x), name(y));
            let sum = format!("bitcast<vec2<u32>>({coordinate_expression}) + {offset}");
            format!("bitcast<vec2<i32>>({sum})")
        } else {
            coordinate_expression
        };
        let (FetchLevel::Mip(value) | FetchLevel::Sample(value)) = level;
        self.line(&format!("let coords = {coordinate_expression};"));
        self.line(&format!("let level = bitcast<i32>({});", name(value)));
        self.line("var texel_bits = vec4<u32>(0u);");
        let count = if binding.view.multisample {
            format!("textureNumSamples({texture})")
        } else {
            format!("textureNumLevels({texture})")
        };
        let level_ok = format!("level >= 0 && u32(level) < u32({count})");
        self.fault(&format!("!({level_ok})"), "128u", pc);
        self.line(&format!("if ({level_ok}) {{"));
        let query =
            if binding.view.multisample { texture.clone() } else { format!("{texture}, level") };
        let dimensions = operation.coordinates.len();
        let (size, bounds) = if dimensions == 1 {
            ("u32".into(), "coords >= 0 && u32(coords) < size".into())
        } else {
            let nonnegative = format!("all(coords >= vec{dimensions}<i32>(0))");
            let within_size = format!("all(vec{dimensions}<u32>(coords) < size)");
            (format!("vec{dimensions}<u32>"), format!("{nonnegative} && {within_size}"))
        };
        // Query casts bridge old/new WGSL result types. Removal criteria:
        // docs/rendering-compatibility.md#texture-query-types.
        self.line(&format!("let size = {size}(textureDimensions({query}));"));
        let mut arguments = vec![texture, "coords".into()];
        let mut bounds: String = bounds;
        if let Some(layer) = operation.layer {
            let layer = name(layer);
            bounds += &format!(" && {layer} < u32(textureNumLayers({}))", arguments[0]);
            arguments.push(format!("i32({layer})"));
        }
        arguments.push("level".into());
        self.fault(&format!("!({bounds})"), "128u", pc);
        self.line(&format!("if ({bounds}) {{"));
        self.line(&format!("let texel = textureLoad({});", arguments.join(", ")));
        let result = match binding.view.value_type {
            TextureValueType::Uint => "texel".into(),
            TextureValueType::Float | TextureValueType::Sint => "bitcast<vec4<u32>>(texel)".into(),
            TextureValueType::Depth => depth_result(binding, "texel"),
        };
        self.line(&format!("texel_bits = {result};"));
        self.line("}");
        self.line("}");
        "texel_bits".into()
    }
}
