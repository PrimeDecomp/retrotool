pub mod chunk;
pub mod cmdl;
pub mod foot;
pub mod mcon;
pub mod mtrl;
pub mod pack;
pub mod rfrm;
pub mod txtr;

use std::{
    fmt::{Debug, Display, Formatter, Write},
    marker::PhantomData,
    num::TryFromIntError,
    string::FromUtf8Error,
};

use binrw::{binrw, BinRead, BinWrite};

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
#[derive(Copy, Clone, Debug, Default)]
pub struct CVector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl CVector3f {
    #[inline]
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }

    #[inline]
    pub fn splat(xyz: f32) -> Self { Self { x: xyz, y: xyz, z: xyz } }

    #[inline]
    pub fn to_array(self) -> [f32; 3] { [self.x, self.y, self.z] }
}
impl From<[f32; 3]> for CVector3f {
    fn from(value: [f32; 3]) -> Self { Self { x: value[0], y: value[1], z: value[2] } }
}
impl From<CVector3f> for [f32; 3] {
    fn from(value: CVector3f) -> Self { value.to_array() }
}

impl From<CVector3f> for mint::Vector3<f32> {
    fn from(value: CVector3f) -> Self { Self::from([value.x, value.y, value.z]) }
}
impl From<mint::Vector3<f32>> for CVector3f {
    fn from(value: mint::Vector3<f32>) -> Self { Self { x: value.x, y: value.y, z: value.z } }
}
impl mint::IntoMint for CVector3f {
    type MintType = mint::Vector3<f32>;
}

#[binrw]
#[derive(Copy, Clone, Debug, Default)]
pub struct CVector4f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl CVector4f {
    #[inline]
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self { Self { x, y, z, w } }

    #[inline]
    pub fn splat(xyzw: f32) -> Self { Self { x: xyzw, y: xyzw, z: xyzw, w: xyzw } }

    #[inline]
    pub fn to_array(self) -> [f32; 4] { [self.x, self.y, self.z, self.w] }
}
impl From<[f32; 4]> for CVector4f {
    fn from(value: [f32; 4]) -> Self { Self { x: value[0], y: value[1], z: value[2], w: value[3] } }
}
impl From<CVector4f> for [f32; 4] {
    fn from(value: CVector4f) -> Self { value.to_array() }
}

impl From<CVector4f> for mint::Vector4<f32> {
    fn from(value: CVector4f) -> Self { Self::from([value.x, value.y, value.z, value.w]) }
}
impl From<mint::Vector4<f32>> for CVector4f {
    fn from(value: mint::Vector4<f32>) -> Self {
        Self { x: value.x, y: value.y, z: value.z, w: value.w }
    }
}
impl mint::IntoMint for CVector4f {
    type MintType = mint::Vector4<f32>;
}

#[binrw]
#[derive(Copy, Clone, Debug)]
pub struct CColor4f {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for CColor4f {
    fn default() -> Self { Self { r: 0.0, g: 0.0, b: 0.0, a: 1.0 } }
}
impl CColor4f {
    #[inline]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self { Self { r, g, b, a } }

    #[inline]
    pub fn splat(rgba: f32) -> Self { Self { r: rgba, g: rgba, b: rgba, a: rgba } }

    #[inline]
    pub fn to_array(self) -> [f32; 4] { [self.r, self.g, self.b, self.a] }
}
impl From<[f32; 4]> for CColor4f {
    fn from(value: [f32; 4]) -> Self { Self { r: value[0], g: value[1], b: value[2], a: value[3] } }
}
impl From<CColor4f> for [f32; 4] {
    fn from(value: CColor4f) -> Self { value.to_array() }
}

impl From<CColor4f> for mint::Vector4<f32> {
    fn from(value: CColor4f) -> Self { Self::from([value.r, value.g, value.b, value.a]) }
}
impl From<mint::Vector4<f32>> for CColor4f {
    fn from(value: mint::Vector4<f32>) -> Self {
        Self { r: value.x, g: value.y, b: value.z, a: value.w }
    }
}
impl mint::IntoMint for CColor4f {
    type MintType = mint::Vector4<f32>;
}

#[binrw]
#[derive(Copy, Clone, Debug, Default)]
pub struct CVector4i {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

impl CVector4i {
    #[inline]
    pub fn new(x: i32, y: i32, z: i32, w: i32) -> Self { Self { x, y, z, w } }

    #[inline]
    pub fn splat(xyzw: i32) -> Self { Self { x: xyzw, y: xyzw, z: xyzw, w: xyzw } }

    #[inline]
    pub fn to_array(self) -> [i32; 4] { [self.x, self.y, self.z, self.w] }
}
impl From<[i32; 4]> for CVector4i {
    fn from(value: [i32; 4]) -> Self { Self { x: value[0], y: value[1], z: value[2], w: value[3] } }
}
impl From<CVector4i> for [i32; 4] {
    fn from(value: CVector4i) -> Self { value.to_array() }
}

impl From<CVector4i> for mint::Vector4<i32> {
    fn from(value: CVector4i) -> Self { Self::from([value.x, value.y, value.z, value.w]) }
}
impl From<mint::Vector4<i32>> for CVector4i {
    fn from(value: mint::Vector4<i32>) -> Self {
        Self { x: value.x, y: value.y, z: value.z, w: value.w }
    }
}
impl mint::IntoMint for CVector4i {
    type MintType = mint::Vector4<i32>;
}

#[binrw]
#[derive(Copy, Clone, Debug)]
pub struct CMatrix4f {
    pub m: [f32; 16],
}

impl Default for CMatrix4f {
    fn default() -> Self {
        Self {
            #[rustfmt::skip]
            m: [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }
}
impl From<CMatrix4f> for mint::RowMatrix4<f32> {
    fn from(value: CMatrix4f) -> Self { Self::from(value.m) }
}
impl From<mint::RowMatrix4<f32>> for CMatrix4f {
    fn from(value: mint::RowMatrix4<f32>) -> Self { Self { m: value.into() } }
}
impl mint::IntoMint for CMatrix4f {
    type MintType = mint::RowMatrix4<f32>;
}

#[binrw]
#[derive(Copy, Clone, Debug)]
pub struct CAABox {
    pub min: CVector3f,
    pub max: CVector3f,
}

impl Default for CAABox {
    fn default() -> Self {
        Self { min: CVector3f::splat(f32::MAX), max: CVector3f::splat(f32::MIN) }
    }
}

#[binrw]
#[derive(Copy, Clone, Debug)]
pub struct CTransform4f {
    m0: CVector4f,
    m1: CVector4f,
    m2: CVector4f,
}

impl Default for CTransform4f {
    fn default() -> Self {
        Self {
            m0: CVector4f::new(1.0, 0.0, 0.0, 0.0),
            m1: CVector4f::new(0.0, 1.0, 0.0, 0.0),
            m2: CVector4f::new(0.0, 0.0, 1.0, 0.0),
        }
    }
}
impl CTransform4f {
    #[inline]
    pub fn translation(&self) -> CVector3f { CVector3f::new(self.m0.w, self.m1.w, self.m2.w) }
}
impl From<CTransform4f> for mint::RowMatrix3x4<f32> {
    fn from(value: CTransform4f) -> Self {
        Self::from([value.m0.into(), value.m1.into(), value.m2.into()])
    }
}
impl From<mint::RowMatrix3x4<f32>> for CTransform4f {
    fn from(value: mint::RowMatrix3x4<f32>) -> Self {
        Self { m0: value.x.into(), m1: value.y.into(), m2: value.z.into() }
    }
}
impl mint::IntoMint for CTransform4f {
    type MintType = mint::RowMatrix3x4<f32>;
}

impl From<CTransform4f> for mint::RowMatrix4<f32> {
    // noinspection DuplicatedCode
    fn from(value: CTransform4f) -> Self {
        Self::from([
            [value.m0.x, value.m0.y, value.m0.z, value.m0.w],
            [value.m1.x, value.m1.y, value.m1.z, value.m1.w],
            [value.m2.x, value.m2.y, value.m2.z, value.m2.w],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
}

impl From<CTransform4f> for mint::ColumnMatrix4<f32> {
    // noinspection DuplicatedCode
    fn from(value: CTransform4f) -> Self {
        Self::from([
            [value.m0.x, value.m1.x, value.m2.x, 0.0],
            [value.m0.y, value.m1.y, value.m2.y, 0.0],
            [value.m0.z, value.m1.z, value.m2.z, 0.0],
            [value.m0.w, value.m1.w, value.m2.w, 1.0],
        ])
    }
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

#[binrw]
#[derive(Clone, Debug, Default)]
struct TaggedVec<C, T>
where
    C: for<'a> BinRead<Args<'a> = ()>
        + for<'a> BinWrite<Args<'a> = ()>
        + Copy
        + TryFrom<usize, Error = TryFromIntError>
        + 'static,
    T: for<'a> BinRead<Args<'a> = ()> + for<'a> BinWrite<Args<'a> = ()> + 'static,
    usize: TryFrom<C, Error = TryFromIntError>,
{
    #[bw(try_calc(data.len().try_into()))]
    count: C,
    #[br(count(count))]
    data: Vec<T>,
    _marker: PhantomData<C>,
}

impl<C, T> TaggedVec<C, T>
where
    C: for<'a> BinRead<Args<'a> = ()>
        + for<'a> BinWrite<Args<'a> = ()>
        + Copy
        + Default
        + TryFrom<usize, Error = TryFromIntError>
        + 'static,
    T: for<'a> BinRead<Args<'a> = ()> + for<'a> BinWrite<Args<'a> = ()> + Default + 'static,
    usize: TryFrom<C, Error = TryFromIntError>,
{
    #[allow(dead_code)]
    fn new(inner: Vec<T>) -> Self {
        #[allow(clippy::needless_update)]
        Self { data: inner, ..Default::default() }
    }
}
