use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::Debug,
    fs,
    fs::File,
    io::{BufWriter, Cursor, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use anyhow::{bail, ensure, Context, Result};
use argh::FromArgs;
use binrw::{binrw, BinReaderExt, BinResult, BinWriterExt, Endian};
use uuid::Uuid;

use crate::{
    array_ref,
    format::{
        adir::{AssetDirectory, AssetDirectoryEntry, K_CHUNK_ADIR},
        chunk::{ChunkDescriptor, ChunkType},
        meta::{Metadata, MetadataEntry},
        peek_four_cc,
        rfrm::{FormDescriptor, K_CHUNK_RFRM, K_FORM_PAK, K_FORM_TOC},
        strg::{StringTable, StringTableEntry, K_CHUNK_STRG},
        FourCC,
    },
    util::{file::map_file, lzss::decompress},
};

#[derive(FromArgs, PartialEq, Debug)]
/// process PAK files
#[argh(subcommand, name = "pak")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Extract(ExtractArgs),
    Package(PackageArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// extract a PAK file
#[argh(subcommand, name = "extract")]
pub struct ExtractArgs {
    #[argh(positional)]
    /// input file
    input: PathBuf,
    #[argh(positional)]
    /// output directory
    output: PathBuf,
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// package a PAK file
#[argh(subcommand, name = "package")]
pub struct PackageArgs {
    #[argh(positional)]
    /// input directory
    input: PathBuf,
    #[argh(positional)]
    /// output file
    output: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Extract(c_args) => extract(c_args),
        SubCommand::Package(c_args) => package(c_args),
    }
}

/// Recursively dump an RFRM + contained chunks
fn dump_rfrm<'a, W: Write>(w: &mut W, data: &'a [u8], indent: usize) -> Result<&'a [u8]> {
    let (rfrm, mut rfrm_data, remain) = FormDescriptor::slice(data, Endian::Little)?;
    let indstr = "  ".repeat(indent);
    writeln!(w, "{indstr}{rfrm:?}")?;
    while !rfrm_data.is_empty() {
        if peek_four_cc(rfrm_data) == K_CHUNK_RFRM {
            rfrm_data = dump_rfrm(w, rfrm_data, indent + 1)?;
        } else {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(rfrm_data, Endian::Little)?;
            writeln!(w, "{indstr}- {desc:?}")?;
            rfrm_data = remain;
        }
    }
    Ok(remain)
}

#[binrw]
#[derive(Clone, Debug)]
pub struct AssetInfo {
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    id: Uuid,
    compression_type: u32,
}

#[derive(Debug, Clone)]
struct Asset<'a> {
    id: Uuid,
    kind: FourCC,
    name: Option<String>,
    // TODO lazy decompression?
    data: Cow<'a, [u8]>,
    meta: Option<Cow<'a, [u8]>>,
    info: AssetInfo,
    version: u32,
    other_version: u32,
}

#[derive(Debug, Clone)]
struct Package<'a> {
    assets: Vec<Asset<'a>>,
}

impl Package<'_> {
    fn read(data: &[u8], e: Endian) -> Result<Package> {
        let (pack, pack_data, _) = FormDescriptor::slice(data, e)?;
        ensure!(pack.id == K_FORM_PAK);
        ensure!(pack.version == 1);
        // log::info!("PACK: {:?}", pack);
        let (tocc, mut tocc_data, _) = FormDescriptor::slice(pack_data, e)?;
        ensure!(tocc.id == K_FORM_TOC);
        ensure!(tocc.version == 3);
        // log::info!("TOCC: {:?}", tocc);
        let mut adir: Option<AssetDirectory> = None;
        let mut meta: HashMap<Uuid, &[u8]> = HashMap::new();
        let mut strg: Option<StringTable> = None;
        while !tocc_data.is_empty() {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(tocc_data, e)?;
            // log::info!("{:?} data size {}", desc, chunk_data.len());
            let header = ChunkType::read(chunk_data, desc.id, e)?;
            match header {
                ChunkType::AssetDirectory(chunk) => {
                    // for entry in &chunk.entries {
                    //     log::info!("- {:?}", entry);
                    // }
                    adir = Some(chunk);
                }
                ChunkType::Metadata(chunk) => {
                    let mut iter = chunk.entries.iter().peekable();
                    while let Some(entry) = iter.next() {
                        let size = if let Some(next) = iter.peek() {
                            (next.offset - entry.offset) as usize
                        } else {
                            chunk_data.len() - entry.offset as usize
                        };
                        // log::info!("- {:?} (size {:#X})", entry, size);
                        meta.insert(
                            entry.asset_id,
                            &chunk_data[entry.offset as usize..entry.offset as usize + size],
                        );
                    }
                }
                ChunkType::StringTable(chunk) => {
                    // for entry in &chunk.entries {
                    //     log::info!("- {:?}", entry);
                    // }
                    strg = Some(chunk);
                }
            }
            tocc_data = remain;
        }
        // log::info!("Remaining PACK data: {:#X}", pack_remain.len());

        let mut package = Package { assets: vec![] };
        if let Some(adir) = adir {
            for asset_entry in adir.entries {
                let name =
                    strg.as_ref().and_then(|table| table.name_for_uuid(asset_entry.asset_id));

                let mut compression_type = 0u32;
                let data: Cow<[u8]> = if asset_entry.size != asset_entry.decompressed_size {
                    let compression_bytes =
                        &data[asset_entry.offset as usize..asset_entry.offset as usize + 4];
                    compression_type = u32::from_le_bytes(compression_bytes.try_into().unwrap());
                    // log::info!("Decompressing {}", compression_type);
                    let mut out = vec![0u8; asset_entry.decompressed_size as usize];
                    let data = &data[asset_entry.offset as usize + 4
                        ..(asset_entry.offset + asset_entry.size) as usize];
                    match compression_type {
                        1 => decompress::<1>(data, &mut out),
                        2 => decompress::<2>(data, &mut out),
                        3 => decompress::<3>(data, &mut out),
                        _ => bail!("Unsupported compression mode {}", compression_type),
                    }
                    Cow::Owned(out)
                } else {
                    Cow::Borrowed(
                        &data[asset_entry.offset as usize
                            ..(asset_entry.offset + asset_entry.size) as usize],
                    )
                };

                // Validate RFRM
                {
                    let (form, _, _) = FormDescriptor::slice(&data, Endian::Little)?;
                    ensure!(asset_entry.version == form.version);
                    ensure!(asset_entry.other_version == form.other_version);
                    ensure!(asset_entry.decompressed_size == form.size + 32);
                }

                package.assets.push(Asset {
                    id: asset_entry.asset_id,
                    kind: asset_entry.asset_type,
                    name,
                    data,
                    meta: meta.get(&asset_entry.asset_id).map(|data| Cow::Borrowed(*data)),
                    info: AssetInfo { id: asset_entry.asset_id, compression_type },
                    version: asset_entry.version,
                    other_version: asset_entry.other_version,
                });
            }
        } else {
            bail!("Failed to locate asset directory");
        }
        Ok(package)
    }

    fn write<W: Write + Seek>(&self, w: &mut W, e: Endian) -> Result<()> {
        let mut asset_directory = AssetDirectory::default();
        let mut metadata = Metadata::default();
        let mut string_table = StringTable::default();
        for asset in &self.assets {
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
                metadata.entries.push(MetadataEntry { asset_id: asset.id, offset: 0 });
            }
            if let Some(name) = &asset.name {
                string_table.entries.push(StringTableEntry {
                    kind: asset.kind,
                    asset_id: asset.id,
                    name: name.as_bytes().to_vec(),
                });
            }
        }
        let mut adir_offset = 0;
        FormDescriptor { size: 0, unk1: 0, id: K_FORM_PAK, version: 1, other_version: 1 }.write(
            w,
            e,
            |w| {
                FormDescriptor { size: 0, unk1: 0, id: K_FORM_TOC, version: 3, other_version: 3 }
                    .write(w, e, |w| {
                    ChunkDescriptor { id: K_CHUNK_ADIR, size: 0, unk: 1, skip: 0 }.write(
                        w,
                        e,
                        |w| {
                            adir_offset = w.stream_position()?;
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
                                w.write_all(asset.meta.as_ref().unwrap())?;
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
                for (asset, entry) in self.assets.iter().zip(&mut asset_directory.entries) {
                    entry.offset = w.stream_position()?;
                    w.write_all(&asset.data)?;
                }
                Ok(())
            },
        )?;
        // Write updated offsets
        w.seek(SeekFrom::Start(adir_offset))?;
        w.write_type(&asset_directory, e)?;
        Ok(())
    }
}

fn extract(args: ExtractArgs) -> Result<()> {
    let data = map_file(args.input)?;
    let package = Package::read(&data, Endian::Little)?;
    for asset in &package.assets {
        let name = asset
            .name
            .as_ref()
            .map(|name| format!("{} ({})", asset.id, name))
            .unwrap_or_else(|| format!("{}", asset.id));
        log::info!(
            "Asset {} {} size {:#X} (compressed {}, meta size {:#X})",
            asset.kind,
            name,
            asset.data.len(),
            asset.data.is_owned(),
            asset.meta.as_ref().map(|m| m.len()).unwrap_or_default()
        );
        let file_name = asset
            .name
            .as_ref()
            .map(|name| format!("{}.{}", name, asset.kind))
            .unwrap_or_else(|| format!("{}.{}", asset.id, asset.kind));
        let path = args.output.join(&file_name);

        let mut file = BufWriter::new(
            File::create(&path)
                .with_context(|| format!("Failed to create file '{}'", path.display()))?,
        );
        file.write_all(&asset.data)?;

        // Write custom footer
        let form_pos = file.stream_position()?;
        let mut form =
            FormDescriptor { size: 0, unk1: 0, id: FourCC(*b"FOOT"), version: 1, other_version: 1 };
        file.write_le(&form)?;
        let data_pos = file.stream_position()?;
        {
            let ainf_chunk = ChunkDescriptor { id: FourCC(*b"AINF"), size: 20, unk: 0, skip: 0 };
            file.write_le(&ainf_chunk)?;
            file.write_le(&asset.info)?;
        }
        if let Some(meta) = &asset.meta {
            let meta_chunk =
                ChunkDescriptor { id: FourCC(*b"META"), size: meta.len() as u64, unk: 0, skip: 0 };
            file.write_le(&meta_chunk)?;
            file.write_all(meta)?;
        }
        if let Some(name) = &asset.name {
            let bytes = name.as_bytes();
            let name_chunk =
                ChunkDescriptor { id: FourCC(*b"NAME"), size: bytes.len() as u64, unk: 0, skip: 0 };
            file.write_le(&name_chunk)?;
            file.write_all(bytes)?;
        }
        // Calculate size and rewrite FOOT header
        form.size = file.stream_position()? - data_pos;
        file.seek(SeekFrom::Start(form_pos))?;
        file.write_le(&form)?;
        file.flush()?;
    }
    Ok(())
}

const K_CHUNK_AINF: FourCC = FourCC(*b"AINF");
const K_CHUNK_META: FourCC = FourCC(*b"META");
const K_CHUNK_NAME: FourCC = FourCC(*b"NAME");

fn package(args: PackageArgs) -> Result<()> {
    let files = fs::read_dir(&args.input)?;
    let mut package = Package { assets: vec![] };
    for result in files {
        let entry = match result {
            Ok(e) => e,
            Err(e) => bail!("Failed to read directory entry: {:?}", e),
        };

        let path = entry.path();
        log::info!("Processing {}", path.display());
        let data = map_file(&path)?;
        let (form, _, remain) = FormDescriptor::slice(&data, Endian::Little)?;
        // log::info!("Found type {} version {}, {}", form.id, form.version, form.other_version);
        let (foot, mut foot_data, _) = FormDescriptor::slice(remain, Endian::Little)?;
        ensure!(foot.id == *b"FOOT");
        ensure!(foot.version == 1);
        let mut aid: Option<Uuid> = None;
        let mut meta: Option<&[u8]> = None;
        let mut name: Option<String> = None;
        while !foot_data.is_empty() {
            let (chunk, chunk_data, remain) = ChunkDescriptor::slice(foot_data, Endian::Little)?;
            match chunk.id {
                K_CHUNK_AINF => {
                    let ainf: AssetInfo = Cursor::new(chunk_data).read_type(Endian::Little)?;
                    // log::info!("AID: {}, compression: {}", ainf.id, ainf.compression_type);
                    aid = Some(ainf.id);
                }
                K_CHUNK_META => {
                    meta = Some(chunk_data);
                }
                K_CHUNK_NAME => {
                    name = Some(String::from_utf8(chunk_data.to_vec())?);
                }
                _ => {}
            }
            foot_data = remain;
        }
        let aid = match aid {
            Some(a) => a,
            None => bail!("Failed to locate asset ID"),
        };
        package.assets.push(Asset {
            id: aid,
            kind: form.id,
            name,
            data: Cow::Owned(data[..data.len() - remain.len()].to_vec()),
            meta: meta.map(|data| Cow::Owned(data.to_vec())),
            info: AssetInfo { id: aid, compression_type: 0 /* TODO */ },
            version: form.version,
            other_version: form.other_version,
        });
    }
    let mut file =
        BufWriter::new(File::create(&args.output).with_context(|| {
            format!("Failed to create output file '{}'", args.output.display())
        })?);
    package.write(&mut file, Endian::Little)?;
    file.flush()?;
    Ok(())
}
