use std::{
    io::{Seek, SeekFrom, Write},
    mem::size_of,
};

use anyhow::{anyhow, ensure, Result};
use zerocopy::{AsBytes, ByteOrder, FromBytes, FromZeroes, U32, U64};

use crate::format::{chunk::ChunkDescriptor, peek_four_cc, FourCC};

// Resource format
pub const K_CHUNK_RFRM: FourCC = FourCC(*b"RFRM");

#[derive(Clone, Debug, PartialEq, FromBytes, FromZeroes, AsBytes)]
#[repr(C, packed)]
pub struct FormDescriptor<O: ByteOrder> {
    pub magic: FourCC,
    pub size: U64<O>,
    pub unk: U64<O>,
    pub id: FourCC,
    pub reader_version: U32<O>,
    pub writer_version: U32<O>,
}

impl<O: ByteOrder> Default for FormDescriptor<O> {
    fn default() -> Self {
        Self {
            magic: K_CHUNK_RFRM,
            size: U64::default(),
            unk: U64::default(),
            id: FourCC::default(),
            reader_version: U32::default(),
            writer_version: U32::default(),
        }
    }
}

impl<O: ByteOrder> FormDescriptor<O> {
    pub fn slice(data: &[u8]) -> Result<(&Self, &[u8], &[u8])> {
        let header = Self::ref_from_prefix(data).ok_or_else(|| anyhow!("Invalid RFRM header"))?;
        ensure!(header.magic == K_CHUNK_RFRM);
        let start = size_of::<Self>();
        let slice = &data[start..(start + header.size.get() as usize)];
        let remain = &data[(start + header.size.get() as usize)..];
        Ok((header, slice, remain))
    }

    pub fn write<W, CB>(&self, w: &mut W, mut cb: CB) -> Result<()>
    where
        W: Write + Seek,
        CB: FnMut(&mut W) -> Result<()>,
    {
        // Skip over the header
        let form_pos = w.stream_position()?;
        let data_pos = form_pos + size_of::<Self>() as u64;
        w.seek(SeekFrom::Start(data_pos))?;

        // Write the data and determine the size
        cb(w)?;
        let end_pos = w.stream_position()?;

        // Return to the start of the form and write the header
        w.seek(SeekFrom::Start(form_pos))?;
        let mut out = self.clone();
        out.size.set(end_pos - data_pos);
        w.write_all(out.as_bytes())?;

        // Seek to the end
        w.seek(SeekFrom::Start(end_pos))?;
        Ok(())
    }
}

/// Recursively dump an RFRM + contained chunks
#[allow(unused)]
pub fn dump_rfrm<'a, O, W>(w: &mut W, data: &'a [u8], indent: usize) -> Result<&'a [u8]>
where
    O: ByteOrder + 'static,
    W: Write,
{
    let (rfrm, mut rfrm_data, remain) = FormDescriptor::<O>::slice(data)?;
    let indstr = "  ".repeat(indent);
    writeln!(w, "{indstr}{rfrm:?}")?;
    while !rfrm_data.is_empty() {
        if peek_four_cc(rfrm_data) == K_CHUNK_RFRM {
            rfrm_data = dump_rfrm::<O, _>(w, rfrm_data, indent + 1)?;
        } else {
            let (desc, _, remain) = ChunkDescriptor::<O>::slice(rfrm_data)?;
            writeln!(w, "{indstr}- {desc:?}")?;
            rfrm_data = remain;
        }
    }
    Ok(remain)
}
