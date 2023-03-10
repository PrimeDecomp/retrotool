use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::Result;
use binrw::{binrw, io::Cursor, BinReaderExt, BinResult, BinWriterExt, Endian};

use crate::format::FourCC;

#[binrw]
#[derive(Clone, Debug)]
pub struct ChunkDescriptor {
    pub id: FourCC,
    pub size: u64,
    pub unk: u32,
    pub skip: u64,
}

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

    pub fn write<W, CB>(&mut self, w: &mut W, e: Endian, mut cb: CB) -> Result<()>
    where
        W: Write + Seek,
        CB: FnMut(&mut W) -> Result<()>,
    {
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
