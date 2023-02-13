pub mod chunk;
pub mod pack;
pub mod rfrm;

use std::fmt::{Debug, Display, Formatter, Write};

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
