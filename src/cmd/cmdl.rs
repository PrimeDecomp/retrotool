use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    fs::DirBuilder,
    io::{Cursor, Read, Write},
    path::PathBuf,
    string::FromUtf8Error,
};

use anyhow::{bail, ensure, Result};
use argh::FromArgs;
use binrw::{binrw, BinReaderExt, BinWriterExt, Endian};
use gltf_json as json;
use half::f16;
use json::validation::Checked::Valid;
use serde_json::json;
use uuid::Uuid;

use crate::{
    format::{
        chunk::ChunkDescriptor,
        pack::{K_CHUNK_META, K_FORM_FOOT},
        rfrm::FormDescriptor,
        FourCC,
    },
    util::{file::map_file, lzss::decompress_buffer},
};

// Cooked model
pub const K_FORM_CMDL: FourCC = FourCC(*b"CMDL");

// Model header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// Material data
pub const K_CHUNK_MTRL: FourCC = FourCC(*b"MTRL");
// Mesh data
pub const K_CHUNK_MESH: FourCC = FourCC(*b"MESH");
// Vertex buffer
pub const K_CHUNK_VBUF: FourCC = FourCC(*b"VBUF");
// Index buffer
pub const K_CHUNK_IBUF: FourCC = FourCC(*b"IBUF");
// GPU data
pub const K_CHUNK_GPU: FourCC = FourCC(*b"GPU ");

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
    Convert(ConvertArgs),
}

#[derive(FromArgs, PartialEq, Eq, Debug)]
/// converts a CMDL to glTF
#[argh(subcommand, name = "convert")]
pub struct ConvertArgs {
    #[argh(positional)]
    /// input CMDL
    input: PathBuf,
    #[argh(positional)]
    /// output directory
    out_dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Convert(c_args) => convert(c_args),
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
    pub read_index: u32,
    pub offset: u32,
    pub size: u32,
    pub dest_size: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelMetaData {
    pub unk: u32,
    pub gpu_offset: u32,
    #[bw(try_calc = read_info.len().try_into())]
    pub read_info_count: u32,
    #[br(count = read_info_count)]
    pub read_info: Vec<SModelReadBufferInfo>,
    #[bw(try_calc = vtx_buffer_info.len().try_into())]
    pub vtx_buffer_count: u32,
    #[br(count = vtx_buffer_count)]
    pub vtx_buffer_info: Vec<SModelBufferInfo>,
    #[bw(try_calc = idx_buffer_info.len().try_into())]
    pub idx_info_count: u32,
    #[br(count = idx_info_count)]
    pub idx_buffer_info: Vec<SModelBufferInfo>,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug)]
pub enum EBufferType {
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
    pub info: Vec<EBufferType>,
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
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum EVertexDataFormat {
    Unknown = u32::MAX,
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

impl EVertexDataFormat {
    fn byte_size(self) -> u32 {
        match self {
            EVertexDataFormat::Unknown => 0,
            EVertexDataFormat::R8Unorm => 1,
            EVertexDataFormat::R8Uint => 1,
            EVertexDataFormat::R8Snorm => 1,
            EVertexDataFormat::R8Sint => 1,
            EVertexDataFormat::R16Unorm => 2,
            EVertexDataFormat::R16Uint => 2,
            EVertexDataFormat::R16Snorm => 2,
            EVertexDataFormat::R16Sint => 2,
            EVertexDataFormat::R16Float => 2,
            EVertexDataFormat::Rg8Unorm => 2,
            EVertexDataFormat::Rg8Uint => 2,
            EVertexDataFormat::Rg8Snorm => 2,
            EVertexDataFormat::Rg8Sint => 2,
            EVertexDataFormat::R32Uint => 4,
            EVertexDataFormat::R32Sint => 4,
            EVertexDataFormat::R32Float => 4,
            EVertexDataFormat::Rg16Unorm => 4,
            EVertexDataFormat::Rg16Uint => 4,
            EVertexDataFormat::Rg16Snorm => 4,
            EVertexDataFormat::Rg16Sint => 4,
            EVertexDataFormat::Rg16Float => 4,
            EVertexDataFormat::Rgba8Unorm => 4,
            EVertexDataFormat::Rgba8Uint => 4,
            EVertexDataFormat::Rgba8Snorm => 4,
            EVertexDataFormat::Rgba8Sint => 4,
            EVertexDataFormat::Rgb10a2Unorm => 4,
            EVertexDataFormat::Rgb10a2Uint => 4,
            EVertexDataFormat::Rg32Uint => 8,
            EVertexDataFormat::Rg32Sint => 8,
            EVertexDataFormat::Rg32Float => 8,
            EVertexDataFormat::Rgba16Unorm => 8,
            EVertexDataFormat::Rgba16Uint => 8,
            EVertexDataFormat::Rgba16Snorm => 8,
            EVertexDataFormat::Rgba16Sint => 8,
            EVertexDataFormat::Rgba16Float => 8,
            EVertexDataFormat::Rgb32Uint => 12,
            EVertexDataFormat::Rgb32Sint => 12,
            EVertexDataFormat::Rgb32Float => 12,
            EVertexDataFormat::Rgba32Uint => 16,
            EVertexDataFormat::Rgba32Sint => 16,
            EVertexDataFormat::Rgba32Float => 16,
        }
    }

    fn normalized(self) -> bool {
        matches!(
            self,
            EVertexDataFormat::R8Unorm
                | EVertexDataFormat::R8Snorm
                | EVertexDataFormat::R16Unorm
                | EVertexDataFormat::R16Snorm
                | EVertexDataFormat::Rg8Unorm
                | EVertexDataFormat::Rg8Snorm
                | EVertexDataFormat::Rg16Unorm
                | EVertexDataFormat::Rg16Snorm
                | EVertexDataFormat::Rgba8Unorm
                | EVertexDataFormat::Rgba8Snorm
                | EVertexDataFormat::Rgb10a2Unorm
                | EVertexDataFormat::Rgba16Unorm
                | EVertexDataFormat::Rgba16Snorm
        )
    }
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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

#[binrw]
#[derive(Clone, Debug)]
pub struct SMeshLoadInformation {
    #[bw(try_calc = meshes.len().try_into())]
    pub mesh_count: u32,
    #[br(count = mesh_count)]
    pub meshes: Vec<CRenderMesh>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CRenderMesh {
    pub material_idx: u16,
    pub vtx_buf_idx: u8,
    pub idx_buf_idx: u8,
    pub index_start: u32,
    pub index_count: u32,
    pub unk_c: u16,
    pub unk_e: u16, // 64
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CVector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CAABox {
    pub min: CVector3f,
    pub max: CVector3f,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SModelHeader {
    pub unk: u32,
    pub bounds: CAABox,
    // TODO
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct R16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct Rg16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub g: f16,
}

#[binrw]
#[derive(Debug, Copy, Clone)]
struct Rgba16F {
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub r: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub g: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub b: f16,
    #[br(map = f16::from_bits)]
    #[bw(map = |f| f.to_f32())]
    pub a: f16,
}

#[derive(Debug, Clone)]
struct VertexBufferAttribute {
    pub in_offset: u32,
    pub out_offset: u32,
    pub in_format: EVertexDataFormat,
    pub in_size: u32,
    pub out_format: EVertexDataFormat,
    pub component: EVertexComponent,
}

#[derive(Debug, Clone, Default)]
struct VertexBufferInfo {
    pub vertex_count: u32,
    pub in_stride: u32,
    pub out_stride: u32,
    pub attributes: Vec<VertexBufferAttribute>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SMaterialChunk {
    pub unk: u32,
    #[bw(try_calc = materials.len().try_into())]
    pub material_count: u32,
    #[br(count = material_count)]
    pub materials: Vec<CMaterialCache>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CMaterialCache {
    #[br(try_map = CStringFixedName::into_string)]
    #[bw(map = CStringFixedName::from_string)]
    pub name: String,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub shader_id: Uuid,
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub unk_guid: Uuid,
    pub unk1: u32,
    pub unk2: u32,
    #[bw(try_calc = types.len().try_into())]
    pub type_count: u32,
    #[br(count = type_count)]
    pub types: Vec<FourCC>,
    #[bw(try_calc = render_types.len().try_into())]
    pub render_type_count: u32,
    #[br(count = render_type_count)]
    pub render_types: Vec<SMaterialRenderTypes>,
    #[bw(try_calc = data_types.len().try_into())]
    pub data_count: u32,
    #[br(count = data_count)]
    pub data_types: Vec<SMaterialType>,
    #[br(count = data_count)]
    pub data: Vec<CMaterialData>,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct CStringFixedName {
    #[bw(try_calc = text.len().try_into())]
    pub size: u32,
    #[br(count = size)]
    pub text: Vec<u8>,
}

impl CStringFixedName {
    fn from_string(str: &String) -> Self {
        #[allow(clippy::needless_update)]
        Self { text: str.as_bytes().to_vec(), ..Default::default() }
    }

    fn into_string(self) -> Result<String, FromUtf8Error> { String::from_utf8(self.text) }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SMaterialRenderTypes {
    pub data_id: FourCC,
    pub data_type: FourCC,
    pub flag1: u8,
    pub flag2: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SMaterialType {
    pub data_id: EMaterialDataId,
    pub data_type: EMaterialDataType,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CMaterialData {
    pub data_id: EMaterialDataId,
    pub data_type: EMaterialDataType,
    #[br(args { id: data_id, ty: data_type })]
    pub data: CMaterialDataInner,
}

#[binrw]
#[br(import { id: EMaterialDataId, ty: EMaterialDataType })]
#[derive(Clone, Debug)]
pub enum CMaterialDataInner {
    #[br(pre_assert(ty == EMaterialDataType::Texture))]
    Texture(CMaterialTextureTokenData),
    #[br(pre_assert(ty == EMaterialDataType::Color))]
    Color(CColor4f),
    #[br(pre_assert(ty == EMaterialDataType::Scalar))]
    Scalar(f32),
    #[br(pre_assert(ty == EMaterialDataType::Int1))]
    Int1(i32),
    #[br(pre_assert(ty == EMaterialDataType::Int4))]
    Int4(CVector4i),
    #[br(pre_assert(ty == EMaterialDataType::Mat4))]
    Mat4(CMatrix4f),
    #[br(pre_assert(ty == EMaterialDataType::Complex && id.is_texture_layered()))]
    LayeredTexture(CLayeredTextureData),
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CMaterialTextureTokenData {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub id: Uuid,
    #[br(if(!id.is_nil()))]
    pub usage: Option<STextureUsageInfo>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CLayeredTextureBaseData {
    unk: u32,
    colors: [CColor4f; 3],
    flags: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CLayeredTextureData {
    base: CLayeredTextureBaseData,
    textures: [CMaterialTextureTokenData; 3],
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureUsageInfo {
    pub flags: u32,
    pub filter: u32,
    pub wrap_x: u32,
    pub wrap_y: u32,
    pub wrap_z: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CColor4f {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CVector4i {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CMatrix4f {
    pub m: [f32; 16],
}

#[binrw]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EMaterialDataType {
    #[brw(magic(b"TXTR"))]
    Texture = 1,
    #[brw(magic(b"COLR"))]
    Color = 2,
    #[brw(magic(b"SCLR"))]
    Scalar = 3,
    #[brw(magic(b"INT1"))]
    Int1 = 4,
    #[brw(magic(b"CPLX"))]
    Complex = 5,
    #[brw(magic(b"INT4"))]
    Int4 = 6,
    #[brw(magic(b"MAT4"))]
    Mat4 = 7,
}

#[binrw]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum EMaterialDataId {
    // TXTR data IDs
    #[brw(magic(b"CBUF"))]
    CBUF = 1,
    #[brw(magic(b"ZBUF"))]
    ZBUF = 2,
    #[brw(magic(b"GBUF"))]
    GBUF = 3,
    #[brw(magic(b"GFLG"))]
    GFLG = 4,
    #[brw(magic(b"OPCT"))]
    OPCT = 5,
    #[brw(magic(b"DIFT"))]
    DIFT = 6,
    #[brw(magic(b"ICAN"))]
    ICAN = 7,
    #[brw(magic(b"SINC"))]
    SINC = 8,
    #[brw(magic(b"NMAP"))]
    NMAP = 9,
    #[brw(magic(b"MNMP"))]
    MNMP = 10,
    #[brw(magic(b"REFL"))]
    REFL = 11,
    #[brw(magic(b"REFS"))]
    REFS = 12,
    #[brw(magic(b"REFV"))]
    REFV = 13,
    #[brw(magic(b"SPCT"))]
    SPCT = 14,
    #[brw(magic(b"LIBD"))]
    LIBD = 15,
    #[brw(magic(b"LIBS"))]
    LIBS = 16,
    #[brw(magic(b"FOGR"))]
    FOGR = 17,
    #[brw(magic(b"INDI"))]
    INDI = 18,
    #[brw(magic(b"OTMP"))]
    OTMP = 19,
    #[brw(magic(b"CGMP"))]
    CGMP = 20,
    #[brw(magic(b"OGMP"))]
    OGMP = 21,
    #[brw(magic(b"VAND"))]
    VAND = 22,
    #[brw(magic(b"BLAT"))]
    BLAT = 23,
    #[brw(magic(b"BCLR"))]
    BCLR = 24,
    #[brw(magic(b"METL"))]
    METL = 25,
    #[brw(magic(b"TCH0"))]
    TCH0 = 26,
    #[brw(magic(b"TCH1"))]
    TCH1 = 27,
    #[brw(magic(b"TCH2"))]
    TCH2 = 28,
    #[brw(magic(b"TCH3"))]
    TCH3 = 29,
    #[brw(magic(b"TCH4"))]
    TCH4 = 30,
    #[brw(magic(b"TCH5"))]
    TCH5 = 31,
    // COLR data IDs
    #[brw(magic(b"DIFC"))]
    DIFC = 32,
    #[brw(magic(b"SHRC"))]
    SHRC = 33,
    #[brw(magic(b"SPCC"))]
    SPCC = 34,
    #[brw(magic(b"ICNC"))]
    ICNC = 35,
    #[brw(magic(b"ICMC"))]
    ICMC = 36,
    #[brw(magic(b"ODAT"))]
    ODAT = 37,
    #[brw(magic(b"MDCI"))]
    MDCI = 38,
    #[brw(magic(b"MDOI"))]
    MDOI = 39,
    #[brw(magic(b"LODC"))]
    LODC = 40,
    #[brw(magic(b"LODP"))]
    LODP = 41,
    #[brw(magic(b"VANP"))]
    VANP = 42,
    #[brw(magic(b"BLAL"))]
    BLAL = 43,
    #[brw(magic(b"BLCM"))]
    BLCM = 44,
    #[brw(magic(b"INDP"))]
    INDP = 45,
    #[brw(magic(b"PVLO"))]
    PVLO = 46,
    #[brw(magic(b"PSXT"))]
    PSXT = 47,
    #[brw(magic(b"PTAI"))]
    PTAI = 48,
    #[brw(magic(b"PCMD"))]
    PCMD = 49,
    #[brw(magic(b"BSAO"))]
    BSAO = 50,
    #[brw(magic(b"CCH0"))]
    CCH0 = 51,
    #[brw(magic(b"CCH1"))]
    CCH1 = 52,
    #[brw(magic(b"CCH2"))]
    CCH2 = 53,
    #[brw(magic(b"CCH3"))]
    CCH3 = 54,
    #[brw(magic(b"CCH4"))]
    CCH4 = 55,
    #[brw(magic(b"CCH5"))]
    CCH5 = 56,
    #[brw(magic(b"CCH6"))]
    CCH6 = 57,
    #[brw(magic(b"BKLT"))]
    BKLT = 58,
    #[brw(magic(b"BKLB"))]
    BKLB = 59,
    #[brw(magic(b"BKLA"))]
    BKLA = 60,
    #[brw(magic(b"BKGL"))]
    BKGL = 61,
    #[brw(magic(b"DYIN"))]
    DYIN = 62,
    #[brw(magic(b"CLP0"))]
    CLP0 = 63,
    #[brw(magic(b"HOTP"))]
    HOTP = 64,
    // INT1 data IDs
    #[brw(magic(b"SHID"))]
    SHID = 65,
    #[brw(magic(b"GBFF"))]
    GBFF = 66,
    #[brw(magic(b"PMOD"))]
    PMOD = 67,
    #[brw(magic(b"PFLG"))]
    PFLG = 68,
    #[brw(magic(b"BLPI"))]
    BLPI = 69,
    #[brw(magic(b"ICH0"))]
    ICH0 = 70,
    #[brw(magic(b"ICH1"))]
    ICH1 = 71,
    #[brw(magic(b"ICH2"))]
    ICH2 = 72,
    // INT4 data IDs
    #[brw(magic(b"AUVI"))]
    AUVI = 73,
    #[brw(magic(b"ECH0"))]
    ECH0 = 74,
    // SCLR data IDs
    #[brw(magic(b"OPCS"))]
    OPCS = 75,
    #[brw(magic(b"SPCP"))]
    SPCP = 76,
    #[brw(magic(b"INDS"))]
    INDS = 77,
    #[brw(magic(b"BLSM"))]
    BLSM = 78,
    #[brw(magic(b"LITS"))]
    LITS = 79,
    #[brw(magic(b"MDOE"))]
    MDOE = 80,
    #[brw(magic(b"VANF"))]
    VANF = 81,
    #[brw(magic(b"OTHS"))]
    OTHS = 82,
    #[brw(magic(b"PZSO"))]
    PZSO = 83,
    #[brw(magic(b"RCH0"))]
    RCH0 = 84,
    #[brw(magic(b"RCH1"))]
    RCH1 = 85,
    #[brw(magic(b"RCH2"))]
    RCH2 = 86,
    // MAT4 data IDs
    #[brw(magic(b"PXFM"))]
    PXFM = 87,
    #[brw(magic(b"MCH0"))]
    MCH0 = 88,
    // CPLX data IDs
    #[brw(magic(b"BCRL"))]
    BCRL = 89, // texture_layered
    #[brw(magic(b"MTLL"))]
    MTLL = 90, // texture_layered
    #[brw(magic(b"NRML"))]
    NRML = 91, // texture_layered
    #[brw(magic(b"SHDD"))]
    SHDD = 92,
    #[brw(magic(b"SKIN"))]
    SKIN = 93,
    #[brw(magic(b"DIMD"))]
    DIMD = 94,
    #[brw(magic(b"LIT "))]
    LIT = 95,
    #[brw(magic(b"ALLD"))]
    ALLD = 96,
    #[brw(magic(b"DLLD"))]
    DLLD = 97,
    #[brw(magic(b"CLLD"))]
    CLLD = 98,
    #[brw(magic(b"AUXF"))]
    AUXF = 99,
    #[brw(magic(b"WIND"))]
    WIND = 100,
    #[brw(magic(b"WATR"))]
    WATR = 101,
    #[brw(magic(b"DFXS"))]
    DFXS = 102,
    #[brw(magic(b"DFXN"))]
    DFXN = 103,
    #[brw(magic(b"MCDD"))]
    MCDD = 104,
    #[brw(magic(b"CAUS"))]
    CAUS = 105,
    #[brw(magic(b"BLPD"))]
    BLPD = 106,
    #[brw(magic(b"BLPT"))]
    BLPT = 107,
    #[brw(magic(b"FOGS"))]
    FOGS = 108,
    #[brw(magic(b"VOLF"))]
    VOLF = 109,
    #[brw(magic(b"VFXB"))]
    VFXB = 110,
    #[brw(magic(b"VFXD"))]
    VFXD = 111,
    #[brw(magic(b"REFP"))]
    REFP = 112,
    #[brw(magic(b"RAIN"))]
    RAIN = 113,
    #[brw(magic(b"XCH0"))]
    XCH0 = 114,
    #[brw(magic(b"XCH1"))]
    XCH1 = 115,
}

impl EMaterialDataId {
    pub fn is_texture_layered(self) -> bool {
        matches!(self, EMaterialDataId::BCRL | EMaterialDataId::MTLL | EMaterialDataId::NRML)
    }
}

#[binrw]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum EMaterialFlag {
    #[brw(magic(b"MFTR"))]
    MFTR = 0,
    #[brw(magic(b"MFMT"))]
    MFMT = 1,
    #[brw(magic(b"MFSR"))]
    MFSR = 2,
    #[brw(magic(b"MFSK"))]
    MFSK = 3,
    #[brw(magic(b"MFVC"))]
    MFVC = 4,
    #[brw(magic(b"MF1B"))]
    MF1B = 5,
    #[brw(magic(b"MFAV"))]
    MFAV = 6,
    #[brw(magic(b"MFIN"))]
    MFIN = 7,
    #[brw(magic(b"MFCA"))]
    MFCA = 8,
    #[brw(magic(b"MFIM"))]
    MFIM = 9,
    #[brw(magic(b"MTSM"))]
    MTSM = 10,
    #[brw(magic(b"MFRL"))]
    MFRL = 11,
    #[brw(magic(b"MFOE"))]
    MFOE = 12,
    #[brw(magic(b"MFOT"))]
    MFOT = 13,
    #[brw(magic(b"MFCI"))]
    MFCI = 14,
    #[brw(magic(b"MFOI"))]
    MFOI = 15,
    #[brw(magic(b"MFVA"))]
    MFVA = 16,
    #[brw(magic(b"MFSU"))]
    MFSU = 17,
    #[brw(magic(b"MFBP"))]
    MFBP = 18,
    #[brw(magic(b"MFBL"))]
    MFBL = 19,
    #[brw(magic(b"MFLB"))]
    MFLB = 20,
    #[brw(magic(b"MF1E"))]
    MF1E = 21,
    #[brw(magic(b"MFC0"))]
    MFC0 = 22,
    #[brw(magic(b"MFC1"))]
    MFC1 = 23,
    #[brw(magic(b"MFC2"))]
    MFC2 = 24,
    #[brw(magic(b"MFC3"))]
    MFC3 = 25,
    #[brw(magic(b"MFC4"))]
    MFC4 = 26,
}

fn decompress_gpu_buffers<'a>(
    data: &'a [u8],
    read_info: &[SModelReadBufferInfo],
    buffer_info: &[SModelBufferInfo],
) -> Result<Vec<Cow<'a, [u8]>>> {
    let mut out = Vec::with_capacity(buffer_info.len());
    for info in buffer_info {
        let read_info = &read_info[info.read_index as usize];
        let read_buffer =
            &data[read_info.offset as usize..(read_info.offset + read_info.size) as usize];
        let comp_buf = &read_buffer[info.offset as usize..(info.offset + info.size) as usize];
        let (_, buf) = decompress_buffer(comp_buf, info.dest_size as u64)?;
        out.push(buf);
    }
    Ok(out)
}

fn convert(args: ConvertArgs) -> Result<()> {
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

    let mut vtx_buffers = decompress_gpu_buffers(&data, &meta.read_info, &meta.vtx_buffer_info)?;
    let idx_buffers = decompress_gpu_buffers(&data, &meta.read_info, &meta.idx_buffer_info)?;

    let mut head: Option<SModelHeader> = None;
    let mut mtrl: Option<SMaterialChunk> = None;
    let mut mesh: Option<SMeshLoadInformation> = None;
    let mut vbuf: Option<SVertexBufferInfoSection> = None;
    let mut ibuf: Option<SIndexBufferInfoSection> = None;
    while !cmdl_data.is_empty() {
        let (chunk_desc, chunk_data, remain) = ChunkDescriptor::slice(cmdl_data, Endian::Little)?;
        match chunk_desc.id {
            K_CHUNK_HEAD => head = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            K_CHUNK_MTRL => mtrl = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            K_CHUNK_MESH => mesh = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            K_CHUNK_VBUF => vbuf = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            K_CHUNK_IBUF => ibuf = Some(Cursor::new(chunk_data).read_type(Endian::Little)?),
            // GPU data decompressed via META
            K_CHUNK_GPU => {}
            id => bail!("Unknown model chunk ID {id:?}"),
        }
        cmdl_data = remain;
    }

    let Some(head) = head else { bail!("Failed to locate HEAD") };
    let Some(mtrl) = mtrl else { bail!("Failed to locate MTRL") };
    let Some(mesh) = mesh else { bail!("Failed to locate MESH") };
    let Some(vbuf) = vbuf else { bail!("Failed to locate VBUF") };
    let Some(ibuf) = ibuf else { bail!("Failed to locate IBUF") };

    // log::debug!("HEAD: {head:#?}");
    // log::debug!("MTRL: {mtrl:#?}");
    // log::debug!("MESH: {mesh:#?}");
    // log::debug!("VBUF: {vbuf:#?}");
    // log::debug!("IBUF: {ibuf:#?}");

    // Build buffer to component index
    let mut buf_infos: Vec<VertexBufferInfo> = Vec::with_capacity(vtx_buffers.len());
    for info in &vbuf.info {
        let num_buffers = info.unk as usize; // guess
        let mut infos =
            vec![
                VertexBufferInfo { vertex_count: info.vertex_count, ..Default::default() };
                num_buffers
            ];
        for component in &info.components {
            let out = &mut infos[component.buffer_index as usize];
            match out.in_stride {
                0 => out.in_stride = component.stride,
                stride if stride != component.stride => {
                    bail!("Mismatched strides: {} != {}", component.stride, stride);
                }
                _ => {}
            }
            out.attributes.push(VertexBufferAttribute {
                in_offset: component.offset,
                out_offset: 0,
                in_format: component.format,
                in_size: component.format.byte_size(),
                out_format: EVertexDataFormat::Unknown,
                component: component.component,
            });
        }
        buf_infos.append(&mut infos);
    }

    // Calculate out strides & offsets
    for info in &mut buf_infos {
        info.attributes.sort_by_key(|c| c.in_offset);
        let mut out_stride = 0u32;
        for attribute in &mut info.attributes {
            attribute.out_offset = out_stride;
            attribute.out_format = match attribute.in_format {
                // Translate f16 to f32 in output
                EVertexDataFormat::R16Float => EVertexDataFormat::R32Float,
                EVertexDataFormat::Rg16Float => EVertexDataFormat::Rg32Float,
                EVertexDataFormat::Rgba16Float => EVertexDataFormat::Rgba32Float,
                format => format,
            };
            out_stride += attribute.out_format.byte_size();
        }
        info.out_stride = out_stride;
    }

    // Rebuild vertex buffers if necessary
    for (buf, info) in vtx_buffers.iter_mut().zip(&buf_infos) {
        // Sanity check buffer size
        ensure!(buf.len() == info.vertex_count as usize * info.in_stride as usize);
        if info.in_stride == info.out_stride {
            // No rebuild necessary
            continue;
        }

        let mut reader = Cursor::new(buf.as_ref());
        let mut new_buf: Vec<u8> =
            Vec::with_capacity(info.vertex_count as usize * info.out_stride as usize);
        let mut tmp_buf = vec![0u8; 16]; // max size of attribute
        for _ in 0..info.vertex_count as usize {
            for attribute in &info.attributes {
                if attribute.in_format == attribute.out_format {
                    let tmp = &mut tmp_buf[0..attribute.in_size as usize];
                    reader.read_exact(tmp)?;
                    new_buf.write_all(tmp)?;
                } else {
                    match (attribute.in_format, attribute.out_format) {
                        (EVertexDataFormat::R16Float, EVertexDataFormat::R32Float) => {
                            let tmp: R16F = reader.read_type(Endian::Little)?;
                            Cursor::new(&mut tmp_buf).write_type(&tmp, Endian::Little)?;
                            new_buf.write_all(&tmp_buf[0..4])?;
                        }
                        (EVertexDataFormat::Rg16Float, EVertexDataFormat::Rg32Float) => {
                            let tmp: Rg16F = reader.read_type(Endian::Little)?;
                            Cursor::new(&mut tmp_buf).write_type(&tmp, Endian::Little)?;
                            new_buf.write_all(&tmp_buf[0..8])?;
                        }
                        (EVertexDataFormat::Rgba16Float, EVertexDataFormat::Rgba32Float) => {
                            let tmp: Rgba16F = reader.read_type(Endian::Little)?;
                            Cursor::new(&mut tmp_buf).write_type(&tmp, Endian::Little)?;
                            new_buf.write_all(&tmp_buf[0..16])?;
                        }
                        (in_format, out_format) => {
                            todo!("Convertion from {in_format:?} => {out_format:?}")
                        }
                    }
                }
            }
        }
        *buf = Cow::Owned(new_buf);
    }

    DirBuilder::new().recursive(true).create(&args.out_dir)?;
    let mut json_buffers = Vec::with_capacity(vtx_buffers.len() + idx_buffers.len());
    for (idx, buf) in vtx_buffers.iter().enumerate() {
        let file_name = format!("vtxbuf{idx}.bin");
        fs::write(args.out_dir.join(&file_name), buf)?;
        json_buffers.push(json::Buffer {
            byte_length: buf.len() as u32,
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            uri: Some(file_name),
        });
    }
    for (idx, buf) in idx_buffers.iter().enumerate() {
        let file_name = format!("idxbuf{idx}.bin");
        fs::write(args.out_dir.join(&file_name), buf)?;
        json_buffers.push(json::Buffer {
            byte_length: buf.len() as u32,
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            uri: Some(file_name),
        });
    }

    let mut cur_buf = 0usize;
    let mut json_buffer_views = Vec::new();
    let mut json_accessors = Vec::new();
    let mut json_attributes: Vec<
        HashMap<json::validation::Checked<json::mesh::Semantic>, json::Index<json::Accessor>>,
    > = Vec::new();
    for buf_info in &vbuf.info {
        let num_buffers = buf_info.unk as usize; // guess?
        let mut attribute_map = HashMap::new();
        for idx in 0..num_buffers {
            let target_vtx_buf = cur_buf + idx;
            let info = &buf_infos[target_vtx_buf];
            json_buffer_views.push(json::buffer::View {
                buffer: json::Index::new(target_vtx_buf as u32),
                byte_length: json_buffers[target_vtx_buf].byte_length,
                byte_offset: None,
                byte_stride: Some(info.out_stride),
                extensions: Default::default(),
                extras: Default::default(),
                name: Some(format!("Vertex buffer view {target_vtx_buf}")),
                target: Some(Valid(json::buffer::Target::ArrayBuffer)),
            });
            for attribute in &info.attributes {
                let accessor = json::Accessor {
                    buffer_view: Some(json::Index::new(target_vtx_buf as u32)),
                    byte_offset: attribute.out_offset,
                    count: info.vertex_count,
                    component_type: Valid(json::accessor::GenericComponentType(
                        match attribute.out_format {
                            EVertexDataFormat::R8Unorm
                            | EVertexDataFormat::R8Uint
                            | EVertexDataFormat::Rg8Unorm
                            | EVertexDataFormat::Rg8Uint
                            | EVertexDataFormat::Rgba8Unorm
                            | EVertexDataFormat::Rgba8Uint => json::accessor::ComponentType::U8,
                            EVertexDataFormat::R8Snorm
                            | EVertexDataFormat::R8Sint
                            | EVertexDataFormat::Rg8Snorm
                            | EVertexDataFormat::Rg8Sint
                            | EVertexDataFormat::Rgba8Snorm
                            | EVertexDataFormat::Rgba8Sint => json::accessor::ComponentType::I8,
                            EVertexDataFormat::R16Unorm
                            | EVertexDataFormat::R16Uint
                            | EVertexDataFormat::Rg16Unorm
                            | EVertexDataFormat::Rg16Uint
                            | EVertexDataFormat::Rgba16Unorm
                            | EVertexDataFormat::Rgba16Uint => json::accessor::ComponentType::U16,
                            EVertexDataFormat::R16Snorm
                            | EVertexDataFormat::R16Sint
                            | EVertexDataFormat::Rg16Snorm
                            | EVertexDataFormat::Rg16Sint
                            | EVertexDataFormat::Rgba16Snorm
                            | EVertexDataFormat::Rgba16Sint => json::accessor::ComponentType::I16,
                            EVertexDataFormat::R32Uint
                            | EVertexDataFormat::Rg32Uint
                            | EVertexDataFormat::Rgb32Uint
                            | EVertexDataFormat::Rgba32Uint => json::accessor::ComponentType::U32,
                            EVertexDataFormat::R32Float
                            | EVertexDataFormat::Rg32Float
                            | EVertexDataFormat::Rgb32Float
                            | EVertexDataFormat::Rgba32Float => json::accessor::ComponentType::F32,
                            format => todo!("Unsupported glTF component type {format:?}"),
                        },
                    )),
                    extensions: Default::default(),
                    extras: Default::default(),
                    type_: Valid(match attribute.out_format {
                        EVertexDataFormat::R8Unorm
                        | EVertexDataFormat::R8Uint
                        | EVertexDataFormat::R8Snorm
                        | EVertexDataFormat::R8Sint
                        | EVertexDataFormat::R16Unorm
                        | EVertexDataFormat::R16Uint
                        | EVertexDataFormat::R16Snorm
                        | EVertexDataFormat::R16Sint
                        | EVertexDataFormat::R32Uint
                        | EVertexDataFormat::R32Float => json::accessor::Type::Scalar,
                        EVertexDataFormat::Rg8Unorm
                        | EVertexDataFormat::Rg8Uint
                        | EVertexDataFormat::Rg8Snorm
                        | EVertexDataFormat::Rg8Sint
                        | EVertexDataFormat::Rg16Unorm
                        | EVertexDataFormat::Rg16Uint
                        | EVertexDataFormat::Rg16Snorm
                        | EVertexDataFormat::Rg16Sint
                        | EVertexDataFormat::Rg32Uint
                        | EVertexDataFormat::Rg32Float => json::accessor::Type::Vec2,
                        EVertexDataFormat::Rgb32Uint | EVertexDataFormat::Rgb32Float => {
                            json::accessor::Type::Vec3
                        }
                        EVertexDataFormat::Rgba8Unorm
                        | EVertexDataFormat::Rgba8Uint
                        | EVertexDataFormat::Rgba8Snorm
                        | EVertexDataFormat::Rgba8Sint
                        | EVertexDataFormat::Rgba16Unorm
                        | EVertexDataFormat::Rgba16Uint
                        | EVertexDataFormat::Rgba16Snorm
                        | EVertexDataFormat::Rgba16Sint
                        | EVertexDataFormat::Rgba32Uint
                        | EVertexDataFormat::Rgba32Float => match attribute.component {
                            EVertexComponent::TexCoord0
                            | EVertexComponent::TexCoord1
                            | EVertexComponent::TexCoord2
                            | EVertexComponent::TexCoord3 => json::accessor::Type::Vec2,
                            EVertexComponent::Position | EVertexComponent::Normal => {
                                json::accessor::Type::Vec3
                            }
                            _ => json::accessor::Type::Vec4,
                        },
                        format => todo!("Unsupported glTF accessor type {format:?}"),
                    }),
                    min: if attribute.component == EVertexComponent::Position {
                        Some(json::Value::Array(vec![
                            json!(head.bounds.min.x),
                            json!(head.bounds.min.y),
                            json!(head.bounds.min.z),
                        ]))
                    } else {
                        None
                    },
                    max: if attribute.component == EVertexComponent::Position {
                        Some(json::Value::Array(vec![
                            json!(head.bounds.max.x),
                            json!(head.bounds.max.y),
                            json!(head.bounds.max.z),
                        ]))
                    } else {
                        None
                    },
                    name: Some(format!(
                        "{:?} {:?} => {:?}",
                        attribute.component, attribute.in_format, attribute.out_format
                    )),
                    normalized: attribute.out_format.normalized(),
                    sparse: None,
                };
                let accessor_idx = json_accessors.len();
                json_accessors.push(accessor);
                let semantic = match attribute.component {
                    EVertexComponent::Position => json::mesh::Semantic::Positions,
                    EVertexComponent::Normal => json::mesh::Semantic::Normals,
                    EVertexComponent::Tangent0 => json::mesh::Semantic::Tangents,
                    EVertexComponent::Tangent1 => json::mesh::Semantic::Extras("TANGENT_1".into()),
                    EVertexComponent::Tangent2 => json::mesh::Semantic::Extras("TANGENT_2".into()),
                    EVertexComponent::TexCoord0 => json::mesh::Semantic::TexCoords(0),
                    EVertexComponent::TexCoord1 => json::mesh::Semantic::TexCoords(1),
                    EVertexComponent::TexCoord2 => json::mesh::Semantic::TexCoords(2),
                    EVertexComponent::TexCoord3 => json::mesh::Semantic::TexCoords(3),
                    // EVertexComponent::Color => json::mesh::Semantic::Colors(0),
                    EVertexComponent::BoneIndices => json::mesh::Semantic::Joints(0),
                    EVertexComponent::BoneWeights => json::mesh::Semantic::Weights(0),
                    EVertexComponent::BakedLightingCoord => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_COORD".into())
                    }
                    EVertexComponent::BakedLightingTangent => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_TANGENT".into())
                    }
                    EVertexComponent::VertInstanceParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceColor => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_COLOR".into())
                    }
                    EVertexComponent::VertTransform0 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_0".into())
                    }
                    EVertexComponent::VertTransform1 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_1".into())
                    }
                    EVertexComponent::VertTransform2 => {
                        json::mesh::Semantic::Extras("VERT_TRANSFORM_2".into())
                    }
                    EVertexComponent::CurrentPosition => {
                        json::mesh::Semantic::Extras("CURRENT_POSITION".into())
                    }
                    EVertexComponent::VertInstanceOpacityParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_OPACITY_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceColorIndexingParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_COLOR_INDEXING_PARAMS".into())
                    }
                    EVertexComponent::VertInstanceOpacityIndexingParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_OPACITY_INDEXING_PARAMS".into())
                    }
                    EVertexComponent::VertInstancePaintParams => {
                        json::mesh::Semantic::Extras("VERT_INSTANCE_PAINT_PARAMS".into())
                    }
                    EVertexComponent::BakedLightingLookup => {
                        json::mesh::Semantic::Extras("BAKED_LIGHTING_LOOKUP".into())
                    }
                    EVertexComponent::MaterialChoice0 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_0".into())
                    }
                    EVertexComponent::MaterialChoice1 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_1".into())
                    }
                    EVertexComponent::MaterialChoice2 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_2".into())
                    }
                    EVertexComponent::MaterialChoice3 => {
                        json::mesh::Semantic::Extras("MATERIAL_CHOICE_3".into())
                    }
                    _ => continue,
                };
                attribute_map.insert(Valid(semantic), json::Index::new(accessor_idx as u32));
            }
        }
        json_attributes.push(attribute_map);
        cur_buf += num_buffers;
    }

    for (idx, _) in ibuf.info.iter().enumerate() {
        let target_buf = cur_buf + idx;
        json_buffer_views.push(json::buffer::View {
            buffer: json::Index::new(target_buf as u32),
            byte_length: json_buffers[target_buf].byte_length,
            byte_offset: None,
            byte_stride: None,
            extensions: Default::default(),
            extras: Default::default(),
            name: Some(format!("Index buffer view {}", target_buf - cur_buf)),
            target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
        });
    }

    let mut json_samplers = Vec::new();
    let mut json_textures = Vec::new();
    let mut json_images = Vec::new();
    let mut texture_map: HashMap<Uuid, usize> = HashMap::new();
    fn add_texture(
        texture: &CMaterialTextureTokenData,
        map: &mut HashMap<Uuid, usize>,
        samplers: &mut Vec<json::texture::Sampler>,
        textures: &mut Vec<json::Texture>,
        images: &mut Vec<json::Image>,
    ) -> Result<json::texture::Info> {
        let Some(usage) = &texture.usage else { bail!("Texture without usage!") };
        let texture_idx = if let Some(&existing) = map.get(&texture.id) {
            existing
        } else {
            let texture_idx = textures.len();
            samplers.push(json::texture::Sampler {
                mag_filter: match usage.filter {
                    0 => Some(Valid(json::texture::MagFilter::Nearest)),
                    1 => Some(Valid(json::texture::MagFilter::Linear)),
                    u32::MAX => None,
                    filter => todo!("Filter {filter}"),
                },
                min_filter: match usage.filter {
                    0 => Some(Valid(json::texture::MinFilter::Nearest)),
                    1 => Some(Valid(json::texture::MinFilter::Linear)),
                    u32::MAX => None,
                    filter => todo!("Filter {filter}"),
                },
                name: Some(format!("{} sampler", texture.id)),
                wrap_s: Valid(match usage.wrap_x {
                    0 => json::texture::WrappingMode::ClampToEdge,
                    1 => json::texture::WrappingMode::Repeat,
                    2 => json::texture::WrappingMode::MirroredRepeat,
                    _ => todo!(),
                }),
                wrap_t: Valid(match usage.wrap_y {
                    0 => json::texture::WrappingMode::ClampToEdge,
                    1 => json::texture::WrappingMode::Repeat,
                    2 => json::texture::WrappingMode::MirroredRepeat,
                    _ => todo!(),
                }),
                extensions: None,
                extras: None,
            });
            textures.push(json::Texture {
                name: Some(format!("{}", texture.id)),
                sampler: Some(json::Index::new(texture_idx as u32)),
                source: json::Index::new(texture_idx as u32),
                extensions: None,
                extras: None,
            });
            images.push(json::Image {
                buffer_view: None,
                mime_type: None,
                name: Some(format!("{}", texture.id)),
                uri: Some(format!("{}.png", texture.id)),
                extensions: None,
                extras: None,
            });
            print!("{} ", texture.id);
            texture_idx
        };
        Ok(json::texture::Info {
            index: json::Index::new(texture_idx as u32),
            tex_coord: usage.flags, // TODO is this right?
            extensions: None,
            extras: None,
        })
    }

    println!("Texture IDs:");
    let mut json_materials = Vec::with_capacity(mtrl.materials.len());
    for mat in &mtrl.materials {
        let mut json_material = json::Material {
            alpha_cutoff: None,
            alpha_mode: Valid(json::material::AlphaMode::Opaque),
            double_sided: false,
            name: Some(mat.name.clone()),
            pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                base_color_factor: json::material::PbrBaseColorFactor([0.0, 0.0, 0.0, 0.0]),
                base_color_texture: None,
                metallic_factor: json::material::StrengthFactor(0.0),
                roughness_factor: json::material::StrengthFactor(0.0),
                metallic_roughness_texture: None,
                extensions: None,
                extras: None,
            },
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
            emissive_factor: Default::default(),
            extensions: None,
            extras: None,
        };
        for data in &mat.data {
            match data.data_id {
                EMaterialDataId::DIFT => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                            )?);
                    }
                    _ => bail!("Unsupported data type for DIFT"),
                },
                EMaterialDataId::DIFC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        json_material.pbr_metallic_roughness.base_color_factor =
                            json::material::PbrBaseColorFactor([
                                color.r, color.g, color.b, color.a,
                            ]);
                    }
                    _ => bail!("Unsupported data type for DIFC"),
                },
                EMaterialDataId::ICAN => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.emissive_texture = Some(add_texture(
                            texture,
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                        )?);
                    }
                    _ => bail!("Unsupported data type for ICAN"),
                },
                EMaterialDataId::ICNC => match &data.data {
                    CMaterialDataInner::Color(color) => {
                        json_material.emissive_factor =
                            json::material::EmissiveFactor([color.r, color.g, color.b]);
                    }
                    _ => bail!("Unsupported data type for ICNC"),
                },
                EMaterialDataId::NMAP => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        let info = add_texture(
                            texture,
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                        )?;
                        json_material.normal_texture = Some(json::material::NormalTexture {
                            index: info.index,
                            scale: 1.0,
                            tex_coord: info.tex_coord,
                            extensions: None,
                            extras: None,
                        });
                    }
                    _ => bail!("Unsupported data type for NMAP"),
                },
                EMaterialDataId::BCLR => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        // json_material.pbr_metallic_roughness.metallic_factor =
                        //     json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                            )?);
                    }
                    _ => bail!("Unsupported data type for BCLR"),
                },
                EMaterialDataId::METL => match &data.data {
                    CMaterialDataInner::Texture(texture) => {
                        json_material.pbr_metallic_roughness.metallic_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.roughness_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.metallic_roughness_texture =
                            Some(add_texture(
                                texture,
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                            )?);
                    }
                    _ => bail!("Unsupported data type for METL"),
                },
                // TODO support layered properly
                EMaterialDataId::BCRL => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        // json_material.pbr_metallic_roughness.metallic_factor =
                        //     json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.base_color_texture =
                            Some(add_texture(
                                &texture.textures[0],
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                            )?);
                    }
                    _ => bail!("Unsupported data type for BCLR"),
                },
                EMaterialDataId::MTLL => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        json_material.pbr_metallic_roughness.metallic_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.roughness_factor =
                            json::material::StrengthFactor(1.0); // metal
                        json_material.pbr_metallic_roughness.metallic_roughness_texture =
                            Some(add_texture(
                                &texture.textures[0],
                                &mut texture_map,
                                &mut json_samplers,
                                &mut json_textures,
                                &mut json_images,
                            )?);
                    }
                    _ => bail!("Unsupported data type for MTLL"),
                },
                EMaterialDataId::NRML => match &data.data {
                    CMaterialDataInner::LayeredTexture(texture) => {
                        let info = add_texture(
                            &texture.textures[0],
                            &mut texture_map,
                            &mut json_samplers,
                            &mut json_textures,
                            &mut json_images,
                        )?;
                        json_material.normal_texture = Some(json::material::NormalTexture {
                            index: info.index,
                            scale: 1.0,
                            tex_coord: info.tex_coord,
                            extensions: None,
                            extras: None,
                        });
                    }
                    _ => bail!("Unsupported data type for NRML"),
                },
                id => {
                    // log::debug!("Ignoring material data ID {id:?}");
                    continue;
                }
            }
        }
        json_materials.push(json_material);
    }
    println!();

    let mut json_meshes = Vec::with_capacity(mesh.meshes.len());
    for (mesh_idx, mesh) in mesh.meshes.iter().enumerate() {
        let index_type = ibuf.info[mesh.idx_buf_idx as usize];
        let index_buf_idx = cur_buf as u32 + mesh.idx_buf_idx as u32;
        let index_accessor_idx = json_accessors.len() as u32;
        json_accessors.push(json::Accessor {
            buffer_view: Some(json::Index::new(index_buf_idx)),
            byte_offset: mesh.index_start
                * match index_type {
                    EBufferType::U8 => 1,
                    EBufferType::U16 => 2,
                    EBufferType::U32 => 4,
                },
            count: mesh.index_count,
            component_type: Valid(json::accessor::GenericComponentType(match index_type {
                EBufferType::U8 => json::accessor::ComponentType::U8,
                EBufferType::U16 => json::accessor::ComponentType::U16,
                EBufferType::U32 => json::accessor::ComponentType::U32,
            })),
            extensions: None,
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Scalar),
            min: None,
            max: None,
            name: Some(format!("Mesh {mesh_idx} indices")),
            normalized: false,
            sparse: None,
        });
        json_meshes.push(json::Mesh {
            extensions: None,
            extras: Default::default(),
            name: Some(format!("Mesh {mesh_idx}")),
            primitives: vec![json::mesh::Primitive {
                attributes: json_attributes[mesh.vtx_buf_idx as usize].clone(),
                extensions: None,
                extras: Default::default(),
                indices: Some(json::Index::new(index_accessor_idx)),
                material: Some(json::Index::new(mesh.material_idx as u32)),
                mode: Default::default(),
                targets: None,
            }],
            weights: None,
        });
    }

    let mut json_scene_nodes = Vec::with_capacity(json_meshes.len());
    let mut json_nodes = Vec::with_capacity(json_meshes.len());
    for (idx, _) in json_meshes.iter().enumerate() {
        json_nodes.push(json::Node {
            camera: None,
            children: None,
            extensions: None,
            extras: None,
            matrix: None,
            mesh: Some(json::Index::new(idx as u32)),
            name: None,
            rotation: None,
            scale: None,
            translation: None,
            skin: None,
            weights: None,
        });
        json_scene_nodes.push(json::Index::new(idx as u32));
    }

    let json_root = json::Root {
        accessors: json_accessors,
        animations: vec![],
        asset: Default::default(),
        buffers: json_buffers,
        buffer_views: json_buffer_views,
        scene: Some(json::Index::new(0)),
        extensions: None,
        extras: Default::default(),
        extensions_used: vec![],
        extensions_required: vec![],
        cameras: vec![],
        images: json_images,
        materials: json_materials,
        meshes: json_meshes,
        nodes: json_nodes,
        samplers: json_samplers,
        scenes: vec![json::Scene {
            extensions: Default::default(),
            extras: Default::default(),
            name: Some("Scene".into()),
            nodes: json_scene_nodes,
        }],
        skins: vec![],
        textures: json_textures,
    };
    let writer = fs::File::create(args.out_dir.join("out.gltf")).expect("I/O error");
    json::serialize::to_writer_pretty(writer, &json_root).expect("Serialization error");

    Ok(())
}
