use super::*;
use crate::format::{CAABox, CVector3f};

fn registry() -> ProbeTileRegistry<u32> {
    let layout = ProbeVolumeLayout::from_bounds(
        CAABox { min: CVector3f::new(-256., -256., -64.), max: CVector3f::new(510., 510., 94.) },
        [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
        0.5,
    )
    .unwrap();
    assert_eq!(layout.grid_origin, [-128, -128, -32]);
    assert_eq!(layout.dimensions, [384, 384, 80]);
    ProbeTileRegistry::new(&layout).unwrap()
}

fn register(registry: &mut ProbeTileRegistry<u32>, id: u32, priority: i32, group: u8) {
    registry
        .register(ProbeTileContext { node_id: id, priority, group }, &[ProbeTile {
            layer: 0,
            position: [0; 3],
            texture: id,
        }])
        .unwrap();
}

fn requests(registry: &mut ProbeTileRegistry<u32>) -> Vec<Option<u32>> {
    registry.drain_mapping_requests().map(|r| r.texture).collect()
}

fn selected(registry: &ProbeTileRegistry<u32>) -> Vec<u32> {
    registry.scheduled_tiles().map(|tile| tile.texture).collect()
}

#[test]
fn registration_keeps_ties_but_removal_prefers_newest_surviving_tie() {
    let mut registry = registry();
    register(&mut registry, 1, 4, 1);
    register(&mut registry, 2, 4, 2);
    register(&mut registry, 3, 4, 3);
    assert_eq!(requests(&mut registry), [Some(1)]);
    assert_eq!(selected(&registry), [1]);
    registry.unregister(1);
    assert_eq!(requests(&mut registry), [Some(3)]);
    assert_eq!(selected(&registry), [3]);
    registry.unregister(3);
    assert_eq!(requests(&mut registry), [Some(2)]);
    registry.unregister(2);
    assert_eq!(requests(&mut registry), [None]);
    assert_eq!(registry.entries().count(), 0);
    assert!(selected(&registry).is_empty());
}

#[test]
fn group_switch_preserves_native_flags_and_request_order() {
    let mut registry = registry();
    register(&mut registry, 1, 10, 1);
    register(&mut registry, 2, 5, 2);
    registry.update_group(Some(2));
    assert_eq!(requests(&mut registry), [Some(1), Some(2)]);
    assert_eq!(selected(&registry), [2]);
    assert!(registry.entries().all(|entry| entry.active));

    // The higher-priority group's old flag was left set; switching back does
    // not remap it. A max-priority or one-active-entry implementation is wrong.
    registry.update_group(Some(1));
    assert!(requests(&mut registry).is_empty());
    assert_eq!(selected(&registry), [2]);
    registry.unregister(2);
    assert_eq!(requests(&mut registry), [Some(1)]);
}

#[test]
fn unchanged_group_and_absent_group_do_not_reselect() {
    let mut registry = registry();
    register(&mut registry, 1, 0, 1);
    registry.update_group(Some(2));
    register(&mut registry, 2, 0, 2);
    registry.update_group(Some(2));
    assert_eq!(requests(&mut registry), [Some(1)]);
    registry.update_group(None);
    assert!(requests(&mut registry).is_empty());
    registry.update_group(Some(2));
    assert_eq!(requests(&mut registry), [Some(2)]);
    assert_eq!(registry.entries().filter(|entry| entry.active).count(), 1);
}

#[test]
fn group_activation_visits_newest_duplicates_first() {
    let mut registry = registry();
    register(&mut registry, 1, 3, 1);
    register(&mut registry, 2, 3, 2);
    register(&mut registry, 3, 3, 2);
    registry.update_group(Some(2));
    // Both inactive group-2 entries are visited, each replacing its predecessor.
    assert_eq!(requests(&mut registry), [Some(1), Some(3), Some(2)]);
    assert_eq!(selected(&registry), [2]);
}

#[test]
fn outside_tiles_have_flags_but_no_mapping_requests() {
    let mut registry = registry();
    registry
        .register(ProbeTileContext { node_id: 1, priority: 0, group: 1 }, &[ProbeTile {
            layer: 2,
            position: [2, 0, 0],
            texture: 1,
        }])
        .unwrap();
    assert!(registry.entries().next().unwrap().active);
    assert!(requests(&mut registry).is_empty());
    assert!(selected(&registry).is_empty());
    registry.unregister(1);
    assert!(requests(&mut registry).is_empty());
}

#[test]
fn invalid_registration_is_atomic_and_keys_do_not_alias() {
    assert_eq!(tile_key([-2, 1, 0]).unwrap(), 0x20000200002);
    assert_eq!(tile_key([0; 3]).unwrap(), 0);
    assert!(tile_key([i32::MIN, 0, 0]).is_err());
    assert!(tile_key([0x100000, 0, 0]).is_err());
    let mut registry = registry();
    let context = ProbeTileContext { node_id: 1, priority: 0, group: 1 };
    let good = ProbeTile { layer: 0, position: [0; 3], texture: 1 };
    for bad in [ProbeTile { layer: 6, ..good }, ProbeTile { position: [0, 0, 0x100000], ..good }] {
        assert!(registry.register(context, &[good, bad]).is_err());
        assert_eq!(registry.entries().count(), 0);
        assert!(requests(&mut registry).is_empty());
        assert!(selected(&registry).is_empty());
    }
}

#[test]
fn event_stream_matches_original_arm64() {
    // Ghidra job b77f6975-4a38-4b21-80c2-1503c415663c preserves the executable
    // oracle and its source. It runs OnRegisterContextNode, OnUnRegisterContextNode
    // and Update, including native multimap insertion/compaction and queue code.
    // Resource getters supply the fixture's physical tile; exclusive refcount
    // stores succeed in one thread. GPU job dispatch/material updates are skipped.
    // Vectors are preallocated. No ownership comparison or mapping is substituted.
    // FNV covers every post-event entry and request word, excluding padding/pointers
    // unrelated to identity. This is a compact native regression, not a file hash.
    let positions =
        [[-2, -2, -2], [0, 0, 0], [1, 1, 1], [3, 3, 2], [-1, 2, -1], [2, 0, 0], [0, 4, 0], [
            0, 0, 3,
        ]];
    let mut random = 0x1a2b3c4du32;
    let mut next = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        random >> 16
    };
    let mut hash = 0xcbf29ce484222325u64;
    let mut word = |value: u64| {
        for byte in value.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    };
    for scenario in 0..16 {
        let mut registry = registry();
        // Replay requests separately to check the public selected-tile view.
        let mut replay = BTreeMap::new();
        for step in 0..96 {
            let op = next() % 4;
            let id = next() % 5 + 1;
            let priority = (next() % 7) as i32 - 3;
            let group = (next() % 5) as u8;
            let mut layer = next() % 6;
            let mut position = (next() % 8) as usize;
            if scenario < 8 {
                layer = 0;
                position = 1;
            }
            let event = scenario * 96 + step;
            match op {
                0 | 1 => registry
                    .register(ProbeTileContext { node_id: id, priority, group: group % 4 }, &[
                        ProbeTile { layer, position: positions[position], texture: 1000 + event },
                    ])
                    .unwrap(),
                2 => registry.unregister(id),
                _ => registry.update_group((group < 4).then_some(group)),
            }
            word(u64::from(event));
            let requests: Vec<_> = registry.drain_mapping_requests().collect();
            for layer in 0..6 {
                let entries: Vec<_> =
                    registry.entries().filter(|e| e.tile.layer == layer).collect();
                word(entries.len() as u64);
                for entry in entries {
                    for v in entry.tile.position {
                        word(u64::from(v as u32));
                    }
                    word(u64::from(entry.tile.layer));
                    word(u64::from(entry.tile.texture));
                    word(u64::from(entry.context.node_id));
                    word(u64::from(entry.context.priority as u32));
                    word(u64::from(entry.context.group));
                    word(u64::from(entry.active));
                }
                let requests: Vec<_> = requests.iter().filter(|r| r.layer == layer).collect();
                word(requests.len() as u64);
                for request in requests {
                    word(u64::from(request.texture.unwrap_or(0)));
                    word(request.mapping.virtual_byte_offset);
                    for v in request.mapping.tile {
                        word(u64::from(v));
                    }
                    word(request.mapping.byte_len);
                    let key = (layer, request.position);
                    if let Some(texture) = request.texture {
                        replay.insert(key, texture);
                    } else {
                        replay.remove(&key);
                    }
                }
            }
            assert_eq!(
                registry
                    .scheduled_tiles()
                    .map(|t| ((t.layer, t.position), t.texture))
                    .collect::<BTreeMap<_, _>>(),
                replay,
            );
        }
    }
    assert_eq!(hash, 0x043d4ca43c441459);
}
