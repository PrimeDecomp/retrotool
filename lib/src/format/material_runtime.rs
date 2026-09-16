//! Cooked MPR EXPT: dynamic runtime IDs map to feature permutations per technique.
//!
//! Engine evidence: CMaterialIdentifierRuntime constructor 0x710017b62c,
//! GetPermutation 0x710017c04c, FindShaderId 0x710017c144. Stored offsets address
//! an allocated runtime arena; the arrays in the file are serialized sequentially.

use anyhow::{ensure, Context, Result};
use uuid::Uuid;

use super::material_archive::Reader;

#[derive(Debug)]
pub struct RuntimeTechnique {
    pub unique_count: u32,
    pub runtime_offset: u32,
    pub permutation_offset: u32,
    /// Sorted keys, stored at the material's runtime_id_size on disk.
    pub runtime_ids: Vec<u64>,
    /// Present when the file uses an indirect shared permutation table.
    pub permutation_indices: Option<Vec<u16>>,
    /// Expanded 128-bit values, parallel to runtime_ids.
    pub permutations: Vec<[u64; 2]>,
}

#[derive(Debug)]
pub struct RuntimeMaterial {
    pub id: Uuid,
    pub hash: u32,
    /// Per-technique masks: bit i skips dynamic feature i when building the
    /// runtime ID (SelectDynamicRuntimeId 0x710014c078; caller 0x710018a644).
    pub dynamic_feature_skip_masks: Vec<u32>,
    pub runtime_id_size: u16,
    pub permutation_size: u16,
    pub unique_offset: u32,
    pub allocation_size: u32,
    pub allocation_alignment: Option<u32>,
    /// Exactly 16 slots, indexed by the runtime EMaterialTechnique enum.
    pub techniques: Vec<RuntimeTechnique>,
    pub unique_permutations: Vec<[u64; 2]>,
}

impl RuntimeMaterial {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let id = Uuid::from_bytes_le(r.take(16)?.try_into()?);
        let hash = r.u32()?;
        let count = r.count(4)?;
        ensure!(count <= 16, "Too many EXPT technique words");
        let dynamic_feature_skip_masks = (0..count).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
        let runtime_id_size = r.u16()?;
        let permutation_size = r.u16()?;
        ensure!(matches!(runtime_id_size, 2 | 4 | 8), "Unsupported EXPT runtime ID size");
        ensure!(matches!(permutation_size, 2 | 4 | 8 | 16), "Unsupported EXPT permutation size");
        let unique_count = r.u32()?;
        let unique_offset = r.u32()?;
        let indirect = unique_count != 0;
        let entry_size = if indirect { 2 } else { permutation_size };
        let mut counts = Vec::with_capacity(16);
        let mut techniques = Vec::with_capacity(16);
        for _ in 0..16 {
            counts.push(r.u32()?);
            techniques.push(RuntimeTechnique {
                unique_count: r.u32()?,
                runtime_offset: r.u32()?,
                permutation_offset: r.u32()?,
                runtime_ids: Vec::new(),
                permutation_indices: None,
                permutations: Vec::new(),
            });
        }
        let allocation_size = r.u32()?;
        let allocation_alignment = if allocation_size != 0 { Some(r.u32()?) } else { None };
        // Bound all allocations using the input length before reading the vectors.
        let serialized_size = counts.iter().map(|&c| u64::from(c)).sum::<u64>()
            * u64::from(runtime_id_size + entry_size)
            + u64::from(unique_count) * u64::from(permutation_size);
        ensure!(serialized_size <= data.len() as u64, "EXPT arrays exceed input length");
        let mut spans = Vec::new();
        let mut check_span = |offset: u32, count: u32, width: u16| -> Result<()> {
            let end = u64::from(offset) + u64::from(count) * u64::from(width);
            ensure!(end <= u64::from(allocation_size), "EXPT array exceeds runtime allocation");
            if count != 0 {
                spans.push((u64::from(offset), end));
            }
            Ok(())
        };
        for (t, &count) in techniques.iter().zip(&counts) {
            check_span(t.runtime_offset, count, runtime_id_size)?;
            check_span(t.permutation_offset, count, entry_size)?;
            ensure!(t.unique_count <= count, "EXPT unique count exceeds runtime count");
        }
        check_span(unique_offset, unique_count, permutation_size)?;
        spans.sort_unstable();
        ensure!(spans.windows(2).all(|w| w[0].1 <= w[1].0), "Overlapping EXPT runtime arrays");
        // All techniques' runtime IDs precede all techniques' permutation entries.
        for (t, &count) in techniques.iter_mut().zip(&counts) {
            t.runtime_ids = (0..count)
                .map(|_| read_value(&mut r, runtime_id_size).map(|v| v[0]))
                .collect::<Result<_>>()?;
            ensure!(
                t.runtime_ids.windows(2).all(|w| w[0] < w[1]),
                "EXPT runtime IDs are not strictly increasing"
            );
        }
        for (t, &count) in techniques.iter_mut().zip(&counts) {
            if indirect {
                t.permutation_indices = Some((0..count).map(|_| r.u16()).collect::<Result<_>>()?);
            } else {
                t.permutations = (0..count)
                    .map(|_| read_value(&mut r, permutation_size))
                    .collect::<Result<_>>()?;
            }
        }
        let unique_permutations = (0..unique_count)
            .map(|_| read_value(&mut r, permutation_size))
            .collect::<Result<Vec<_>>>()?;
        r.finish()?;
        if indirect {
            let mut start = 0usize;
            for t in &mut techniques {
                let end =
                    start.checked_add(t.unique_count as usize).context("EXPT count overflow")?;
                ensure!(end <= unique_permutations.len(), "EXPT unique range exceeds table");
                let mut used = vec![false; end - start];
                for &index in t.permutation_indices.as_ref().unwrap() {
                    let index = usize::from(index);
                    ensure!((start..end).contains(&index), "EXPT index outside technique range");
                    used[index - start] = true;
                    t.permutations.push(unique_permutations[index]);
                }
                ensure!(used.iter().all(|&v| v), "Unreferenced EXPT unique permutation");
                start = end;
            }
            ensure!(start == unique_permutations.len(), "EXPT unique technique counts mismatch");
        }
        Ok(Self {
            id,
            hash,
            dynamic_feature_skip_masks,
            runtime_id_size,
            permutation_size,
            unique_offset,
            allocation_size,
            allocation_alignment,
            techniques,
            unique_permutations,
        })
    }

    /// Resolve an already-selected runtime ID. Selecting it from model material
    /// data and feature requirements is a separate operation. Match the engine's
    /// narrowing to the serialized key width, but return None for missing keys.
    pub fn find_permutation(&self, technique: usize, runtime_id: u64) -> Option<[u64; 2]> {
        let t = self.techniques.get(technique)?;
        let key = match self.runtime_id_size {
            2 => u64::from(runtime_id as u16),
            4 => u64::from(runtime_id as u32),
            8 => runtime_id,
            _ => return None,
        };
        t.runtime_ids.binary_search(&key).ok().map(|i| t.permutations[i])
    }
}

fn read_value(r: &mut Reader<'_>, size: u16) -> Result<[u64; 2]> {
    let mut bytes = [0u8; 16];
    bytes[..usize::from(size)].copy_from_slice(r.take(usize::from(size))?);
    Ok([u64::from_le_bytes(bytes[..8].try_into()?), u64::from_le_bytes(bytes[8..].try_into()?)])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two techniques at slots 1 and 4, unequal runtime/unique counts. Arena
    // offsets deliberately differ from file positions and leave allocation gaps.
    fn fixture(indirect: bool, width: u16) -> Vec<u8> {
        let mut d = vec![0; 16];
        let word = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        word(&mut d, 0x12345678);
        word(&mut d, 0); // No optional technique words.
        d.extend_from_slice(&4u16.to_le_bytes());
        d.extend_from_slice(&width.to_le_bytes());
        word(&mut d, if indirect { 3 } else { 0 });
        word(&mut d, 160);
        for i in 0..16 {
            let values = match i {
                1 => [3, 2, 16, 64],
                4 => [1, 1, 48, 128],
                _ => [0; 4],
            };
            for v in values {
                word(&mut d, v);
            }
        }
        word(&mut d, 256);
        word(&mut d, 16);
        for v in [3, 7, 12, 99] {
            word(&mut d, v);
        }
        let append = |d: &mut Vec<u8>, v: u64| {
            let bits = u128::from(v) | (0xaabbccddeeff0011u128 << 64);
            d.extend_from_slice(&bits.to_le_bytes()[..usize::from(width)]);
        };
        if indirect {
            for v in [0u16, 1, 0, 2] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            for v in [0x12, 0x34, 0x56] {
                append(&mut d, v);
            }
        } else {
            for v in [0x12, 0x34, 0x12, 0x56] {
                append(&mut d, v);
            }
        }
        d
    }

    #[test]
    fn resolves_direct_and_indirect_permutations_at_all_widths() {
        for indirect in [false, true] {
            for width in [2, 4, 8, 16] {
                let bytes = fixture(indirect, width);
                // A borrowed slice beginning at an unaligned address is valid.
                let mut unaligned = vec![0xff];
                unaligned.extend_from_slice(&bytes);
                let r = RuntimeMaterial::parse(&unaligned[1..]).unwrap();
                let high = if width == 16 { 0xaabbccddeeff0011 } else { 0 };
                assert_eq!(r.find_permutation(1, 7), Some([0x34, high]));
                assert_eq!(r.find_permutation(1, 12), Some([0x12, high]));
                assert_eq!(r.find_permutation(4, 99), Some([0x56, high]));
                assert_eq!(r.find_permutation(1, 6), None);
                assert_eq!(r.find_permutation(0, 3), None);
                assert_eq!(r.find_permutation(16, 3), None);
                assert_eq!(r.find_permutation(1, 0x100000007), Some([0x34, high]));
            }
        }
    }

    #[test]
    fn rejects_bad_indices_order_ranges_counts_and_truncation() {
        let good = fixture(true, 8);
        // Unique index 3 is out of range; index 2 belongs to the other technique.
        for index in [2u16, 3] {
            let mut d = good.clone();
            d[316..318].copy_from_slice(&index.to_le_bytes());
            assert!(RuntimeMaterial::parse(&d).is_err());
        }
        for (offset, value) in [(300, 7u32), (60, 255), (52, u32::MAX), (28, u32::MAX)] {
            let mut d = good.clone();
            d[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(RuntimeMaterial::parse(&d).is_err());
        }
        for len in [0, 23, 299, good.len() - 1] {
            assert!(RuntimeMaterial::parse(&good[..len]).is_err());
        }
        let mut trailing = good;
        trailing.push(0);
        assert!(RuntimeMaterial::parse(&trailing).is_err());
    }
}
