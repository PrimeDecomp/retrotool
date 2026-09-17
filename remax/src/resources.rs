//! Bound bytes shared by Maxwell constant reads and host shader storage.
//!
//! This is a translator ABI, not an MPR file format. The first 64 u32 words
//! contain 32 (word offset, byte length) pairs, followed by little-endian data.
//! Length bit31 indicates a per-byte validity bitmap after the bank's padded
//! data. Absent banks have length zero. Padding does not extend readable ranges.

pub mod memory;
mod packed;
pub mod texture;

use anyhow::{ensure, Result};
use serde_derive::Serialize;

const BANK_COUNT: usize = 32;
const HEADER_WORDS: usize = BANK_COUNT * 2;
const MAX_BANK_BYTES: usize = 0x10000;
const VALIDITY_FLAG: u32 = 0x80000000;

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

/// A validated immutable packed snapshot. Constructors establish complete
/// header, payload and validity-bitmap storage; absent/unknown bytes remain
/// unavailable. Backend binding contracts can rely on this layout while still
/// checking dynamic access bounds, alignment and byte availability.
#[derive(Clone, Debug)]
pub struct ConstantMemory {
    words: Vec<u32>,
}

impl Default for ConstantMemory {
    fn default() -> Self { Self { words: vec![0; HEADER_WORDS] } }
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
        let mut words = vec![0u32; HEADER_WORDS];
        let mut present = [false; BANK_COUNT];
        for &ConstantBank { bank, bytes, known } in banks {
            let bank = usize::from(bank);
            ensure!(bank < BANK_COUNT, "invalid constant-buffer bank");
            ensure!(!present[bank], "duplicate constant-buffer bank");
            ensure!(bytes.len() <= MAX_BANK_BYTES, "constant buffer exceeds 64 KiB");
            if let Some(known) = known {
                ensure!(known.len() == bytes.len(), "constant validity length mismatch");
            }
            let sparse = known.is_some_and(|known| known.iter().any(|&valid| !valid));
            present[bank] = true;
            words[bank * 2] = words.len() as u32;
            words[bank * 2 + 1] = bytes.len() as u32 | if sparse { VALIDITY_FLAG } else { 0 };
            packed::append_bytes(&mut words, bytes);
            if sparse {
                packed::append_validity(&mut words, known.expect("sparse bank has validity flags"));
            }
        }
        Ok(Self { words })
    }

    /// Upload these u32 words as little-endian bytes to the generated binding.
    pub fn words(&self) -> &[u32] { &self.words }

    /// Read established bytes without exposing upload padding or inventing data
    /// for an absent bank. Alignment requirements belong to the requesting
    /// instruction; this byte accessor also serves sub-word loads.
    pub fn read_bytes<const N: usize>(&self, bank: u8, address: u32) -> Result<[u8; N]> {
        self.validate_range(bank, address, N)?;
        let start = self.words[usize::from(bank) * 2] as usize;
        let data = &self.words[start..];
        Ok(std::array::from_fn(|index| packed::read_byte(data, address as usize + index)))
    }

    /// Check a byte span without copying data. Padding and unknown bytes are
    /// excluded; instruction-specific alignment remains the caller's responsibility.
    pub fn validate_range(&self, bank: u8, address: u32, byte_count: usize) -> Result<()> {
        let bank = usize::from(bank);
        ensure!(bank < BANK_COUNT, "invalid constant-buffer bank");
        let start = self.words[bank * 2] as usize;
        let tagged_size = self.words[bank * 2 + 1];
        let size = tagged_size & !VALIDITY_FLAG;
        ensure!(
            address <= size && byte_count <= (size - address) as usize,
            "constant read c[{bank}][{address:#x}] of {byte_count} bytes exceeds {size:#x} bound bytes"
        );
        let address = address as usize;
        if tagged_size & VALIDITY_FLAG != 0 {
            let bitmap = start + (size as usize).div_ceil(4);
            for byte in address..address + byte_count {
                ensure!(
                    packed::byte_is_known(&self.words[bitmap..], byte),
                    "constant read c[{bank}][{address:#x}] uses unknown byte {byte:#x}"
                );
            }
        }
        Ok(())
    }

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
                let bank = read.buffer;
                let address = u32::from(read.byte_offset);
                ensure!(address.is_multiple_of(4), "unaligned direct constant read");
                let bytes = self.read_bytes::<4>(bank, address)?;
                Ok(u32::from_le_bytes(bytes))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_reads_cross_storage_words_without_exposing_padding() {
        let memory = ConstantMemory::new(&[(3, &[0x11, 0x22, 0x33, 0x44, 0x55])]).unwrap();
        assert_eq!(memory.read_bytes::<4>(3, 1).unwrap(), [0x22, 0x33, 0x44, 0x55]);
        assert!(memory.read_bytes::<2>(3, 4).is_err());
        assert!(memory.read_bytes::<1>(3, u32::MAX).is_err());
        assert!(ConstantMemory::default().read_bytes::<1>(0, 0).is_err());
    }

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
