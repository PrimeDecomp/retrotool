use binrw::binrw;
use uuid::Uuid;

use crate::format::FourCC;

// Metadata
pub const K_CHUNK_META: FourCC = FourCC(*b"META");

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<MetadataEntry>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct MetadataEntry {
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    pub offset: u32,
}
