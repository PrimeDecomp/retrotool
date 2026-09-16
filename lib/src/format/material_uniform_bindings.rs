//! Ordinary material values consumed by the INT1/INT4/FLT1/FLT4/MAT4 binders.
//! These features use their first data ID and bind only option1; flags are ignored.
//! Missing typed provider data makes the native callback return false/no write.

use anyhow::{bail, ensure, Context, Result};
use serde_derive::Serialize;

use super::{
    cmdl::{CMaterialData, CMaterialDataInner, EMaterialDataId},
    material_features::FeatureVariable,
    shader_nvn_uniforms::NvnUniformValue,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MaterialUniformKind {
    Int,
    Int4,
    Float,
    Color,
    Matrix4,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct MaterialUniformBinding<'a> {
    pub data_id: &'a str,
    pub kind: MaterialUniformKind,
}

impl FeatureVariable<'_> {
    /// Enabled source request, independent of runtime provider presence. Packed
    /// 0xffff is retained here: native callbacks forward it to the sink. Callers
    /// must validate the reflected uniform before attempting a buffer write.
    pub fn material_uniform_binding(&self) -> Result<Option<MaterialUniformBinding<'_>>> {
        if self.definition_option != 1 {
            return Ok(None);
        }
        let kind = match self.definition {
            "INT1" => MaterialUniformKind::Int,
            "INT4" => MaterialUniformKind::Int4,
            "FLT1" => MaterialUniformKind::Float,
            "FLT4" => MaterialUniformKind::Color,
            "MAT4" => MaterialUniformKind::Matrix4,
            _ => return Ok(None),
        };
        ensure!(self.semantic == "Value", "Invalid ordinary uniform semantic");
        Ok(Some(MaterialUniformBinding {
            data_id: self.data_ids.first().copied().context("Uniform feature needs a data ID")?,
            kind,
        }))
    }
}

impl MaterialUniformBinding<'_> {
    /// Convert already resolved typed material data. Does not merge providers or
    /// substitute defaults for missing values. MAT4 keeps native row order here;
    /// NvnUniformValue applies the NVN binder's transpose when producing bytes.
    pub fn value_from_data(&self, data: &CMaterialData) -> Result<NvnUniformValue<'static>> {
        ensure!(
            data.data_id == EMaterialDataId::from_fourcc(self.data_id)?,
            "Material uniform data ID mismatch"
        );
        Ok(match (self.kind, &data.data) {
            (MaterialUniformKind::Int, CMaterialDataInner::Int1(v)) => NvnUniformValue::Int(*v),
            (MaterialUniformKind::Int4, CMaterialDataInner::Int4(v)) => {
                NvnUniformValue::Int4(v.to_array())
            }
            (MaterialUniformKind::Float, CMaterialDataInner::Scalar(v)) => {
                NvnUniformValue::Float(*v)
            }
            (MaterialUniformKind::Color, CMaterialDataInner::Color(v)) => {
                NvnUniformValue::Vector4(v.to_array())
            }
            (MaterialUniformKind::Matrix4, CMaterialDataInner::Mat4(v)) => {
                NvnUniformValue::Matrix4(v.m)
            }
            _ => bail!("Material uniform source has the wrong data type"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{cmdl::EMaterialDataType, CColor4f};

    #[test]
    fn color_preserves_raw_components_and_checks_source_identity() -> Result<()> {
        let binding = MaterialUniformBinding { data_id: "DIFC", kind: MaterialUniformKind::Color };
        let mut data = CMaterialData {
            data_id: EMaterialDataId::from_fourcc("DIFC")?,
            data_type: EMaterialDataType::Color,
            data: CMaterialDataInner::Color(CColor4f {
                r: f32::from_bits(0x7fc12345),
                g: -0.0,
                b: 2.0,
                a: -1.0,
            }),
        };
        let bytes = binding.value_from_data(&data)?.bytes().into_owned();
        assert_eq!(
            bytes,
            [0x7fc12345u32, 0x80000000, 0x40000000, 0xbf800000]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>()
        );
        data.data_id = EMaterialDataId::from_fourcc("SPCC")?;
        assert!(binding.value_from_data(&data).is_err());
        data.data_id = EMaterialDataId::from_fourcc("DIFC")?;
        data.data = CMaterialDataInner::Scalar(1.0);
        assert!(binding.value_from_data(&data).is_err());
        Ok(())
    }
}
