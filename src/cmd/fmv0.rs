use std::{
    fs,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{ensure, Context, Result};
use argh::FromArgs;
use binrw::Endian;

use crate::{
    format::{pack::K_FORM_FOOT, rfrm::FormDescriptor, FourCC},
    util::file::map_file,
};

// Video
pub const K_FORM_FMV0: FourCC = FourCC(*b"FMV0");

#[derive(FromArgs, PartialEq, Debug)]
/// process FMV0 files
#[argh(subcommand, name = "fmv0")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Extract(ExtractArgs),
    Replace(ReplaceArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// extracts video from FMV0
#[argh(subcommand, name = "extract")]
pub struct ExtractArgs {
    #[argh(positional)]
    /// input FMV0
    input: PathBuf,
    #[argh(positional)]
    /// output MP4
    output: PathBuf,
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// replaces FMV0 contents with a new video
#[argh(subcommand, name = "replace")]
pub struct ReplaceArgs {
    #[argh(positional)]
    /// existing FMV0
    fmv0: PathBuf,
    #[argh(positional)]
    /// input MP4
    video: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Extract(c_args) => extract(c_args),
        SubCommand::Replace(c_args) => replace(c_args),
    }
}

fn extract(args: ExtractArgs) -> Result<()> {
    let data = map_file(&args.input)?;
    let (fmv0_desc, chunk_data, _) = FormDescriptor::slice(&data, Endian::Little)?;
    ensure!(fmv0_desc.id == K_FORM_FMV0);
    fs::write(&args.output, chunk_data)
        .with_context(|| format!("Failed to write output file '{}'", args.output.display()))?;
    Ok(())
}

fn replace(args: ReplaceArgs) -> Result<()> {
    let (mut fmv0_desc, mut footer_desc, footer_data) = {
        let fmv0_data = map_file(&args.fmv0)?;
        let (fmv0_desc, _, remain) = FormDescriptor::slice(&fmv0_data, Endian::Little)?;
        ensure!(fmv0_desc.id == K_FORM_FMV0);
        let (footer_desc, footer_data, _) = FormDescriptor::slice(remain, Endian::Little)?;
        ensure!(footer_desc.id == K_FORM_FOOT);
        (fmv0_desc, footer_desc, footer_data.to_vec())
    };

    let data = map_file(&args.video)?;
    let mut file = BufWriter::new(
        File::create(&args.fmv0)
            .with_context(|| format!("Failed to create output file '{}'", args.fmv0.display()))?,
    );
    fmv0_desc.write(&mut file, Endian::Little, |w| {
        w.write_all(&data)?;
        Ok(())
    })?;
    footer_desc.write(&mut file, Endian::Little, |w| {
        w.write_all(&footer_data)?;
        Ok(())
    })?;
    file.flush()?;
    Ok(())
}
