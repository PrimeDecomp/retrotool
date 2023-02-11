use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use anyhow::Result;
use binrw::{binrw, BinReaderExt, BinResult, BinWriterExt, Endian};

use crate::format::FourCC;

// Resource format
pub const K_CHUNK_RFRM: FourCC = FourCC(*b"RFRM");
// Package file
pub const K_FORM_PAK: FourCC = FourCC(*b"PACK");
// Table of contents
pub const K_FORM_TOC: FourCC = FourCC(*b"TOCC");

#[binrw]
#[brw(magic = b"RFRM")]
#[derive(Clone, Debug)]
pub struct FormDescriptor {
    pub size: u64,
    pub unk1: u64,
    pub id: FourCC,
    pub version: u32,
    pub other_version: u32,
}

impl FormDescriptor {
    #[inline]
    pub fn read<R: Read + Seek>(reader: &mut R, e: Endian) -> BinResult<Self> {
        reader.read_type(e)
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
