use binrw::binrw;
use uuid::Uuid;

use crate::format::FourCC;

// Asset directory
pub const K_CHUNK_ADIR: FourCC = FourCC(*b"ADIR");

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct AssetDirectory {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<AssetDirectoryEntry>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct AssetDirectoryEntry {
    pub asset_type: FourCC,
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    pub version: u32,
    pub other_version: u32,
    pub offset: u64,
    pub decompressed_size: u64,
    pub size: u64,
}
