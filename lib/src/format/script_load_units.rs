//! ROOM load-unit records, initial resource requests and layer-selection rules.
//!
//! The caller supplies environment state and loaded-area order. These helpers
//! evaluate requests and conditions; streaming and Entity activation remain
//! responsibilities of the scene lifecycle.

use std::{collections::BTreeSet, io::Cursor};

use anyhow::{anyhow, bail, ensure, Result};
use binrw::{binrw, BinReaderExt};
use zerocopy::ByteOrder;

use crate::format::{
    load_unit_logic::LoadUnitLogic,
    room::{LayerHeader, SGameAreaMetaData, K_CHUNK_LLYR, K_CHUNK_LRES, K_CHUNK_LUHD},
    slice_chunks, CObjectId, CStringFixed, TaggedVec,
};

/// LUHD, as read by CScriptLoadUnitResourceData7100478aac. Unlike that loader,
/// retain the name and second blob so tools can round-trip the complete record.
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct LoadUnitHeader {
    pub name: CStringFixed,
    pub id: CObjectId,
    /// Environment variable controlling persisted unit state. Nil falls back
    /// to id; when both GUIDs are nil, the unit is requested without a lookup.
    pub state_variable_id: CObjectId,
    /// Authored flags. InitializeLoadUnits can replace these with runtime flags
    /// from the first matching override; do not use them as final game-mode state.
    pub flags: u16,
    /// Prefix LoadUnitLogic condition. Empty means no condition; raw bytes are
    /// retained for serialization. Evaluate even when state value 2 forces true.
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub data: Vec<u8>,
    /// Serialized after data, but skipped by the native MPR resource loader.
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub skipped_data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct LoadUnit {
    pub header: LoadUnitHeader,
    /// LRES: resource IDs requested from the resource store.
    pub resource_ids: Vec<CObjectId>,
    /// LLYR: serialized layer IDs. The native layer-manager queries below use
    /// LHED membership instead; a nil-ID unit need not appear in those lists.
    pub layer_ids: Vec<CObjectId>,
}

impl LoadUnit {
    /// Read the contents of a LUNT form, excluding its RFRM descriptor.
    pub fn slice_data<O: ByteOrder + 'static>(data: &[u8]) -> Result<Self> {
        let mut header = None;
        let mut resource_ids = None;
        let mut layer_ids = None;
        slice_chunks::<O, _, _>(
            data,
            |chunk, data| {
                let mut reader = Cursor::new(data);
                match chunk.id {
                    K_CHUNK_LUHD => {
                        ensure!(header.is_none(), "Duplicate LUHD chunk");
                        header = Some(reader.read_le::<LoadUnitHeader>()?);
                    }
                    K_CHUNK_LRES | K_CHUNK_LLYR => {
                        let out = if chunk.id == K_CHUNK_LRES {
                            &mut resource_ids
                        } else {
                            &mut layer_ids
                        };
                        ensure!(out.is_none(), "Duplicate {} chunk", chunk.id);
                        *out = Some(reader.read_le::<TaggedVec<u32, CObjectId>>()?.data);
                    }
                    id => bail!("Unknown LUNT chunk: {id:?}"),
                }
                ensure!(reader.position() == data.len() as u64, "Trailing {} bytes", chunk.id);
                Ok(())
            },
            |form, _| bail!("Unknown LUNT form: {:?}", form.id),
        )?;
        Ok(Self {
            header: header.ok_or_else(|| anyhow!("Missing LUHD chunk"))?,
            resource_ids: resource_ids.ok_or_else(|| anyhow!("Missing LRES chunk"))?,
            layer_ids: layer_ids.ok_or_else(|| anyhow!("Missing LLYR chunk"))?,
        })
    }
}

/// Minimal runtime input to the queries, in CLoadUnitManager order. Both flags
/// and load_state are runtime values, not inferred from serialized metadata.
#[derive(Clone, Copy, Debug)]
pub struct LoadUnitState {
    pub id: CObjectId,
    pub flags: u16,
    /// Native GetLoadState: zero means unloaded; queries retain every other value.
    pub load_state: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadUnitFlagOverride {
    pub id: CObjectId,
    pub flags: u16,
}

#[derive(Clone, Debug, Default)]
pub struct InitialLoadUnits {
    pub requested: Vec<CObjectId>,
    pub overrides: Vec<LoadUnitFlagOverride>,
}

/// InitializeLoadUnits71003fbb98 uses the first override, including nil GUIDs.
pub fn effective_load_unit_flags(
    id: CObjectId,
    authored: u16,
    overrides: &[LoadUnitFlagOverride],
) -> u16 {
    overrides.iter().find(|o| o.id == id).map_or(authored, |o| o.flags)
}

pub fn load_unit_state_variable(id: CObjectId, state_variable_id: CObjectId) -> Option<CObjectId> {
    if !state_variable_id.is_nil() {
        Some(state_variable_id)
    } else if !id.is_nil() {
        Some(id)
    } else {
        None
    }
}

/// EvaluateLoadUnits71003fb2d8 modifies only bits 0/1. State 1 requests a unit;
/// state 2 additionally forces the condition to pass. The condition is still
/// evaluated by the caller in that case. Other state values clear bit 0.
pub fn evaluated_load_unit_flags(authored: u16, state_value: i32, condition: Option<bool>) -> u16 {
    (authored & !3)
        | u16::from(matches!(state_value, 1 | 2))
        | (u16::from(state_value == 2 || condition.unwrap_or(true)) << 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvironmentValue {
    pub id: CObjectId,
    pub value: i32,
}

#[derive(Clone, Debug)]
pub struct AreaEnvironment {
    pub id: CObjectId,
    /// Native GetValue uses the first matching GUID, including a stored zero.
    pub variables: Vec<EnvironmentValue>,
}

#[derive(Clone, Copy, Debug)]
pub struct LoadedEnvironmentArea {
    pub id: CObjectId,
    pub parent: CObjectId,
}

/// Explicit saved/current environment and loaded-area order. This is not an
/// automatic reconstruction of world streaming or performance-group activity.
#[derive(Clone, Debug, Default)]
pub struct LoadUnitEnvironment {
    pub area_states: Vec<AreaEnvironment>,
    pub loaded_areas: Vec<LoadedEnvironmentArea>,
}

impl LoadUnitEnvironment {
    fn value_in_area(&self, area: CObjectId, variable: CObjectId) -> Option<i32> {
        self.area_states
            .iter()
            .find(|a| a.id == area)
            .and_then(|a| a.variables.iter().find(|v| v.id == variable))
            .map(|v| v.value)
    }

    /// Current area first, then only loaded parents, beginning at parent. A
    /// stored zero stops the lookup. Missing variables return zero. Cycles are
    /// rejected instead of reproducing an unbounded native parent walk.
    pub fn logic_variable(
        &self,
        area: CObjectId,
        parent: CObjectId,
        variable: CObjectId,
    ) -> Result<i32> {
        if let Some(value) = self.value_in_area(area, variable) {
            return Ok(value);
        }
        let mut next = parent;
        let mut visited = BTreeSet::new();
        while let Some(loaded) = self.loaded_areas.iter().find(|a| a.id == next) {
            ensure!(visited.insert(next), "Cyclic load-unit environment parent chain");
            if let Some(value) = self.value_in_area(next, variable) {
                return Ok(value);
            }
            next = loaded.parent;
        }
        Ok(0)
    }

    fn state_value_or_insert(&mut self, area: CObjectId, variable: CObjectId, default: i32) -> i32 {
        let index = self.area_states.iter().position(|a| a.id == area).unwrap_or_else(|| {
            self.area_states.push(AreaEnvironment { id: area, variables: Vec::new() });
            self.area_states.len() - 1
        });
        let variables = &mut self.area_states[index].variables;
        if let Some(value) = variables.iter().find(|v| v.id == variable) {
            value.value
        } else {
            variables.push(EnvironmentValue { id: variable, value: default });
            default
        }
    }

    /// EvaluateLoadUnits71003fb2d8. Add missing metadata variables only when the
    /// area's state already exists; then evaluate units in metadata order.
    /// Missing unit-state variables are inserted with authored bit 0 as default.
    /// Both result lists are appended to. This selects resource requests; layer
    /// readiness, activation, callbacks and scene registration are separate.
    pub fn append_initial_load_units(
        &mut self,
        area: CObjectId,
        parent: CObjectId,
        metadata: &SGameAreaMetaData,
        output: &mut InitialLoadUnits,
    ) -> Result<()> {
        if let Some(state) = self.area_states.iter_mut().find(|a| a.id == area) {
            for v in &metadata.environment_vars {
                if !state.variables.iter().any(|existing| existing.id == v.id) {
                    state.variables.push(EnvironmentValue { id: v.id, value: v.initial_value });
                }
            }
        }
        for unit in &metadata.load_units {
            let state_value = load_unit_state_variable(unit.id, unit.state_variable_id)
                .map(|id| self.state_value_or_insert(area, id, i32::from(unit.flags & 1)))
                .unwrap_or(1);
            let condition = if unit.data.is_empty() {
                None
            } else {
                Some(
                    LoadUnitLogic::parse(&unit.data)?
                        .evaluate(|id| self.logic_variable(area, parent, id))?
                        .value,
                )
            };
            let flags = evaluated_load_unit_flags(unit.flags, state_value, condition);
            if flags & 3 == 3 {
                output.requested.push(unit.id);
            }
            output.overrides.push(LoadUnitFlagOverride { id: unit.id, flags });
        }
        Ok(())
    }
}

/// NGameArea::should_layer_be_loaded71003df970. An existing layer with no unit
/// membership is eligible immediately. Other layers require at least one unit
/// in state 2 (loaded), not merely a nonzero state. First matching entries win.
pub fn should_layer_be_loaded<'a>(
    id: CObjectId,
    layers: impl IntoIterator<Item = &'a LayerHeader>,
    units: &[LoadUnitState],
) -> bool {
    layers.into_iter().find(|l| l.id == id).is_some_and(|l| {
        l.load_unit_ids.is_empty()
            || l.load_unit_ids
                .iter()
                .any(|id| units.iter().find(|u| u.id == *id).is_some_and(|u| u.load_state == 2))
    })
}

/// FilterLoadUnitsForGameMode71003de78c appends matching units whose runtime
/// flag bit 1 is set. Manager order and duplicates are retained. Repeated request
/// IDs do not duplicate a manager entry. Existing output is not cleared.
pub fn append_load_units_for_game_mode(
    units: &[LoadUnitState],
    requested: &[CObjectId],
    output: &mut Vec<CObjectId>,
) {
    output.extend(
        units.iter().filter(|u| u.flags & 2 != 0 && requested.contains(&u.id)).map(|u| u.id),
    );
}

/// GetAllLayersForLoadUnits71003db5a8: one result per matching manager entry,
/// in manager order. An empty LHED membership list never matches, including
/// a request for the nil GUID. This is not the startup-layer selection rule.
pub fn layers_for_load_units<'a>(
    layers: impl IntoIterator<Item = &'a LayerHeader>,
    requested: &[CObjectId],
) -> Vec<CObjectId> {
    layers
        .into_iter()
        .filter(|l| l.load_unit_ids.iter().any(|id| requested.contains(id)))
        .map(|l| l.id)
        .collect()
}

/// GetLayersForReloadSets71003db334 uses literal GUID equality; a nil request
/// can match nil reload-set IDs. Preserve manager order and duplicate entries.
pub fn layers_for_reload_sets<'a>(
    layers: impl IntoIterator<Item = &'a LayerHeader>,
    requested: &[CObjectId],
) -> Vec<CObjectId> {
    layers.into_iter().filter(|l| requested.contains(&l.reload_set_id)).map(|l| l.id).collect()
}

/// GetLayersToUnloadForUnloadingLoadUnits71003db890 selects layers referenced
/// by a requested unit when no other referenced unit has a nonzero load state.
/// Missing units count as unloaded. The first matching runtime unit wins.
pub fn layers_to_unload<'a>(
    layers: impl IntoIterator<Item = &'a LayerHeader>,
    units: &[LoadUnitState],
    requested: &[CObjectId],
) -> Vec<CObjectId> {
    layers
        .into_iter()
        .filter(|l| {
            l.load_unit_ids.iter().any(|id| requested.contains(id))
                && !l.load_unit_ids.iter().any(|id| {
                    !requested.contains(id)
                        && units.iter().find(|u| u.id == *id).is_some_and(|u| u.load_state != 0)
                })
        })
        .map(|l| l.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use binrw::BinWrite;
    use zerocopy::LittleEndian;

    use super::*;

    #[test]
    fn load_unit_parser_preserves_opaque_data_and_checks_envelopes() {
        let header = LoadUnitHeader {
            flags: 0xffff,
            data: vec![3, 0, 1, 0, 0xff],
            skipped_data: vec![0, 1, 0x80, 0xff],
            ..Default::default()
        };
        let mut encoded = Cursor::new(Vec::new());
        header.write_le(&mut encoded).unwrap();
        fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut b = id.to_vec();
            b.extend((payload.len() as u64).to_le_bytes());
            b.extend([0; 12]);
            b.extend(payload);
            b
        }
        let h = chunk(b"LUHD", &encoded.into_inner());
        let r = chunk(b"LRES", &[0; 4]);
        let l = chunk(b"LLYR", &[0; 4]);
        let valid = [h.clone(), r.clone(), l.clone()].concat();
        let parsed = LoadUnit::slice_data::<LittleEndian>(&valid).unwrap();
        assert_eq!(parsed.header.flags, 0xffff);
        assert_eq!(parsed.header.data, header.data);
        assert_eq!(parsed.header.skipped_data, header.skipped_data);
        for bad in [
            [h.clone(), r.clone()].concat(),
            [valid.clone(), h.clone()].concat(),
            [h, r, chunk(b"LLYR", &[0; 5])].concat(),
            valid[..valid.len() - 1].to_vec(),
        ] {
            assert!(LoadUnit::slice_data::<LittleEndian>(&bad).is_err());
        }
    }

    #[test]
    fn selection_preserves_manager_order_and_shared_layer_residency() {
        let id = |n| CObjectId::from(uuid::Uuid::from_u128(n));
        let layer = |n, units| LayerHeader {
            name: String::new(),
            id: id(n),
            entity_count: 0,
            generated_object_count: 0,
            load_unit_ids: units,
            reload_set_id: id(0),
            reset_on_activation: 0,
        };
        let layers = [layer(9, vec![id(1), id(2), id(1)]), layer(8, vec![]), layer(7, vec![id(1)])];
        let mut units = [LoadUnitState { id: id(2), flags: 2, load_state: 4 }, LoadUnitState {
            id: id(1),
            flags: 1,
            load_state: 0,
        }];
        assert!(!should_layer_be_loaded(id(9), &layers, &units));
        assert!(should_layer_be_loaded(id(8), &layers, &units));
        units[0].load_state = 2;
        assert!(should_layer_be_loaded(id(9), &layers, &units));
        units[0].load_state = 4;
        assert_eq!(layers_for_load_units(&layers, &[id(1), id(1)]), [id(9), id(7)]);
        assert!(layers_for_load_units(&layers, &[id(0)]).is_empty());
        assert_eq!(layers_for_reload_sets(&layers, &[id(0)]), [id(9), id(8), id(7)]);
        assert_eq!(layers_to_unload(&layers, &units, &[id(1)]), [id(7)]);
        units[0].load_state = 0;
        assert_eq!(layers_to_unload(&layers, &units, &[id(1)]), [id(9), id(7)]);
        let mut out = vec![id(99)];
        append_load_units_for_game_mode(&units, &[id(1), id(2), id(2)], &mut out);
        assert_eq!(out, [id(99), id(2)]);
    }

    #[test]
    fn stored_zero_stops_parent_lookup_and_unloaded_parents_stop_search() {
        let id = |n| CObjectId::from(uuid::Uuid::from_u128(n));
        let mut scene = LoadUnitEnvironment {
            area_states: vec![
                AreaEnvironment {
                    id: id(1),
                    variables: vec![EnvironmentValue { id: id(9), value: 0 }],
                },
                AreaEnvironment {
                    id: id(2),
                    variables: vec![EnvironmentValue { id: id(9), value: 1 }],
                },
            ],
            loaded_areas: vec![LoadedEnvironmentArea { id: id(2), parent: id(0) }],
        };
        assert_eq!(scene.logic_variable(id(1), id(2), id(9)).unwrap(), 0);
        scene.area_states[0].variables.clear();
        assert_eq!(scene.logic_variable(id(1), id(2), id(9)).unwrap(), 1);
        scene.loaded_areas.clear();
        assert_eq!(scene.logic_variable(id(1), id(2), id(9)).unwrap(), 0);
    }

    #[test]
    fn initial_requests_preserve_saved_state_and_append_overrides() {
        use crate::format::{
            load_unit_logic::LoadUnitLogicToken,
            room::{SEnvironmentVar, SScriptLoadUnitMetaData},
        };
        let id = |n| CObjectId::from(uuid::Uuid::from_u128(n));
        let mut raw = vec![0; 71];
        raw[0] = 25;
        let mut metadata: SGameAreaMetaData = Cursor::new(raw).read_le().unwrap();
        metadata.environment_vars =
            vec![SEnvironmentVar { id: id(2), initial_value: 0, ..Default::default() }];
        let condition = LoadUnitLogic {
            tokens: vec![
                LoadUnitLogicToken::Operator(0),
                LoadUnitLogicToken::Integer(0),
                LoadUnitLogicToken::Integer(1),
            ],
            ..Default::default()
        }
        .to_bytes()
        .unwrap();
        metadata.load_units = vec![SScriptLoadUnitMetaData {
            id: id(3),
            state_variable_id: id(2),
            flags: 0x8000,
            data: condition,
        }];
        let mut scene = LoadUnitEnvironment {
            area_states: vec![AreaEnvironment {
                id: id(1),
                variables: vec![EnvironmentValue { id: id(2), value: 2 }],
            }],
            ..Default::default()
        };
        let mut output = InitialLoadUnits {
            requested: vec![id(99)],
            overrides: vec![LoadUnitFlagOverride { id: id(3), flags: 17 }],
        };
        scene.append_initial_load_units(id(1), id(0), &metadata, &mut output).unwrap();
        assert_eq!(output.requested, [id(99), id(3)]);
        assert_eq!(output.overrides[1].flags, 0x8003);
        assert_eq!(effective_load_unit_flags(id(3), 0, &output.overrides), 17);
        assert_eq!(scene.area_states[0].variables[0].value, 2);
    }
}
