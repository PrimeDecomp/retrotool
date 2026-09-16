use std::{io::Cursor, marker::PhantomData};

use anyhow::{bail, ensure, Context, Result};
use binrw::{binrw, BinReaderExt, Endian};
use zerocopy::ByteOrder;

use crate::format::{
    chunk::ChunkDescriptor,
    rfrm::FormDescriptor,
    txtr::{STextureMetaData, TextureData},
    CAABox, CVector3f, CVector3i, FourCC,
};

// Texture
pub const K_FORM_LTPB: FourCC = FourCC(*b"LTPB");

// Probe header
pub const K_CHUNK_PHDR: FourCC = FourCC(*b"PHDR");
// Probe texture
pub const K_CHUNK_PTEX: FourCC = FourCC(*b"PTEX");

#[binrw]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CBakedLightingUniformProbeGridIndex {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

impl CBakedLightingUniformProbeGridIndex {
    pub fn to_array(self) -> [i16; 3] { [self.x, self.y, self.z] }

    /// Probe position in storage coordinates, before the storage-to-world transform.
    pub fn position(self, spacing: CVector3f) -> CVector3f {
        CVector3f::new(
            spacing.x * f32::from(self.x),
            spacing.y * f32::from(self.y),
            spacing.z * f32::from(self.z),
        )
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LightProbeBundleHeader {
    pub unk1: u32,
    pub unk2: u32,
    /// Distance between probe centers along each storage axis.
    pub grid_spacing: CVector3f,
    pub min_index: CBakedLightingUniformProbeGridIndex,
    /// Inclusive upper grid index.
    pub max_index: CBakedLightingUniformProbeGridIndex,
}

impl LightProbeBundleHeader {
    /// Native GetIdxDimensions: signed maximum minus minimum, plus one.
    pub fn grid_dimensions(&self) -> CVector3i {
        let min = self.min_index.to_array();
        let max = self.max_index.to_array();
        CVector3i::from(std::array::from_fn(|i| i32::from(max[i]) - i32::from(min[i]) + 1))
    }

    /// Native GetGridBounds, including half a grid cell around the end probes.
    /// These are storage bounds; the ROOM placement transform is separate.
    pub fn grid_bounds(&self) -> CAABox {
        let min = self.min_index.position(self.grid_spacing).to_array();
        let max = self.max_index.position(self.grid_spacing).to_array();
        let spacing = self.grid_spacing.to_array();
        CAABox {
            min: CVector3f::from(std::array::from_fn(|i| min[i] - spacing[i] * 0.5)),
            max: CVector3f::from(std::array::from_fn(|i| max[i] + spacing[i] * 0.5)),
        }
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LightProbeExtra {
    /// Signed physical tile position (NProbeResourceFormat::STileMetaData).
    pub tile_position: CVector3i,
    /// NLightProbe::EProbeNonlinearGPUTextureLayer. Keep unknown selectors intact.
    pub layer: u32,
}

#[derive(Clone, Debug)]
struct SLightProbeMetaData {
    _unk1: u32,
    _unk2: u32,
    meta_offsets: Vec<u64>,
    txtr_offsets: Vec<u64>,
}

impl SLightProbeMetaData {
    fn slice(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let unk1 = reader.read_type(Endian::Little)?;
        let unk2 = reader.read_type(Endian::Little)?;
        let mut read_offsets = || -> Result<Vec<u64>> {
            let count: u32 = reader.read_type(Endian::Little)?;
            ensure!(
                u64::from(count) <= (data.len() as u64 - reader.position()) / 8,
                "Truncated LTPB metadata offsets"
            );
            (0..count).map(|_| reader.read_type(Endian::Little).map_err(Into::into)).collect()
        };
        let meta_offsets = read_offsets()?;
        let txtr_offsets = read_offsets()?;
        ensure!(reader.position() == data.len() as u64, "Trailing LTPB metadata");
        Ok(Self { _unk1: unk1, _unk2: unk2, meta_offsets, txtr_offsets })
    }
}

#[cfg(test)]
mod tests {
    use binrw::{BinReaderExt, BinWriterExt};
    use zerocopy::LittleEndian;

    use super::*;

    #[test]
    fn signed_grid_range_and_bounds() -> Result<()> {
        let mut wire = Cursor::new(Vec::new());
        wire.write_le(&[0u32, 2])?;
        wire.write_le(&[2.0f32, 0.5, 8.0])?;
        wire.write_le(&[-32768i16, -4, 2, 32767, 3, 2])?;
        wire.set_position(0);
        let head: LightProbeBundleHeader = wire.read_le()?;
        ensure!(head.grid_dimensions().to_array() == [65536, 8, 1]);
        ensure!(head.grid_bounds().min.to_array() == [-65537.0, -2.25, 12.0]);
        ensure!(head.grid_bounds().max.to_array() == [65535.0, 1.75, 20.0]);
        let mut encoded = Cursor::new(Vec::new());
        encoded.write_le(&head)?;
        ensure!(encoded.into_inner() == wire.into_inner());
        Ok(())
    }

    #[test]
    fn metadata_counts_and_offsets_are_bounded() -> Result<()> {
        let mut meta = Cursor::new(Vec::new());
        meta.write_le(&[1u32, 0, u32::MAX])?;
        ensure!(SLightProbeMetaData::slice(meta.get_ref()).is_err());

        let form = FormDescriptor::<LittleEndian> {
            id: K_FORM_LTPB,
            reader_version: 66.into(),
            writer_version: 73.into(),
            ..Default::default()
        };
        let mut data = Cursor::new(Vec::new());
        form.write(&mut data, |out| {
            ChunkDescriptor::<LittleEndian> { id: K_CHUNK_PHDR, ..Default::default() }.write(
                out,
                |out| {
                    out.write_le(&[0u8; 32])?;
                    Ok(())
                },
            )?;
            ChunkDescriptor::<LittleEndian> { id: K_CHUNK_PTEX, ..Default::default() }
                .write(out, |_| Ok(()))
        })?;
        for offset in [0u64, 56, data.get_ref().len() as u64, u64::MAX] {
            let mut meta = Cursor::new(Vec::new());
            meta.write_le(&[1u32, 0, 1])?;
            meta.write_le(&offset)?;
            meta.write_le(&1u32)?;
            meta.write_le(&u64::MAX)?;
            ensure!(LightProbeData::<LittleEndian>::slice(data.get_ref(), meta.get_ref()).is_err());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LightProbeData<O: ByteOrder> {
    pub head: LightProbeBundleHeader,
    pub textures: Vec<TextureData<O>>,
    pub extra: Vec<LightProbeExtra>,
    _marker: PhantomData<O>,
}

impl<O: ByteOrder> LightProbeData<O> {
    pub fn slice(data: &[u8], meta: &[u8]) -> Result<Self> {
        let (ltpb_desc, mut ltpb_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(ltpb_desc.id == K_FORM_LTPB);
        ensure!(ltpb_desc.reader_version.get() == 66);
        ensure!(ltpb_desc.writer_version.get() == 73);

        let meta = SLightProbeMetaData::slice(meta)?;
        ensure!(meta.meta_offsets.len() == meta.txtr_offsets.len());
        let texture_count = meta.meta_offsets.len();

        let mut head: Option<LightProbeBundleHeader> = None;
        let form_end = std::mem::size_of::<FormDescriptor<O>>() + ltpb_data.len();
        let mut ptex = None;
        while !ltpb_data.is_empty() {
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(ltpb_data)?;
            let mut reader = Cursor::new(chunk_data);
            match chunk_desc.id {
                K_CHUNK_PHDR => {
                    ensure!(head.is_none(), "Duplicate LTPB PHDR");
                    head = Some(reader.read_type(Endian::Little)?);
                    ensure!(reader.position() == chunk_data.len() as u64, "Trailing PHDR data");
                }
                K_CHUNK_PTEX => {
                    ensure!(ptex.is_none(), "Duplicate LTPB PTEX");
                    let end = form_end - remain.len();
                    ptex = Some(end - chunk_data.len()..end);
                }
                id => bail!("Unknown LTPB chunk ID {id:?}"),
            }
            ltpb_data = remain;
        }
        let Some(head) = head else { bail!("Failed to locate PHDR") };
        let ptex = ptex.context("Failed to locate PTEX")?;

        let mut textures = Vec::with_capacity(texture_count);
        let mut extra: Vec<LightProbeExtra> = Vec::with_capacity(texture_count);
        for (meta_offset, txtr_offset) in meta.meta_offsets.into_iter().zip(meta.txtr_offsets) {
            let meta_offset = usize::try_from(meta_offset)?;
            let txtr_offset = usize::try_from(txtr_offset)?;
            ensure!(
                ptex.contains(&meta_offset)
                    && ptex.contains(&txtr_offset)
                    && meta_offset < txtr_offset,
                "LTPB tile offsets outside PTEX or out of order"
            );
            let meta = &data[meta_offset..txtr_offset];

            // Skip metadata to read extra fields
            let mut reader = Cursor::new(meta);
            reader.read_type::<STextureMetaData>(Endian::Little)?;
            extra.push(reader.read_type(Endian::Little)?);
            ensure!(reader.position() == meta.len() as u64, "Trailing LTPB tile metadata");

            textures.push(TextureData::<O>::slice(&data[txtr_offset..ptex.end], meta)?);
        }
        Ok(Self { head, textures, extra, _marker: PhantomData })
    }
}
