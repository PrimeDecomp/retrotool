//! Material inputs consumed by MPR's TEXS and LAYR feature binders.
//!
//! This connects selected feature semantics to their source material texture
//! records. Runtime instance/provider resolution, resource readiness and actual
//! shader binding locations remain separate. Other feature definitions may bind
//! textures through scene data; None here does not mean they have no resources.

use anyhow::{bail, ensure, Context, Result};
use serde_derive::Serialize;

use super::{
    cmdl::{CMaterialData, CMaterialDataInner, CMaterialTextureTokenData, EMaterialDataId},
    material_features::FeatureVariable,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MaterialTextureValue {
    Texture {
        bindless: bool,
    },
    TexCoordIndex,
    /// Float4: width, height, reciprocal width, reciprocal height of the texture
    /// token. Native reciprocals use FRECPE and two refinement steps.
    SizeParams,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialTextureBinding<'a> {
    pub data_id: &'a str,
    /// None selects a TXTR material value. Some selects Texture0/1/BlendTexture
    /// from a layered complex value, independent of its tangent-frame option.
    pub layer: Option<u8>,
    pub value: MaterialTextureValue,
}

impl FeatureVariable<'_> {
    /// Enabled source requests for the native TEXS/LAYR callbacks. Actual writes
    /// also depend on provider presence and texture readiness: absent material
    /// data produces no writes; UV writes can survive a null graphics token,
    /// while texture/size writes require one. This method does not emulate that
    /// runtime state or the callback's success return.
    /// TEXS dispatcher0x71001645a0; LAYR dispatcher0x710016f8d4 and callbacks
    /// 0x710016ff70/0x7100170134/0x7100170464. No shader dimension is inferred.
    /// Unbound packed variables still return a request: the native feature can
    /// query material data and forward them. The NVN uniform sink does not check
    /// 0xffff; host code must validate before submitting a reflected write.
    pub fn material_texture_binding(&self) -> Result<Option<MaterialTextureBinding<'_>>> {
        let flags = self.binding_flags;
        let (layer, value) = match self.definition {
            "TEXS" => {
                ensure!(flags & !15 == 0, "Unknown TEXS binding flags {flags:#x}");
                if !(1..=10).contains(&self.definition_option) {
                    return Ok(None);
                }
                let value = match self.semantic {
                    "Texture" if flags & 8 == 0 => {
                        MaterialTextureValue::Texture { bindless: flags & 1 != 0 }
                    }
                    "TexCoordIndex" if flags & 2 != 0 => MaterialTextureValue::TexCoordIndex,
                    "TextureSizeParams" if flags & 4 != 0 => MaterialTextureValue::SizeParams,
                    _ => return Ok(None),
                };
                (None, value)
            }
            "LAYR" => {
                ensure!(flags & !127 == 0, "Unknown LAYR binding flags {flags:#x}");
                let count = match self.definition_option {
                    1 => 1,
                    2 | 3 | 5 | 6 | 8 | 9 => 2,
                    4 | 7 | 10 => 3,
                    _ => return Ok(None),
                };
                let (slot, value) = match self.semantic {
                    "Texture0" => (0, MaterialTextureValue::Texture { bindless: false }),
                    "Texture1" => (1, MaterialTextureValue::Texture { bindless: false }),
                    "BlendTexture" => (2, MaterialTextureValue::Texture { bindless: false }),
                    "TexCoordIndex0" => (0, MaterialTextureValue::TexCoordIndex),
                    "TexCoordIndex1" => (1, MaterialTextureValue::TexCoordIndex),
                    "BlendTexCoordIndex" => (2, MaterialTextureValue::TexCoordIndex),
                    "Texture0SizeParams" => (0, MaterialTextureValue::SizeParams),
                    "Texture1SizeParams" => (1, MaterialTextureValue::SizeParams),
                    "BlendTextureSizeParams" => (2, MaterialTextureValue::SizeParams),
                    _ => return Ok(None),
                };
                if slot >= count {
                    return Ok(None);
                }
                let enabled = match value {
                    MaterialTextureValue::Texture { .. } => {
                        // Native three-texture callback tests bit3 with the
                        // opposite sense to bits1/2, despite its name
                        // IgnoreBlendTexture. Verified original ARM64 execution.
                        if slot == 2 {
                            flags & 8 != 0
                        } else {
                            flags & (2 << slot) == 0
                        }
                    }
                    MaterialTextureValue::TexCoordIndex => flags & 1 != 0,
                    MaterialTextureValue::SizeParams => flags & (16 << slot) != 0,
                };
                if !enabled {
                    return Ok(None);
                }
                (Some(slot), value)
            }
            _ => return Ok(None),
        };
        let data_id = *self.data_ids.first().context("Texture feature needs a material data ID")?;
        Ok(Some(MaterialTextureBinding { data_id, layer, value }))
    }
}

impl MaterialTextureBinding<'_> {
    /// Select the texture reference/usage from an already resolved material
    /// value. Does not merge providers or treat a nil asset ID as a loaded token.
    pub fn texture_from_data<'a>(
        &self,
        data: &'a CMaterialData,
    ) -> Result<&'a CMaterialTextureTokenData> {
        ensure!(
            data.data_id == EMaterialDataId::from_fourcc(self.data_id)?,
            "Material texture data ID mismatch"
        );
        match (&data.data, self.layer) {
            (CMaterialDataInner::Texture(texture), None) => Ok(texture),
            (CMaterialDataInner::LayeredTexture(layers), Some(index)) => {
                layers.textures.get(usize::from(index)).context("Invalid material layer slot")
            }
            _ => bail!("Material texture source has the wrong data type"),
        }
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::format::cmdl::EMaterialDataType;

    fn variable(definition: &'static str, semantic: &'static str) -> FeatureVariable<'static> {
        FeatureVariable {
            feature_path: vec!["texture"],
            definition,
            data_ids: vec!["DIFT"],
            data_origin: 0,
            definition_option: 4,
            binding_flags: 0,
            semantic,
            binding: None,
            packed: u16::MAX,
        }
    }

    #[test]
    fn layered_blend_flag_retains_native_inverted_sense() {
        let mut blend = variable("LAYR", "BlendTexture");
        assert!(blend.material_texture_binding().unwrap().is_none());
        blend.binding_flags = 8;
        let request = blend.material_texture_binding().unwrap().unwrap();
        assert_eq!(request.layer, Some(2));
        assert_eq!(request.value, MaterialTextureValue::Texture { bindless: false });
        // The two ordinary ignore flags disable their own slots, while bit3
        // enables blend. Unbound variables still query their material source.
        for (semantic, flag) in [("Texture0", 2), ("Texture1", 4)] {
            let mut texture = variable("LAYR", semantic);
            assert!(texture.material_texture_binding().unwrap().is_some());
            texture.binding_flags = flag;
            assert!(texture.material_texture_binding().unwrap().is_none());
        }
        blend.definition_option = 2; // Embedded blend has no third texture.
        assert!(blend.material_texture_binding().unwrap().is_none());
        let mut size = variable("LAYR", "BlendTextureSizeParams");
        size.binding_flags = 64; // Independent of the blend-texture enable bit.
        assert_eq!(
            size.material_texture_binding().unwrap().unwrap().value,
            MaterialTextureValue::SizeParams
        );
    }

    #[test]
    fn ignored_texture_keeps_auxiliary_requests_and_preserves_nil_source() {
        let mut variable = variable("TEXS", "Texture");
        variable.binding_flags = 15;
        assert!(variable.material_texture_binding().unwrap().is_none());
        variable.semantic = "TexCoordIndex";
        let request = variable.material_texture_binding().unwrap().unwrap();
        assert_eq!(request.value, MaterialTextureValue::TexCoordIndex);
        let mut data = CMaterialData {
            data_id: EMaterialDataId::DIFT,
            data_type: EMaterialDataType::Texture,
            data: CMaterialDataInner::Texture(CMaterialTextureTokenData {
                id: Uuid::nil(),
                usage: None,
            }),
        };
        assert!(request.texture_from_data(&data).unwrap().id.is_nil());
        data.data_id = EMaterialDataId::BCRL;
        assert!(request.texture_from_data(&data).is_err());
        data.data_id = EMaterialDataId::DIFT;
        data.data = CMaterialDataInner::Int1(0);
        assert!(request.texture_from_data(&data).is_err());
        variable.binding_flags = 16;
        assert!(variable.material_texture_binding().is_err());
    }
}
