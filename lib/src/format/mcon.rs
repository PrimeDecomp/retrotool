use std::io::Cursor;

use anyhow::{ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use binrw_derive::binread;
use uuid::Uuid;

use crate::format::{
    chunk::ChunkDescriptor, peek_four_cc, rfrm::FormDescriptor, CColor4f, CTransform4f, FourCC,
    TaggedVec,
};

// Texture
pub const K_FORM_MCON: FourCC = FourCC(*b"MCON");

const K_CHUNK_MCVD: FourCC = FourCC(*b"MCVD");

#[binrw]
#[derive(Clone, Debug)]
struct SModConHeader {
    unk: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ObjectTransform {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub id: Uuid,
    pub xf: CTransform4f,
}

#[binread]
#[derive(Clone, Debug)]
pub struct SModConVisualData {
    #[br(map = |v: TaggedVec<u32, uuid::Bytes>| v.data.into_iter().map(Uuid::from_bytes_le).collect())]
    pub models: Vec<Uuid>,
    #[br(map = |v: TaggedVec<u32, uuid::Bytes>| v.data.into_iter().map(Uuid::from_bytes_le).collect())]
    pub ids_2: Vec<Uuid>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub colors: Vec<CColor4f>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub transforms: Vec<CTransform4f>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub object_transforms: Vec<ObjectTransform>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_1: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub shorts_1: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub shorts_2: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_2: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_3: Vec<u8>,
    // TODO
}

#[derive(Debug, Clone)]
pub struct ModConData {
    pub visual_data: Option<SModConVisualData>,
}

impl ModConData {
    pub fn slice(data: &[u8], e: Endian) -> Result<Self> {
        let (mcon_desc, mut mcon_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(mcon_desc.id == K_FORM_MCON);
        ensure!(mcon_desc.reader_version == 41);
        ensure!(mcon_desc.writer_version == 44);

        let mut data = ModConData { visual_data: None };
        while !mcon_data.is_empty() {
            if peek_four_cc(mcon_data) == *b"PEEK" {
                break;
            }
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::slice(mcon_data, e)?;
            if chunk_desc.id == K_CHUNK_MCVD {
                data.visual_data = Some(Cursor::new(chunk_data).read_type(e)?);
            }
            mcon_data = remain;
        }
        Ok(data)
    }
}
