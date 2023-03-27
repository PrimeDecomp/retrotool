use std::io::{Cursor, Read};

use anyhow::{ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use flate2::bufread::ZlibDecoder;

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
pub struct MaterialData {
    pub decompressed: Vec<u8>,
}

impl MaterialData {
    pub fn slice(data: &[u8], meta: &[u8], e: Endian) -> Result<MaterialData> {
        let (mtrl_desc, _, _) = FormDescriptor::slice(data, e)?;
        ensure!(mtrl_desc.id == K_FORM_MTRL);
        ensure!(mtrl_desc.reader_version == 168);
        ensure!(mtrl_desc.writer_version == 168);

        let meta: SMaterialMetaData = Cursor::new(meta).read_type(e)?;
        let mut reader = ZlibDecoder::new(
            &data[meta.file_offset as usize..(meta.file_offset + meta.compressed_size) as usize],
        );
        let mut decompressed = vec![0u8; meta.decompressed_size as usize];
        reader.read_exact(&mut decompressed)?;

        Ok(MaterialData { decompressed })
    }
}
