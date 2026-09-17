//! Scene-provided values and declarations for values supplied at draw time.

use anyhow::{ensure, Context, Result};

use super::{word, SceneMaterialSnapshot, SceneTextureView};
use crate::format::{
    cmdl::{CMaterialData, CMaterialDataInner, EMaterialDataId, EMaterialDataType},
    material_predicates::{MaterialSelectionState, SelectionValue},
    model_data::wind::WindEnvironment,
};

/// Ordinary native material values whose payload is supplied separately for
/// each draw/view. Declaring a source participates in material selection while
/// leaving its bytes unknown during shader translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawUniform {
    /// LODP = (culling camera world position, 1), SContext71001af4cc.
    LodParameters,
    /// BLAL = per-placement baked-lightmap offset, scale and preserved fourth word.
    BakedAtlasLookup,
}

impl DrawUniform {
    pub fn data_id(self) -> EMaterialDataId {
        match self {
            Self::LodParameters => EMaterialDataId::LODP,
            Self::BakedAtlasLookup => EMaterialDataId::BLAL,
        }
    }
}

/// An ordinary TEXS provider backed by a host-resolved texture. The view must
/// include the effective sampler for this draw; no TXTR usage is inferred.
#[derive(Clone, Copy, Debug)]
pub struct SceneTextureInput {
    pub token: u64,
    pub view: SceneTextureView,
    pub tex_coord: Option<u32>,
}

#[derive(Debug)]
enum UniformSource {
    Value(CMaterialData),
    Draw(DrawUniform),
}

impl UniformSource {
    fn data_id(&self) -> EMaterialDataId {
        match self {
            Self::Value(data) => data.data_id,
            Self::Draw(source) => source.data_id(),
        }
    }
}

/// Caller-resolved draw-context data. Missing Complex entries are unresolved;
/// an entry containing None is explicitly absent. Ordinary uniform values take
/// precedence over model providers. This is not a scene extractor.
#[derive(Debug, Default)]
pub struct SceneMaterialInputs<'a> {
    pub flags: u32,
    /// Shared renderer buffer, independent of material data IDs. None remains
    /// unresolved if the selected shader requires GSV_.
    pub global_system_values: Option<crate::format::shader_nvn_globals::NvnGlobalSystemValues>,
    /// Environment for authored model WIND sets. None does not select native
    /// fallback values automatically; callers choose preview or live scene state.
    pub wind_environment: Option<WindEnvironment>,
    data: Vec<(EMaterialDataId, Option<SceneMaterialSnapshot<'a>>)>,
    uniforms: Vec<UniformSource>,
    texture_views: Vec<(u64, Option<u64>, SceneTextureView)>,
    textures: Vec<(EMaterialDataId, SceneTextureInput)>,
}

impl<'a> SceneMaterialInputs<'a> {
    /// Supply an ordinary texture before the model's providers. Selection and
    /// binding use the same view declaration, including its texture dimension.
    pub fn set_texture(&mut self, id: EMaterialDataId, texture: SceneTextureInput) -> Result<()> {
        ensure!(id.data_type() == EMaterialDataType::Texture, "Scene texture ID is not Texture");
        ensure!(texture.token != 0, "Scene texture token is null");
        if let Some(entry) = self.textures.iter_mut().find(|entry| entry.0 == id) {
            entry.1 = texture;
        } else {
            self.textures.push((id, texture));
        }
        Ok(())
    }

    pub fn texture(&self, id: EMaterialDataId) -> Option<&SceneTextureInput> {
        self.textures.iter().find(|entry| entry.0 == id).map(|entry| &entry.1)
    }

    /// Supply a resolved ordinary draw-context value, before the model's own
    /// providers. Uses the existing typed INT/FLT/MAT4 binders; no scene defaults
    /// are inferred. BLCM and BLAL are examples of scene color values.
    pub fn set_uniform(&mut self, data: CMaterialData) -> Result<()> {
        let kind = match &data.data {
            CMaterialDataInner::Int1(_) => EMaterialDataType::Int1,
            CMaterialDataInner::Int4(_) => EMaterialDataType::Int4,
            CMaterialDataInner::Scalar(_) => EMaterialDataType::Scalar,
            CMaterialDataInner::Color(_) => EMaterialDataType::Color,
            CMaterialDataInner::Mat4(_) => EMaterialDataType::Mat4,
            _ => anyhow::bail!("Scene uniform is not ordinary typed data"),
        };
        ensure!(
            data.data_type == kind && data.data_id.data_type() == kind,
            "Scene uniform data ID/type mismatch"
        );
        self.set_uniform_source(UniformSource::Value(data));
        Ok(())
    }

    /// Resolve this material data ID from per-draw inputs. Replaces any fixed
    /// value for the same ID; a later set_uniform can make it fixed again.
    pub fn set_draw_uniform(&mut self, source: DrawUniform) {
        self.set_uniform_source(UniformSource::Draw(source));
    }

    fn set_uniform_source(&mut self, source: UniformSource) {
        if let Some(entry) =
            self.uniforms.iter_mut().find(|entry| entry.data_id() == source.data_id())
        {
            *entry = source;
        } else {
            self.uniforms.push(source);
        }
    }

    pub fn uniform_data(&self, id: EMaterialDataId) -> Option<&CMaterialData> {
        match self.uniforms.iter().find(|entry| entry.data_id() == id)? {
            UniformSource::Value(data) => Some(data),
            UniformSource::Draw(_) => None,
        }
    }

    pub fn draw_uniform(&self, id: EMaterialDataId) -> Option<DrawUniform> {
        match self.uniforms.iter().find(|entry| entry.data_id() == id)? {
            UniformSource::Draw(source) => Some(*source),
            UniformSource::Value(_) => None,
        }
    }

    /// Resolve the exact default/custom sampler request made by a scene binder.
    /// A texture token with a different sampler request needs a separate entry.
    pub fn set_texture_view(&mut self, texture: u64, sampler: Option<u64>, view: SceneTextureView) {
        if let Some(entry) =
            self.texture_views.iter_mut().find(|v| v.0 == texture && v.1 == sampler)
        {
            entry.2 = view;
        } else {
            self.texture_views.push((texture, sampler, view));
        }
    }

    pub fn texture_view(&self, texture: u64, sampler: Option<u64>) -> Result<SceneTextureView> {
        self.texture_views
            .iter()
            .find(|v| v.0 == texture && v.1 == sampler)
            .map(|v| v.2)
            .with_context(|| {
                format!("Unresolved scene texture view {texture:#x}, sampler {sampler:?}")
            })
    }

    pub fn set(
        &mut self,
        id: EMaterialDataId,
        data: Option<SceneMaterialSnapshot<'a>>,
    ) -> Result<()> {
        ensure!(id.data_type() == EMaterialDataType::Complex, "Scene data ID is not Complex");
        if let Some(entry) = self.data.iter_mut().find(|(key, _)| *key == id) {
            entry.1 = data;
        } else {
            self.data.push((id, data));
        }
        Ok(())
    }

    pub fn get(&self, id: EMaterialDataId) -> Result<Option<&SceneMaterialSnapshot<'a>>> {
        Ok(self
            .data
            .iter()
            .find(|(key, _)| *key == id)
            .with_context(|| format!("Unresolved scene material {id:?}"))?
            .1
            .as_ref())
    }

    /// Presence and typed selection values must agree with the bound snapshots.
    pub fn selection_state(&self) -> MaterialSelectionState {
        let mut state = MaterialSelectionState::default();
        state.flags = self.flags;
        for (id, texture) in &self.textures {
            state.set(*id, SelectionValue::Texture(texture.view.kind));
        }
        for source in &self.uniforms {
            match source {
                UniformSource::Value(data) => state.set_material_data(data),
                UniformSource::Draw(source) => state.set(source.data_id(), SelectionValue::Present),
            }
        }
        for (id, data) in &self.data {
            if let Some(data) = data {
                let value = match data {
                    SceneMaterialSnapshot::AnimatedUv(data) => {
                        SelectionValue::AnimatedUv { mode: data.mode }
                    }
                    SceneMaterialSnapshot::BakedProbeTextures(probe) => {
                        SelectionValue::BakedProbeTextures {
                            textures: probe.texture_tokens().map(|token| token != 0),
                        }
                    }
                    SceneMaterialSnapshot::ClusteredLights(cluster) => {
                        SelectionValue::ClusteredLights {
                            mask_source: word(cluster.raw, 0x40),
                            has_shadow_maps: cluster.raw[0x68] != 0,
                        }
                    }
                    _ => SelectionValue::Present,
                };
                state.set(*id, value);
            }
        }
        state
    }
}
