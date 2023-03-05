use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use anyhow::Result;
use binrw::{binrw, BinReaderExt, BinResult, BinWriterExt, Endian};

use crate::format::{chunk::ChunkDescriptor, peek_four_cc, FourCC};

// Resource format
pub const K_CHUNK_RFRM: FourCC = FourCC(*b"RFRM");

#[binrw]
#[brw(magic = b"RFRM")]
#[derive(Clone, Debug)]
pub struct FormDescriptor {
    pub size: u64,
    pub unk: u64,
    pub id: FourCC,
    pub reader_version: u32,
    pub writer_version: u32,
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

/// Recursively dump an RFRM + contained chunks
#[allow(unused)]
pub fn dump_rfrm<'a, W: Write>(w: &mut W, data: &'a [u8], indent: usize) -> Result<&'a [u8]> {
    let (rfrm, mut rfrm_data, remain) = FormDescriptor::slice(data, Endian::Little)?;
    let indstr = "  ".repeat(indent);
    writeln!(w, "{indstr}{rfrm:?}")?;
    while !rfrm_data.is_empty() {
        if peek_four_cc(rfrm_data) == K_CHUNK_RFRM {
            rfrm_data = dump_rfrm(w, rfrm_data, indent + 1)?;
        } else {
            let (desc, _, remain) = ChunkDescriptor::slice(rfrm_data, Endian::Little)?;
            writeln!(w, "{indstr}- {desc:?}")?;
            rfrm_data = remain;
        }
    }
    Ok(remain)
}
