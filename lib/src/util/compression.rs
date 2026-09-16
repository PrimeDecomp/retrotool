use std::borrow::Cow;

use anyhow::{bail, ensure, Result};
use flate2::{Decompress, FlushDecompress, Status};

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
        _ => bail!("Unsupported compression mode {}", mode),
    } {
        bail!("Decompression failed");
    }
    Ok(mode)
}

/// Decode one complete zlib stream with exact input and output sizes.
/// Grows with actual output instead of allocating the untrusted declared size.
pub fn decompress_zlib_exact(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut decoder = Decompress::new(true);
    let mut decompressed = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let input_before = decoder.total_in();
        let output_before = decoder.total_out();
        let status = decoder.decompress(
            &compressed[input_before as usize..],
            &mut buffer,
            FlushDecompress::None,
        )?;
        ensure!(decoder.total_out() <= expected_size as u64, "output exceeds declared size");
        decompressed.extend_from_slice(&buffer[..(decoder.total_out() - output_before) as usize]);
        if status == Status::StreamEnd {
            break;
        }
        // The Read adapter can report EOF with a missing zlib trailer. Require
        // StreamEnd explicitly so incomplete streams cannot pass size checks.
        ensure!(
            decoder.total_in() != input_before || decoder.total_out() != output_before,
            "Truncated zlib stream"
        );
    }
    ensure!(decompressed.len() == expected_size, "decompressed size mismatch");
    ensure!(decoder.total_in() == compressed.len() as u64, "Trailing data in compressed range");
    Ok(decompressed)
}
