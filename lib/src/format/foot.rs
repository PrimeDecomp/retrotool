use anyhow::{anyhow, ensure, Result};
use binrw::Endian;

use crate::format::{chunk::ChunkDescriptor, pack::K_CHUNK_META, rfrm::FormDescriptor, FourCC};

// Custom footer for extracted files
pub const K_FORM_FOOT: FourCC = FourCC(*b"FOOT");
// Custom footer asset information
pub const K_CHUNK_AINF: FourCC = FourCC(*b"AINF");
// Custom footer asset name
pub const K_CHUNK_NAME: FourCC = FourCC(*b"NAME");

/// Locate the meta section in extracted files
pub fn locate_meta(file_data: &[u8], e: Endian) -> Result<&[u8]> {
    let (_, _, remain) = FormDescriptor::slice(file_data, e)?;
    let (foot_desc, mut foot_data, remain) = FormDescriptor::slice(remain, Endian::Little)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.version_a == 1);
    ensure!(foot_desc.version_b == 1);
    ensure!(remain.is_empty());

    while !foot_data.is_empty() {
        let (desc, data, remain) = ChunkDescriptor::slice(foot_data, e)?;
        if desc.id == K_CHUNK_META {
            return Ok(data);
        }
        foot_data = remain;
    }
    Err(anyhow!("Failed to locate META chunk"))
}
