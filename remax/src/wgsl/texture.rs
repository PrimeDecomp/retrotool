//! Explicit host texture bindings for translated TEXS/TLDS instructions.
//! The shader word supplies dimensions and operands, but the bound descriptor
//! supplies the result type. The caller must declare it instead of guessing from
//! register use. Sampler filtering, wraps, mip clamps and texture views are also
//! supplied by the caller; implicit LOD bias is explicit because WGSL samplers
//! have no bias field. Unsupported combinations return errors.

use anyhow::{ensure, Context, Result};
use serde_derive::{Deserialize, Serialize};

use crate::maxwell::texture::{Texs, TextureDimension, TextureLod, Tlds};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TextureValueType {
    Float,
    Uint,
    Sint,
    Depth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TextureFilter {
    Nearest,
    Linear,
}

/// Properties of the actual bound view and effective sampler, not the shader.
/// A host must validate these when creating/replacing resources. Unknown view
/// ranges must omit this record rather than assume a single mip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureSampling {
    pub mip_level_count: u32,
    pub min_filter: TextureFilter,
    pub mag_filter: TextureFilter,
    /// One means anisotropy is disabled; hosts with a separate enable flag must
    /// turn it off, rather than enable anisotropy with a maximum of one.
    pub max_anisotropy: u8,
}

impl TextureSampling {
    /// Check view and sampler properties supported by the current backend.
    pub fn validate(self) -> Result<()> {
        ensure!(self.mip_level_count > 0, "texture view must contain at least one mip level");
        ensure!(
            matches!(self.max_anisotropy, 1 | 2 | 4 | 8 | 16),
            "unsupported texture sampling anisotropy"
        );
        Ok(())
    }

    fn fixed_level_zero(self) -> bool {
        self.mip_level_count == 1 && self.min_filter == self.mag_filter && self.max_anisotropy == 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextureBinding {
    /// Raw bound descriptor-table word offset, not an NVN API texture slot.
    pub texture_index: u16,
    pub dimension: TextureDimension,
    pub value_type: TextureValueType,
    #[serde(default)]
    pub multisample: bool,
    /// Sampler LOD bias used for implicit sampling. Explicit LOD omits bias.
    #[serde(default)]
    pub lod_bias: f32,
    #[serde(default)]
    pub sampling: Option<TextureSampling>,
}

impl TextureBinding {
    /// For ordinary floating-point sampling, a one-level view with matching
    /// min/mag filters and no anisotropy needs no derivative-dependent choices.
    /// Comparison and integer sampling retain their separate provider rules.
    pub fn implicit_lod_is_fixed(self) -> bool {
        !self.multisample
            && self.value_type == TextureValueType::Float
            && self.sampling.is_some_and(TextureSampling::fixed_level_zero)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextureResource {
    pub binding: TextureBinding,
    pub group: u32,
    pub texture_binding: u32,
    pub sampler_binding: Option<u32>,
    pub uses_implicit_lod: bool,
    /// The original instruction uses implicit LOD, but known resource properties
    /// permit sampling at view-relative level zero without derivatives.
    pub uses_fixed_implicit_lod: bool,
}

#[derive(Default)]
pub(crate) struct TextureArguments {
    pub coordinates: Vec<String>,
    pub layer: Option<String>,
    pub lod: Option<String>,
    pub comparison: Option<String>,
    pub offset: Option<String>,
    pub sample: Option<String>,
}

impl TextureResource {
    pub(crate) fn declarations(&self, slot: usize) -> String {
        let dim = dimension_name(self.binding.dimension);
        let ty = match (self.binding.value_type, self.binding.multisample) {
            (TextureValueType::Depth, true) => format!("texture_depth_multisampled_{dim}"),
            (TextureValueType::Depth, false) => format!("texture_depth_{dim}"),
            (value_type, multisample) => format!(
                "texture_{}{dim}<{}>",
                if multisample { "multisampled_" } else { "" },
                match value_type {
                    TextureValueType::Float => "f32",
                    TextureValueType::Uint => "u32",
                    TextureValueType::Sint => "i32",
                    TextureValueType::Depth => unreachable!(),
                }
            ),
        };
        let mut text = format!(
            "@group({}) @binding({}) var mpr_texture_{slot}:{ty};\n",
            self.group, self.texture_binding
        );
        if let Some(binding) = self.sampler_binding {
            text += &format!(
                "@group({}) @binding({binding}) var mpr_sampler_{slot}:{};\n",
                self.group,
                if self.binding.value_type == TextureValueType::Depth {
                    "sampler_comparison"
                } else {
                    "sampler"
                }
            );
        }
        text
    }
}

fn dimension_name(d: TextureDimension) -> &'static str {
    match d {
        TextureDimension::D1 => "1d",
        TextureDimension::D2 => "2d",
        TextureDimension::D2Array => "2d_array",
        TextureDimension::D3 => "3d",
        TextureDimension::Cube => "cube",
    }
}

#[allow(clippy::too_many_arguments)] // Existing texture-emission state and instruction operands.
pub(crate) fn resource(
    bindings: &[TextureBinding],
    resources: &mut Vec<TextureResource>,
    texture_index: u16,
    dimension: TextureDimension,
    multisample: bool,
    sampled: bool,
    implicit: bool,
    fragment: bool,
) -> Result<usize> {
    let mut matching = bindings.iter().filter(|b| b.texture_index == texture_index);
    let binding =
        *matching.next().context("texture instruction requires a declared host binding")?;
    ensure!(matching.next().is_none(), "duplicate texture binding declaration");
    ensure!(binding.dimension == dimension, "texture binding dimension does not match instruction");
    ensure!(binding.multisample == multisample, "texture binding sample count class mismatch");
    ensure!(
        binding.lod_bias.is_finite() && (-16.0..16.0).contains(&binding.lod_bias),
        "texture LOD bias is outside the supported WGSL range"
    );
    if let Some(sampling) = binding.sampling {
        sampling.validate()?;
    }
    ensure!(
        !multisample || dimension == TextureDimension::D2,
        "multisampled host textures require 2D"
    );
    ensure!(
        binding.value_type != TextureValueType::Depth
            || matches!(dimension, TextureDimension::D2 | TextureDimension::D2Array),
        "unsupported depth texture dimension"
    );
    let slot = if let Some(slot) =
        resources.iter().position(|r| r.binding.texture_index == texture_index)
    {
        slot
    } else {
        let slot = resources.len();
        resources.push(TextureResource {
            binding,
            group: if fragment { 3 } else { 2 },
            texture_binding: slot as u32 * 2,
            sampler_binding: None,
            uses_implicit_lod: false,
            uses_fixed_implicit_lod: false,
        });
        slot
    };
    if sampled {
        resources[slot].sampler_binding = Some(slot as u32 * 2 + 1);
    }
    resources[slot].uses_implicit_lod |= implicit;
    resources[slot].uses_fixed_implicit_lod |= implicit && binding.implicit_lod_is_fixed();
    Ok(slot)
}

fn coordinates(args: &TextureArguments, float: bool) -> String {
    let ty = if float { "f32" } else { "i32" };
    let words = args.coordinates.iter().map(|v| format!("bitcast<{ty}>({v})")).collect::<Vec<_>>();
    if words.len() == 1 {
        words[0].clone()
    } else {
        format!("vec{}<{ty}>({})", words.len(), words.join(","))
    }
}

/// Returns a vec4<u32> expression. Capture it before writing any destination.
pub(crate) fn sample_call(
    t: Texs,
    args: &TextureArguments,
    slot: usize,
    resource: &TextureResource,
) -> Result<String> {
    let mode = t.mode.context("unknown TEXS mode")?;
    ensure!(
        mode.dimension != TextureDimension::D1,
        "WGSL filtered 1D texture sampling requires a separate provider"
    );
    let depth = resource.binding.value_type == TextureValueType::Depth;
    ensure!(depth == mode.depth_compare, "texture comparison mode does not match declared type");
    ensure!(
        depth || resource.binding.value_type == TextureValueType::Float,
        "integer TEXS sampling requires an integer sampler provider"
    );
    ensure!(
        !depth || t.component_mask == Some(1),
        "depth comparison supports the verified R result only"
    );
    let mut call_args =
        vec![format!("mpr_texture_{slot}"), format!("mpr_sampler_{slot}"), coordinates(args, true)];
    if let Some(layer) = &args.layer {
        call_args.push(format!("i32({layer} & 0xffffu)"));
    }
    if let Some(reference) = &args.comparison {
        call_args.push(format!("bitcast<f32>({reference})"));
    }
    let lod = if mode.lod == TextureLod::Implicit && resource.binding.implicit_lod_is_fixed() {
        TextureLod::Zero
    } else {
        mode.lod
    };
    let function = match (depth, lod) {
        (true, TextureLod::Implicit) => {
            ensure!(
                resource.binding.lod_bias == 0.0,
                "implicit comparison LOD bias requires a separate provider"
            );
            "textureSampleCompare"
        }
        (true, TextureLod::Zero) => "textureSampleCompareLevel",
        (true, TextureLod::Explicit) => {
            anyhow::bail!("explicit comparison LOD requires a separate provider")
        }
        (false, TextureLod::Implicit) => {
            call_args.push(format!("{:e}", resource.binding.lod_bias));
            "textureSampleBias"
        }
        (false, TextureLod::Zero) => {
            call_args.push("0.0".into());
            "textureSampleLevel"
        }
        (false, TextureLod::Explicit) => {
            call_args.push(format!(
                "bitcast<f32>({})",
                args.lod.as_ref().context("missing texture LOD")?
            ));
            "textureSampleLevel"
        }
    };
    let call = format!("{function}({})", call_args.join(","));
    Ok(if depth {
        format!("vec4<u32>(bitcast<u32>({call}),0u,0u,0u)")
    } else {
        format!("bitcast<vec4<u32>>({call})")
    })
}

/// Emits a fetch into `result`. Out-of-range coordinates/layers/levels/samples
/// return zero explicitly. This is a host fault policy, not native NVN behavior.
pub(crate) fn load_call(
    t: Tlds,
    args: &TextureArguments,
    slot: usize,
    resource: &TextureResource,
) -> Result<String> {
    let mode = t.mode.context("unknown TLDS mode")?;
    let depth = resource.binding.value_type == TextureValueType::Depth;
    ensure!(
        !depth || t.component_mask == Some(1),
        "depth fetch supports the verified R result only"
    );
    let tex = format!("mpr_texture_{slot}");
    let mut coordinate = coordinates(args, false);
    if let Some(offset) = &args.offset {
        coordinate = format!("({coordinate} + vec2<i32>(bitcast<i32>({offset} << 28u) >> 28u, bitcast<i32>({offset} << 24u) >> 28u))");
    }
    let level =
        args.lod.as_ref().map(|v| format!("bitcast<i32>({v})")).unwrap_or_else(|| "0".into());
    let mut text =
        format!("let coords={coordinate};\nlet level={level};\nvar result=vec4<u32>(0u);\n");
    let outer = if mode.multisample {
        let sample = args.sample.as_ref().context("missing texture sample index")?;
        text += &format!("let sample=bitcast<i32>({sample});\n");
        format!("sample>=0 && u32(sample)<u32(textureNumSamples({tex}))")
    } else {
        format!("level>=0 && u32(level)<u32(textureNumLevels({tex}))")
    };
    let dim_args = if mode.multisample { tex.clone() } else { format!("{tex},level") };
    // Naga 0.11 returns signed texture query results; current WGSL returns
    // unsigned results. Explicit conversion supports both versions.
    let size_type = if mode.dimension == TextureDimension::D1 {
        "u32".into()
    } else {
        format!("vec{}<u32>", args.coordinates.len())
    };
    text += &format!("if ({outer}) {{\nlet size={size_type}(textureDimensions({dim_args}));\n");
    let mut bounds = if mode.dimension == TextureDimension::D1 {
        "coords>=0 && u32(coords)<size".into()
    } else {
        format!(
            "all(coords>=vec{}<i32>(0)) && all(vec{}<u32>(coords)<size)",
            args.coordinates.len(),
            args.coordinates.len()
        )
    };
    let mut call_args = vec![tex.clone(), "coords".into()];
    if let Some(layer) = &args.layer {
        text += &format!("let layer={layer} & 0xffffu;\n");
        bounds += &format!(" && layer<u32(textureNumLayers({tex}))");
        call_args.push("i32(layer)".into());
    }
    call_args.push(if mode.multisample { "sample" } else { "level" }.into());
    let call = format!("textureLoad({})", call_args.join(","));
    let value = match resource.binding.value_type {
        TextureValueType::Uint => call,
        TextureValueType::Float | TextureValueType::Sint => format!("bitcast<vec4<u32>>({call})"),
        TextureValueType::Depth => format!("vec4<u32>(bitcast<u32>({call}),0u,0u,0u)"),
    };
    text += &format!("if ({bounds}) {{ result={value}; }}\n}}\n");
    Ok(text)
}
