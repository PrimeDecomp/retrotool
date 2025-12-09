use std::borrow::Cow;

use anyhow::{bail, Result};

use crate::util::lzss;

pub fn decompress_buffer<'a>(
    compressed_data: &'a [u8],
    decompressed_size: u64,
) -> Result<(u32, Cow<'a, [u8]>)> {
    if compressed_data.len() < 4 {
        bail!("Invalid compressed data size: {}", compressed_data.len());
    }
    if compressed_data[0..4] == [0u8; 4] {
        // Shortcut for uncompressed data
        return Ok((0, Cow::Borrowed(&compressed_data[4..])));
    }
    let mut out = vec![0u8; decompressed_size as usize];
    let mode = decompress_into(compressed_data, &mut out)?;
    Ok((mode, Cow::Owned(out)))
}

pub fn decompress_into(compressed_data: &[u8], out: &mut [u8]) -> Result<u32> {
    if compressed_data.len() < 4 {
        bail!("Invalid compressed data size: {}", compressed_data.len());
    }
    let mode = u32::from_le_bytes(compressed_data[0..4].try_into().unwrap());
    let data = &compressed_data[4..];
    if !match mode {
        0 => {
            if data.len() == out.len() {
                out.copy_from_slice(data);
                true
            } else {
                false
            }
        }
        1 => lzss::decompress::<1>(data, out),
        2 => lzss::decompress::<2>(data, out),
        3 => lzss::decompress::<3>(data, out),
        12 => lzss::decompress_huffman::<1>(data, out),
        13 => lzss::decompress_huffman::<2>(data, out),
        14 => lzss::decompress_huffman::<3>(data, out),
        _ => bail!("Unsupported compression mode {}", mode),
    } {
        bail!("Decompression failed");
    }
    Ok(mode)
}
