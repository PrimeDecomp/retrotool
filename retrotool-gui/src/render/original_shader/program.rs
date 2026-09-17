//! Compiler selection and the resource contract shared by the Bevy renderer.

use anyhow::{ensure, Result};
use remax::{
    maxwell::graphics::{ColorOutputRouting, GraphicsProgram, QuadFaultMethod},
    program::{
        interpolation::OffsetModel,
        wgsl::{self, Profile},
        F2iNanBehavior,
    },
    resources::ConstantMemoryBinding,
    wgsl::{graphics::GraphicsKernel, texture::TextureBinding},
};
use retrolib::format::material_shader_draw::{
    self, ModelShaderBindings, ModelShaderDraw, ModelShaderFrame, TranslationOptions,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compiler {
    #[default]
    Legacy,
    Remax(Profile),
}

impl Compiler {
    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy => "Legacy translator",
            Self::Remax(Profile::Rendering) => "Remax — rendering",
            Self::Remax(Profile::Checked) => "Remax — checked",
        }
    }
}

pub(super) struct MaterialProgram {
    pub bindings: ModelShaderBindings,
    code: Code,
}

enum Code {
    Legacy(Box<GraphicsKernel>),
    Semantic(Box<GraphicsProgram>),
}

/// Compilers describe the same texture allocation using different manifest types.
/// Normalize only host slots; retain the original view/sampling declaration.
pub(super) struct TextureResource {
    pub view: TextureBinding,
    pub texture: u32,
    pub sampler: Option<u32>,
}

pub(super) struct ConstantBuffers {
    pub inputs: [Vec<u32>; 2],
    pub memories: Vec<(u32, Vec<u32>)>,
}

impl MaterialProgram {
    pub fn compile(
        bindings: ModelShaderBindings,
        compiler: Compiler,
        color_routing: ColorOutputRouting,
    ) -> Result<Self> {
        match compiler {
            Compiler::Legacy => {
                ensure!(
                    color_routing == ColorOutputRouting::Attachments,
                    "Dual-source blending requires Remax"
                );
                let draw = ModelShaderDraw::compile(bindings)?;
                Ok(Self { bindings: draw.bindings, code: Code::Legacy(Box::new(draw.kernel)) })
            }
            Compiler::Remax(profile) => {
                // The same explicit preview policy used by the independent vine
                // replay. The game's SET_SHADER_CONTROL NaN policy is unverified.
                let options = wgsl::Options {
                    profile,
                    approximate_arithmetic: true,
                    approximate_special_functions: true,
                    f2i_nan_behavior: Some(F2iNanBehavior::Zero),
                    offset_interpolation: Some(OffsetModel::SignedSixteenths),
                    ..Default::default()
                };
                let program = material_shader_draw::translate(&bindings, TranslationOptions {
                    vertex: options,
                    fragment: options,
                    quad_faults: Some(QuadFaultMethod::QuadBroadcast),
                    color_routing,
                })?;
                ensure!(
                    color_routing == ColorOutputRouting::DualSource
                        || (program.fragment_outputs.colors[0].iter().all(Option::is_some)
                            && program.fragment_outputs.colors[1..]
                                .iter()
                                .flatten()
                                .all(Option::is_none)),
                    "Viewer requires a single RGBA color target"
                );
                Ok(Self { bindings, code: Code::Semantic(Box::new(program)) })
            }
        }
    }

    pub fn attributes(&self) -> &[u16] {
        match &self.code {
            Code::Legacy(program) => &program.vertex.input_attributes,
            Code::Semantic(program) => &program.attributes,
        }
    }

    pub fn vertex_words(&self, vertices: &[[u32; 256]]) -> Vec<u32> {
        match &self.code {
            Code::Legacy(_) => vertices.iter().flatten().copied().collect(),
            Code::Semantic(program) => {
                let count = program.attributes.len().max(1);
                let mut words = Vec::with_capacity(vertices.len() * count * 2);
                for vertex in vertices {
                    if program.attributes.is_empty() {
                        words.extend([0, 0]);
                    } else {
                        words.extend(
                            program.attributes.iter().map(|&offset| vertex[offset as usize / 4]),
                        );
                        words.extend(std::iter::repeat_n(1, count));
                    }
                }
                words
            }
        }
    }

    pub fn stages(&self) -> [(&str, &'static str); 2] {
        match &self.code {
            Code::Legacy(program) => {
                [(&program.wgsl, "mpr_vertex_main"), (&program.wgsl, "mpr_fragment_main")]
            }
            Code::Semantic(program) => [
                (&program.vertex.source, program.vertex.entry_point),
                (&program.fragment.source, program.fragment.entry_point),
            ],
        }
    }

    pub fn has_faults(&self) -> bool { matches!(self.code, Code::Semantic(_)) }

    pub fn uses_clip_distances(&self) -> bool {
        matches!(&self.code, Code::Semantic(program) if program.clip_distance_mask != 0)
    }

    pub fn memory_bindings(&self) -> [Option<ConstantMemoryBinding>; 2] {
        match &self.code {
            Code::Legacy(program) => {
                [program.vertex.constant_memory_binding, program.fragment.constant_memory_binding]
            }
            Code::Semantic(_) => [None, None],
        }
    }

    pub fn textures(&self, stage: usize) -> Vec<TextureResource> {
        match &self.code {
            Code::Legacy(program) => {
                let kernel = [&program.vertex, &program.fragment][stage];
                kernel
                    .textures
                    .iter()
                    .map(|texture| TextureResource {
                        view: texture.binding,
                        texture: texture.texture_binding,
                        sampler: texture.sampler_binding,
                    })
                    .collect()
            }
            Code::Semantic(program) => {
                let kernel = [&program.vertex, &program.fragment][stage];
                kernel
                    .kernel
                    .textures
                    .iter()
                    .map(|texture| TextureResource {
                        view: texture.binding.view,
                        texture: texture.binding.texture_binding,
                        sampler: texture.binding.sampler_binding.filter(|_| texture.uses_sampler),
                    })
                    .collect()
            }
        }
    }

    pub fn constants(&self, frame: &ModelShaderFrame<'_>) -> Result<ConstantBuffers> {
        let memory = self.bindings.constants(frame)?;
        let memories = [&memory.vertex, &memory.fragment];
        match &self.code {
            Code::Legacy(program) => {
                let kernels = [&program.vertex, &program.fragment];
                let mut inputs = [Vec::new(), Vec::new()];
                let mut dynamic = Vec::new();
                for (stage, kernel) in kernels.into_iter().enumerate() {
                    kernel.validate_constant_assumptions(memories[stage])?;
                    inputs[stage] = memories[stage].compact_reads(&kernel.constant_reads)?;
                    if let Some(binding) = kernel.constant_memory_binding {
                        dynamic.push((binding.binding, memories[stage].words().to_vec()));
                    }
                }
                Ok(ConstantBuffers { inputs, memories: dynamic })
            }
            Code::Semantic(program) => {
                let mut inputs = [Vec::new(), Vec::new()];
                for (stage, module) in [&program.vertex, &program.fragment].into_iter().enumerate()
                {
                    inputs[stage] =
                        module.kernel.constant_preconditions.bind(memories[stage])?.to_vec();
                }
                Ok(ConstantBuffers { inputs, memories: Vec::new() })
            }
        }
    }
}
