//! Host storage for addressable Maxwell constant buffers.
//!
//! This is a translator ABI, not an MPR file format. The first 64 u32 words
//! contain 32 (word offset, byte length) pairs, followed by little-endian data.
//! Length bit31 indicates a per-byte validity bitmap after the bank's padded
//! data. Absent banks have length zero. Padding does not extend readable ranges.

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConstantRead {
    pub buffer: u8,
    pub byte_offset: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ConstantMemoryBinding {
    pub group: u32,
    pub binding: u32,
}

#[derive(Clone, Debug)]
pub struct ConstantMemory {
    words: Vec<u32>,
}

/// A supplied hardware bank. None means every byte is known; otherwise each
/// byte has a corresponding validity bit. Unknown bytes cannot be read by the
/// generated LDC helper, including when neighboring bytes are known.
#[derive(Clone, Copy, Debug)]
pub struct ConstantBank<'a> {
    pub bank: u8,
    pub bytes: &'a [u8],
    pub known: Option<&'a [bool]>,
}

impl ConstantMemory {
    /// Supply the actual bound bytes. Banks are hardware indices, not API slots.
    /// Individual constant buffers can contain at most 64 KiB.
    pub fn new(banks: &[(u8, &[u8])]) -> Result<Self> {
        Self::from_banks(
            &banks
                .iter()
                .map(|&(bank, bytes)| ConstantBank { bank, bytes, known: None })
                .collect::<Vec<_>>(),
        )
    }

    /// Preserve partially recovered buffers. A sparse bank appends ceil(size/32)
    /// bitmap words after ceil(size/4) data words; bit i marks byte i as known.
    /// Fully known banks retain the original ABI without a bitmap.
    pub fn from_banks(banks: &[ConstantBank<'_>]) -> Result<Self> {
        let mut words = vec![0u32; 64];
        let mut present = [false; 32];
        for &ConstantBank { bank, bytes, known } in banks {
            let bank = usize::from(bank);
            ensure!(bank < 32, "invalid constant-buffer bank");
            ensure!(!present[bank], "duplicate constant-buffer bank");
            ensure!(bytes.len() <= 0x10000, "constant buffer exceeds 64 KiB");
            if let Some(known) = known {
                ensure!(known.len() == bytes.len(), "constant validity length mismatch");
            }
            let sparse = known.is_some_and(|known| known.iter().any(|v| !v));
            present[bank] = true;
            words[bank * 2] = words.len() as u32;
            words[bank * 2 + 1] = bytes.len() as u32 | if sparse { 0x80000000 } else { 0 };
            for chunk in bytes.chunks(4) {
                let mut word = [0u8; 4];
                word[..chunk.len()].copy_from_slice(chunk);
                words.push(u32::from_le_bytes(word));
            }
            if sparse {
                for chunk in known.unwrap().chunks(32) {
                    words.push(
                        chunk
                            .iter()
                            .enumerate()
                            .fold(0u32, |mask, (i, &valid)| mask | (u32::from(valid) << i)),
                    );
                }
            }
        }
        Ok(Self { words })
    }

    /// Upload these u32 words as little-endian bytes to the generated binding.
    pub fn words(&self) -> &[u32] { &self.words }

    /// Build the existing compact constant argument from the same bank data.
    /// This keeps direct ALU and indexed LDC reads coherent. Returns one zero
    /// word for an empty manifest, matching the generated minimum array length.
    pub fn compact_reads(&self, reads: &[ConstantRead]) -> Result<Vec<u32>> {
        if reads.is_empty() {
            return Ok(vec![0]);
        }
        reads
            .iter()
            .map(|read| {
                let bank = usize::from(read.buffer);
                ensure!(bank < 32, "invalid constant-buffer bank");
                let start = self.words[bank * 2] as usize;
                let tagged_size = self.words[bank * 2 + 1];
                let size = tagged_size & 0x7fffffff;
                let address = u32::from(read.byte_offset);
                ensure!(address % 4 == 0, "unaligned direct constant read");
                ensure!(
                    address <= size && size - address >= 4,
                    "direct constant read c[{bank}][{address:#x}] exceeds {size:#x} bound bytes"
                );
                if tagged_size & 0x80000000 != 0 {
                    let bitmap = start + (size as usize).div_ceil(4);
                    for byte in address as usize..address as usize + 4 {
                        ensure!(
                            self.words[bitmap + byte / 32] & (1 << (byte % 32)) != 0,
                            "direct constant read c[{bank}][{address:#x}] uses unknown byte {byte:#x}"
                        );
                    }
                }
                self.words
                    .get(start + address as usize / 4)
                    .copied()
                    .context("invalid constant memory span")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_and_addressable_constants_share_bytes_without_readable_padding() {
        let memory = ConstantMemory::new(&[(3, &[0x44, 0x33, 0x22, 0x11, 0xff])]).unwrap();
        assert_eq!(&memory.words()[6..8], &[64, 5]);
        assert_eq!(&memory.words()[64..], &[0x11223344, 0xff]);
        assert_eq!(memory.compact_reads(&[ConstantRead { buffer: 3, byte_offset: 0 }]).unwrap(), [
            0x11223344
        ]);
        assert!(memory.compact_reads(&[ConstantRead { buffer: 3, byte_offset: 4 }]).is_err());
        assert!(memory.compact_reads(&[ConstantRead { buffer: 0, byte_offset: 0 }]).is_err());
        assert!(ConstantMemory::new(&[(3, &[]), (3, &[1])]).is_err());
        assert!(ConstantMemory::new(&[(32, &[])]).is_err());
    }

    #[test]
    fn sparse_banks_distinguish_unknown_bytes_from_zero_and_padding() {
        let data = [0; 37];
        let mut known = [false; 37];
        known[28..36].fill(true);
        let memory = ConstantMemory::from_banks(&[ConstantBank {
            bank: 0,
            bytes: &data,
            known: Some(&known),
        }])
        .unwrap();
        assert_eq!(memory.words()[1], 0x80000025);
        // Ten data words, then two validity words; the final padded data bytes
        // are not valid reads and cannot satisfy a missing byte at offset36.
        assert_eq!(&memory.words()[74..], &[0xf0000000, 15]);
        assert_eq!(
            memory
                .compact_reads(&[ConstantRead { buffer: 0, byte_offset: 28 }, ConstantRead {
                    buffer: 0,
                    byte_offset: 32
                }])
                .unwrap(),
            [0, 0]
        );
        assert!(memory.compact_reads(&[ConstantRead { buffer: 0, byte_offset: 24 }]).is_err());
        assert!(memory.compact_reads(&[ConstantRead { buffer: 0, byte_offset: 36 }]).is_err());
        assert!(ConstantMemory::from_banks(&[ConstantBank {
            bank: 0,
            bytes: &data,
            known: Some(&known[..36])
        }])
        .is_err());
    }
}
