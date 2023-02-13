pub mod chunk;
pub mod pack;
pub mod rfrm;

use std::fmt::{Debug, Display, Formatter, Write};

use binrw::binrw;

use crate::array_ref;

#[binrw]
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    #[inline]
    const fn swap(self) -> Self { Self([self.0[3], self.0[2], self.0[1], self.0[0]]) }
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
