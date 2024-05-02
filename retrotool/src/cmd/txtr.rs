use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use argh::FromArgs;
use retrolib::{
    format::{foot::locate_meta, txtr::TextureData},
    util::{astc::write_astc, dds::write_dds, file::map_file},
};
use zerocopy::LittleEndian;

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
/// converts a TXTR file to DDS/ASTC
#[argh(subcommand, name = "convert")]
pub struct ConvertArgs {
    #[argh(positional)]
    /// input TXTR
    input: PathBuf,
    #[argh(switch, short = 'a')]
    /// write ASTC file instead of DDS (no mips)
    astc: bool,
}

#[allow(unused)]
pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Convert(c_args) => convert(c_args),
    }
}

fn convert(args: ConvertArgs) -> Result<()> {
    let data = map_file(&args.input)?;
    let meta = locate_meta::<LittleEndian>(&data)?;
    let TextureData { head, data, .. } = TextureData::<LittleEndian>::slice(&data, meta)?;

    log::info!("Texture info:");
    log::info!("  Type: {}", head.kind);
    log::info!("  Format: {}", head.format);
    log::info!("  Size: {}x{}x{}", head.width, head.height, head.layers);
    log::info!("  Mip count: {}", head.mip_sizes.len());

    let path = if args.astc {
        if !head.format.is_astc() {
            bail!("Expected ASTC format, got {:?}", head.format);
        }
        args.input.with_extension("astc")
    } else {
        args.input.with_extension("dds")
    };
    let mut file = BufWriter::new(
        File::create(&path)
            .with_context(|| format!("Failed to create output file '{}'", path.display()))?,
    );
    log::info!("Writing {}", path.display());
    if args.astc {
        write_astc(&mut file, &head, &data)?;
    } else {
        write_dds(&mut file, &head, data)?;
    }
    file.flush()?;

    Ok(())
}
