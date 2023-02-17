use std::{fs, io::Cursor, path::PathBuf};

use anyhow::{bail, ensure, Result};
use argh::FromArgs;
use binrw::{binrw, BinReaderExt, Endian};

use crate::{
    format::{
        chunk::ChunkDescriptor,
        pack::{K_CHUNK_META, K_FORM_FOOT},
        rfrm::FormDescriptor,
        FourCC,
    },
    util::{file::map_file, lzss::decompress_into},
};

// Cooked model
pub const K_FORM_CMDL: FourCC = FourCC(*b"CMDL");

// Model header
// pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// Material data
// pub const K_CHUNK_MTRL: FourCC = FourCC(*b"MTRL");
// Mesh data
// pub const K_CHUNK_MESH: FourCC = FourCC(*b"MESH");
// Vertex buffer
pub const K_CHUNK_VBUF: FourCC = FourCC(*b"VBUF");
// Index buffer
pub const K_CHUNK_IBUF: FourCC = FourCC(*b"IBUF");
// GPU data
// pub const K_CHUNK_GPU: FourCC = FourCC(*b"GPU ");

#[derive(FromArgs, PartialEq, Debug)]
/// process CMDL files
#[argh(subcommand, name = "cmdl")]
pub struct Args {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Info(InfoArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// displays model information
#[argh(subcommand, name = "info")]
pub struct InfoArgs {
    #[argh(positional)]
    /// input CMDL
    input: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Info(c_args) => info(c_args),
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelReadBufferInfo {
    pub size: u32,
    pub offset: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelBufferInfo {
    pub index: u32,
    pub offset: u32,
    pub size: u32,
    pub dest_size: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelMetaData {
    pub unk1: u32,
    pub gpu_offset: u32,
    #[bw(try_calc = read_info.len().try_into())]
    pub read_info_count: u32,
    #[br(count = read_info_count)]
    pub read_info: Vec<SModelReadBufferInfo>,
    #[bw(try_calc = buffer_info.len().try_into())]
    pub buffer_count: u32,
    #[br(count = buffer_count)]
    pub buffer_info: Vec<SModelBufferInfo>,
    #[bw(try_calc = idx_buffer_info.len().try_into())]
    pub idx_info_count: u32,
    #[br(count = idx_info_count)]
    pub idx_buffer_info: Vec<SModelBufferInfo>,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug)]
pub enum EIndexBufferType {
    U8 = 0, // ?
    U16 = 1,
    U32 = 2,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SIndexBufferInfoSection {
    #[bw(try_calc = info.len().try_into())]
    pub count: u32,
    #[br(count = count)]
    pub info: Vec<EIndexBufferType>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexBufferInfoSection {
    #[bw(try_calc = info.len().try_into())]
    pub count: u32,
    #[br(count = count)]
    pub info: Vec<SVertexBufferInfo>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexBufferInfo {
    vertex_count: u32,
    #[bw(try_calc = components.len().try_into())]
    component_count: u32,
    #[br(count = component_count)]
    components: Vec<SVertexDataComponent>,
    unk: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexDataComponent {
    buffer_index: u32,
    offset: u32,
    stride: u32,
    format: EVertexDataFormat,
    component: EVertexComponent,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug)]
enum EVertexDataFormat {
    R8Unorm = 0,
    R8Uint = 1,
    R8Snorm = 2,
    R8Sint = 3,
    R16Unorm = 4,
    R16Uint = 5,
    R16Snorm = 6,
    R16Sint = 7,
    R16Float = 8,
    Rg8Unorm = 9,
    Rg8Uint = 10,
    Rg8Snorm = 11,
    Rg8Sint = 12,
    R32Uint = 13,
    R32Sint = 14,
    R32Float = 15,
    Rg16Unorm = 16,
    Rg16Uint = 17,
    Rg16Snorm = 18,
    Rg16Sint = 19,
    Rg16Float = 20,
    Rgba8Unorm = 21,
    Rgba8Uint = 22,
    Rgba8Snorm = 23,
    Rgba8Sint = 24,
    Rgb10a2Unorm = 25,
    Rgb10a2Uint = 26,
    Rg32Uint = 27,
    Rg32Sint = 28,
    Rg32Float = 29,
    Rgba16Unorm = 30,
    Rgba16Uint = 31,
    Rgba16Snorm = 32,
    Rgba16Sint = 33,
    Rgba16Float = 34,
    Rgb32Uint = 35,
    Rgb32Sint = 36,
    Rgb32Float = 37,
    Rgba32Uint = 38,
    Rgba32Sint = 39,
    Rgba32Float = 40,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug)]
enum EVertexComponent {
    Position = 0,                           // in_position
    Normal = 1,                             // in_normal
    Tangent0 = 2,                           // in_tangent[0]
    Tangent1 = 3,                           // in_tangent[1]
    Tangent2 = 4,                           // in_tangent[2]
    TexCoord0 = 5,                          // in_texCoord[0]
    TexCoord1 = 6,                          // in_texCoord[1]
    TexCoord2 = 7,                          // in_texCoord[2]
    TexCoord3 = 8,                          // in_texCoord[3]
    Color = 9,                              // in_color
    BoneIndices = 10,                       // in_boneIndices
    BoneWeights = 11,                       // in_boneWeights
    BakedLightingCoord = 12,                // in_bakedLightingCoord
    BakedLightingTangent = 13,              // in_bakedLightingTangent
    VertInstanceParams = 14,                // in_vertInstanceParams
    VertInstanceColor = 15,                 // in_vertInstanceColor
    VertTransform0 = 16,                    // in_vertTransform[0]
    VertTransform1 = 17,                    // in_vertTransform[1]
    VertTransform2 = 18,                    // in_vertTransform[2]
    CurrentPosition = 19,                   // in_currentPosition
    VertInstanceOpacityParams = 20,         // in_vertInstanceOpacityParams
    VertInstanceColorIndexingParams = 21,   // in_vertInstanceColorIndexingParams
    VertInstanceOpacityIndexingParams = 22, // in_vertInstanceOpacityIndexingParams
    VertInstancePaintParams = 23,           // in_vertInstancePaintParams
    BakedLightingLookup = 24,               // in_bakedLightingLookup
    MaterialChoice0 = 25,                   // in_materialChoice[0]
    MaterialChoice1 = 26,                   // in_materialChoice[1]
    MaterialChoice2 = 27,                   // in_materialChoice[2]
    MaterialChoice3 = 28,                   // in_materialChoice[3]
}

fn decompress_gpu_buffer(
    data: &[u8],
    read_info: &[SModelReadBufferInfo],
    buffer_info: &[SModelBufferInfo],
) -> Result<Vec<u8>> {
    let buffer_size = buffer_info.iter().map(|b| b.dest_size as usize).sum();
    let mut out = vec![0u8; buffer_size];
    let mut out_cur = 0usize;
    for buffer_info in buffer_info {
        let read_info = &read_info[buffer_info.index as usize];
        let read_buffer =
            &data[read_info.offset as usize..(read_info.offset + read_info.size) as usize];
        let comp_buf = &read_buffer
            [buffer_info.offset as usize..(buffer_info.offset + buffer_info.size) as usize];
        decompress_into(comp_buf, &mut out[out_cur..out_cur + buffer_info.dest_size as usize])?;
        out_cur += buffer_info.dest_size as usize;
    }
    Ok(out)
}

fn info(args: InfoArgs) -> Result<()> {
    let data = map_file(&args.input)?;

    let (cmdl_desc, mut cmdl_data, remain) = FormDescriptor::slice(&data, Endian::Little)?;
    ensure!(cmdl_desc.id == K_FORM_CMDL);
    ensure!(cmdl_desc.version_a == 114);
    ensure!(cmdl_desc.version_b == 125);
    let (foot_desc, mut foot_data, remain) = FormDescriptor::slice(remain, Endian::Little)?;
    ensure!(foot_desc.id == K_FORM_FOOT);
    ensure!(foot_desc.version_a == 1);
    ensure!(foot_desc.version_b == 1);
    ensure!(remain.is_empty());

    let mut meta: Option<SModelMetaData> = None;
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

    // TODO multiple buffers
    let buffer = decompress_gpu_buffer(&data, &meta.read_info, &meta.buffer_info)?;
    let idx_buffer = decompress_gpu_buffer(&data, &meta.read_info, &meta.idx_buffer_info)?;

    let mut vbuf: Option<SVertexBufferInfoSection> = None;
    let mut ibuf: Option<SIndexBufferInfoSection> = None;
    while !cmdl_data.is_empty() {
        let (chunk_desc, chunk_data, remain) = ChunkDescriptor::slice(cmdl_data, Endian::Little)?;
        match chunk_desc.id {
            K_CHUNK_VBUF => vbuf = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            K_CHUNK_IBUF => ibuf = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            _ => {}
        }
        cmdl_data = remain;
    }

    log::debug!("VBUF: {vbuf:#?}");
    log::debug!("IBUF: {ibuf:#?}");

    fs::write("vtxbuf", buffer)?;
    fs::write("idxbuf", idx_buffer)?;

    todo!()
}
