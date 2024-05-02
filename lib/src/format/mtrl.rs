use std::{
    io::{Cursor, Read},
    marker::PhantomData,
};

use anyhow::{ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use flate2::bufread::ZlibDecoder;
use zerocopy::ByteOrder;

use crate::format::{rfrm::FormDescriptor, FourCC};

// Texture
pub const K_FORM_MTRL: FourCC = FourCC(*b"MTRL");

#[binrw]
#[derive(Clone, Debug)]
struct SMaterialMetaData {
    unk1: u32, // count?
    unk2: u32, // reader version?
    compressed_size: u32,
    decompressed_size: u32,
    file_offset: u32,
}

#[derive(Debug, Clone)]
pub struct MaterialData<O: ByteOrder> {
    pub decompressed: Vec<u8>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> MaterialData<O> {
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (mtrl_desc, _, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(mtrl_desc.id == K_FORM_MTRL);
        ensure!(mtrl_desc.reader_version.get() == 168);
        ensure!(mtrl_desc.writer_version.get() == 168);

        let meta: SMaterialMetaData = Cursor::new(meta).read_type(Endian::Little)?;
        let mut reader = ZlibDecoder::new(
            &data[meta.file_offset as usize..(meta.file_offset + meta.compressed_size) as usize],
        );
        let mut decompressed = vec![0u8; meta.decompressed_size as usize];
        reader.read_exact(&mut decompressed)?;

        Ok(Self { decompressed, _marker: PhantomData })
    }
}
