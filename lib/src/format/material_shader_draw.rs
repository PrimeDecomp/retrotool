//! Resolve native model bindings and connect them to a shader compiler.
//! Selection, resources and uniform updates live in [`ModelShaderBindings`].
//! [`translate`] uses the semantic compiler; [`ModelShaderDraw`] retains the
//! legacy translation path for comparison during migration.

use anyhow::{Context, Result};
use remax::resources::{ConstantBank, ConstantMemory};

use super::shader_nvn_graphics::{
    lower_graphics_with_bound_constants, GraphicsKernel, GraphicsOptions,
};

mod animation;
mod bindings;
mod frame;
mod translate;

#[cfg(test)]
mod tests;
pub use bindings::{
    ModelShaderBindings, ModelShaderMemory, ModelShaderOptions, ModelShaderTexture,
    ModelShaderTextureSource,
};
pub use frame::ModelShaderFrame;
pub use translate::{translate, TranslationOptions};

#[derive(Clone, Debug)]
pub struct ModelShaderDraw {
    pub kernel: GraphicsKernel,
    pub bindings: ModelShaderBindings,
}

#[derive(Clone, Debug)]
pub struct ModelShaderConstants {
    pub vertex_compact: Vec<u32>,
    pub fragment_compact: Vec<u32>,
    pub vertex_memory: ConstantMemory,
    pub fragment_memory: ConstantMemory,
}

impl ModelShaderDraw {
    /// Compile the resolved pair, specializing only from established material
    /// bytes. Transforms and sample-driver values are supplied at draw time.
    pub fn compile(bindings: ModelShaderBindings) -> Result<Self> {
        let [vertex, fragment] = bindings.programs()?;
        let textures = |stage| {
            bindings
                .textures
                .iter()
                .filter(|t| t.stage == stage)
                .map(|t| t.binding)
                .collect::<Vec<_>>()
        };
        let memory = |stage, bytes| {
            bindings.uniforms.checked_graphics_memory(stage, &[ConstantBank {
                bank: 1,
                bytes,
                known: None,
            }])
        };
        let vertex_memory = memory(0, vertex.constant_data)?;
        let fragment_memory = memory(1, fragment.constant_data)?;
        let kernel = lower_graphics_with_bound_constants(
            &vertex,
            &fragment,
            GraphicsOptions { sample_shading: bindings.sample_shading(), ..Default::default() },
            &textures(0),
            &textures(1),
            [&vertex_memory, &fragment_memory],
        )?;
        Ok(Self { kernel, bindings })
    }

    pub fn constants(&self, frame: &ModelShaderFrame<'_>) -> Result<ModelShaderConstants> {
        self.compact(self.bindings.constants(frame)?)
    }

    fn compact(&self, memory: ModelShaderMemory) -> Result<ModelShaderConstants> {
        self.kernel.vertex.validate_constant_assumptions(&memory.vertex)?;
        self.kernel.fragment.validate_constant_assumptions(&memory.fragment)?;
        Ok(ModelShaderConstants {
            vertex_compact: memory
                .vertex
                .compact_reads(&self.kernel.vertex.constant_reads)
                .context("vertex shader constants")?,
            fragment_compact: memory
                .fragment
                .compact_reads(&self.kernel.fragment.constant_reads)
                .context("fragment shader constants")?,
            vertex_memory: memory.vertex,
            fragment_memory: memory.fragment,
        })
    }
}
