//! Standalone MPR material instances (MATI reader 11, writer 13).

use std::io::Cursor;

use anyhow::{bail, ensure, Context, Result};
use binrw::BinReaderExt;
use uuid::Uuid;
use zerocopy::LittleEndian;

use super::data::{
    CMaterialData, CMaterialDataInner, CMaterialTextureTokenData, CMaterialVariableDesc,
    EMaterialDataType, EMaterialFlag,
};
use crate::format::{chunk::ChunkDescriptor, rfrm::FormDescriptor, txtr::ETextureType, FourCC};

pub const K_FORM_MATI: FourCC = FourCC(*b"MATI");
const K_CHUNK_MDAT: FourCC = FourCC(*b"MDAT");

/// MATI's byte tags are distinct from the FourCC types used by model caches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PropertyKind {
    Int = 2,
    Color = 3,
    Float4 = 4,
    Texture2d = 5,
    Texture2dArray = 6,
    TextureCube = 7,
    Texture3d = 8,
    Int4 = 10,
}

/// One property, retaining its serialized tag alongside the common typed value.
#[derive(Clone, Debug)]
pub struct Property {
    kind: PropertyKind,
    data: CMaterialData,
}

impl Property {
    pub fn kind(&self) -> PropertyKind { self.kind }

    /// The value supplied to the material builder. For nil texture IDs, consult
    /// [`Self::texture_fallback`]; those values are present black textures.
    pub fn data(&self) -> &CMaterialData { &self.data }

    /// Native AddTextureRaw substitutes a black texture of this dimension for a
    /// nil ID. It does not apply the serialized usage overrides in that case.
    /// Non-nil IDs use the referenced TXTR's runtime type, not the MATI tag.
    pub fn texture_fallback(&self) -> Option<ETextureType> {
        let CMaterialDataInner::Texture(texture) = &self.data.data else { return None };
        if !texture.id.is_nil() {
            return None;
        }
        match self.kind {
            PropertyKind::Texture2d => Some(ETextureType::D2),
            PropertyKind::Texture2dArray => Some(ETextureType::D2Array),
            PropertyKind::TextureCube => Some(ETextureType::Cube),
            PropertyKind::Texture3d => Some(ETextureType::D3),
            _ => None,
        }
    }

    fn read(reader: &mut Cursor<&[u8]>) -> Result<Self> {
        let tag: u8 = reader.read_le()?;
        let kind = match tag {
            2 => PropertyKind::Int,
            3 => PropertyKind::Color,
            4 => PropertyKind::Float4,
            5 => PropertyKind::Texture2d,
            6 => PropertyKind::Texture2dArray,
            7 => PropertyKind::TextureCube,
            8 => PropertyKind::Texture3d,
            10 => PropertyKind::Int4,
            _ => bail!("Unsupported MATI property type {tag}"),
        };
        let data_id = reader.read_le()?;
        let (data_type, data) = match kind {
            PropertyKind::Int => {
                (EMaterialDataType::Int1, CMaterialDataInner::Int1(reader.read_le()?))
            }
            PropertyKind::Color | PropertyKind::Float4 => {
                (EMaterialDataType::Color, CMaterialDataInner::Color(reader.read_le()?))
            }
            PropertyKind::Texture2d
            | PropertyKind::Texture2dArray
            | PropertyKind::TextureCube
            | PropertyKind::Texture3d => (
                EMaterialDataType::Texture,
                CMaterialDataInner::Texture(CMaterialTextureTokenData {
                    id: Uuid::from_bytes_le(reader.read_le()?),
                    usage: Some(reader.read_le()?),
                }),
            ),
            PropertyKind::Int4 => {
                (EMaterialDataType::Int4, CMaterialDataInner::Int4(reader.read_le()?))
            }
        };
        Ok(Self { kind, data: CMaterialData { data_id, data_type, data } })
    }
}

/// Static values and render-state requests; defaults belong to the consuming
/// renderer (decals, particles, etc.), not to the resource parser.
#[derive(Clone, Debug)]
pub struct MaterialInstance {
    pub material_id: Uuid,
    pub material_resource_id: Uuid,
    pub material_set_hash: u32,
    /// ETrait bit mask, unlike the FourCC trait list in a model cache.
    pub traits: u8,
    /// Serialized ECullMode. The native resource stores its low byte; 0xff is unset.
    pub cull_mode: i32,
    /// Serialized EBlendMode; -1 is unset.
    pub blend_mode: i32,
    pub variable_descs: Vec<CMaterialVariableDesc>,
    pub properties: Vec<Property>,
    /// Preserve names and order, including names unknown to the native registry.
    pub material_flags: Vec<FourCC>,
}

impl MaterialInstance {
    /// Parse the little-endian MPR resource. A following asset footer is allowed;
    /// the form itself must contain exactly one fully consumed MDAT chunk.
    pub fn slice(data: &[u8]) -> Result<Self> {
        let (form, payload, _) = FormDescriptor::<LittleEndian>::slice(data)?;
        ensure!(form.id == K_FORM_MATI, "Expected MATI form");
        ensure!(
            form.reader_version.get() == 11 && form.writer_version.get() == 13,
            "Unsupported MATI version {}/{}",
            form.reader_version.get(),
            form.writer_version.get()
        );
        let (chunk, data, rest) = ChunkDescriptor::<LittleEndian>::slice(payload)?;
        ensure!(chunk.id == K_CHUNK_MDAT, "Expected MATI MDAT chunk");
        ensure!(rest.is_empty(), "Unexpected data after MATI MDAT chunk");
        Self::read_data(data).context("MATI MDAT")
    }

    /// The native loader stops at the first unknown flag name. Later names remain
    /// available in `material_flags`, but do not contribute to this runtime mask.
    pub fn flags(&self) -> u32 {
        self.material_flags
            .iter()
            .map_while(|name| Cursor::new(name.0).read_le::<EMaterialFlag>().ok())
            .fold(0, |mask, flag| mask | (1 << flag as u32))
    }

    fn read_data(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let material_id = Uuid::from_bytes_le(reader.read_le()?);
        let material_resource_id = Uuid::from_bytes_le(reader.read_le()?);
        let material_set_hash = reader.read_le()?;
        let traits = reader.read_le()?;
        let cull_mode = reader.read_le()?;
        let blend_mode = reader.read_le()?;

        let variable_count = read_count(&mut reader, 10, "variable")?;
        let variable_descs =
            (0..variable_count).map(|_| reader.read_le()).collect::<Result<Vec<_>, _>>()?;

        let property_count = read_count(&mut reader, 9, "property")?;
        let properties = (0..property_count)
            .map(|index| Property::read(&mut reader).with_context(|| format!("property {index}")))
            .collect::<Result<Vec<_>>>()?;

        let flag_count = read_count(&mut reader, 4, "flag")?;
        let material_flags =
            (0..flag_count).map(|_| reader.read_le()).collect::<Result<Vec<_>, _>>()?;
        ensure!(reader.position() == data.len() as u64, "Unconsumed MATI property data");
        Ok(Self {
            material_id,
            material_resource_id,
            material_set_hash,
            traits,
            cull_mode,
            blend_mode,
            variable_descs,
            properties,
            material_flags,
        })
    }
}

/// Bound counts against the remaining bytes before allocating any arrays.
fn read_count(reader: &mut Cursor<&[u8]>, minimum_size: u64, name: &str) -> Result<u32> {
    let count: u32 = reader.read_le()?;
    let remaining = reader.get_ref().len() as u64 - reader.position();
    ensure!(u64::from(count) <= remaining / minimum_size, "MATI {name} count exceeds chunk");
    Ok(count)
}

#[cfg(test)]
mod tests;
