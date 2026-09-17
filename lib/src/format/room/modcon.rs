use std::io::Cursor;

use anyhow::{ensure, Result};
use binrw::BinReaderExt;

use super::{super::room_lighting::ScriptProperty, CDataEnumBitField, CObjectId};

pub const MODCON_COMPONENT: u32 = 0x451740eb;

/// SLdrModCon definitions. Loading the resource does not establish whether its
/// owning Entity or layer is active. Native EntityLoaded uses the area's
/// transform, independently of the Entity's authored transform.
#[derive(Clone, Debug, Default)]
pub struct ModConProperties {
    pub resource_id: CObjectId,
    /// Authored data-enum selection; its consumer is not yet represented here.
    pub flags: Option<CDataEnumBitField>,
    /// RenderTargetScene CRC; absent and unrecognized values use runtime scene0.
    pub render_scene: Option<u32>,
    /// Property61be7d93, default false. Runtime meaning remains unresolved.
    pub unknown_flag: bool,
    pub properties: Vec<ScriptProperty>,
}

impl ModConProperties {
    /// Parse the component payload, without its component-type prefix. Native
    /// SLdrModCon::Load71007a4930 applies repeated properties in stream order.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for property in &properties {
            match property.id {
                0xa8e2ba93 => {
                    ensure!(property.data.len() == 16, "Wrong size for ModCon resource ID");
                    result.resource_id = Cursor::new(&property.data).read_le()?;
                }
                0xf068d36b => {
                    let mut reader = Cursor::new(&property.data);
                    result.flags = Some(reader.read_le()?);
                    ensure!(
                        reader.position() == property.data.len() as u64,
                        "Trailing ModCon data-enum bytes"
                    );
                }
                0x356db82b => result.render_scene = Some(property.words::<1>()?[0]),
                0x61be7d93 => result.unknown_flag = property.boolean()?,
                _ => {}
            }
        }
        result.properties = properties;
        Ok(result)
    }

    pub fn runtime_render_scene(&self) -> u32 {
        match self.render_scene {
            Some(0x90130085) => 1,
            Some(0x986d766d) => 2,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_properties_keep_native_defaults_and_stream_order() {
        let defaults = ModConProperties::parse(&[0; 2]).unwrap();
        assert!(defaults.resource_id.is_nil());
        assert!(defaults.flags.is_none());
        assert!(!defaults.unknown_flag);
        assert_eq!(defaults.runtime_render_scene(), 0);

        let mut data = 5u16.to_le_bytes().to_vec();
        for (id, bytes) in [
            (0x356db82bu32, 0x90130085u32.to_le_bytes().to_vec()),
            (0x61be7d93, vec![255]),
            (0x356db82b, 0x986d766du32.to_le_bytes().to_vec()),
            (0xa8e2ba93, uuid::Uuid::from_u128(42).to_bytes_le().to_vec()),
            (0x12345678, vec![1, 2, 3]),
        ] {
            data.extend(id.to_le_bytes());
            data.extend((bytes.len() as u16).to_le_bytes());
            data.extend(bytes);
        }
        let properties = ModConProperties::parse(&data).unwrap();
        assert_eq!(properties.resource_id.into_inner(), uuid::Uuid::from_u128(42));
        assert_eq!(properties.runtime_render_scene(), 2);
        assert!(properties.unknown_flag);
        assert_eq!(properties.properties[4].data, [1, 2, 3]);
        for end in 2..data.len() {
            assert!(ModConProperties::parse(&data[..end]).is_err());
        }
    }
}
