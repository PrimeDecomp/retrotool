//! Material value types shared by model caches and standalone MATI resources.

use std::io::Cursor;

use anyhow::{ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use uuid::Uuid;

use crate::format::{CColor4f, CMatrix4f, CVector4i, FourCC};

#[binrw]
#[derive(Clone, Debug)]
/// CVariableDesc stream constructor 0x71001fccb8. Ten bytes on disk;
/// runtime compresses data_id to u16, making the in-memory record eight bytes.
pub struct CMaterialVariableDesc {
    pub name: FourCC,
    pub data_id: FourCC,
    /// External variable type byte, a different enum from EMaterialDataType.
    pub data_type: u8,
    /// 0xff is the default; other values select a destination vector channel.
    pub packing_rule: u8,
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
    Texture(#[br(args { always_read_usage: true })] CMaterialTextureTokenData),
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
#[br(import { always_read_usage: bool = false })]
#[derive(Clone, Debug)]
pub struct CMaterialTextureTokenData {
    #[br(map = Uuid::from_bytes_le)]
    #[bw(map = Uuid::to_bytes_le)]
    pub id: Uuid,
    // Layer slots omit usage for nil IDs; top-level TXTR values always store it.
    #[br(if(always_read_usage || !id.is_nil()))]
    pub usage: Option<STextureUsageInfo>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CLayeredTextureBaseData {
    pub blend_control: ELayeredTextureBlend,
    // SBlendRegs: shader blend coefficients, not three independent texture tints.
    pub colors: [CColor4f; 3],
    // Bits 0/1 request the one/two tangent stream shader variants, respectively.
    pub flags: u8,
}

/// CLayeredTextureBaseData::EBlendControl. See CFeatureTextureLayered::SelectOption
/// at 0x710016f8f8 in MPR; these select shader features, not fixed function blending.
#[binrw]
#[brw(repr(u32))]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ELayeredTextureBlend {
    Embedded = 0,
    Separate = 1,
    External = 2,
    Disabled = 3,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CLayeredTextureData {
    pub base: CLayeredTextureBaseData,
    // Texture0, Texture1, BlendTexture (CFeatureTextureLayered::ESemanticId).
    pub textures: [CMaterialTextureTokenData; 3],
}

#[binrw]
#[derive(Clone, Debug)]
pub struct STextureUsageInfo {
    pub tex_coord: u32,
    /// -1 inherits both filters; 0/1 overrides min, mag AND mip filtering.
    pub filter: i32,
    /// Wrap overrides: -1 inherits the corresponding TXTR header value.
    pub wrap_x: i32,
    pub wrap_y: i32,
    pub wrap_z: i32,
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
    /// Layer height scales/offsets: primary scale, primary offset, overlay scale, overlay offset.
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
    /// PhysicallyBased texture height-blend smoothness.
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
    /// Decode the four-byte name used in featured-material data ID lists.
    pub fn from_fourcc(name: &str) -> Result<Self> {
        ensure!(name.len() == 4, "Material data ID must contain four bytes");
        Ok(Cursor::new(name.as_bytes()).read_type(Endian::Little)?)
    }

    /// ID categories tested by the engine at 0x7100161cd0..0x7100161d40.
    /// These classify the ID itself, independently of its serialized value tag.
    pub fn data_type(self) -> EMaterialDataType {
        match self as u32 {
            1..=31 => EMaterialDataType::Texture,
            32..=64 => EMaterialDataType::Color,
            65..=72 => EMaterialDataType::Int1,
            73..=74 => EMaterialDataType::Int4,
            75..=86 => EMaterialDataType::Scalar,
            87..=88 => EMaterialDataType::Mat4,
            89..=115 => EMaterialDataType::Complex,
            _ => unreachable!(),
        }
    }

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
