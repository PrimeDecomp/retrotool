//! Baked atlas inputs for isolated ROOM inspection. Probe volumes and runtime
//! lighting registration are separate from this per-placement texture binding.

use anyhow::Result;
use bevy::prelude::*;
use retrolib::format::{
    cmdl::{CMaterialData, CMaterialDataInner, EMaterialDataId, EMaterialDataType},
    material_scene_bindings::{
        DrawUniform, SceneMaterialInputs, SceneTextureInput, SceneTextureView,
    },
    room::SAtlasLookup,
    texture_sampler::{TextureSamplerBinding, TextureSamplerParams},
};

use crate::loaders::texture::TextureAsset;

// Tokens are local to each prepared draw, alongside scene_preview's BRDF token.
pub(super) const ATLAS_TOKEN: u64 = 2;

#[derive(Clone, Debug)]
pub struct BakedAtlasLighting {
    pub texture: Handle<TextureAsset>,
    pub lookup: SAtlasLookup,
}

impl BakedAtlasLighting {
    pub fn same_binding(&self, other: &Self) -> bool {
        self.texture == other.texture
            && self.lookup.0.to_array().map(f32::to_bits)
                == other.lookup.0.to_array().map(f32::to_bits)
    }

    pub fn bind(&self, texture: &TextureAsset, scene: &mut SceneMaterialInputs<'_>) -> Result<()> {
        let header = &texture.inner.head;
        // BuildRenderThreadResources71001cd088 supplies default usage: UV0 and
        // inherited sampler fields. The viewer requests ordinary sample shading.
        let sampler = TextureSamplerParams::from_header(&header.sampler_data)
            .for_binding(TextureSamplerBinding::Regular { sample_shading: false }, 0xff)?;
        scene.set_texture(EMaterialDataId::BLAT, SceneTextureInput {
            token: ATLAS_TOKEN,
            tex_coord: Some(0),
            view: SceneTextureView {
                kind: header.kind,
                format: header.format,
                sampler,
                mip_level_count: Some(header.mip_sizes.len() as u32),
            },
        })?;
        scene.set_draw_uniform(DrawUniform::BakedAtlasLookup);
        // The isolated ROOM starts with the native white modulation. Script
        // ColorModifier/effects state is not reconstructed by this viewer mode.
        scene.set_uniform(CMaterialData {
            data_id: EMaterialDataId::BLCM,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color([1.0; 4].into()),
        })?;
        // Avoid adding the standalone model viewer's full-bright ambient to
        // irradiance already stored in the lightmap.
        scene.set(EMaterialDataId::ALLD, None)?;
        Ok(())
    }
}
