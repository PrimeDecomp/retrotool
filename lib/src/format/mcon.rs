use std::{io::Cursor, marker::PhantomData};

use anyhow::{bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use binrw_derive::binread;
use uuid::Uuid;
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor, peek_four_cc, rfrm::FormDescriptor, CColor4f, CTransform4f, FourCC,
    TaggedVec,
};

// Texture
pub const K_FORM_MCON: FourCC = FourCC(*b"MCON");

const K_CHUNK_MCVD: FourCC = FourCC(*b"MCVD");
const K_CHUNK_MCHD: FourCC = FourCC(*b"MCHD");
const K_CHUNK_MCCD: FourCC = FourCC(*b"MCCD");

#[binrw]
#[derive(Clone, Debug)]
#[allow(unused)]
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

#[binrw]
#[derive(Clone, Debug)]
pub struct SUnknown {
    pub f0: f32,
    pub f1: f32,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub ints: Vec<u32>,
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
    pub unknowns: Vec<SUnknown>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_2: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_3: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_4: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_5: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_6: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub shorts_1: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub shorts_2: Vec<u16>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_7: Vec<u8>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bytes_8: Vec<u8>,
    // TODO
}

#[derive(Debug, Clone)]
pub struct ModConData<O: ByteOrder> {
    pub visual_data: Option<SModConVisualData>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> ModConData<O> {
    pub fn slice(data: &[u8]) -> Result<Self> {
        let (mcon_desc, mut mcon_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(mcon_desc.id == K_FORM_MCON);
        ensure!(mcon_desc.reader_version.get() == 72);
        ensure!(mcon_desc.writer_version.get() == 72);

        let mut data = Self { visual_data: None, _marker: PhantomData };
        while !mcon_data.is_empty() {
            if peek_four_cc(mcon_data) == *b"PEEK" {
                break;
            }
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(mcon_data)?;
            match chunk_desc.id {
                K_CHUNK_MCVD => {
                    data.visual_data = Some(Cursor::new(chunk_data).read_type(Endian::Little)?)
                }
                K_CHUNK_MCHD => { /* TODO */ }
                K_CHUNK_MCCD => { /* TODO */ }
                id => bail!("Unknown MCON chunk ID {id:?}"),
            }
            mcon_data = remain;
        }
        Ok(data)
    }
}
