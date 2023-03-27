use std::{
    borrow::Cow,
    collections::{hash_map, HashMap},
    io::{Cursor, Seek, SeekFrom, Write},
};

use anyhow::{anyhow, bail, ensure, Result};
use binrw::{binrw, BinReaderExt, BinWriterExt, Endian};
use uuid::Uuid;

use crate::{
    format::{
        chunk::ChunkDescriptor,
        foot::{K_CHUNK_AINF, K_CHUNK_NAME, K_FORM_FOOT},
        rfrm::FormDescriptor,
        FourCC,
    },
    util::compression::decompress_buffer,
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
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct AssetDirectory {
    #[bw(try_calc = entries.len().try_into())]
    pub entry_count: u32,
    #[br(count = entry_count)]
    pub entries: Vec<AssetDirectoryEntry>,
}

/// PACK::TOCC::ADIR chunk entry
#[binrw]
#[derive(Clone, Debug)]
pub struct AssetDirectoryEntry {
    pub asset_type: FourCC,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub asset_id: Uuid,
    pub version: u32,
    pub other_version: u32,
    pub offset: u64,
    pub decompressed_size: u64,
    pub size: u64,
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
#[derive(Debug, Clone)]
pub struct Package<'a> {
    pub assets: Vec<Asset<'a>>,
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

impl Package<'_> {
    pub fn read_header(data: &[u8], e: Endian) -> Result<Vec<u8>> {
        let (mut pack, pack_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version == 1);
        let (mut tocc, tocc_data, _) = FormDescriptor::slice(pack_data, e)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version == 3);

        // Rewrite PACK with only TOCC chunk
        let mut out = Cursor::new(Vec::new());
        pack.write(&mut out, e, |w| {
            tocc.write(w, e, |w| {
                w.write_all(tocc_data)?;
                Ok(())
            })
        })?;
        Ok(out.into_inner())
    }

    pub fn read_sparse(data: &[u8], e: Endian) -> Result<Vec<SparsePackageEntry>> {
        let (pack, pack_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version == 1);
        let (tocc, mut tocc_data, _) = FormDescriptor::slice(pack_data, e)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version == 3);
        let mut adir: Option<AssetDirectory> = None;
        let mut strg: HashMap<Uuid, Vec<String>> = HashMap::new();
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(tocc_data, e)?;
            let mut reader = Cursor::new(chunk_data);
            match desc.id {
                K_CHUNK_ADIR => {
                    adir = Some(reader.read_type(e)?);
                }
                K_CHUNK_META => {}
                K_CHUNK_STRG => {
                    let chunk: StringTable = reader.read_type(e)?;
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
            .entries
            .into_iter()
            .filter_map(|asset_entry| {
                if matches!(last_id, Some(id) if id == asset_entry.asset_id) {
                    return None;
                }
                last_id = Some(asset_entry.asset_id);
                Some(SparsePackageEntry {
                    id: asset_entry.asset_id,
                    kind: asset_entry.asset_type,
                    names: strg.get(&asset_entry.asset_id).cloned().unwrap_or_default(),
                    reader_version: asset_entry.version,
                    writer_version: asset_entry.other_version,
                })
            })
            .collect();
        Ok(entries)
    }

    pub fn read_asset(data: &[u8], id: Uuid, e: Endian) -> Result<Vec<u8>> {
        let (pack, pack_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version == 1);
        let (tocc, mut tocc_data, _) = FormDescriptor::slice(pack_data, e)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version == 3);

        let mut asset: Option<AssetDirectoryEntry> = None;
        let mut meta: Option<&[u8]> = None;
        let mut name: Option<String> = None;
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(tocc_data, e)?;
            let mut reader = Cursor::new(chunk_data);
            match desc.id {
                K_CHUNK_ADIR => {
                    let adir: AssetDirectory = reader.read_type(e)?;
                    asset = Some(
                        adir.entries
                            .into_iter()
                            .find(|asset| asset.asset_id == id)
                            .ok_or_else(|| anyhow!("Failed to locate asset {}", id))?,
                    );
                }
                K_CHUNK_META => {
                    let chunk: MetadataTable = reader.read_type(e)?;
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
                    let chunk: StringTable = reader.read_type(e)?;
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
        let compressed_data = &data[asset.offset as usize..(asset.offset + asset.size) as usize];
        let (compression_mode, data) = if asset.size != asset.decompressed_size {
            decompress_buffer(compressed_data, asset.decompressed_size)?
        } else {
            (0, Cow::Borrowed(compressed_data))
        };

        // Validate RFRM
        {
            let (form, _, _) = FormDescriptor::slice(&data, e)?;
            ensure!(asset.asset_type == form.id);
            ensure!(asset.version == form.reader_version);
            ensure!(asset.other_version == form.writer_version);
            ensure!(asset.decompressed_size == form.size + 32 /* RFRM */);
        }

        let len = data.len() as u64;
        let mut w = Cursor::new(data.into_owned());
        w.set_position(len); // set to append

        // Write custom footer
        FormDescriptor { size: 0, unk: 0, id: K_FORM_FOOT, reader_version: 1, writer_version: 1 }
            .write(&mut w, e, |w| {
            ChunkDescriptor { id: K_CHUNK_AINF, size: 0, unk: 0, skip: 0 }.write(w, e, |w| {
                w.write_le(&AssetInfo { id, compression_mode, orig_offset: asset.offset })?;
                Ok(())
            })?;
            if let Some(meta) = meta {
                let meta_chunk =
                    ChunkDescriptor { id: K_CHUNK_META, size: meta.len() as u64, unk: 0, skip: 0 };
                w.write_le(&meta_chunk)?;
                w.write_all(meta)?;
            }
            if let Some(name) = &name {
                let bytes = name.as_bytes();
                let name_chunk =
                    ChunkDescriptor { id: K_CHUNK_NAME, size: bytes.len() as u64, unk: 0, skip: 0 };
                w.write_le(&name_chunk)?;
                w.write_all(bytes)?;
            }
            Ok(())
        })?;

        Ok(w.into_inner())
    }

    pub fn read_full(data: &[u8], e: Endian) -> Result<Package> {
        let (pack, pack_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(pack.id == K_FORM_PACK);
        ensure!(pack.reader_version == 1);
        log::debug!("PACK: {:?}", pack);
        let (tocc, mut tocc_data, _) = FormDescriptor::slice(pack_data, e)?;
        ensure!(tocc.id == K_FORM_TOCC);
        ensure!(tocc.reader_version == 3);
        log::debug!("TOCC: {:?}", tocc);
        let mut adir: Option<AssetDirectory> = None;
        let mut meta: HashMap<Uuid, &[u8]> = HashMap::new();
        let mut strg: HashMap<Uuid, Vec<String>> = HashMap::new();
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(tocc_data, e)?;
            let mut reader = Cursor::new(chunk_data);
            log::debug!("{:?} data size {}", desc, chunk_data.len());
            match desc.id {
                K_CHUNK_ADIR => {
                    let chunk: AssetDirectory = reader.read_type(e)?;
                    for entry in &chunk.entries {
                        log::debug!("- {:?}", entry);
                    }
                    adir = Some(chunk);
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
        let mut package = Package { assets: Vec::with_capacity(adir.entries.len()) };
        for asset_entry in &adir.entries {
            let compressed_data = &data
                [asset_entry.offset as usize..(asset_entry.offset + asset_entry.size) as usize];
            let (compression_mode, data) = if asset_entry.size != asset_entry.decompressed_size {
                decompress_buffer(compressed_data, asset_entry.decompressed_size)?
            } else {
                (0, Cow::Borrowed(compressed_data))
            };

            // Validate RFRM
            {
                let (form, _, _) = FormDescriptor::slice(&data, e)?;
                ensure!(asset_entry.asset_type == form.id);
                ensure!(asset_entry.version == form.reader_version);
                ensure!(asset_entry.other_version == form.writer_version);
                ensure!(asset_entry.decompressed_size == form.size + 32 /* RFRM */);
            }

            package.assets.push(Asset {
                id: asset_entry.asset_id,
                kind: asset_entry.asset_type,
                names: strg.get(&asset_entry.asset_id).cloned().unwrap_or_default(),
                data,
                meta: meta.get(&asset_entry.asset_id).map(|data| Cow::Borrowed(*data)),
                info: AssetInfo {
                    id: asset_entry.asset_id,
                    compression_mode,
                    orig_offset: asset_entry.offset,
                },
                version: asset_entry.version,
                other_version: asset_entry.other_version,
            });
        }
        Ok(package)
    }

    pub fn write<W: Write + Seek>(&self, w: &mut W, e: Endian) -> Result<()> {
        let mut asset_directory = AssetDirectory::default();
        let mut metadata = MetadataTable::default();
        let mut string_table = StringTable::default();
        let mut last_uuid = Uuid::nil();
        for asset in &self.assets {
            ensure!(asset.id >= last_uuid, "Assets must be ordered by ID ascending");
            last_uuid = asset.id;

            asset_directory.entries.push(AssetDirectoryEntry {
                asset_type: asset.kind,
                asset_id: asset.id,
                version: asset.version,
                other_version: asset.other_version,
                offset: 0,
                decompressed_size: asset.data.len() as u64,
                size: asset.data.len() as u64,
            });
            if asset.meta.is_some() {
                metadata.entries.push(MetadataTableEntry { asset_id: asset.id, offset: 0 });
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
        FormDescriptor { size: 0, unk: 0, id: K_FORM_PACK, reader_version: 1, writer_version: 1 }
            .write(w, e, |w| {
            FormDescriptor {
                size: 0,
                unk: 0,
                id: K_FORM_TOCC,
                reader_version: 3,
                writer_version: 3,
            }
            .write(w, e, |w| {
                ChunkDescriptor { id: K_CHUNK_ADIR, size: 0, unk: 1, skip: 0 }.write(
                    w,
                    e,
                    |w| {
                        adir_pos = w.stream_position()?;
                        w.write_type(&asset_directory, e)?;
                        Ok(())
                    },
                )?;
                ChunkDescriptor { id: K_CHUNK_META, size: 0, unk: 1, skip: 0 }.write(
                    w,
                    e,
                    |w| {
                        let start = w.stream_position()?;
                        w.write_type(&metadata, e)?;
                        for (asset, entry) in self
                            .assets
                            .iter()
                            .filter(|a| a.meta.is_some())
                            .zip(&mut metadata.entries)
                        {
                            entry.offset = (w.stream_position()? - start) as u32;
                            let data = asset.meta.as_ref().unwrap();
                            w.write_type(&(data.len() as u32), e)?;
                            w.write_all(data)?;
                        }
                        let end = w.stream_position()?;
                        w.seek(SeekFrom::Start(start))?;
                        w.write_type(&metadata, e)?;
                        w.seek(SeekFrom::Start(end))?;
                        Ok(())
                    },
                )?;
                ChunkDescriptor { id: K_CHUNK_STRG, size: 0, unk: 1, skip: 0 }.write(
                    w,
                    e,
                    |w| {
                        w.write_type(&string_table, e)?;
                        Ok(())
                    },
                )?;
                Ok(())
            })?;
            let mut entries: Vec<(&Asset, &mut AssetDirectoryEntry)> =
                self.assets.iter().zip(&mut asset_directory.entries).collect();
            entries.sort_by_key(|(a, _)| a.info.orig_offset);
            for (asset, entry) in entries {
                entry.offset = w.stream_position()?;
                w.write_all(&asset.data)?;
            }
            Ok(())
        })?;

        // Write updated ADIR offsets
        let pos = w.stream_position()?;
        w.seek(SeekFrom::Start(adir_pos))?;
        w.write_type(&asset_directory, e)?;
        w.seek(SeekFrom::Start(pos))?;

        // Align 16
        let aligned_end = (pos + 15) & !15;
        w.write_all(&vec![0u8; (aligned_end - pos) as usize])?;
        Ok(())
    }
}
