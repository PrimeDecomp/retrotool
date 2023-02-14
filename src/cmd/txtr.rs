use std::{
    fs::File,
    io::{BufWriter, Cursor, Write},
    path::PathBuf,
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use argh::FromArgs;
use binrw::{BinReaderExt, Endian};

use crate::{
    format::{
        chunk::ChunkDescriptor,
        pack::{K_CHUNK_META, K_FORM_FOOT},
        rfrm::FormDescriptor,
        txtr::{deswizzle, STextureHeader, STextureMetaData},
        FourCC,
    },
    util::{astc::write_astc, dds::write_dds, file::map_file, lzss::decompress_into},
};

// Texture
pub const K_FORM_TXTR: FourCC = FourCC(*b"TXTR");
// Texture header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// GPU data
pub const K_CHUNK_GPU: FourCC = FourCC(*b"GPU ");

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

    let (txtr_desc, txtr_data, remain) = FormDescriptor::slice(&data, Endian::Little)?;
    ensure!(txtr_desc.id == K_FORM_TXTR);
    ensure!(txtr_desc.version_a == 47);
    ensure!(txtr_desc.version_b == 51);
    let (foot_desc, mut foot_data, remain) = FormDescriptor::slice(remain, Endian::Little)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.version_a == 1);
    ensure!(foot_desc.version_b == 1);
    ensure!(remain.is_empty());

    let mut meta: Option<STextureMetaData> = None;
    while !foot_data.is_empty() {
        let (desc, data, remain) = ChunkDescriptor::slice(foot_data, Endian::Little)?;
        if desc.id == K_CHUNK_META {
            meta = Some(Cursor::new(data).read_type(Endian::Little)?);
            break;
        }
        foot_data = remain;
    }
    let Some(meta) = meta else {
        bail!("Failed to locate meta chunk");
    };

    let (head_desc, head_data, remain) = ChunkDescriptor::slice(txtr_data, Endian::Little)?;
    ensure!(head_desc.id == K_CHUNK_HEAD);
    let head: STextureHeader = Cursor::new(head_data).read_type(Endian::Little)?;
    let (gpu_desc, _, remain) = ChunkDescriptor::slice(remain, Endian::Little)?;
    ensure!(gpu_desc.id == K_CHUNK_GPU);
    ensure!(remain.is_empty());

    log::debug!("META: {meta:#?}");
    log::debug!("HEAD: {head:#?}");

    log::info!("Texture info:");
    log::info!("  Type: {:?}", head.kind);
    log::info!("  Format: {:?}", head.format);
    log::info!("  Size: {}x{}x{}", head.width, head.height, head.layers);
    log::info!("  Mip count: {}", head.mip_sizes.len());

    let mut buffer = vec![0u8; meta.decompressed_size as usize];
    for info in &meta.buffers {
        let read = meta
            .info
            .iter()
            .find(|i| i.index as u32 == info.index)
            .ok_or_else(|| anyhow!("Failed to locate read info for buffer {}", info.index))?;
        let read_buf = &data[read.offset as usize..(read.offset + read.size) as usize];
        let comp_buf = &read_buf[info.offset as usize..(info.offset + info.size) as usize];
        decompress_into(
            comp_buf,
            &mut buffer[info.dest_offset as usize..(info.dest_offset + info.dest_size) as usize],
        )?;
    }

    let deswizzled = deswizzle(&head, &buffer)?;
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
        write_astc(&mut file, &head, &deswizzled)?;
    } else {
        write_dds(&mut file, &head, deswizzled)?;
    }
    file.flush()?;

    Ok(())
}
