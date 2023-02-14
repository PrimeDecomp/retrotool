use std::borrow::Cow;

use anyhow::{bail, Result};

/// https://wiki.axiodl.com/w/LZSS_Compression
pub fn decompress<const M: u8>(mut input: &[u8], output: &mut [u8]) -> bool {
    let group_len = 2usize.pow(M as u32 - 1);
    let mut out_cur = 0usize;

    let mut header_byte = 0u8;
    let mut group = 0u8;
    while !input.is_empty() {
        if group == 0 {
            header_byte = input[0];
            input = &input[1..];
            group = 8;
        }

        if header_byte & 0x80 == 0 {
            output[out_cur..group_len + out_cur].copy_from_slice(&input[..group_len]);
            input = &input[group_len..];
            out_cur += group_len;
        } else {
            let count = (input[0] as usize >> 4) + (4 - M as usize);
            let length = (((input[0] as usize & 0xF) << 0x8) | input[1] as usize) << (M - 1);
            input = &input[2..];

            let seek = out_cur - length;
            for n in 0..count * group_len {
                output[out_cur + n] = output[seek + n];
            }
            out_cur += count * group_len;
        }

        header_byte <<= 1;
        group -= 1;
    }

    out_cur == output.len()
}

pub fn decompress_buffer(
    compressed_data: &[u8],
    decompressed_size: u64,
) -> Result<(u32, Cow<[u8]>)> {
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
        1 => decompress::<1>(data, out),
        2 => decompress::<2>(data, out),
        3 => decompress::<3>(data, out),
        _ => bail!("Unsupported compression mode {}", mode),
    } {
        bail!("Decompression failed");
    }
    Ok(mode)
}
