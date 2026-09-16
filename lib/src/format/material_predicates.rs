//! Definition-specific SelectOption predicates from MPR's ARM64 executable.
//!
//! A snapshot represents the already merged CMaterialInstance data mask, flags
//! and typed getter results. It does not reproduce provider precedence or create
//! lighting data. `Present` permits presence tests but errors if a predicate needs
//! its unknown payload; `Null` explicitly represents a null typed getter result.

use anyhow::{bail, Context, Result};

use crate::format::{
    cmdl::{
        CMaterialData, CMaterialDataInner, ELayeredTextureBlend, EMaterialDataId,
        EMaterialDataType, SMaterialChunk,
    },
    material_features::{Feature, FeatureBody},
    txtr::ETextureType,
};

#[derive(Clone, Copy, Debug)]
pub enum SelectionValue {
    /// ID is in the data mask; payload has not been resolved.
    Present,
    /// ID is in the data mask, but its typed getter returns null.
    Null,
    Int(i32),
    /// CAnimatedUVMaterialData +0x10. Only mode1 selects U-translation options.
    AnimatedUv {
        mode: u32,
    },
    Texture(ETextureType),
    LayeredTexture {
        blend: ELayeredTextureBlend,
        tangent_flags: u8,
    },
    /// Counts at CDirectionalLightingMaterialData +0x20 and +0x40.
    DirectionalLights {
        unshadowed: u32,
        shadowed: u32,
    },
    /// CClusteredLightingMaterialData +0x40 and +0x68. The latter is true
    /// when either the spot or point shadow-map data array is nonempty.
    ClusteredLights {
        mask_source: u32,
        has_shadow_maps: bool,
    },
    /// Nonnull texture pointers at +0x70,+0x80,...,+0xc0. The first binds
    /// Ambient in option 2, or MidpointRGB in option 1; the other five bind
    /// RangeRGB, ExponentRGB, GradientDirR/G/B in option 1.
    BakedProbeTextures {
        textures: [bool; 6],
    },
}

#[derive(Clone, Debug)]
pub struct MaterialSelectionState {
    /// The resolved CMaterialInstance flag word at +0x18; EMaterialFlag bit indices.
    pub flags: u32,
    values: [Option<SelectionValue>; 116],
}

impl Default for MaterialSelectionState {
    /// An explicitly empty instance, with no scene data or flags.
    fn default() -> Self { Self { flags: 0, values: [None; 116] } }
}

impl MaterialSelectionState {
    /// Merge an already resolved lower-priority provider. Normal CMaterialInstance
    /// lookup takes the first nonnull getter result, while masks/flags are ORed.
    /// Unresolved payloads retain precedence and will error if their value is needed.
    /// This is not the engine's separate AppendBottom storage/traversal path.
    pub fn append_fallback(&mut self, fallback: &Self) {
        self.flags |= fallback.flags;
        for (value, &lower) in self.values.iter_mut().zip(&fallback.values) {
            if matches!(value, None | Some(SelectionValue::Null)) && lower.is_some() {
                *value = lower;
            }
        }
    }

    /// Assemble the static providers used for a model material: its own values
    /// and flags, shared model data, then generated BKLB presence when any model
    /// material has the BKLT trait. Draw-context data may be placed before this
    /// state with append_fallback. No lights or instancing flags are invented.
    /// The resolver returns the runtime texture type; None leaves it unresolved.
    pub fn from_model_material(
        materials: &SMaterialChunk,
        index: usize,
        mut texture_type: impl FnMut(uuid::Uuid) -> Result<Option<ETextureType>>,
    ) -> Result<Self> {
        let material = materials.materials.get(index).context("Invalid model material index")?;
        let mut state = Self { flags: material.flags, ..Self::default() };
        for data in materials.material_values(index)? {
            state.set_material_data(data);
            if let CMaterialDataInner::Texture(texture) = &data.data {
                if let Some(kind) = texture_type(texture.id)? {
                    state.set(data.data_id, SelectionValue::Texture(kind));
                }
            }
        }
        if materials.materials.iter().any(|m| m.traits.iter().any(|t| t.0 == *b"BKLT")) {
            let mut bounds = Self::default();
            // LoadMaterialCache creates BKLB = (minY,maxY,1/height,-minY/height),
            // using reciprocal1 for near-zero height. Predicates only need presence.
            bounds.set(EMaterialDataId::BKLB, SelectionValue::Present);
            state.append_fallback(&bounds);
        }
        Ok(state)
    }

    pub fn set(&mut self, id: EMaterialDataId, value: SelectionValue) {
        self.values[id as usize] = Some(value);
    }

    pub fn remove(&mut self, id: EMaterialDataId) { self.values[id as usize] = None; }

    pub fn contains(&self, id: EMaterialDataId) -> bool { self.values[id as usize].is_some() }

    /// Add one parsed material value. Textures remain unresolved until the caller
    /// supplies their runtime texture type (or an explicit null getter result).
    /// This does not set model/render flags or merge the engine's provider stack.
    pub fn set_material_data(&mut self, data: &CMaterialData) {
        let value = match &data.data {
            CMaterialDataInner::Int1(value) => SelectionValue::Int(*value),
            CMaterialDataInner::LayeredTexture(value) => SelectionValue::LayeredTexture {
                blend: value.base.blend_control,
                tangent_flags: value.base.flags,
            },
            _ => SelectionValue::Present,
        };
        self.set(data.data_id, value);
    }

    fn get(&self, id: EMaterialDataId) -> Result<Option<SelectionValue>> {
        match self.values[id as usize] {
            Some(SelectionValue::Present) => bail!("Material data payload {id:?} is unresolved"),
            None | Some(SelectionValue::Null) => Ok(None),
            value => Ok(value),
        }
    }

    /// Callback adapter for FeatureSet::select_permutation / select_dynamic_runtime_id.
    pub fn select_feature(&self, feature: &Feature<'_>, allowed: u32) -> Result<Option<u32>> {
        let FeatureBody::Simple(simple) = &feature.body else {
            bail!("Definition selection requires a simple feature");
        };
        self.select_definition(simple.definition, &simple.data_ids, allowed)
    }

    /// Returns a DEFINITION option, before the feature instance's option_map.
    /// Unknown definitions or unresolved required payloads return errors. None is
    /// the engine's 0xffffffff selection failure, not an unsupported-definition marker.
    pub fn select_definition(
        &self,
        definition: &str,
        data_ids: &[&str],
        allowed: u32,
    ) -> Result<Option<u32>> {
        let first = || -> Result<EMaterialDataId> {
            EMaterialDataId::from_fourcc(data_ids.first().context("Feature needs a data ID")?)
        };
        let enabled =
            |condition| if condition && permits(allowed, 1) { Some(1) } else { zero(allowed) };
        if let Some(bit) = material_flag_bit(definition) {
            // NFeatureMaterialFlag::select_option 0x7100161870.
            return Ok(enabled(self.flags & (1 << bit) != 0));
        }
        let category = match definition {
            "FLT1" => Some(EMaterialDataType::Scalar),
            "FLT4" => Some(EMaterialDataType::Color),
            "INT1" => Some(EMaterialDataType::Int1),
            "INT4" => Some(EMaterialDataType::Int4),
            "MAT4" => Some(EMaterialDataType::Mat4),
            "POD_" | "VFOG" | "REFL" => Some(EMaterialDataType::Complex),
            _ => None,
        };
        if let Some(category) = category {
            let id = first()?;
            return Ok(enabled(self.contains(id) && id.data_type() == category));
        }
        Ok(match definition {
            "ANUV" => {
                let id = first()?;
                if self.contains(id) && id.data_type() == EMaterialDataType::Complex {
                    match self.get(id)? {
                        Some(SelectionValue::AnimatedUv { mode }) => {
                            if mode != 1 && permits(allowed, 1) {
                                Some(1)
                            } else if mode == 1 && permits(allowed, 2) {
                                Some(2)
                            } else if mode == 1 && permits(allowed, 3) {
                                Some(3)
                            } else {
                                zero(allowed)
                            }
                        }
                        None => zero(allowed),
                        _ if allowed & 0xe == 0 => zero(allowed),
                        _ => bail!("ANUV requires animated UV mode"),
                    }
                } else {
                    zero(allowed)
                }
            }
            "GSV_" => enabled(true), // 0x7100171b74: no instance-data test.
            "BLPB" | "SBUF" => enabled(self.contains(first()?)), // 0x710016c92c.
            "AMBS" | "HFUD" => {
                let id = first()?;
                if self.contains(id)
                    && id.data_type() == EMaterialDataType::Complex
                    && permits(allowed, 1)
                {
                    Some(1)
                } else {
                    // Both can fall back to 1 even without usable instance data.
                    zero(allowed).or_else(|| permits(allowed, 1).then_some(1))
                }
            }
            "DEFX" => {
                let a = first()?;
                // LDPSW x0,x20,[x2,#4] at 0x7100163c38 loads IDs 0 and 1.
                // Ghidra's current decompilation incorrectly reports +0xc for
                // the second load; original ARM64 execution confirms +0x8.
                let b = EMaterialDataId::from_fourcc(
                    data_ids.get(1).context("DEFX needs two data IDs")?,
                )?;
                enabled(
                    self.contains(a)
                        && self.contains(b)
                        && a.data_type() == EMaterialDataType::Complex
                        && b.data_type() == EMaterialDataType::Complex,
                )
            }
            "PMOD" => {
                // NFeatureEnumOption::select_option 0x7100161820. The original
                // accepts 32 and tests mask bit 0 due to ARM64's wrapped shift.
                match self.get(first()?)? {
                    Some(SelectionValue::Int(value))
                        if (value as u32) <= 32 && permits(allowed, value as u32 & 31) =>
                    {
                        Some(value as u32)
                    }
                    Some(SelectionValue::Int(_)) | None => None,
                    _ => bail!("PMOD requires an integer getter result"),
                }
            }
            "TEXS" => {
                let id = first()?;
                if self.contains(id) && id.data_type() == EMaterialDataType::Texture {
                    let mut specific = None;
                    if allowed & 0x7fc != 0 {
                        match self.get(id)? {
                            Some(SelectionValue::Texture(kind)) => {
                                let option = kind as u32 + 2;
                                specific = permits(allowed, option).then_some(option);
                            }
                            None => (),
                            _ => bail!("TEXS requires a texture getter result"),
                        }
                    }
                    specific.or_else(|| enabled(true))
                } else {
                    zero(allowed)
                }
            }
            "LAYR" => {
                let id = first()?;
                if self.contains(id) && id.data_type() == EMaterialDataType::Complex {
                    match self.get(id)? {
                        Some(SelectionValue::LayeredTexture { blend, tangent_flags }) => {
                            let candidates = match blend {
                                ELayeredTextureBlend::Embedded => [8, 5, 2],
                                ELayeredTextureBlend::Separate => [10, 7, 4],
                                ELayeredTextureBlend::External => [9, 6, 3],
                                ELayeredTextureBlend::Disabled => [1, 1, 1],
                            };
                            candidates
                                .into_iter()
                                .enumerate()
                                .find(|&(i, option)| {
                                    (i == 2 || tangent_flags & (2 >> i) != 0)
                                        && permits(allowed, option)
                                })
                                .map(|(_, option)| option)
                                .or_else(|| zero(allowed))
                        }
                        None => zero(allowed),
                        _ => bail!("LAYR requires layered texture data"),
                    }
                } else {
                    zero(allowed)
                }
            }
            "BPTX" => match self.get(first()?)? {
                Some(SelectionValue::BakedProbeTextures { textures }) if textures[0] => {
                    if textures[1..].iter().all(|&v| v) && permits(allowed, 1) {
                        Some(1)
                    } else if permits(allowed, 2) {
                        Some(2)
                    } else {
                        zero(allowed)
                    }
                }
                Some(SelectionValue::BakedProbeTextures { .. }) | None => zero(allowed),
                _ => bail!("BPTX requires baked probe texture data"),
            },
            "CLPP" => match self.get(first()?)? {
                Some(SelectionValue::ClusteredLights { mask_source, has_shadow_maps }) => {
                    if mask_source == 0 && has_shadow_maps && permits(allowed, 2) {
                        Some(2)
                    } else if mask_source == 0 && permits(allowed, 1) {
                        Some(1)
                    } else {
                        clustered_fallback(allowed)
                    }
                }
                None => clustered_fallback(allowed),
                _ => bail!("CLPP requires clustered lighting data"),
            },
            "DDLT" => {
                let id = first()?;
                let (unshadowed, shadowed) = if self.contains(id) && allowed & 0xe != 0 {
                    match self.get(id)? {
                        Some(SelectionValue::DirectionalLights { unshadowed, shadowed }) => {
                            (unshadowed, shadowed)
                        }
                        None => (0, 0),
                        _ => bail!("DDLT requires directional lighting data"),
                    }
                } else {
                    (0, 0)
                };
                directional(allowed, unshadowed, shadowed)
            }
            _ => bail!("Unsupported material feature predicate {definition}"),
        })
    }
}

fn permits(mask: u32, option: u32) -> bool { mask & (1 << option) != 0 }
fn zero(mask: u32) -> Option<u32> { permits(mask, 0).then_some(0) }
fn clustered_fallback(mask: u32) -> Option<u32> { (0..3).find(|&i| permits(mask, i)) }

fn directional(mask: u32, unshadowed: u32, shadowed: u32) -> Option<u32> {
    if unshadowed == 0 && shadowed == 0 && permits(mask, 0) {
        return Some(0);
    }
    if unshadowed == 1 && shadowed == 0 && permits(mask, 2) {
        return Some(2);
    }
    if unshadowed == 0 && shadowed == 1 && permits(mask, 3) {
        return Some(3);
    }
    if permits(mask, 1) {
        return Some(1);
    }
    if shadowed == 0 && permits(mask, 2) {
        return Some(2);
    }
    if unshadowed == 0 && permits(mask, 3) {
        return Some(3);
    }
    if (unshadowed != 0 || shadowed != 0) && mask & 0xc != 0 {
        return Some(if permits(mask, 3) { 3 } else { 2 });
    }
    zero(mask)
}

/// Feature definition names differ from serialized material flag names for bit12:
/// MFAE selects EMaterialFlag::MFOE. There is no MFSU definition in this registry.
pub fn material_flag_bit(definition: &str) -> Option<u32> {
    Some(match definition {
        "MFTR" => 0,
        "MFMT" => 1,
        "MFSR" => 2,
        "MFSK" => 3,
        "MFVC" => 4,
        "MF1B" => 5,
        "MFAV" => 6,
        "MFIN" => 7,
        "MFCA" => 8,
        "MFIM" => 9,
        "MTSM" => 10,
        "MFRL" => 11,
        "MFAE" => 12,
        "MFOT" => 13,
        "MFCI" => 14,
        "MFOI" => 15,
        "MFVA" => 16,
        "MFBP" => 18,
        "MFBL" => 19,
        "MFLB" => 20,
        "MF1E" => 21,
        "MFC0" => 22,
        "MFC1" => 23,
        "MFC2" => 24,
        "MFC3" => 25,
        "MFC4" => 26,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_provider_skips_null_but_cannot_guess_unresolved_values() {
        let mut first = MaterialSelectionState::default();
        first.flags = 1;
        first.set(EMaterialDataId::PMOD, SelectionValue::Null);
        let mut second = MaterialSelectionState::default();
        second.flags = 128;
        second.set(EMaterialDataId::PMOD, SelectionValue::Int(3));
        first.append_fallback(&second);
        assert_eq!(first.flags, 129);
        assert_eq!(first.select_definition("PMOD", &["PMOD"], 15).unwrap(), Some(3));
        first.set(EMaterialDataId::PMOD, SelectionValue::Int(2));
        first.append_fallback(&second);
        assert_eq!(first.select_definition("PMOD", &["PMOD"], 15).unwrap(), Some(2));
        first.set(EMaterialDataId::PMOD, SelectionValue::Present);
        first.append_fallback(&second);
        assert!(first.select_definition("PMOD", &["PMOD"], 15).is_err());
    }

    #[test]
    fn defx_uses_first_two_ids_despite_ghidra_pair_load_error() {
        let mut state = MaterialSelectionState::default();
        state.set(EMaterialDataId::DFXS, SelectionValue::Present);
        state.set(EMaterialDataId::DFXN, SelectionValue::Present);
        assert_eq!(state.select_definition("DEFX", &["DFXS", "DFXN"], 3).unwrap(), Some(1));
        assert_eq!(state.select_definition("DEFX", &["DFXS", "DFXN", "DIFT"], 3).unwrap(), Some(1));
        assert_eq!(state.select_definition("DEFX", &["DFXS", "DIFT", "DFXN"], 3).unwrap(), Some(0));
    }

    #[test]
    fn presence_null_and_unresolved_getters_are_distinct() {
        let mut state = MaterialSelectionState::default();
        state.set(EMaterialDataId::DFXS, SelectionValue::Null);
        assert_eq!(state.select_definition("POD_", &["DFXS"], 3).unwrap(), Some(1));
        assert_eq!(state.select_definition("FLT4", &["DFXS"], 3).unwrap(), Some(0));
        state.set(EMaterialDataId::DIFT, SelectionValue::Present);
        assert_eq!(state.select_definition("TEXS", &["DIFT"], 3).unwrap(), Some(1));
        assert!(state.select_definition("TEXS", &["DIFT"], 15).is_err());
        state.set(EMaterialDataId::DIFT, SelectionValue::Null);
        assert_eq!(state.select_definition("TEXS", &["DIFT"], 15).unwrap(), Some(1));
        state.remove(EMaterialDataId::DIFT);
        assert_eq!(state.select_definition("TEXS", &["DIFT"], 15).unwrap(), Some(0));
        assert!(state.select_definition("UNKNOWN", &[], 1).is_err());
    }

    #[test]
    fn missing_data_fallbacks_and_enum_shift_boundary_match_arm64() {
        let mut state = MaterialSelectionState::default();
        assert_eq!(state.select_definition("AMBS", &["ALLD"], 2).unwrap(), Some(1));
        assert_eq!(state.select_definition("HFUD", &["XCH0"], 2).unwrap(), Some(1));
        assert_eq!(state.select_definition("CLPP", &["CLLD"], 4).unwrap(), Some(2));
        assert_eq!(state.select_definition("REFL", &["REFP"], 2).unwrap(), None);
        state.flags = 1 << 12;
        assert_eq!(state.select_definition("MFAE", &[], 3).unwrap(), Some(1));
        state.set(EMaterialDataId::PMOD, SelectionValue::Int(32));
        assert_eq!(state.select_definition("PMOD", &["PMOD"], 1).unwrap(), Some(32));
        assert_eq!(state.select_definition("PMOD", &["PMOD"], 2).unwrap(), None);
        state.set(EMaterialDataId::PMOD, SelectionValue::Int(-1));
        assert_eq!(state.select_definition("PMOD", &["PMOD"], u32::MAX).unwrap(), None);
    }

    #[test]
    fn layer_tangent_fallback_is_definition_order_not_local_order() {
        let mut state = MaterialSelectionState::default();
        state.set(EMaterialDataId::NRML, SelectionValue::LayeredTexture {
            blend: ELayeredTextureBlend::Separate,
            tangent_flags: 3,
        });
        assert_eq!(state.select_definition("LAYR", &["NRML"], 0x7ff).unwrap(), Some(10));
        assert_eq!(state.select_definition("LAYR", &["NRML"], 0x3ff).unwrap(), Some(7));
        assert_eq!(state.select_definition("LAYR", &["NRML"], 0x7f).unwrap(), Some(4));
        assert_eq!(state.select_definition("LAYR", &["NRML"], 1).unwrap(), Some(0));
    }
}
