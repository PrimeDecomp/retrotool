use binrw::binrw;
use uuid::Uuid;

use crate::format::FourCC;

// String table
pub const K_CHUNK_STRG: FourCC = FourCC(*b"STRG");

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct StringTable {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<StringTableEntry>,
}

impl StringTable {
    pub fn name_for_uuid(&self, id: Uuid) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.asset_id == id)
            .map(|e| String::from_utf8(e.name.clone()).unwrap())
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct StringTableEntry {
    #[br(map = FourCC::swap)]
    #[bw(map = |&f| f.swap())]
    pub kind: FourCC,
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    #[bw(try_calc = name.len().try_into())]
    pub name_length: u32,
    #[br(count = name_length)]
    pub name: Vec<u8>,
}
