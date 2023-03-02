pub mod chunk;
pub mod cmdl;
pub mod foot;
pub mod mtrl;
pub mod pack;
pub mod rfrm;
pub mod txtr;

use std::{
    fmt::{Debug, Display, Formatter, Write},
    string::FromUtf8Error,
};

use binrw::binrw;

use crate::array_ref;

#[binrw]
#[derive(Copy, Clone, Eq, PartialEq, Hash, Default)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    #[inline]
    fn from_u32(value: u32) -> Self {
        Self([(value >> 24) as u8, (value >> 16) as u8, (value >> 8) as u8, value as u8])
    }

    #[inline]
    fn as_u32(&self) -> u32 {
        ((self.0[0] as u32) << 24)
            | ((self.0[1] as u32) << 16)
            | ((self.0[2] as u32) << 8)
            | (self.0[3] as u32)
    }
}

impl Display for FourCC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for c in self.0 {
            f.write_char(c as char)?;
        }
        Ok(())
    }
}

impl Debug for FourCC {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_char('"')?;
        for c in self.0 {
            f.write_char(c as char)?;
        }
        f.write_char('"')?;
        Ok(())
    }
}

impl PartialEq<[u8; 4]> for FourCC {
    fn eq(&self, other: &[u8; 4]) -> bool { &self.0 == other }
}

#[inline]
pub fn peek_four_cc(data: &[u8]) -> FourCC { FourCC(*array_ref!(data, 0, 4)) }

#[binrw]
#[derive(Clone, Debug)]
pub struct CVector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
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
#[derive(Clone, Debug)]
pub struct CAABox {
    pub min: CVector3f,
    pub max: CVector3f,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CTransform4f {
    m00: f32,
    m01: f32,
    m02: f32,
    m03: f32,
    m10: f32,
    m11: f32,
    m12: f32,
    m13: f32,
    m20: f32,
    m21: f32,
    m22: f32,
    m23: f32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct COBBox {
    xf: CTransform4f,
    extents: CVector3f,
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
