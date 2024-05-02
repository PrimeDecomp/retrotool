use std::io::Cursor;

use anyhow::{anyhow, ensure, Result};
use binrw::{BinReaderExt, Endian};
use uuid::Uuid;
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor,
    pack::{AssetInfo, K_CHUNK_META},
    rfrm::FormDescriptor,
    FourCC,
};

// Custom footer for extracted files
pub const K_FORM_FOOT: FourCC = FourCC(*b"FOOT");
// Custom footer asset information
pub const K_CHUNK_AINF: FourCC = FourCC(*b"AINF");
// Custom footer asset name
pub const K_CHUNK_NAME: FourCC = FourCC(*b"NAME");

/// Locate the meta section in extracted files
pub fn locate_meta<O>(file_data: &[u8]) -> Result<&[u8]>
where O: ByteOrder + 'static {
    let (_, _, remain) = FormDescriptor::<O>::slice(file_data)?;
    let (foot_desc, mut foot_data, remain) = FormDescriptor::<O>::slice(remain)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.reader_version.get() == 1);
    ensure!(foot_desc.writer_version.get() == 1);
    ensure!(remain.is_empty());

    while !foot_data.is_empty() {
        let (desc, data, remain) = ChunkDescriptor::<O>::slice(foot_data)?;
        if desc.id == K_CHUNK_META {
            return Ok(data);
        }
        foot_data = remain;
    }
    Err(anyhow!("Failed to locate META chunk"))
}

/// Locate the asset ID in extracted files
pub fn locate_asset_id<O: ByteOrder>(file_data: &[u8]) -> Result<Uuid> {
    let (_, _, remain) = FormDescriptor::<O>::slice(file_data)?;
    let (foot_desc, mut foot_data, remain) = FormDescriptor::<O>::slice(remain)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.reader_version.get() == 1);
    ensure!(foot_desc.writer_version.get() == 1);
    ensure!(remain.is_empty());

    while !foot_data.is_empty() {
        let (desc, data, remain) = ChunkDescriptor::<O>::slice(foot_data)?;
        if desc.id == K_CHUNK_AINF {
            let asset_info: AssetInfo = Cursor::new(data).read_type(Endian::Little)?;
            return Ok(asset_info.id);
        }
        foot_data = remain;
    }
    Err(anyhow!("Failed to locate AINF chunk"))
}
