//! Immutable snapshots of a shader's global address space. Virtual addresses
//! are caller-assigned integers; they never become pointers into host memory.

use anyhow::{ensure, Context, Result};

use super::packed;

const REGION_HEADER_WORDS: usize = 5;

/// One nonempty mapped range. Adjacent ranges may satisfy a single read;
/// overlapping ranges are rejected rather than resolved by insertion order.
#[derive(Clone, Copy, Debug)]
pub struct Region<'a> {
    pub address: u64,
    pub bytes: &'a [u8],
    /// None means every byte is known; otherwise one validity flag per byte.
    pub known: Option<&'a [bool]>,
}

/// A validated, immutable memory image, shared by interpreter and WGSL.
#[derive(Clone, Debug)]
pub struct Snapshot {
    words: Vec<u32>,
}

impl Default for Snapshot {
    fn default() -> Self { Self { words: vec![0] } }
}

impl Snapshot {
    pub fn new(regions: &[Region<'_>]) -> Result<Self> {
        let mut regions = regions.to_vec();
        regions.sort_by_key(|region| region.address);
        let word_count = validate_regions(&regions)?;
        let header_words = 1 + regions.len() * REGION_HEADER_WORDS;
        let mut words = Vec::with_capacity(word_count);
        words.resize(header_words, 0);
        words[0] = regions.len() as u32;

        for (index, region) in regions.iter().enumerate() {
            let data_offset = words.len() as u32;
            packed::append_bytes(&mut words, region.bytes);
            let validity_offset = if let Some(known) = region.known {
                let offset = words.len() as u32;
                packed::append_validity(&mut words, known);
                offset
            } else {
                0
            };
            let header = RegionHeader {
                address: region.address,
                byte_len: region.bytes.len() as u32,
                data_offset,
                validity_offset,
            };
            let start = 1 + index * REGION_HEADER_WORDS;
            words[start..start + REGION_HEADER_WORDS].copy_from_slice(&header.encode());
        }
        Ok(Self { words })
    }

    /// Upload as little-endian words. The ABI is a region count followed by
    /// five words per sorted region: address low/high, byte length, data word
    /// offset, validity word offset (zero means all bytes known), then payloads.
    /// Word padding does not make additional bytes readable.
    pub fn words(&self) -> &[u32] { &self.words }

    /// Read established bytes, including across adjacent regions. Alignment is
    /// an instruction/provider policy, not a property of a byte-address map.
    /// On error, output may contain bytes read before the unavailable address.
    pub fn read(&self, address: u64, output: &mut [u8]) -> Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        ensure!(
            address.checked_add((output.len() - 1) as u64).is_some(),
            "memory read wraps the address space"
        );
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = self.read_byte(address + index as u64)?;
        }
        Ok(())
    }

    fn read_byte(&self, address: u64) -> Result<u8> {
        let region = self
            .find_region(address)
            .with_context(|| format!("unmapped memory byte {address:#x}"))?;
        let byte_offset = (address - region.address) as usize;
        if region.validity_offset != 0 {
            let bitmap = &self.words[region.validity_offset as usize..];
            ensure!(packed::byte_is_known(bitmap, byte_offset), "unknown memory byte {address:#x}");
        }
        let data = &self.words[region.data_offset as usize..];
        Ok(packed::read_byte(data, byte_offset))
    }

    fn find_region(&self, address: u64) -> Option<RegionHeader> {
        let count = self.words[0] as usize;
        let table = &self.words[1..1 + count * REGION_HEADER_WORDS];
        let (entries, _) = table.as_chunks::<REGION_HEADER_WORDS>();
        let insertion =
            entries.partition_point(|entry| RegionHeader::decode(entry).address <= address);
        let index = insertion.checked_sub(1)?;
        let region = RegionHeader::decode(&entries[index]);
        if address - region.address < u64::from(region.byte_len) {
            Some(region)
        } else {
            None
        }
    }
}

/// The packed header is decoded in one place so host lookups use named fields.
struct RegionHeader {
    address: u64,
    byte_len: u32,
    data_offset: u32,
    validity_offset: u32,
}

impl RegionHeader {
    fn encode(&self) -> [u32; REGION_HEADER_WORDS] {
        [
            self.address as u32,
            (self.address >> 32) as u32,
            self.byte_len,
            self.data_offset,
            self.validity_offset,
        ]
    }

    fn decode(words: &[u32; REGION_HEADER_WORDS]) -> Self {
        Self {
            address: u64::from(words[0]) | (u64::from(words[1]) << 32),
            byte_len: words[2],
            data_offset: words[3],
            validity_offset: words[4],
        }
    }
}

fn validate_regions(regions: &[Region<'_>]) -> Result<usize> {
    u32::try_from(regions.len()).context("too many memory regions")?;
    let mut word_count = regions
        .len()
        .checked_mul(REGION_HEADER_WORDS)
        .and_then(|count| count.checked_add(1))
        .context("memory map header overflow")?;
    // u128 permits an exclusive end one past u64::MAX.
    let mut previous_end = 0u128;
    for region in regions {
        ensure!(!region.bytes.is_empty(), "empty memory region");
        ensure!(region.bytes.len() <= u32::MAX as usize, "memory region exceeds 32-bit length");
        ensure!(u128::from(region.address) >= previous_end, "overlapping memory regions");
        previous_end = u128::from(region.address) + region.bytes.len() as u128;
        ensure!(previous_end <= 1u128 << 64, "memory region wraps the address space");

        let data_words = region.bytes.len().div_ceil(4);
        let validity_words = if let Some(known) = region.known {
            ensure!(known.len() == region.bytes.len(), "memory validity length mismatch");
            known.len().div_ceil(32)
        } else {
            0
        };
        word_count = word_count
            .checked_add(data_words)
            .and_then(|count| count.checked_add(validity_words))
            .context("memory snapshot size overflow")?;
    }
    ensure!(word_count <= u32::MAX as usize, "memory snapshot exceeds its word-offset ABI");
    Ok(word_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_regions_cross_word_and_address_boundaries() {
        let snapshot = Snapshot::new(&[
            Region { address: 0x100000001, bytes: &[5, 6, 7, 8, 9], known: None },
            Region { address: 0xfffffffd, bytes: &[1, 2, 3, 4], known: None },
        ])
        .unwrap();
        let mut bytes = [0; 9];
        snapshot.read(0xfffffffd, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(snapshot.read(0xfffffffc, &mut [0]).is_err());
        assert!(snapshot.read(0x100000006, &mut [0]).is_err());
    }

    #[test]
    fn gaps_unknown_bytes_and_upload_padding_do_not_become_memory() {
        let snapshot = Snapshot::new(&[
            Region { address: 5, bytes: &[0x55, 0x66, 0x77], known: Some(&[true, false, true]) },
            Region { address: 9, bytes: &[0x99], known: None },
        ])
        .unwrap();
        let mut byte = [0];
        snapshot.read(7, &mut byte).unwrap();
        assert_eq!(byte, [0x77]);
        for address in [4, 6, 8, 10, u64::MAX] {
            assert!(snapshot.read(address, &mut byte).is_err());
        }
        assert!(Snapshot::default().read(0, &mut byte).is_err());
    }

    #[test]
    fn ambiguous_and_wrapping_maps_are_rejected_but_last_byte_is_addressable() {
        let a = Region { address: u64::MAX, bytes: &[0xaa], known: None };
        let snapshot = Snapshot::new(&[a]).unwrap();
        let mut byte = [0];
        snapshot.read(u64::MAX, &mut byte).unwrap();
        assert_eq!(byte, [0xaa]);
        assert!(snapshot.read(u64::MAX, &mut [0; 2]).is_err());
        assert!(Snapshot::new(&[a, a]).is_err());
        assert!(Snapshot::new(&[Region { bytes: &[0; 2], ..a }]).is_err());
        assert!(Snapshot::new(&[Region { bytes: &[], ..a }]).is_err());
        assert!(Snapshot::new(&[Region { known: Some(&[]), ..a }]).is_err());
    }
}
