//! Probe tile registration and mapping decisions, recovered from the native
//! CBakedLightingProbeTexture. Callers supply scene events in their actual order;
//! this module does not infer residency from serialized ROOM definitions.

use std::collections::BTreeMap;

use anyhow::{ensure, Result};

use super::{ProbeLayerLayout, ProbeTileMapping, ProbeVolumeLayout};

/// Values copied from the scene node when its physical tiles are registered.
/// The node ID is the runtime 32-bit scene identity, not the manager slot index
/// or a serialized object UUID. Subsequent priority edits do not update copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTileContext {
    pub node_id: u32,
    pub priority: i32,
    pub group: u8,
}

/// A physical tile and an opaque caller-owned texture handle or reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTile<T> {
    pub layer: u32,
    pub position: [i32; 3],
    pub texture: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTileState<T> {
    pub tile: ProbeTile<T>,
    pub context: ProbeTileContext,
    /// Native bookkeeping flag. More than one entry at a coordinate can be
    /// active; this flag does not identify the last texture requested there.
    pub active: bool,
}

/// A queued sparse-page change. None unmaps the page; it does not fill it black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTileMappingRequest<T> {
    pub layer: u32,
    pub position: [i32; 3],
    pub mapping: ProbeTileMapping,
    pub texture: Option<T>,
}

#[derive(Clone, Copy)]
struct Entry<T> {
    key: u64,
    state: ProbeTileState<T>,
    mapping: Option<ProbeTileMapping>,
}

/// Event-ordered reference for native tile ownership. Bounds are fixed for this
/// instance; reconstruct it when the native volume is destroyed/reinitialized.
/// Registered duplicates are retained, just as in the native multimap.
///
/// Mapping requests are recorded synchronously. The game's double-buffered GPU
/// job submission and completion are outside this model. `scheduled_tiles`
/// describes the result after all emitted requests, not current GPU residency.
pub struct ProbeTileRegistry<T> {
    layers: [ProbeLayerLayout; 6],
    entries: [Vec<Entry<T>>; 6],
    requests: [Vec<ProbeTileMappingRequest<T>>; 6],
    scheduled: [BTreeMap<u64, ProbeTile<T>>; 6],
    active_group: Option<u8>,
}

impl<T: Copy> ProbeTileRegistry<T> {
    pub fn new(layout: &ProbeVolumeLayout) -> Result<Self> {
        let layers = (0..6).map(|layer| layout.layer(layer)).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            layers: layers.try_into().unwrap(),
            entries: std::array::from_fn(|_| Vec::new()),
            requests: std::array::from_fn(|_| Vec::new()),
            scheduled: std::array::from_fn(|_| BTreeMap::new()),
            active_group: None,
        })
    }

    /// OnRegisterContextNode71001ca01c. A strictly greater priority replaces the
    /// first active entry at a coordinate; ties keep that entry. New duplicates
    /// are inserted before old ones. All inputs are validated before mutation.
    pub fn register(&mut self, context: ProbeTileContext, tiles: &[ProbeTile<T>]) -> Result<()> {
        let incoming = tiles
            .iter()
            .map(|&tile| {
                ensure!(tile.layer < 6, "Unknown probe tile layer {}", tile.layer);
                Ok(Entry {
                    key: tile_key(tile.position)?,
                    state: ProbeTileState { tile, context, active: false },
                    mapping: self.layers[tile.layer as usize].map_tile(tile.position)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for mut entry in incoming {
            let layer = entry.state.tile.layer as usize;
            let entries = &mut self.entries[layer];
            let range = equal_range(entries, entry.key);
            let old = entries[range.clone()].iter_mut().find(|e| e.state.active);
            let map = match old {
                Some(old) if old.state.context.priority < context.priority => {
                    old.state.active = false;
                    true
                }
                Some(_) => false,
                None => true,
            };
            if map {
                entry.state.active = true;
                emit(&entry, true, &mut self.requests[layer], &mut self.scheduled[layer]);
            }
            entries.insert(range.start, entry);
        }
        Ok(())
    }

    /// Update71001c9bd8 / EnsureGroupIdTilesActive71001c9c48. Only a change to a
    /// present group triggers activation. None preserves mappings, and registering
    /// more tiles does not force a repeated group to run again.
    pub fn update_group(&mut self, group: Option<u8>) {
        if let Some(group) = group.filter(|_| group != self.active_group) {
            for layer in 0..6 {
                let entries = &mut self.entries[layer];
                for index in 0..entries.len() {
                    let incoming = entries[index];
                    if incoming.state.context.group != group || incoming.state.active {
                        continue;
                    }
                    let range = equal_range(entries, incoming.key);
                    if let Some(old) = entries[range].iter_mut().find(|e| {
                        e.state.active
                            && e.state.context.priority <= incoming.state.context.priority
                    }) {
                        old.state.active = false;
                    }
                    // Native always requests this tile, even if a higher-priority
                    // active entry was left flagged. Do not turn this into max().
                    entries[index].state.active = true;
                    emit(&incoming, true, &mut self.requests[layer], &mut self.scheduled[layer]);
                }
            }
        }
        self.active_group = group;
    }

    /// OnUnRegisterContextNode71001ca650 / predicate71001cba28. Removing an active
    /// entry maps the highest-priority other node at that coordinate, keeping the
    /// first equal-priority candidate in current multimap order. Group is ignored.
    pub fn unregister(&mut self, node_id: u32) {
        for layer in 0..6 {
            let entries = &mut self.entries[layer];
            let mut write = 0;
            for read in 0..entries.len() {
                let outgoing = entries[read];
                if outgoing.state.context.node_id != node_id {
                    // Preserve the native remove_if's in-place compaction: later
                    // predicates search this partly compacted, full-length array.
                    entries[write] = entries[read];
                    write += 1;
                    continue;
                }
                if !outgoing.state.active {
                    continue;
                }
                let mut replacement = None;
                for index in equal_range(entries, outgoing.key) {
                    let candidate = &entries[index];
                    if candidate.state.context.node_id == node_id {
                        continue;
                    }
                    if replacement.is_none_or(|old: usize| {
                        candidate.state.context.priority > entries[old].state.context.priority
                    }) {
                        replacement = Some(index);
                    }
                }
                entries[read].state.active = false;
                match replacement {
                    Some(index) => {
                        entries[index].state.active = true;
                        emit(
                            &entries[index],
                            true,
                            &mut self.requests[layer],
                            &mut self.scheduled[layer],
                        );
                    }
                    None => emit(
                        &outgoing,
                        false,
                        &mut self.requests[layer],
                        &mut self.scheduled[layer],
                    ),
                }
            }
            entries.truncate(write);
        }
    }

    /// Native layer/key order, with newest duplicates first until removals.
    pub fn entries(&self) -> impl Iterator<Item = ProbeTileState<T>> + '_ {
        self.entries.iter().flatten().map(|entry| entry.state)
    }

    /// Selected physical tiles after all queued requests. Safe to insert into a
    /// fresh `ProbeVolume`; the result has at most one owner per layer/coordinate.
    pub fn scheduled_tiles(&self) -> impl Iterator<Item = ProbeTile<T>> + '_ {
        self.scheduled.iter().flat_map(|layer| layer.values().copied())
    }

    /// Preserve request order within each layer, including intermediate remaps.
    pub fn drain_mapping_requests(
        &mut self,
    ) -> impl Iterator<Item = ProbeTileMappingRequest<T>> + '_ {
        self.requests.iter_mut().flat_map(|layer| layer.drain(..))
    }
}

fn emit<T: Copy>(
    entry: &Entry<T>,
    map: bool,
    requests: &mut Vec<ProbeTileMappingRequest<T>>,
    scheduled: &mut BTreeMap<u64, ProbeTile<T>>,
) {
    // Both native queue helpers update active even outside the virtual bounds;
    // only the actual mapping request is skipped. Callers handle the flag.
    let Some(mapping) = entry.mapping else { return };
    let tile = entry.state.tile;
    requests.push(ProbeTileMappingRequest {
        layer: tile.layer,
        position: tile.position,
        mapping,
        texture: map.then_some(tile.texture),
    });
    if map {
        scheduled.insert(entry.key, tile);
    } else {
        scheduled.remove(&entry.key);
    }
}

fn equal_range<T>(entries: &[Entry<T>], key: u64) -> std::ops::Range<usize> {
    entries.partition_point(|e| e.key < key)..entries.partition_point(|e| e.key <= key)
}

fn tile_key(position: [i32; 3]) -> Result<u64> {
    let mut key = 0;
    for (axis, value) in position.into_iter().enumerate() {
        // BuildTileKey71001ca330 packs magnitude and a positive-sign bit. Reject
        // coordinates that would alias after the native 20-bit magnitude mask.
        let magnitude = value.unsigned_abs();
        ensure!(magnitude <= 0xfffff, "Probe tile coordinate exceeds native key range");
        key |= (u64::from(magnitude) | (u64::from(value > 0) << 20)) << (axis * 21);
    }
    Ok(key)
}

#[cfg(test)]
mod tests;
