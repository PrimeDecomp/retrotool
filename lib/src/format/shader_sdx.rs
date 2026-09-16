//! MPR SDX source envelope, established from the startup MTRL v22 archive.
//! The Switch executable retains the common source reader, but no D3D loader
//! was identified. The byte between the two lengths remains unresolved.

use anyhow::{ensure, Context, Result};

use crate::{format::shader_dxbc::Dxbc, util::compression::decompress_zlib_exact};

#[derive(Debug)]
pub struct SdxShader {
    pub unknown: u8,
    /// Complete DXBC container, including its original checksum and chunks.
    pub dxbc: Vec<u8>,
}

impl SdxShader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let header = data.get(..9).context("Truncated SDX header")?;
        let size = u32::from_le_bytes(header[..4].try_into()?) as usize;
        let compressed_size = u32::from_le_bytes(header[5..9].try_into()?) as usize;
        ensure!(data.len() - 9 == compressed_size, "SDX compressed size mismatch");
        let dxbc = decompress_zlib_exact(&data[9..], size)?;
        Dxbc::parse(&dxbc).context("Invalid SDX DXBC container")?;
        Ok(Self { unknown: header[4], dxbc })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::ZlibEncoder, Compression};

    use super::*;

    #[test]
    fn exact_compressed_envelope_and_unaligned_header() {
        let mut dxbc = b"DXBC".to_vec();
        dxbc.extend([0; 16]);
        for v in [1u32, 32, 0] {
            dxbc.extend(v.to_le_bytes());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&dxbc).unwrap();
        let zlib = encoder.finish().unwrap();
        let mut data = (dxbc.len() as u32).to_le_bytes().to_vec();
        data.push(7); // Preserve the unknown byte instead of requiring zero.
        data.extend((zlib.len() as u32).to_le_bytes());
        data.extend(zlib);
        let mut unaligned = vec![0];
        unaligned.extend(&data);
        let shader = SdxShader::parse(&unaligned[1..]).unwrap();
        assert_eq!(shader.unknown, 7);
        assert_eq!(shader.dxbc, dxbc);
        for end in 0..data.len() {
            assert!(SdxShader::parse(&data[..end]).is_err());
        }
        let mut truncated = data.clone();
        truncated.pop();
        let compressed_size = (truncated.len() - 9) as u32;
        truncated[5..9].copy_from_slice(&compressed_size.to_le_bytes());
        assert!(SdxShader::parse(&truncated).is_err()); // Missing zlib trailer.
        data[0] += 1;
        assert!(SdxShader::parse(&data).is_err());
        data[0] -= 1;
        data.push(0);
        assert!(SdxShader::parse(&data).is_err());
    }
}
