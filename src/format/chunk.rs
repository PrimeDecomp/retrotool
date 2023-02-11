use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{anyhow, Result};
use binrw::{binrw, io::Cursor, BinReaderExt, BinResult, BinWriterExt, Endian};

use crate::format::{
    adir::{AssetDirectory, K_CHUNK_ADIR},
    meta::{Metadata, K_CHUNK_META},
    strg::{StringTable, K_CHUNK_STRG},
    FourCC,
};

#[binrw]
#[derive(Clone, Debug)]
pub struct ChunkDescriptor {
    pub id: FourCC,
    pub size: u64,
    pub unk: u32,
    // game skips this amount of bytes before continuing
    // but always 0?
    pub skip: u64,
}

pub const CHUNK_DESCRIPTOR_SIZE: usize = 24;

impl ChunkDescriptor {
    #[inline]
    pub fn read<R: Read + Seek>(reader: &mut R, e: Endian) -> BinResult<Self> {
        let desc: ChunkDescriptor = reader.read_type(e)?;
        reader.seek(SeekFrom::Current(desc.skip as i64))?;
        Ok(desc)
    }

    #[inline]
    pub fn slice(data: &[u8], e: Endian) -> BinResult<(Self, &[u8], &[u8])> {
        let mut reader = Cursor::new(data);
        let header = Self::read(&mut reader, e)?;
        let start = reader.position();
        let slice = &data[start as usize..(start + header.size) as usize];
        let remain = &data[(start + header.size) as usize..];
        Ok((header, slice, remain))
    }

    pub fn write<W: Write + Seek, CB>(&mut self, w: &mut W, e: Endian, mut cb: CB) -> Result<()>
    where CB: FnMut(&mut W) -> Result<()> {
        let form_pos = w.stream_position()?;
        w.write_type(self, e)?;
        let data_pos = w.stream_position()?;
        cb(w)?;
        let end_pos = w.stream_position()?;
        w.seek(SeekFrom::Start(form_pos))?;
        self.size = end_pos - data_pos;
        w.write_type(self, e)?;
        w.seek(SeekFrom::Start(end_pos))?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum ChunkType {
    AssetDirectory(AssetDirectory),
    Metadata(Metadata),
    StringTable(StringTable),
}

impl ChunkType {
    #[inline]
    pub fn read(data: &[u8], kind: FourCC, e: Endian) -> Result<Self> {
        let mut reader = Cursor::new(data);
        match kind {
            K_CHUNK_ADIR => Ok(Self::AssetDirectory(reader.read_type(e)?)),
            K_CHUNK_META => Ok(Self::Metadata(reader.read_type(e)?)),
            K_CHUNK_STRG => Ok(Self::StringTable(reader.read_type(e)?)),
            _ => Err(anyhow!("Unknown chunk type {:?}", kind)),
        }
    }
}
