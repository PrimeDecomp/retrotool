//! Resolve NVN program metadata and runtime frequency for the shader translator.

use anyhow::{ensure, Context, Result};
pub use remax::wgsl::graphics::GraphicsKernel;
use remax::{
    resources::ConstantMemory,
    wgsl::{graphics, texture::TextureBinding, LoweringOptions},
};

use super::{shader_nvn_program::NvnProgram, shader_nvn_samples::NvnSampleShading};

#[derive(Clone, Copy, Debug, Default)]
pub struct GraphicsOptions {
    pub lowering: LoweringOptions,
    /// Match CGraphicsDeviceContext's selected NVN program variant.
    pub sample_shading: NvnSampleShading,
}

/// Generate a graphics program for the supported stage/resource subset. The
/// supplied vertex shader must emit positions in the host's clip-space convention
/// (WGSL depth0..1). No projection/depth/Y adjustment is guessed here. All required
/// outputs must be assigned on every exit; missing inputs or outputs are errors.
pub fn lower_graphics(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
) -> Result<GraphicsKernel> {
    lower_graphics_with_state(vertex, fragment, GraphicsOptions::default())
}

pub fn lower_graphics_with_options(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
    options: LoweringOptions,
) -> Result<GraphicsKernel> {
    lower_graphics_with_state(vertex, fragment, GraphicsOptions {
        lowering: options,
        ..Default::default()
    })
}

/// Resolve shader frequency from its CPU control record and the runtime override.
/// Sample-frequency entry points read sample_index and emit a one-bit output
/// sample mask, preserving current-sample coverage and making the invocation's
/// effect depend on its index (WGSL sample_index semantics). The caller supplies
/// the render target's sample count. Fragment-frequency PIXLD.MY_INDEX remains
/// unsupported because its representative-index behavior is not established.
pub fn lower_graphics_with_state(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
    options: GraphicsOptions,
) -> Result<GraphicsKernel> {
    lower_graphics_with_textures(vertex, fragment, options, &[], &[])
}

/// Texture view types must come from actual bound resources. Stage bindings use
/// separate groups (vertex2, fragment3); their manifests name texture/sampler slots.
pub fn lower_graphics_with_textures(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
    options: GraphicsOptions,
    vertex_textures: &[TextureBinding],
    fragment_textures: &[TextureBinding],
) -> Result<GraphicsKernel> {
    lower_graphics_bound(vertex, fragment, options, vertex_textures, fragment_textures, None)
}

/// Specialize compiler-generated indirect attribute selections from actual known
/// material constants. The returned manifests and GPU guards enforce those values;
/// changing an address selector requires translating the draw again.
pub fn lower_graphics_with_bound_constants(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
    options: GraphicsOptions,
    vertex_textures: &[TextureBinding],
    fragment_textures: &[TextureBinding],
    constants: [&ConstantMemory; 2],
) -> Result<GraphicsKernel> {
    lower_graphics_bound(
        vertex,
        fragment,
        options,
        vertex_textures,
        fragment_textures,
        Some(constants),
    )
}

fn lower_graphics_bound(
    vertex: &NvnProgram<'_>,
    fragment: &NvnProgram<'_>,
    options: GraphicsOptions,
    vertex_textures: &[TextureBinding],
    fragment_textures: &[TextureBinding],
    constants: Option<[&ConstantMemory; 2]>,
) -> Result<GraphicsKernel> {
    ensure!(vertex.stage.kind == 0 && fragment.stage.kind == 1, "expected vertex and pixel stages");
    let vertex_header = vertex.header.as_ref().context("vertex program has no hardware header")?;
    let fragment_header =
        fragment.header.as_ref().context("pixel program has no hardware header")?;
    let fragment_frequency = options.sample_shading.frequency(
        fragment
            .control
            .fragment_sample_shading
            .context("pixel program lacks frequency metadata")?,
    );
    graphics::lower_graphics(
        graphics::GraphicsStage { code: vertex.code, header: vertex_header },
        graphics::GraphicsStage { code: fragment.code, header: fragment_header },
        graphics::GraphicsOptions { lowering: options.lowering, fragment_frequency },
        graphics::GraphicsBindings { vertex_textures, fragment_textures, constants },
    )
}
