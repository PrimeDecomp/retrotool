use std::path::PathBuf;

use anyhow::{ensure, Result};
use argh::FromArgs;
use binrw::Endian;

use crate::{
    format::{chunk::ChunkDescriptor, pack::K_CHUNK_META, rfrm::FormDescriptor, FourCC},
    util::file::map_file,
};

// Texture
pub const K_FORM_TXTR: FourCC = FourCC(*b"TXTR");
// Texture header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");

#[derive(FromArgs, PartialEq, Debug)]
/// process TXTR files
#[argh(subcommand, name = "txtr")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Convert(ConvertArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// converts a TXTR file
#[argh(subcommand, name = "convert")]
pub struct ConvertArgs {
    #[argh(positional)]
    /// input file
    input: PathBuf,
    #[argh(positional)]
    /// output directory
    output: PathBuf,
}

#[allow(unused)]
pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Convert(c_args) => convert(c_args),
    }
}

// struct TextureHeader {}

// struct TextureMeta {}

fn convert(args: ConvertArgs) -> Result<()> {
    let mmap = map_file(args.input)?;
    let (meta, _, remain) = ChunkDescriptor::slice(&mmap, Endian::Little)?;
    ensure!(meta.id == K_CHUNK_META);

    let (desc, data, _) = FormDescriptor::slice(remain, Endian::Little)?;
    ensure!(desc.id == K_FORM_TXTR);
    ensure!(desc.version == 47);
    let (desc, _, _) = ChunkDescriptor::slice(data, Endian::Little)?;
    ensure!(desc.id == K_CHUNK_HEAD);

    todo!()
}
