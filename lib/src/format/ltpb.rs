use std::{io::Cursor, marker::PhantomData};

use anyhow::{bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor,
    rfrm::FormDescriptor,
    txtr::{STextureMetaData, TextureData},
    CVector3f, CVector3i, FourCC, TaggedVec,
};

// Texture
pub const K_FORM_LTPB: FourCC = FourCC(*b"LTPB");

// Probe header
pub const K_CHUNK_PHDR: FourCC = FourCC(*b"PHDR");
// Probe texture
pub const K_CHUNK_PTEX: FourCC = FourCC(*b"PTEX");

#[binrw]
#[derive(Clone, Debug)]
pub struct CBakedLightingUniformProbeGridIndex {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LightProbeBundleHeader {
    pub unk1: u32,
    pub unk2: u32,
    pub unk_vec: CVector3f,
    pub grid_idx1: CBakedLightingUniformProbeGridIndex,
    pub grid_idx2: CBakedLightingUniformProbeGridIndex,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LightProbeExtra {
    pub vec: CVector3i,
    pub unk: u32,
}

#[binrw]
#[derive(Clone, Debug)]
struct SLightProbeMetaData {
    unk1: u32,
    unk2: u32,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    meta_offsets: Vec<u64>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    txtr_offsets: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct LightProbeData<O: ByteOrder> {
    pub head: LightProbeBundleHeader,
    pub textures: Vec<TextureData<O>>,
    pub extra: Vec<LightProbeExtra>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> LightProbeData<O> {
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (ltpb_desc, mut ltpb_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(ltpb_desc.id == K_FORM_LTPB);
        ensure!(ltpb_desc.reader_version.get() == 66);
        ensure!(ltpb_desc.writer_version.get() == 73);

        let meta: SLightProbeMetaData = Cursor::new(meta).read_type(Endian::Little)?;
        ensure!(meta.meta_offsets.len() == meta.txtr_offsets.len());
        let texture_count = meta.meta_offsets.len();

        let mut head: Option<LightProbeBundleHeader> = None;
        while !ltpb_data.is_empty() {
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(ltpb_data)?;
            let mut reader = Cursor::new(chunk_data);
            match chunk_desc.id {
                K_CHUNK_PHDR => head = Some(reader.read_type(Endian::Little)?),
                K_CHUNK_PTEX => {}
                id => bail!("Unknown LTPB chunk ID {id:?}"),
            }
            ltpb_data = remain;
        }
        let Some(head) = head else { bail!("Failed to locate PHDR") };

        let mut textures = Vec::with_capacity(texture_count);
        let mut extra: Vec<LightProbeExtra> = Vec::with_capacity(texture_count);
        for (meta_offset, txtr_offset) in meta.meta_offsets.into_iter().zip(meta.txtr_offsets) {
            let meta = &data[meta_offset as usize..];

            // Skip metadata to read extra fields
            let mut reader = Cursor::new(meta);
            reader.read_type::<STextureMetaData>(Endian::Little)?;
            extra.push(reader.read_type(Endian::Little)?);

            textures.push(TextureData::<O>::slice(&data[txtr_offset as usize..], meta)?);
        }
        Ok(Self { head, textures, extra, _marker: PhantomData })
    }
}
