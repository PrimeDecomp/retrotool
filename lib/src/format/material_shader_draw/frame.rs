//! Per-draw inputs, separate from a prepared material and its shared GPU objects.

use anyhow::{ensure, Context, Result};

use crate::format::{
    material_scene_bindings::DrawUniform,
    room::SAtlasLookup,
    shader_nvn_globals::NvnGlobalTransforms,
    shader_nvn_samples::NvnSampleConstants,
    shader_nvn_uniforms::{NvnUniformBuffers, NvnUniformValue},
};

/// Values for one draw in one view. Preparation declares which additional
/// inputs are needed; supplying a frame never changes material selection.
#[derive(Clone, Copy, Debug)]
pub struct ModelShaderFrame<'a> {
    pub transforms: &'a NvnGlobalTransforms,
    pub samples: NvnSampleConstants,
    /// None retains the prepared ANUV/WIND snapshot. Some evaluates those
    /// providers at this absolute time; real ROOM wind uses its modular clock.
    pub animation_time: Option<f64>,
    /// Required only for a declared, selected LODP draw source. The culling
    /// camera may differ from the rendering camera; no matrix inference occurs.
    pub cull_camera_position: Option<[f32; 3]>,
    /// Required only when BLAL was declared as a draw source. Kept per placement
    /// so distinct atlas regions can share one compiled material.
    pub baked_atlas_lookup: Option<SAtlasLookup>,
}

impl<'a> ModelShaderFrame<'a> {
    pub fn new(transforms: &'a NvnGlobalTransforms, samples: NvnSampleConstants) -> Self {
        Self {
            transforms,
            samples,
            animation_time: None,
            cull_camera_position: None,
            baked_atlas_lookup: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DrawUniformBinding {
    pub packed: u16,
    pub source: DrawUniform,
}

impl DrawUniformBinding {
    pub fn write(
        &self,
        uniforms: &mut NvnUniformBuffers,
        frame: &ModelShaderFrame<'_>,
    ) -> Result<()> {
        let value = match self.source {
            DrawUniform::LodParameters => {
                let position = frame
                    .cull_camera_position
                    .context("LODP requires the culling camera's world position")?;
                ensure!(position.iter().all(|v| v.is_finite()), "Nonfinite LODP camera position");
                NvnUniformValue::Vector4([position[0], position[1], position[2], 1.0])
            }
            DrawUniform::BakedAtlasLookup => {
                let lookup = frame
                    .baked_atlas_lookup
                    .context("BLAL requires the placement's atlas lookup")?;
                ensure!(lookup.is_valid(), "Invalid BLAL atlas lookup");
                NvnUniformValue::Vector4(lookup.0.to_array())
            }
        };
        uniforms.write(self.packed, u32::MAX, &value)?;
        Ok(())
    }
}
