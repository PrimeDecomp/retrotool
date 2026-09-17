//! Runtime scene material data and their native binding operations.
//!
//! These are memory snapshots, not additional serialized MTRL records. A caller
//! resolves providers and memory, then maps packed variables through reflection.
//! Texture tokens remain opaque; this module does not invent GPU resources.

use std::borrow::Cow;

use anyhow::{ensure, Context, Result};
use remax::resources::texture::TextureBinding;
use serde_derive::Serialize;

mod inputs;
pub use inputs::{DrawUniform, SceneMaterialInputs, SceneTextureInput};

use super::{
    material_ambient::AmbientSource,
    material_feature_semantics::semantic_names,
    material_features::FeatureVariable,
    model_data::{animated_uv::AnimatedUvSnapshot, wind::WindSnapshot},
    texture_sampler::TextureSamplerParams,
    txtr::{ETextureFormat, ETextureType},
};
use crate::format::shader_nvn_texture::{texture_binding_from_view, texture_sampling_from_sampler};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SceneMaterialKind {
    AmbientLight,
    BakedProbeTextures,
    ClusteredLights,
    ReflectionProbe,
    Wind,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct SceneBindingRequest<'a> {
    pub data_id: &'a str,
    pub material_kind: SceneMaterialKind,
    pub semantic_index: usize,
}

impl FeatureVariable<'_> {
    /// Recognize a complete scene feature group even if its first write is disabled.
    pub fn scene_material_kind(&self) -> Option<SceneMaterialKind> {
        match (self.definition, self.definition_option) {
            ("AMBS", 1) => Some(SceneMaterialKind::AmbientLight),
            ("BPTX", 1 | 2) => Some(SceneMaterialKind::BakedProbeTextures),
            ("CLPP", 1 | 2) => Some(SceneMaterialKind::ClusteredLights),
            ("REFL", 1) => Some(SceneMaterialKind::ReflectionProbe),
            ("WIND", 1) => Some(SceneMaterialKind::Wind),
            _ => None,
        }
    }

    /// Enabled scene requests. BPTX option1 honors flag bit0
    /// for its two matrices; option2 always writes its matrix and first texture.
    /// Optional shared shadow textures additionally test packed != 0xffff.
    /// Other enabled semantics retain unbound variables until the final sink.
    pub fn scene_binding_request(&self) -> Result<Option<SceneBindingRequest<'_>>> {
        let Some(material_kind) = self.scene_material_kind() else { return Ok(None) };
        let index = semantic_names(self.definition)
            .unwrap()
            .iter()
            .position(|&s| s == self.semantic)
            .context("Unknown scene feature semantic")?;
        if material_kind == SceneMaterialKind::ClusteredLights
            && ((self.definition_option == 1 && !matches!(index, 0..=3 | 5))
                || (matches!(index, 8 | 11) && self.packed == u16::MAX))
        {
            return Ok(None);
        }
        if material_kind == SceneMaterialKind::BakedProbeTextures
            && match self.definition_option {
                1 => index == 2 || (index < 2 && self.binding_flags & 1 != 0),
                2 => !matches!(index, 0 | 2),
                _ => unreachable!(),
            }
        {
            return Ok(None);
        }
        Ok(Some(SceneBindingRequest {
            data_id: self.data_ids.first().copied().context("Scene feature needs a data ID")?,
            material_kind,
            semantic_index: index,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SceneTexture {
    /// Original opaque graphics-token address; zero remains zero for REFL/BPTX.
    Token(u64),
    /// CGraphicsUtilCache::GetZeroTexture, not a guessed asset UUID/type.
    Zero,
    /// CGraphicsUtilCache::GetWhiteTexture.
    White,
}

/// An explicitly resolved runtime texture view and effective sampler. The caller
/// supplies any streaming/sample-shading overrides already applied by the engine.
#[derive(Clone, Copy, Debug)]
pub struct SceneTextureView {
    pub kind: ETextureType,
    pub format: ETextureFormat,
    pub sampler: TextureSamplerParams,
    /// Actual bound view range; None means the caller has not resolved it.
    pub mip_level_count: Option<u32>,
}

/// The cache's white/zero images are both 1x1 2D with one mip. Zero uses four
/// unsigned32 channels; white uses four UNORM8 channels. Preserve that distinction
/// when creating host resources, even when a shader only reads zero from R.
#[derive(Debug)]
pub struct SceneCacheTexture {
    pub format: ETextureFormat,
    pub pixels: &'static [u8],
    pub sampler: TextureSamplerParams,
}

impl SceneTexture {
    /// Original CGraphicsUtilCache123c14 / color helper1206c0 CPU construction.
    /// Token variants need the caller's actual descriptor; they have no default.
    pub fn cache_data(self) -> Option<SceneCacheTexture> {
        let (format, pixels): (_, &[u8]) = match self {
            Self::Token(_) => return None,
            Self::Zero => (ETextureFormat::Rgba32Uint, &[0; 16]),
            Self::White => (ETextureFormat::Rgba8Unorm, &[255; 4]),
        };
        Some(SceneCacheTexture {
            format,
            pixels,
            sampler: TextureSamplerParams::simple(0, [1; 3], -1),
        })
    }
}

impl SceneCacheTexture {
    pub fn declaration(&self, texture_index: u16) -> Result<TextureBinding> {
        let mut binding =
            texture_binding_from_view(texture_index, ETextureType::D2, self.format, &self.sampler)?;
        binding.sampling = Some(texture_sampling_from_sampler(1, &self.sampler)?);
        Ok(binding)
    }
}

#[derive(Debug, Serialize)]
pub enum SceneBindingValue<'a> {
    Float4([u8; 16]),
    Int4([u8; 16]),
    /// Raw bytes at destination offset0, including native one-byte sentinels.
    Raw(Cow<'a, [u8]>),
    Texture {
        texture: SceneTexture,
        /// None invokes the default-sampler sink. Opaque nonnull token otherwise.
        sampler: Option<u64>,
    },
}

#[derive(Debug, Serialize)]
pub struct SceneBindingWrite<'a> {
    pub semantic_index: usize,
    pub packed: u16,
    pub value: SceneBindingValue<'a>,
}

#[derive(Debug, Serialize)]
pub struct SceneBindingResult<'a> {
    pub success: bool,
    /// Native callback order; notably CLPP unshadowed lights precede shadow mask.
    pub writes: Vec<SceneBindingWrite<'a>>,
}

/// CClusteredLightingMaterialData, 0x120 bytes plus five external record arrays.
/// Raw header/array bytes retain fields whose deeper meaning is still unknown.
#[derive(Debug)]
pub struct ClusteredLightingSnapshot<'a> {
    pub raw: &'a [u8; 0x120],
    /// Unshadowed lights, spot lights, spot shadow data, point lights, point data.
    pub arrays: [&'a [u8]; 5],
}

impl<'a> ClusteredLightingSnapshot<'a> {
    pub const ARRAY_LAYOUTS: [(usize, usize); 5] =
        [(0x70, 0x50), (0x80, 0x50), (0x90, 0x70), (0xd0, 0x50), (0xe0, 0x80)];

    /// The resolver must return the exact requested span from captured memory.
    /// Empty arrays do not dereference their pointer. Rejects truncated spans
    /// and byte-count overflow instead of attempting malformed native accesses.
    pub fn parse(
        bytes: &'a [u8],
        mut memory: impl FnMut(u64, usize) -> Result<&'a [u8]>,
    ) -> Result<Self> {
        let raw: &[u8; 0x120] =
            bytes.try_into().context("Invalid clustered material header size")?;
        let mut arrays: [&[u8]; 5] = [&[]; 5];
        for (i, (offset, stride)) in Self::ARRAY_LAYOUTS.into_iter().enumerate() {
            let count = word(raw, offset);
            let length = count.checked_mul(stride as u32).context("Scene array size overflow")?;
            if length != 0 {
                let pointer = pointer(raw, offset + 8);
                ensure!(pointer != 0, "Nonnull scene array count has a null pointer");
                let data = memory(pointer, length as usize)?;
                ensure!(data.len() == length as usize, "Scene memory resolver returned wrong span");
                arrays[i] = data;
            }
        }
        Ok(Self { raw, arrays })
    }
}

#[derive(Debug)]
pub struct ReflectionProbeSnapshot<'a> {
    /// CReflectionProbeMaterialData: BRDF token+0x10, raw16-byte parameters+0x20.
    pub raw: &'a [u8; 0x30],
}

impl<'a> ReflectionProbeSnapshot<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        Ok(Self { raw: bytes.try_into().context("Invalid reflection material header size")? })
    }
}

/// CBakedLightingProbeTextureMaterialData: two matrices and six texture references.
/// Reference controls and the 16-byte material base remain uninterpreted.
#[derive(Debug)]
pub struct BakedProbeTextureSnapshot<'a> {
    pub raw: &'a [u8; 0xd0],
}

impl<'a> BakedProbeTextureSnapshot<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        Ok(Self { raw: bytes.try_into().context("Invalid baked probe material header size")? })
    }

    pub fn texture_tokens(&self) -> [u64; 6] {
        std::array::from_fn(|i| pointer(self.raw, 0x70 + i * 16))
    }
}

#[derive(Debug)]
pub enum SceneMaterialSnapshot<'a> {
    AnimatedUv(AnimatedUvSnapshot),
    Wind(WindSnapshot),
    AmbientLight(AmbientSource),
    BakedProbeTextures(BakedProbeTextureSnapshot<'a>),
    ClusteredLights(ClusteredLightingSnapshot<'a>),
    ReflectionProbe(ReflectionProbeSnapshot<'a>),
}

/// Resolve one complete leaf feature's variable group. `None` data means known
/// absence, not an unresolved provider: AMBS/CLPP supply native defaults, while
/// BPTX succeeds with no writes and REFL/WIND report failure with no writes.
/// Unknown definitions/inactive options
/// return None. The caller maps writes through shader reflection and supplies
/// actual cache textures, token views and sampler choices.
pub fn bind_scene_feature<'a>(
    variables: &[FeatureVariable<'_>],
    data: Option<&SceneMaterialSnapshot<'a>>,
) -> Result<Option<SceneBindingResult<'a>>> {
    let first = variables.first().context("Empty scene feature variable group")?;
    let Some(kind) = first.scene_material_kind() else { return Ok(None) };
    let names = semantic_names(first.definition).unwrap();
    ensure!(variables.len() == names.len(), "Incomplete scene feature variable group");
    ensure!(
        variables.iter().zip(names).all(|(v, &name)| {
            v.feature_path == first.feature_path
                && v.definition == first.definition
                && v.data_ids == first.data_ids
                && v.definition_option == first.definition_option
                && v.binding_flags == first.binding_flags
                && v.data_origin == first.data_origin
                && v.semantic == name
        }),
        "Mixed or reordered scene feature variable group"
    );
    ensure!(!first.data_ids.is_empty(), "Scene feature needs a data ID");
    let mut result = SceneBindingResult { success: true, writes: Vec::new() };
    let mut write = |index: usize, value| {
        result.writes.push(SceneBindingWrite {
            semantic_index: index,
            packed: variables[index].packed,
            value,
        });
    };
    match kind {
        SceneMaterialKind::Wind => {
            if let Some(data) = data {
                let SceneMaterialSnapshot::Wind(snapshot) = data else {
                    anyhow::bail!("Wrong scene material type for WIND");
                };
                write(0, SceneBindingValue::Raw(Cow::Owned(snapshot.to_bytes().to_vec())));
            } else {
                result.success = false;
            }
        }
        SceneMaterialKind::BakedProbeTextures => {
            if let Some(data) = data {
                let SceneMaterialSnapshot::BakedProbeTextures(probe) = data else {
                    anyhow::bail!("Wrong scene material type for BPTX");
                };
                if first.definition_option == 2 || first.binding_flags & 1 == 0 {
                    write(0, SceneBindingValue::Raw(Cow::Borrowed(&probe.raw[0x10..0x40])));
                }
                if first.definition_option == 1 {
                    if first.binding_flags & 1 == 0 {
                        write(1, SceneBindingValue::Raw(Cow::Borrowed(&probe.raw[0x40..0x70])));
                    }
                    for (i, token) in probe.texture_tokens().into_iter().enumerate() {
                        write(3 + i, texture(SceneTexture::Token(token), None));
                    }
                } else {
                    write(2, texture(SceneTexture::Token(pointer(probe.raw, 0x70)), None));
                }
            }
        }
        SceneMaterialKind::AmbientLight => {
            let source = match data {
                Some(SceneMaterialSnapshot::AmbientLight(source)) => source,
                None => &AmbientSource::black(),
                _ => anyhow::bail!("Wrong scene material type for AMBS"),
            };
            write(0, SceneBindingValue::Raw(Cow::Owned(source.flat_bytes().to_vec())));
        }
        SceneMaterialKind::ReflectionProbe => {
            if let Some(data) = data {
                let SceneMaterialSnapshot::ReflectionProbe(probe) = data else {
                    anyhow::bail!("Wrong scene material type for REFL");
                };
                write(0, texture(SceneTexture::Token(pointer(probe.raw, 0x10)), None));
                write(1, SceneBindingValue::Raw(Cow::Borrowed(&probe.raw[0x20..0x30])));
            } else {
                result.success = false;
            }
        }
        SceneMaterialKind::ClusteredLights => {
            let cluster = match data {
                Some(SceneMaterialSnapshot::ClusteredLights(cluster)) => Some(cluster),
                None => None,
                _ => anyhow::bail!("Wrong scene material type for CLPP"),
            };
            let prefix = |offset| cluster.map(|c| vector(c.raw, offset)).unwrap_or([0; 16]);
            let mask = |offset| {
                let token = cluster.map(|c| pointer(c.raw, offset)).unwrap_or(0);
                texture(
                    if token == 0 { SceneTexture::Zero } else { SceneTexture::Token(token) },
                    None,
                )
            };
            let array = |index: usize| {
                SceneBindingValue::Raw(match cluster {
                    Some(c) => Cow::Borrowed(if c.arrays[index].is_empty() {
                        &[0xff]
                    } else {
                        c.arrays[index]
                    }),
                    None => Cow::Owned(match index {
                        2 => default_shadow_data(false),
                        4 => default_shadow_data(true),
                        _ => black_light(),
                    }),
                })
            };
            let shadow = |offset| {
                let token = cluster.map(|c| pointer(c.raw, offset)).unwrap_or(0);
                if token == 0 {
                    texture(SceneTexture::White, None)
                } else {
                    let sampler = pointer(cluster.unwrap().raw, offset + 0x10);
                    texture(SceneTexture::Token(token), (sampler != 0).then_some(sampler))
                }
            };
            write(0, SceneBindingValue::Float4(prefix(0x10)));
            write(1, SceneBindingValue::Float4(prefix(0x20)));
            write(2, SceneBindingValue::Int4(prefix(0x30)));
            write(3, mask(0x48));
            write(5, array(0));
            if first.definition_option == 2 {
                write(4, mask(0x58));
                write(6, array(1));
                write(7, array(2));
                if variables[8].packed != u16::MAX {
                    write(8, shadow(0xb0));
                }
                write(9, array(3));
                write(10, array(4));
                if variables[11].packed != u16::MAX {
                    write(11, shadow(0x100));
                }
            }
        }
    }
    Ok(Some(result))
}

fn texture<'a>(texture: SceneTexture, sampler: Option<u64>) -> SceneBindingValue<'a> {
    SceneBindingValue::Texture { texture, sampler }
}
fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn pointer(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn vector(bytes: &[u8], offset: usize) -> [u8; 16] {
    bytes[offset..offset + 16].try_into().unwrap()
}
fn float_ones(size: usize, indices: &[usize]) -> Vec<u8> {
    let mut bytes = vec![0; size];
    for &i in indices {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&1.0f32.to_le_bytes());
    }
    bytes
}
// Original BlackPointLight16b5a4 and shadow POD constructors172930/172898.
fn black_light() -> Vec<u8> { float_ones(0x50, &[7, 12, 16, 17, 18, 19]) }
fn default_shadow_data(point: bool) -> Vec<u8> {
    float_ones(if point { 0x80 } else { 0x70 }, &[8, 13, 18, 23, 27])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::cmdl::{
        CMaterialData, CMaterialDataInner, EMaterialDataId, EMaterialDataType,
    };

    #[test]
    fn resolved_scene_presence_matches_selection_and_does_not_invent_absence() -> Result<()> {
        let mut scene = SceneMaterialInputs::default();
        assert!(scene.get(EMaterialDataId::ALLD).is_err());
        scene.set(EMaterialDataId::ALLD, None)?;
        assert!(scene.get(EMaterialDataId::ALLD)?.is_none());
        assert_eq!(scene.selection_state().select_definition("AMBS", &["ALLD"], 3)?, Some(0));
        scene.set(
            EMaterialDataId::ALLD,
            Some(SceneMaterialSnapshot::AmbientLight(AmbientSource::full_bright())),
        )?;
        assert_eq!(scene.selection_state().select_definition("AMBS", &["ALLD"], 3)?, Some(1));
        assert!(scene.set(EMaterialDataId::DIFT, None).is_err());
        Ok(())
    }

    #[test]
    fn wind_binding_preserves_payload_and_native_missing_data_failure() -> Result<()> {
        let snapshot = WindSnapshot {
            vectors: std::array::from_fn(|row| {
                std::array::from_fn(|column| (row * 4 + column) as f32)
            }),
            bounds_padding: [9.0; 3],
        };
        let data = SceneMaterialSnapshot::Wind(snapshot);
        let mut variables = variables("WIND", 1);
        variables[0].data_ids = vec!["WIND"];
        variables[0].packed = 3;
        let result = bind_scene_feature(&variables, Some(&data))?.unwrap();
        assert!(result.success);
        assert_eq!(result.writes.len(), 1);
        assert_eq!(result.writes[0].packed, 3);
        let SceneBindingValue::Raw(bytes) = &result.writes[0].value else {
            panic!("Expected raw WIND data")
        };
        assert_eq!(bytes.as_ref(), snapshot.to_bytes());
        let missing = bind_scene_feature(&variables, None)?.unwrap();
        assert!(!missing.success);
        assert!(missing.writes.is_empty());
        variables[0].definition_option = 0;
        assert!(bind_scene_feature(&variables, Some(&data))?.is_none());
        Ok(())
    }

    fn variables(definition: &'static str, option: u32) -> Vec<FeatureVariable<'static>> {
        semantic_names(definition)
            .unwrap()
            .iter()
            .map(|&semantic| FeatureVariable {
                feature_path: vec!["scene"],
                definition,
                data_ids: vec!["CLLD"],
                data_origin: 1,
                definition_option: option,
                binding_flags: u32::MAX, // These binders ignore flags.
                semantic,
                binding: None,
                packed: u16::MAX,
            })
            .collect()
    }

    #[test]
    fn missing_scene_and_empty_arrays_have_distinct_native_uploads() {
        let raw = [0; 0x120];
        let cluster = ClusteredLightingSnapshot::parse(&raw, |_, _| unreachable!()).unwrap();
        let vars = variables("CLPP", 2);
        let data = SceneMaterialSnapshot::ClusteredLights(cluster);
        let present = bind_scene_feature(&vars, Some(&data)).unwrap().unwrap();
        let missing = bind_scene_feature(&vars, None).unwrap().unwrap();
        assert!(present.success && missing.success);
        assert_eq!(present.writes.iter().map(|w| w.semantic_index).collect::<Vec<_>>(), [
            0, 1, 2, 3, 5, 4, 6, 7, 9, 10
        ]); // Optional shared textures skip packedffff; the other writes survive.
        let SceneBindingValue::Raw(empty) = &present.writes[4].value else { unreachable!() };
        let SceneBindingValue::Raw(black) = &missing.writes[4].value else { unreachable!() };
        assert_eq!(empty.as_ref(), [0xff]);
        assert_eq!(black.len(), 0x50);
        let probe = bind_scene_feature(&variables("REFL", 1), None).unwrap().unwrap();
        assert!(!probe.success && probe.writes.is_empty());
    }

    #[test]
    fn scene_textures_select_the_declared_view_and_override_model_providers() -> Result<()> {
        use super::super::material_predicates::{MaterialSelectionState, SelectionValue};

        let mut scene = SceneMaterialInputs::default();
        let mut input = SceneTextureInput {
            token: 7,
            tex_coord: Some(0),
            view: SceneTextureView {
                kind: ETextureType::D2,
                format: ETextureFormat::BptcUfloat,
                sampler: TextureSamplerParams::simple(1, [0; 3], 0),
                mip_level_count: Some(1),
            },
        };
        assert!(scene.set_texture(EMaterialDataId::BLCM, input).is_err());
        scene.set_texture(EMaterialDataId::BLAT, input)?;
        let mut model = MaterialSelectionState::default();
        model.set(EMaterialDataId::BLAT, SelectionValue::Texture(ETextureType::Cube));
        let mut selection = scene.selection_state();
        selection.append_fallback(&model);
        assert_eq!(selection.select_definition("TEXS", &["BLAT"], 0x7ff)?, Some(3));

        input.token = 8;
        input.view.kind = ETextureType::D3;
        scene.set_texture(EMaterialDataId::BLAT, input)?;
        assert_eq!(scene.texture(EMaterialDataId::BLAT).unwrap().token, 8);
        assert_eq!(scene.selection_state().select_definition("TEXS", &["BLAT"], 0x7ff)?, Some(4));
        input.token = 0;
        assert!(scene.set_texture(EMaterialDataId::BLAT, input).is_err());
        assert_eq!(scene.texture(EMaterialDataId::BLAT).unwrap().token, 8);
        Ok(())
    }

    #[test]
    fn scene_uniforms_participate_in_selection_and_replace_prior_values() -> Result<()> {
        use crate::format::CColor4f;
        let mut scene = SceneMaterialInputs::default();
        let color = |r| CMaterialData {
            data_id: EMaterialDataId::BLCM,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color(CColor4f { r, g: 0.5, b: 0.25, a: 1.0 }),
        };
        scene.set_uniform(color(0.0))?;
        scene.set_uniform(color(0.75))?;
        assert_eq!(scene.selection_state().select_definition("FLT4", &["BLCM"], 3)?, Some(1));
        let request = super::super::material_uniform_bindings::MaterialUniformBinding {
            data_id: "BLCM",
            kind: super::super::material_uniform_bindings::MaterialUniformKind::Color,
        };
        let bytes = request
            .value_from_data(scene.uniform_data(EMaterialDataId::BLCM).unwrap())?
            .bytes()
            .into_owned();
        assert_eq!(
            bytes,
            [0.75f32, 0.5, 0.25, 1.0].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>()
        );
        let mut wrong = color(1.0);
        wrong.data_id = EMaterialDataId::BLPT;
        assert!(scene.set_uniform(wrong).is_err());
        Ok(())
    }

    #[test]
    fn draw_uniform_declarations_replace_fixed_values_without_inventing_bytes() -> Result<()> {
        let mut scene = SceneMaterialInputs::default();
        let camera = || CMaterialData {
            data_id: EMaterialDataId::LODP,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color([2.0, 3.0, 4.0, 1.0].into()),
        };
        scene.set_uniform(camera())?;
        scene.set_draw_uniform(DrawUniform::LodParameters);
        assert!(scene.uniform_data(EMaterialDataId::LODP).is_none());
        assert_eq!(scene.draw_uniform(EMaterialDataId::LODP), Some(DrawUniform::LodParameters));
        assert_eq!(scene.selection_state().select_definition("FLT4", &["LODP"], 3)?, Some(1));

        scene.set_uniform(camera())?;
        assert!(scene.draw_uniform(EMaterialDataId::LODP).is_none());
        assert!(scene.uniform_data(EMaterialDataId::LODP).is_some());
        Ok(())
    }

    #[test]
    fn baked_probe_matrix_flag_keeps_the_texture_group_active() -> Result<()> {
        let raw = [0x5a; 0xd0];
        let data =
            SceneMaterialSnapshot::BakedProbeTextures(BakedProbeTextureSnapshot::parse(&raw)?);
        let vars = variables("BPTX", 1); // Includes flag bit0.
        assert_eq!(vars[0].scene_material_kind(), Some(SceneMaterialKind::BakedProbeTextures));
        assert!(vars[0].scene_binding_request()?.is_none());
        assert_eq!(vars[3].scene_binding_request()?.unwrap().semantic_index, 3);
        let full = bind_scene_feature(&vars, Some(&data))?.unwrap();
        assert_eq!(full.writes.iter().map(|w| w.semantic_index).collect::<Vec<_>>(), [
            3, 4, 5, 6, 7, 8
        ]);
        let fallback = bind_scene_feature(&variables("BPTX", 2), Some(&data))?.unwrap();
        assert_eq!(fallback.writes.iter().map(|w| w.semantic_index).collect::<Vec<_>>(), [0, 2]);
        let absent = bind_scene_feature(&vars, None)?.unwrap();
        assert!(absent.success && absent.writes.is_empty());
        Ok(())
    }

    #[test]
    fn snapshot_and_variable_group_reject_incomplete_data() {
        assert!(BakedProbeTextureSnapshot::parse(&[0; 0xcf]).is_err());
        assert!(ClusteredLightingSnapshot::parse(&[0; 0x11f], |_, _| unreachable!()).is_err());
        assert!(ReflectionProbeSnapshot::parse(&[0; 0x2f]).is_err());
        let mut raw = [0; 0x120];
        raw[0x70..0x74].copy_from_slice(&1u32.to_le_bytes());
        assert!(ClusteredLightingSnapshot::parse(&raw, |_, _| unreachable!()).is_err());
        raw[0x78..0x80].copy_from_slice(&0x1234u64.to_le_bytes());
        assert!(ClusteredLightingSnapshot::parse(&raw, |address, size| {
            assert_eq!((address, size), (0x1234, 0x50));
            Ok(&[0; 0x4f])
        })
        .is_err());
        let mut vars = variables("CLPP", 1);
        vars.swap(0, 1);
        assert!(bind_scene_feature(&vars, None).is_err());
        assert!(bind_scene_feature(&variables("CLPP", 0), None).unwrap().is_none());
    }
}
