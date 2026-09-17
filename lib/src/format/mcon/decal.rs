//! Geometry-decal placements in MCDD, separate from the CMDL visual tables.

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use binrw::BinReaderExt;
use binrw_derive::binread;
use uuid::Uuid;

use crate::format::CTransform4f;

/// One geometry decal. The native reader packs this record into 73 bytes on
/// disk; its aligned in-memory representation occupies 76 bytes.
#[binread]
#[derive(Clone, Debug)]
pub struct Decal {
    #[br(map = Uuid::from_bytes_le)]
    pub id: Uuid,
    /// Local placement, including scale. The owner composes its transform first,
    /// then extracts axis scales and halves them to obtain projection extents.
    pub transform: CTransform4f,
    /// Index into [`DecalData::materials`].
    pub material_index: u16,
    /// Passed as a CRelAngle; geometry generation uploads its cosine.
    pub angle_radians: f32,
    /// The high half of the geometry-decal sort key.
    pub sort_order: i16,
    /// Read by CModConDecal, but not consumed by BuildModConDecals.
    pub unknown: u8,
}

/// Shared MATI resources and their decal placements. The remaining header words
/// are consumed but ignored by the native loader; preserve them without assigning
/// a meaning or requiring the zero values found in current assets.
#[derive(Clone, Debug)]
pub struct DecalData {
    pub materials: Vec<Uuid>,
    pub decals: Vec<Decal>,
    pub unknown_short: u16,
    pub unknown_words: [u32; 2],
}

#[binread]
struct Header {
    decal_count: u32,
    material_count: u16,
    unknown_short: u16,
    unknown_words: [u32; 2],
}

impl DecalData {
    pub(super) fn slice(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let header: Header = reader.read_le().context("MCON decal header")?;
        // Validate the complete packed length before counts can allocate memory.
        let expected_size =
            16 + u64::from(header.material_count) * 16 + u64::from(header.decal_count) * 73;
        ensure!(expected_size == data.len() as u64, "MCON decal chunk size differs from counts");
        let materials = (0..header.material_count)
            .map(|_| reader.read_le().map(Uuid::from_bytes_le))
            .collect::<Result<Vec<_>, _>>()?;
        let decals = (0..header.decal_count)
            .map(|_| reader.read_le::<Decal>())
            .collect::<Result<Vec<_>, _>>()?;
        for (index, decal) in decals.iter().enumerate() {
            ensure!(
                usize::from(decal.material_index) < materials.len(),
                "MCON decal {index} material index {} is out of range",
                decal.material_index
            );
        }
        Ok(Self {
            materials,
            decals,
            unknown_short: header.unknown_short,
            unknown_words: header.unknown_words,
        })
    }
}

#[cfg(test)]
mod tests;
