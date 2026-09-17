//! Connect resolved NVN material bindings to the semantic compiler.

use anyhow::{Context, Result};
use remax::{
    maxwell::graphics::{
        self, ColorOutputRouting, FragmentFrequency, GraphicsProgram, QuadFaultMethod, StageSource,
    },
    program::wgsl::{self, texture::Binding},
};

use super::ModelShaderBindings;

/// Numeric permissions, analysis profile and draw state for a material translation.
/// Constant and texture slots are assigned by this adapter, not by these options.
#[derive(Clone, Copy, Debug, Default)]
pub struct TranslationOptions {
    pub vertex: wgsl::Options,
    pub fragment: wgsl::Options,
    pub quad_faults: Option<QuadFaultMethod>,
    /// Resolve from the effective blend state after technique, mesh draw context
    /// and material overrides have been merged. Shader output count is insufficient.
    pub color_routing: ColorOutputRouting,
}

/// Translate the selected native stages without specializing on uniform values.
/// Group 0 contains vertices, vertex/fragment constants and faults at slots 0–3.
/// Groups 2/3 contain vertex/fragment textures; group 1 is unused. Every constant
/// upload must satisfy the returned kernel's `constant_preconditions` contract.
/// Numeric permissions and helper participation are the caller's responsibility.
pub fn translate(
    bindings: &ModelShaderBindings,
    options: TranslationOptions,
) -> Result<GraphicsProgram> {
    let [vertex, fragment] = bindings.programs()?;
    let frequency = bindings.sample_shading().frequency(
        fragment.control.fragment_sample_shading.context("missing native fragment frequency")?,
    );
    let frequency = match frequency {
        remax::wgsl::graphics::FragmentFrequency::PerFragment => FragmentFrequency::Fragment,
        remax::wgsl::graphics::FragmentFrequency::PerSample => FragmentFrequency::Sample,
    };
    let mut graphics = graphics::Options::new(frequency);
    graphics.vertex =
        wgsl::Options { constant_memory: graphics.vertex.constant_memory, ..options.vertex };
    graphics.fragment =
        wgsl::Options { constant_memory: graphics.fragment.constant_memory, ..options.fragment };
    graphics.quad_faults = options.quad_faults;
    graphics.color_routing = options.color_routing;
    // The shipped NVN SDK initializes SET_ATTRIBUTE_DEFAULT to 0xe: missing
    // generic output-map components use (0, 0, 0, 1). See docs/shaders.md.
    graphics.generic_attribute_defaults = Some(graphics::GenericAttributeDefaults::ZeroOne);
    // The SDK initializes USER_CLIP_ENABLE to 0xff. Only distances declared by
    // the vertex SPH participate; this is NVN state, not a compiler default.
    graphics.clip_distance_mask = Some(0xff);
    let textures = |stage| {
        bindings
            .textures
            .iter()
            .filter(|texture| texture.stage == stage)
            .enumerate()
            .map(|(index, texture)| Binding {
                view: texture.binding,
                group: stage as u32 + 2,
                texture_binding: index as u32 * 2,
                sampler_binding: Some(index as u32 * 2 + 1),
                depth_components: None,
            })
            .collect::<Vec<_>>()
    };
    Ok(graphics::translate(
        StageSource { code: vertex.code, header: vertex.header.as_ref().context("vertex SPH")? },
        StageSource {
            code: fragment.code,
            header: fragment.header.as_ref().context("fragment SPH")?,
        },
        graphics,
        &textures(0),
        &textures(1),
    )?)
}
