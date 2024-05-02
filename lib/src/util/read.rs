use std::{io, io::Read};

use zerocopy::{AsBytes, ByteOrder, FromBytes, FromZeroes};

#[inline(always)]
pub fn read_from<T, R>(reader: &mut R) -> io::Result<T>
where
    T: FromBytes + FromZeroes + AsBytes,
    R: Read + ?Sized,
{
    let mut ret = <T>::new_zeroed();
    reader.read_exact(ret.as_bytes_mut())?;
    Ok(ret)
}

#[inline(always)]
pub fn read_vec<T, R>(reader: &mut R, count: usize) -> io::Result<Vec<T>>
where
    T: FromBytes + FromZeroes + AsBytes,
    R: Read + ?Sized,
{
    let mut ret = <T>::new_vec_zeroed(count);
    reader.read_exact(ret.as_mut_slice().as_bytes_mut())?;
    Ok(ret)
}

#[inline(always)]
pub fn read_box<T, R>(reader: &mut R) -> io::Result<Box<T>>
where
    T: FromBytes + FromZeroes + AsBytes,
    R: Read + ?Sized,
{
    let mut ret = <T>::new_box_zeroed();
    reader.read_exact(ret.as_mut().as_bytes_mut())?;
    Ok(ret)
}

#[inline(always)]
pub fn read_box_slice<T, R>(reader: &mut R, count: usize) -> io::Result<Box<[T]>>
where
    T: FromBytes + FromZeroes + AsBytes,
    R: Read + ?Sized,
{
    let mut ret = <T>::new_box_slice_zeroed(count);
    reader.read_exact(ret.as_mut().as_bytes_mut())?;
    Ok(ret)
}

#[inline(always)]
pub fn read_u16<O, R>(reader: &mut R) -> io::Result<u16>
where
    O: ByteOrder,
    R: Read + ?Sized,
{
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(O::read_u16(&buf))
}

#[inline(always)]
pub fn read_u32<O, R>(reader: &mut R) -> io::Result<u32>
where
    O: ByteOrder,
    R: Read + ?Sized,
{
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(O::read_u32(&buf))
}

#[inline(always)]
pub fn read_u64<O, R>(reader: &mut R) -> io::Result<u64>
where
    O: ByteOrder,
    R: Read + ?Sized,
{
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(O::read_u64(&buf))
}
