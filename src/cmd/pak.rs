use std::{
    borrow::Cow,
    fmt::Debug,
    fs,
    fs::File,
    io::{BufWriter, Cursor, Write},
    path::PathBuf,
};

use anyhow::{bail, ensure, Context, Result};
use argh::FromArgs;
use binrw::{BinReaderExt, BinWriterExt, Endian};

use crate::{
    format::{
        chunk::ChunkDescriptor,
        pack::{Asset, AssetInfo, Package, K_CHUNK_AINF, K_CHUNK_META, K_CHUNK_NAME, K_FORM_FOOT},
        rfrm::FormDescriptor,
    },
    util::file::map_file,
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
            asset.info.compression_mode != 0,
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
        FormDescriptor { size: 0, unk1: 0, id: K_FORM_FOOT, version: 1, other_version: 1 }.write(
            &mut file,
            Endian::Little,
            |w| {
                ChunkDescriptor { id: K_CHUNK_AINF, size: 0, unk: 0, skip: 0 }.write(
                    w,
                    Endian::Little,
                    |w| {
                        w.write_le(&asset.info)?;
                        Ok(())
                    },
                )?;
                if let Some(meta) = &asset.meta {
                    let meta_chunk = ChunkDescriptor {
                        id: K_CHUNK_META,
                        size: meta.len() as u64,
                        unk: 0,
                        skip: 0,
                    };
                    w.write_le(&meta_chunk)?;
                    w.write_all(meta)?;
                }
                if let Some(name) = &asset.name {
                    let bytes = name.as_bytes();
                    let name_chunk = ChunkDescriptor {
                        id: K_CHUNK_NAME,
                        size: bytes.len() as u64,
                        unk: 0,
                        skip: 0,
                    };
                    w.write_le(&name_chunk)?;
                    w.write_all(bytes)?;
                }
                Ok(())
            },
        )?;
        file.flush()?;
    }
    Ok(())
}

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
        ensure!(foot.id == K_FORM_FOOT);
        ensure!(foot.version == 1);
        let mut ainfo: Option<AssetInfo> = None;
        let mut meta: Option<&[u8]> = None;
        let mut name: Option<String> = None;
        while !foot_data.is_empty() {
            let (chunk, chunk_data, remain) = ChunkDescriptor::slice(foot_data, Endian::Little)?;
            match chunk.id {
                K_CHUNK_AINF => {
                    ainfo = Some(Cursor::new(chunk_data).read_type(Endian::Little)?);
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
        let ainfo = match ainfo {
            Some(a) => a,
            None => bail!("Failed to locate asset info footer"),
        };
        package.assets.push(Asset {
            id: ainfo.id,
            kind: form.id,
            name,
            data: Cow::Owned(data[..data.len() - remain.len()].to_vec()),
            meta: meta.map(|data| Cow::Owned(data.to_vec())),
            info: ainfo,
            version: form.version,
            other_version: form.other_version,
        });
    }
    package.assets.sort_by_key(|a| a.info.entry_idx);
    let mut file =
        BufWriter::new(File::create(&args.output).with_context(|| {
            format!("Failed to create output file '{}'", args.output.display())
        })?);
    package.write(&mut file, Endian::Little)?;
    file.flush()?;
    Ok(())
}
