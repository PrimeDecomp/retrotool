use std::io::Cursor;

use anyhow::{bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use uuid::Uuid;

use crate::{
    format::{
        chunk::ChunkDescriptor, rfrm::FormDescriptor, CAABox, CColor4f, CMatrix4f,
        CStringFixedName, CVector4i, FourCC,
    },
    util::compression::decompress_buffer,
};

// Cooked model
pub const K_FORM_CMDL: FourCC = FourCC(*b"CMDL");
pub const K_FORM_SMDL: FourCC = FourCC(*b"SMDL");
pub const K_FORM_WMDL: FourCC = FourCC(*b"WMDL");

// Model header
pub const K_CHUNK_HEAD: FourCC = FourCC(*b"HEAD");
// World header
pub const K_CHUNK_WDHD: FourCC = FourCC(*b"WDHD");
// Skinned header
pub const K_CHUNK_SKHD: FourCC = FourCC(*b"SKHD");
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
    pub vertex_count: u32,
    #[bw(try_calc = components.len().try_into())]
    pub component_count: u32,
    #[br(count = component_count)]
    pub components: Vec<SVertexDataComponent>,
    pub unk: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct SVertexDataComponent {
    pub buffer_index: u32,
    pub offset: u32,
    pub stride: u32,
    pub format: EVertexDataFormat,
    pub component: EVertexComponent,
}

#[binrw]
#[repr(u32)]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EVertexDataFormat {
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
    pub fn byte_size(self) -> u32 {
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

    pub fn normalized(self) -> bool {
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
pub enum EVertexComponent {
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
pub struct SModelHeader {
    pub unk: u32,
    pub bounds: CAABox,
    // TODO
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
    pub unk: u32,
    pub colors: [CColor4f; 3],
    pub flags: u8,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CLayeredTextureData {
    pub base: CLayeredTextureBaseData,
    pub textures: [CMaterialTextureTokenData; 3],
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

fn decompress_gpu_buffers(
    file_data: &[u8],
    read_info: &[SModelReadBufferInfo],
    buffer_info: &[SModelBufferInfo],
) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::with_capacity(buffer_info.len());
    for info in buffer_info {
        let read_info = &read_info[info.read_index as usize];
        let read_buffer =
            &file_data[read_info.offset as usize..(read_info.offset + read_info.size) as usize];
        let comp_buf = &read_buffer[info.offset as usize..(info.offset + info.size) as usize];
        let (_, buf) = decompress_buffer(comp_buf, info.dest_size as u64)?;
        out.push(buf.into_owned());
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ModelData {
    pub head: SModelHeader,
    pub mtrl: SMaterialChunk,
    pub mesh: SMeshLoadInformation,
    pub vbuf: SVertexBufferInfoSection,
    pub ibuf: SIndexBufferInfoSection,
    pub vtx_buffers: Vec<Vec<u8>>,
    pub idx_buffers: Vec<Vec<u8>>,
}

impl ModelData {
    pub fn slice(data: &[u8], meta: &[u8], e: Endian) -> Result<ModelData> {
        let (cmdl_desc, mut cmdl_data, _) = FormDescriptor::slice(data, Endian::Little)?;
        ensure!(cmdl_desc.id == K_FORM_CMDL || cmdl_desc.id == K_FORM_SMDL || cmdl_desc.id == K_FORM_WMDL);
        if cmdl_desc.id == K_FORM_CMDL {
            ensure!(cmdl_desc.reader_version == 114);
            ensure!(cmdl_desc.writer_version == 125);
        } else if cmdl_desc.id == K_FORM_SMDL {
            ensure!(cmdl_desc.reader_version == 127);
            ensure!(cmdl_desc.writer_version == 133);
        } else if cmdl_desc.id == K_FORM_WMDL {
            ensure!(cmdl_desc.reader_version == 118);
            ensure!(cmdl_desc.writer_version == 124);
        }


        let meta: SModelMetaData = Cursor::new(meta).read_type(e)?;
        let vtx_buffers = decompress_gpu_buffers(data, &meta.read_info, &meta.vtx_buffer_info)?;
        let idx_buffers = decompress_gpu_buffers(data, &meta.read_info, &meta.idx_buffer_info)?;

        let mut head: Option<SModelHeader> = None;
        let mut mtrl: Option<SMaterialChunk> = None;
        let mut mesh: Option<SMeshLoadInformation> = None;
        let mut vbuf: Option<SVertexBufferInfoSection> = None;
        let mut ibuf: Option<SIndexBufferInfoSection> = None;
        while !cmdl_data.is_empty() {
            let (chunk_desc, chunk_data, remain) = ChunkDescriptor::slice(cmdl_data, e)?;
            match chunk_desc.id {
                K_CHUNK_WDHD => head = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_SKHD => head = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_HEAD => head = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_MTRL => mtrl = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_MESH => mesh = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_VBUF => vbuf = Some(Cursor::new(chunk_data).read_type(e)?),
                K_CHUNK_IBUF => ibuf = Some(Cursor::new(chunk_data).read_type(e)?),
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

        Ok(ModelData { head, mtrl, mesh, vbuf, ibuf, vtx_buffers, idx_buffers })
    }
}
