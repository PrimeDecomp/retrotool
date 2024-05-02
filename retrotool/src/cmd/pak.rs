use std::{
    borrow::Cow,
    fmt::Debug,
    fs,
    fs::{DirBuilder, File},
    io::{BufWriter, Cursor, Write},
    path::PathBuf,
};

use anyhow::{bail, ensure, Context, Result};
use argh::FromArgs;
use binrw::{BinReaderExt, BinWriterExt, Endian};
use retrolib::{
    format::{
        chunk::ChunkDescriptor,
        foot::{K_CHUNK_AINF, K_CHUNK_NAME, K_FORM_FOOT},
        pack::{Asset, AssetInfo, Package, K_CHUNK_META},
        rfrm::FormDescriptor,
    },
    util::file::map_file,
};
use zerocopy::{AsBytes, LittleEndian, U32, U64};

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
    let package = Package::<LittleEndian>::read_full(&data, Endian::Little)?;
    for asset in &package.assets {
        let asset_names = asset.names.join(", ");
        let name = if asset_names.is_empty() {
            format!("{}", asset.id)
        } else {
            format!("{} ({})", asset.id, asset_names)
        };
        log::info!(
            "Asset {} {} size {:#X} (compressed {}, meta size {:#X})",
            asset.kind,
            name,
            asset.data.len(),
            asset.info.compression_mode != 0,
            asset.meta.as_ref().map(|m| m.len()).unwrap_or_default()
        );
        let file_name = asset
            .names
            .first()
            .map(|name| format!("{}.{}", name, asset.kind))
            .unwrap_or_else(|| format!("{}.{}", asset.id, asset.kind));
        let path = args.output.join(&file_name);
        if let Some(parent) = path.parent() {
            DirBuilder::new().recursive(true).create(parent)?;
        }

        let mut file = BufWriter::new(
            File::create(&path)
                .with_context(|| format!("Failed to create file '{}'", path.display()))?,
        );
        file.write_all(&asset.data)?;

        // Write custom footer
        FormDescriptor::<LittleEndian> {
            id: K_FORM_FOOT,
            reader_version: U32::new(1),
            writer_version: U32::new(1),
            ..Default::default()
        }
        .write(&mut file, |w| {
            ChunkDescriptor::<LittleEndian> { id: K_CHUNK_AINF, ..Default::default() }.write(
                w,
                |w| {
                    w.write_le(&asset.info)?;
                    Ok(())
                },
            )?;
            if let Some(meta) = &asset.meta {
                w.write_all(
                    ChunkDescriptor::<LittleEndian> {
                        id: K_CHUNK_META,
                        size: U64::new(meta.len() as u64),
                        ..Default::default()
                    }
                    .as_bytes(),
                )?;
                w.write_all(meta)?;
            }
            for name in &asset.names {
                let bytes = name.as_bytes();
                w.write_all(
                    ChunkDescriptor::<LittleEndian> {
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
        file.flush()?;
    }
    Ok(())
}

fn package(args: PackageArgs) -> Result<()> {
    let files = fs::read_dir(&args.input)?;
    let mut package = Package::<LittleEndian>::default();
    for result in files {
        let entry = match result {
            Ok(e) => e,
            Err(e) => bail!("Failed to read directory entry: {:?}", e),
        };

        let path = entry.path();
        log::info!("Processing {}", path.display());
        let data = map_file(&path)?;
        let (form, _, remain) = FormDescriptor::<LittleEndian>::slice(&data)?;
        // log::info!("Found type {} version {}, {}", form.id, form.version, form.other_version);
        let (foot, mut foot_data, _) = FormDescriptor::<LittleEndian>::slice(remain)?;
        ensure!(foot.id == K_FORM_FOOT);
        ensure!(foot.reader_version.get() == 1);
        let mut ainfo: Option<AssetInfo> = None;
        let mut meta: Option<&[u8]> = None;
        let mut names: Vec<String> = vec![];
        while !foot_data.is_empty() {
            let (chunk, chunk_data, remain) = ChunkDescriptor::<LittleEndian>::slice(foot_data)?;
            match chunk.id {
                K_CHUNK_AINF => {
                    ainfo = Some(Cursor::new(chunk_data).read_type(Endian::Little)?);
                }
                K_CHUNK_META => {
                    meta = Some(chunk_data);
                }
                K_CHUNK_NAME => {
                    names.push(String::from_utf8(chunk_data.to_vec())?);
                }
                _ => {}
            }
            foot_data = remain;
        }
        let Some(ainfo) = ainfo else {
            bail!("Failed to locate asset info footer");
        };
        package.assets.push(Asset {
            id: ainfo.id,
            kind: form.id,
            names,
            data: Cow::Owned(data[..data.len() - remain.len()].to_vec()),
            meta: meta.map(|data| Cow::Owned(data.to_vec())),
            info: ainfo,
            version: form.reader_version.get(),
            other_version: form.writer_version.get(),
        });
    }
    package.assets.sort_by_key(|a| a.id);
    let mut file =
        BufWriter::new(File::create(&args.output).with_context(|| {
            format!("Failed to create output file '{}'", args.output.display())
        })?);
    package.write(&mut file)?;
    file.flush()?;
    Ok(())
}
