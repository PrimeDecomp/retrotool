use std::io::{Seek, Write};

use anyhow::{bail, Result};
use binrw::{binrw, BinWriterExt};

use crate::format::txtr::STextureHeader;

pub fn write_astc<W: Write + Seek>(w: &mut W, head: &STextureHeader, data: &[u8]) -> Result<()> {
    if !head.format.is_astc() {
        bail!("Expected ASTC format, got {:?}", head.format);
    }
    let (block_x, block_y, block_z) = head.format.block_size();
    w.write_ne(&AstcHeader {
        block_x,
        block_y,
        block_z,
        dim_x: head.width,
        dim_y: head.height,
        dim_z: head.layers,
    })?;
    w.write_all(&data[..head.mip_sizes[0] as usize])?;
    Ok(())
}

#[binrw]
#[derive(Debug, Clone)]
#[brw(magic = b"\x13\xAB\xA1\x5C")]
pub struct AstcHeader {
    pub block_x: u8,
    pub block_y: u8,
    pub block_z: u8,
    #[br(map = AstcU24::as_u32)]
    #[bw(map = AstcU24::from_u32)]
    pub dim_x: u32,
    #[br(map = AstcU24::as_u32)]
    #[bw(map = AstcU24::from_u32)]
    pub dim_y: u32,
    #[br(map = AstcU24::as_u32)]
    #[bw(map = AstcU24::from_u32)]
    pub dim_z: u32,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct AstcU24([u8; 3]);

impl AstcU24 {
    fn from_u32(&value: &u32) -> Self {
        Self([value as u8, (value >> 8) as u8, (value >> 16) as u8])
    }

    fn as_u32(self) -> u32 {
        self.0[0] as u32 | ((self.0[1] as u32) << 8) | ((self.0[2] as u32) << 16)
    }
}
