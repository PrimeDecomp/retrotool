use std::{
    io::{Seek, SeekFrom, Write},
    mem::size_of,
};

use anyhow::{anyhow, Result};
use zerocopy::{AsBytes, ByteOrder, FromBytes, FromZeroes, U32, U64};

use crate::format::FourCC;

#[derive(Clone, Debug, Default, PartialEq, FromBytes, FromZeroes, AsBytes)]
#[repr(C, packed)]
pub struct ChunkDescriptor<O: ByteOrder> {
    pub id: FourCC,
    pub size: U64<O>,
    pub unk: U32<O>,
    pub skip: U64<O>,
}

impl<O: ByteOrder> ChunkDescriptor<O> {
    pub fn slice(data: &[u8]) -> Result<(&Self, &[u8], &[u8])> {
        let header = Self::ref_from_prefix(data).ok_or_else(|| anyhow!("Invalid chunk header"))?;
        let start = size_of::<Self>() + header.skip.get() as usize;
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
        let data_pos = form_pos + size_of::<Self>() as u64 + self.skip.get();
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
