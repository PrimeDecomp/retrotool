use std::{
    fmt::{Debug, Display, Formatter, Write},
    fs,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::PathBuf,
};

use anyhow::{anyhow, bail, ensure, Result};
use argh::FromArgs;
use binrw::{BinRead, BinReaderExt, BinResult, Endian, NullString};
use binrw_derive::{binrw, BinWrite};
use uuid::Uuid;

use crate::util::file::map_file;

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

#[binrw]
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    #[inline]
    const fn swap(self) -> Self { Self([self.0[3], self.0[2], self.0[1], self.0[0]]) }
}

impl Display for FourCC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for c in self.0 {
            f.write_char(c as char)?;
        }
        Ok(())
    }
}

impl Debug for FourCC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_char('"')?;
        for c in self.0 {
            f.write_char(c as char)?;
        }
        f.write_char('"')?;
        Ok(())
    }
}

impl PartialEq<[u8; 4]> for FourCC {
    fn eq(&self, other: &[u8; 4]) -> bool { &self.0 == other }
}

// PAK file
const K_FORM_PAK: FourCC = FourCC(*b"PACK");
// Table of contents
const K_FORM_TOC: FourCC = FourCC(*b"TOCC");

#[binrw]
#[brw(magic = b"RFRM")]
#[derive(Clone, Debug)]
pub struct FormDescriptor {
    pub size: u64,
    pub unk: u64,
    pub id: FourCC,
    pub version: u32,
    pub other_version: u32, // ?
}

impl FormDescriptor {
    #[inline]
    fn read<R: Read + Seek>(reader: &mut R, e: Endian) -> BinResult<Self> { reader.read_type(e) }

    #[inline]
    fn slice(data: &[u8], e: Endian) -> BinResult<(Self, &[u8], &[u8])> {
        let mut reader = Cursor::new(data);
        let header = Self::read(&mut reader, e)?;
        let start = reader.position();
        let slice = &data[start as usize..(start + header.size) as usize];
        let remain = &data[(start + header.size) as usize..];
        Ok((header, slice, remain))
    }
}

// Asset directory
const K_CHUNK_ADIR: FourCC = FourCC(*b"ADIR");
// Metadata
const K_CHUNK_META: FourCC = FourCC(*b"META");
// String table
const K_CHUNK_STRG: FourCC = FourCC(*b"STRG");

#[binrw]
#[derive(Clone, Debug)]
pub struct ChunkDescriptor {
    pub id: FourCC,
    pub size: u64,
    pub unk: u32,
    // game skips this amount of bytes before continuing
    // but always 0?
    pub skip: u64,
}

impl ChunkDescriptor {
    #[inline]
    fn read<R: Read + Seek>(reader: &mut R, e: Endian) -> BinResult<Self> {
        let desc: ChunkDescriptor = reader.read_type(e)?;
        reader.seek(SeekFrom::Current(desc.skip as i64))?;
        Ok(desc)
    }

    #[inline]
    fn slice(data: &[u8], e: Endian) -> BinResult<(Self, &[u8], &[u8])> {
        let mut reader = Cursor::new(data);
        let header = Self::read(&mut reader, e)?;
        let start = reader.position();
        let slice = &data[start as usize..(start + header.size) as usize];
        let remain = &data[(start + header.size) as usize..];
        Ok((header, slice, remain))
    }
}

#[derive(Clone, Debug)]
enum ChunkType {
    AssetDirectory(AssetDirectory),
    Metadata(Metadata),
    StringTable(StringTable),
}

impl ChunkType {
    #[inline]
    fn read(data: &[u8], kind: FourCC, e: Endian) -> Result<Self> {
        let mut reader = Cursor::new(data);
        match kind {
            K_CHUNK_ADIR => Ok(Self::AssetDirectory(reader.read_type(e)?)),
            K_CHUNK_META => Ok(Self::Metadata(reader.read_type(e)?)),
            K_CHUNK_STRG => Ok(Self::StringTable(reader.read_type(e)?)),
            _ => Err(anyhow!("Unknown chunk type {:?}", kind)),
        }
    }
}

#[binrw]
#[derive(Clone, Debug)]
struct AssetDirectory {
    #[bw(try_calc = entries.len().try_into())]
    entry_count: u32,
    #[br(count = entry_count)]
    entries: Vec<AssetDirectoryEntry>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct AssetDirectoryEntry {
    pub asset_type: FourCC,
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    pub unk1: u32,
    pub unk2: u32,
    pub offset: u64,
    pub size: u64,
    pub unk3: u64,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct Metadata {
    #[bw(try_calc = entries.len().try_into())]
    entry_count: u32,
    #[br(count = entry_count)]
    entries: Vec<MetadataEntry>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct MetadataEntry {
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    pub offset: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct StringTable {
    #[bw(try_calc = entries.len().try_into())]
    entry_count: u32,
    #[br(count = entry_count)]
    entries: Vec<StringTableEntry>,
}

impl StringTable {
    fn name_for_uuid(&self, id: Uuid) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e.asset_id == id)
            .map(|e| String::from_utf8(e.name.clone()).unwrap())
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct StringTableEntry {
    #[br(map = FourCC::swap)]
    #[bw(map = |&f| f.swap())]
    pub kind: FourCC,
    #[br(map = Uuid::from_u128)]
    #[bw(map = Uuid::as_u128)]
    pub asset_id: Uuid,
    #[bw(try_calc = name.len().try_into())]
    pub name_length: u32,
    #[br(count = name_length)]
    pub name: Vec<u8>,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Extract(c_args) => extract(c_args),
    }
}

/// Recursively dump an RFRM + contained chunks
fn dump_rfrm<'a, W: std::io::Write>(w: &mut W, data: &'a [u8], indent: usize) -> Result<&'a [u8]> {
    let (rfrm, mut rfrm_data, remain) = FormDescriptor::slice(data, Endian::Little)?;
    let indstr = "  ".repeat(indent);
    writeln!(w, "{indstr}{rfrm:?}")?;
    while !rfrm_data.is_empty() {
        if rfrm_data[0..4] == *b"RFRM" {
            rfrm_data = dump_rfrm(w, rfrm_data, indent + 1)?;
        } else {
            let (desc, chunk_data, remain) = ChunkDescriptor::slice(rfrm_data, Endian::Little)?;
            writeln!(w, "{indstr}- {desc:?}")?;
            rfrm_data = remain;
        }
    }
    Ok(remain)
}

fn extract(args: ExtractArgs) -> Result<()> {
    let data = map_file(args.input)?;
    let (pack, pack_data, _) = FormDescriptor::slice(&data, Endian::Little)?;
    ensure!(pack.id == K_FORM_PAK);
    ensure!(pack.version == 1);
    log::info!("PACK: {:?}", pack);
    let (tocc, mut tocc_data, pack_remain) = FormDescriptor::slice(pack_data, Endian::Little)?;
    ensure!(tocc.id == K_FORM_TOC);
    ensure!(tocc.version == 3);
    log::info!("TOCC: {:?}", tocc);
    let mut adir: Option<AssetDirectory> = None;
    let mut meta: Option<Metadata> = None;
    let mut strg: Option<StringTable> = None;
    while !tocc_data.is_empty() {
        let (desc, chunk_data, remain) = ChunkDescriptor::slice(tocc_data, Endian::Little)?;
        log::info!("{:?} data size {}", desc, chunk_data.len());
        let header = ChunkType::read(chunk_data, desc.id, Endian::Little)?;
        match header {
            ChunkType::AssetDirectory(chunk) => {
                for entry in &chunk.entries {
                    log::info!("- {:?}", entry);
                }
                adir = Some(chunk);
            }
            ChunkType::Metadata(chunk) => {
                for entry in &chunk.entries {
                    log::info!("- {:?}", entry);
                }
                meta = Some(chunk);
            }
            ChunkType::StringTable(chunk) => {
                for entry in &chunk.entries {
                    log::info!("- {:?}", entry);
                }
                strg = Some(chunk);
            }
        }
        tocc_data = remain;
    }
    log::info!("Remaining PACK data: {:#X}", pack_remain.len());
    if let Some(adir) = adir {
        for asset_entry in adir.entries {
            let name = strg
                .as_ref()
                .and_then(|table| table.name_for_uuid(asset_entry.asset_id))
                .unwrap_or_else(|| format!("{}", asset_entry.asset_id));
            let filename = format!("{}.{}", name, asset_entry.asset_type);
            let path = args.output.join(filename);
            // log::info!("Extracting {}", path.display());
            let mut data = &data
                [asset_entry.offset as usize..(asset_entry.offset + asset_entry.size) as usize];
            if asset_entry.asset_type == *b"FMV0" {
                let (hdr, fmv_data, _) = FormDescriptor::slice(data, Endian::Little)?;
                // log::info!("{:?}", hdr);
                data = fmv_data;
            } else if asset_entry.asset_type == *b"ROOM" {
                let mut file = File::create("ROOM-format.txt")?;
                dump_rfrm(&mut file, data, 0)?;
            }
            fs::write(path, data)?;
        }
    } else {
        bail!("Failed to locate ADIR chunk");
    }
    Ok(())
}
