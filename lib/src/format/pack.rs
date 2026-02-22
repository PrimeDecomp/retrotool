use std::{
    borrow::Cow,
    collections::{hash_map, HashMap},
    io::{Cursor, Seek, SeekFrom, Write},
    marker::PhantomData,
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use binrw::{binrw, BinReaderExt, BinWriterExt, Endian};
use uuid::Uuid;
use zerocopy::{AsBytes, ByteOrder, FromBytes, FromZeroes, U32, U64};

use crate::{
    format::{
        chunk::ChunkDescriptor,
        foot::{K_CHUNK_AINF, K_CHUNK_NAME, K_FORM_FOOT},
        rfrm::FormDescriptor,
        ByteOrderExt, ByteOrderUuid, FourCC,
    },
    util::{compression::decompress_buffer, read::read_u32},
};

// Package file
pub const K_FORM_PACK: FourCC = FourCC(*b"PACK");
// Table of contents
pub const K_FORM_TOCC: FourCC = FourCC(*b"TOCC");
// Metadata
pub const K_CHUNK_META: FourCC = FourCC(*b"META");
// String table
pub const K_CHUNK_STRG: FourCC = FourCC(*b"STRG");
// Asset directory
pub const K_CHUNK_ADIR: FourCC = FourCC(*b"ADIR");

/// PACK::TOCC::ADIR chunk
type AssetDirectory<O> = Vec<AssetDirectoryEntry<O>>;

/// PACK::TOCC::ADIR chunk entry
#[derive(Clone, Debug, AsBytes, FromBytes, FromZeroes)]
#[repr(C, packed)]
pub struct AssetDirectoryEntry<O: ByteOrder> {
    pub asset_type: FourCC,
    pub asset_id: ByteOrderUuid<O>,
    pub version: U32<O>,
    pub other_version: U32<O>,
    pub offset: U64<O>,
    pub decompressed_size: U64<O>,
    pub size: U64<O>,
}

/// PACK::TOCC::META chunk
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct MetadataTable {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<MetadataTableEntry>,
}

/// PACK::TOCC::META chunk entry
#[binrw]
#[derive(Clone, Debug)]
pub struct MetadataTableEntry {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub asset_id: Uuid,
    pub offset: u32,
}

/// PACK::TOCC::STRG chunk
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct StringTable {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<StringTableEntry>,
}

/// PACK::TOCC::STRG chunk entry
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct StringTableEntry {
    // Byteswapped
    #[br(map = FourCC::from_u32)]
    #[bw(map = FourCC::as_u32)]
    pub kind: FourCC,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub asset_id: Uuid,
    #[bw(try_calc = name.len().try_into())]
    pub name_length: u32,
    #[br(count = name_length)]
    pub name: Vec<u8>,
}

/// Custom AINF chunk
#[binrw]
#[derive(Clone, Debug)]
pub struct AssetInfo {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub id: Uuid,
    pub compression_mode: u32,
    pub orig_offset: u64,
}

/// Combined asset representation
#[derive(Debug, Clone)]
pub struct Asset<'a> {
    pub id: Uuid,
    pub kind: FourCC,
    pub names: Vec<String>,
    // TODO lazy decompression?
    pub data: Cow<'a, [u8]>,
    pub meta: Option<Cow<'a, [u8]>>,
    pub info: AssetInfo,
    pub version: u32,
    pub other_version: u32,
}

/// Combined package information
#[derive(Debug, Clone, Default)]
pub struct Package<'a, O: ByteOrder> {
    pub assets: Vec<Asset<'a>>,
    _marker: PhantomData<O>,
}

/// Asset header information
#[derive(Debug, Clone)]
pub struct SparsePackageEntry {
    pub id: Uuid,
    pub kind: FourCC,
    pub names: Vec<String>,
    pub reader_version: u32,
    pub writer_version: u32,
}

impl<O> Package<'_, O>
where O: ByteOrderExt + 'static
{
    pub fn read_header(data: &[u8]) -> Result<Vec<u8>> {
        let (pack, pack_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version.get() == 1);
        let (tocc, tocc_data, _) = FormDescriptor::<O>::slice(pack_data)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version.get() == 3);

        // Rewrite PACK with only TOCC chunk
        let mut out = Cursor::new(Vec::new());
        pack.write(&mut out, |w| {
            tocc.write(w, |w| {
                w.write_all(tocc_data)?;
                Ok(())
            })
        })?;
        Ok(out.into_inner())
    }

    pub fn read_sparse(data: &[u8]) -> Result<Vec<SparsePackageEntry>> {
        let (pack, pack_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version.get() == 1);
        let (tocc, mut tocc_data, _) = FormDescriptor::<O>::slice(pack_data)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version.get() == 3);
        let mut adir: Option<&[AssetDirectoryEntry<O>]> = None;
        let mut strg: HashMap<Uuid, Vec<String>> = HashMap::new();
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(tocc_data)?;
            let mut reader = Cursor::new(chunk_data);
            match desc.id {
                K_CHUNK_ADIR => {
                    let count = read_u32::<O, _>(&mut reader)?;
                    let (entries, _) = AssetDirectoryEntry::<O>::slice_from_prefix(
                        &chunk_data[4..],
                        count as usize,
                    )
                    .context("Failed to read ADIR chunk")?;
                    for entry in entries {
                        log::debug!("- {:?}", entry);
                    }
                    adir = Some(entries);
                }
                K_CHUNK_META => {}
                K_CHUNK_STRG => {
                    let chunk: StringTable = reader.read_type(Endian::Little)?;
                    for entry in chunk.entries {
                        let name = String::from_utf8(entry.name)?;
                        match strg.entry(entry.asset_id) {
                            hash_map::Entry::Occupied(e) => {
                                e.into_mut().push(name);
                            }
                            hash_map::Entry::Vacant(e) => {
                                e.insert(vec![name]);
                            }
                        }
                    }
                }
                kind => bail!("Unhandled TOCC chunk {:?}", kind),
            }
            tocc_data = remain;
        }

        let Some(adir) = adir else {
            bail!("Failed to locate asset directory");
        };
        let mut last_id: Option<Uuid> = None;
        let entries = adir
            .iter()
            .filter_map(|asset_entry| {
                let asset_id = asset_entry.asset_id.get();
                if matches!(last_id, Some(id) if id == asset_id) {
                    return None;
                }
                last_id = Some(asset_id);
                Some(SparsePackageEntry {
                    id: asset_id,
                    kind: asset_entry.asset_type,
                    names: strg.get(&asset_id).cloned().unwrap_or_default(),
                    reader_version: asset_entry.version.get(),
                    writer_version: asset_entry.other_version.get(),
                })
            })
            .collect();
        Ok(entries)
    }

    pub fn read_asset(data: &[u8], id: Uuid) -> Result<Vec<u8>> {
        let (pack, pack_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version.get() == 1);
        let (tocc, mut tocc_data, _) = FormDescriptor::<O>::slice(pack_data)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version.get() == 3);

        let mut asset: Option<AssetDirectoryEntry<O>> = None;
        let mut meta: Option<&[u8]> = None;
        let mut name: Option<String> = None;
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(tocc_data)?;
            let mut reader = Cursor::new(chunk_data);
            match desc.id {
                K_CHUNK_ADIR => {
                    if chunk_data.len() < 4 {
                        bail!("Invalid ADIR chunk");
                    }
                    let count = O::read_u32(chunk_data);
                    let (slice, _) = AssetDirectoryEntry::<O>::slice_from_prefix(
                        &chunk_data[4..],
                        count as usize,
                    )
                    .context("Failed to read ADIR chunk")?;
                    asset = Some(
                        slice
                            .iter()
                            .find(|entry| entry.asset_id.get() == id)
                            .cloned()
                            .ok_or_else(|| anyhow!("Failed to locate asset {}", id))?,
                    );
                }
                K_CHUNK_META => {
                    let chunk: MetadataTable = reader.read_type(Endian::Little)?;
                    for entry in chunk.entries {
                        if entry.asset_id != id {
                            continue;
                        }
                        let meta_size = u32::from_le_bytes(
                            chunk_data[entry.offset as usize..entry.offset as usize + 4]
                                .try_into()
                                .unwrap(),
                        );
                        let meta_data = &chunk_data
                            [entry.offset as usize + 4..(entry.offset + 4 + meta_size) as usize];
                        meta = Some(meta_data);
                    }
                }
                K_CHUNK_STRG => {
                    let chunk: StringTable = reader.read_type(Endian::Little)?;
                    for entry in chunk.entries {
                        if entry.asset_id != id {
                            continue;
                        }
                        name = Some(String::from_utf8(entry.name)?);
                        break;
                    }
                }
                kind => bail!("Unhandled TOCC chunk {:?}", kind),
            }
            tocc_data = remain;
        }

        let Some(asset) = asset else {
            bail!("Failed to locate asset directory");
        };
        let compressed_data =
            &data[asset.offset.get() as usize..(asset.offset.get() + asset.size.get()) as usize];
        let (compression_mode, data) = if asset.size != asset.decompressed_size {
            decompress_buffer(compressed_data, asset.decompressed_size.get())?
        } else {
            (0, Cow::Borrowed(compressed_data))
        };

        // Validate RFRM
        {
            let (form, _, _) = FormDescriptor::<O>::slice(&data)?;
            ensure!(asset.asset_type == form.id);
            ensure!(asset.version == form.reader_version);
            ensure!(asset.other_version == form.writer_version);
            ensure!(asset.decompressed_size.get() == form.size.get() + 32 /* RFRM */);
        }

        let len = data.len() as u64;
        let mut w = Cursor::new(data.into_owned());
        w.set_position(len); // set to append

        // Write custom footer
        FormDescriptor::<O> {
            id: K_FORM_FOOT,
            reader_version: U32::new(1),
            writer_version: U32::new(1),
            ..Default::default()
        }
        .write(&mut w, |w| {
            ChunkDescriptor::<O> { id: K_CHUNK_AINF, ..Default::default() }.write(w, |w| {
                w.write_le(&AssetInfo { id, compression_mode, orig_offset: asset.offset.get() })?;
                Ok(())
            })?;
            if let Some(meta) = meta {
                w.write_all(
                    ChunkDescriptor::<O> {
                        id: K_CHUNK_META,
                        size: U64::new(meta.len() as u64),
                        ..Default::default()
                    }
                    .as_bytes(),
                )?;
                w.write_all(meta)?;
            }
            if let Some(name) = &name {
                let bytes = name.as_bytes();
                w.write_all(
                    ChunkDescriptor::<O> {
                        id: K_CHUNK_NAME,
                        size: U64::new(bytes.len() as u64),
                        ..Default::default()
                    }
                    .as_bytes(),
                )?;
                w.write_all(bytes)?;
            }
            Ok(())
        })?;

        Ok(w.into_inner())
    }

    pub fn read_full<'a>(data: &'a [u8], e: Endian) -> Result<Package<'a, O>> {
        let (pack, pack_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version.get() == 1);
        log::debug!("PACK: {:?}", pack);
        let (tocc, mut tocc_data, _) = FormDescriptor::<O>::slice(pack_data)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version.get() == 3);
        log::debug!("TOCC: {:?}", tocc);
        let mut adir: Option<&[AssetDirectoryEntry<O>]> = None;
        let mut meta: HashMap<Uuid, &[u8]> = HashMap::new();
        let mut strg: HashMap<Uuid, Vec<String>> = HashMap::new();
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::<O>::slice(tocc_data)?;
            let mut reader = Cursor::new(chunk_data);
            log::debug!("{:?} data size {}", desc, chunk_data.len());
            match desc.id {
                K_CHUNK_ADIR => {
                    let count = read_u32::<O, _>(&mut reader)?;
                    let (entries, _) = AssetDirectoryEntry::<O>::slice_from_prefix(
                        &chunk_data[4..],
                        count as usize,
                    )
                    .context("Failed to read ADIR chunk")?;
                    for entry in entries {
                        log::debug!("- {:?}", entry);
                    }
                    adir = Some(entries);
                }
                K_CHUNK_META => {
                    let chunk: MetadataTable = reader.read_type(e)?;
                    for entry in chunk.entries {
                        let meta_size = u32::from_le_bytes(
                            chunk_data[entry.offset as usize..entry.offset as usize + 4]
                                .try_into()
                                .unwrap(),
                        );
                        let meta_data = &chunk_data
                            [entry.offset as usize + 4..(entry.offset + 4 + meta_size) as usize];
                        log::debug!("- {:?} (size {:#X})", entry, meta_size);
                        meta.insert(entry.asset_id, meta_data);
                    }
                }
                K_CHUNK_STRG => {
                    let chunk: StringTable = reader.read_type(e)?;
                    for entry in chunk.entries {
                        log::debug!("- {:?}", entry);
                        let name = String::from_utf8(entry.name)?;
                        match strg.entry(entry.asset_id) {
                            hash_map::Entry::Occupied(e) => {
                                e.into_mut().push(name);
                            }
                            hash_map::Entry::Vacant(e) => {
                                e.insert(vec![name]);
                            }
                        }
                    }
                }
                kind => bail!("Unhandled TOCC chunk {:?}", kind),
            }
            tocc_data = remain;
        }

        let Some(adir) = adir else {
            bail!("Failed to locate asset directory");
        };
        let mut package =
            Package::<'a, O> { assets: Vec::with_capacity(adir.len()), _marker: PhantomData };
        for asset_entry in adir {
            let compressed_data = &data[asset_entry.offset.get() as usize
                ..(asset_entry.offset.get() + asset_entry.size.get()) as usize];
            let (compression_mode, data) = if asset_entry.size != asset_entry.decompressed_size {
                decompress_buffer(compressed_data, asset_entry.decompressed_size.get())?
            } else {
                (0, Cow::Borrowed(compressed_data))
            };

            // Validate RFRM
            {
                let (form, _, _) = FormDescriptor::<O>::slice(&data)?;
                ensure!(asset_entry.asset_type == form.id);
                ensure!(asset_entry.version.get() == form.reader_version.get());
                ensure!(asset_entry.other_version.get() == form.writer_version.get());
                ensure!(asset_entry.decompressed_size.get() == form.size.get() + 32 /* RFRM */);
            }

            let asset_id = asset_entry.asset_id.get();
            package.assets.push(Asset {
                id: asset_id,
                kind: asset_entry.asset_type,
                names: strg.get(&asset_id).cloned().unwrap_or_default(),
                data,
                meta: meta.get(&asset_id).map(|data| Cow::Borrowed(*data)),
                info: AssetInfo {
                    id: asset_id,
                    compression_mode,
                    orig_offset: asset_entry.offset.get(),
                },
                version: asset_entry.version.get(),
                other_version: asset_entry.other_version.get(),
            });
        }
        Ok(package)
    }

    pub fn write<W: Write + Seek>(&self, w: &mut W) -> Result<()> {
        let mut asset_directory = AssetDirectory::default();
        let mut metadata = MetadataTable::default();
        let mut string_table = StringTable::default();
        let mut last_uuid = Uuid::nil();
        for asset in &self.assets {
            ensure!(asset.id >= last_uuid, "Assets must be ordered by ID ascending");
            last_uuid = asset.id;

            for _ in 0..std::cmp::max(1, asset.names.len()) {
                asset_directory.push(AssetDirectoryEntry {
                    asset_type: asset.kind,
                    asset_id: ByteOrderUuid::new(asset.id),
                    version: U32::new(asset.version),
                    other_version: U32::new(asset.other_version),
                    offset: U64::new(0),
                    decompressed_size: U64::new(asset.data.len() as u64),
                    size: U64::new(asset.data.len() as u64),
                });
                if asset.meta.is_some() {
                    metadata.entries.push(MetadataTableEntry { asset_id: asset.id, offset: 0 });
                }
            }
            for name in &asset.names {
                // Default::default makes the IDE happy,
                // just need to suppress clippy
                #[allow(clippy::needless_update)]
                string_table.entries.push(StringTableEntry {
                    kind: asset.kind,
                    asset_id: asset.id,
                    name: name.as_bytes().to_vec(),
                    ..Default::default()
                });
            }
        }
        let mut adir_pos = 0;
        FormDescriptor::<O> {
            id: K_FORM_PACK,
            reader_version: U32::new(1),
            writer_version: U32::new(1),
            ..Default::default()
        }
        .write(w, |w| {
            FormDescriptor::<O> {
                id: K_FORM_TOCC,
                reader_version: U32::new(3),
                writer_version: U32::new(3),
                ..Default::default()
            }
            .write(w, |w| {
                ChunkDescriptor::<O> { id: K_CHUNK_ADIR, unk: U32::new(1), ..Default::default() }
                    .write(w, |w| {
                    w.write_type(&(asset_directory.len() as u32), Endian::Little)?;
                    adir_pos = w.stream_position()?;
                    w.write_all(asset_directory.as_slice().as_bytes())?;
                    Ok(())
                })?;
                ChunkDescriptor::<O> { id: K_CHUNK_META, unk: U32::new(1), ..Default::default() }
                    .write(w, |w| {
                    let start = w.stream_position()?;
                    w.write_type(&metadata, Endian::Little)?;
                    for (asset, entry_chunk) in
                        self.assets.iter()
                        .filter(|a| a.meta.is_some())
                        .zip(&mut metadata.entries
                            .chunk_by_mut(|e1, e2| e1.asset_id == e2.asset_id)
                        )
                    {
                        for entry in entry_chunk {
                            entry.offset = (w.stream_position()? - start) as u32;
                        }
                        let data = asset.meta.as_ref().unwrap();
                        w.write_type(&(data.len() as u32), Endian::Little)?;
                        w.write_all(data)?;
                    }
                    let end = w.stream_position()?;
                    w.seek(SeekFrom::Start(start))?;
                    w.write_type(&metadata, Endian::Little)?;
                    w.seek(SeekFrom::Start(end))?;
                    Ok(())
                })?;
                ChunkDescriptor::<O> { id: K_CHUNK_STRG, unk: U32::new(1), ..Default::default() }
                    .write(w, |w| {
                    w.write_type(&string_table, Endian::Little)?;
                    Ok(())
                })?;
                Ok(())
            })?;
            let mut entry_chunks: Vec<(&Asset, &mut [AssetDirectoryEntry<O>])> =
                self.assets.iter()
                    .zip(&mut asset_directory
                        .chunk_by_mut(|e1, e2| e1.asset_id == e2.asset_id)
                    )
                    .collect();
            entry_chunks.sort_by_key(|(a, _)| a.info.orig_offset);
            for (asset, entry_chunk) in entry_chunks {
                for entry in entry_chunk {
                    entry.offset.set(w.stream_position()?);
                }
                w.write_all(&asset.data)?;
            }
            Ok(())
        })?;

        // Write updated ADIR offsets
        let pos = w.stream_position()?;
        w.seek(SeekFrom::Start(adir_pos))?;
        w.write_all(asset_directory.as_slice().as_bytes())?;
        w.seek(SeekFrom::Start(pos))?;

        // Align 16
        let aligned_end = (pos + 15) & !15;
        w.write_all(&vec![0u8; (aligned_end - pos) as usize])?;
        Ok(())
    }
}
