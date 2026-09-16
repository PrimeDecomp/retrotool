//! ROOM performance-group records and requests issued to the layer manager.
//!
//! A request batch is a call boundary, not proof that its layers are loaded or
//! active. The scene lifecycle must apply the requests and run the resulting
//! construction, Entity notifications and registration in native order.

use std::io::Cursor;

use anyhow::{ensure, Result};
use binrw::{binrw, BinReaderExt};

use crate::format::{CObjectId, CStringFixed};

/// CScriptPerformanceGroupResourceData710047ac6c. The native loader skips the
/// name and normalizes active to bool; retain both raw fields for round-trips.
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct PerformanceGroupResource {
    pub name: CStringFixed,
    /// Lookup key used by Activate/DeactivatePerformanceGroup. Nil is valid.
    pub id: CObjectId,
    pub active: u8,
    #[br(temp)]
    #[bw(try_calc = layer_ids.len().try_into())]
    count: u16,
    #[br(count = count)]
    pub layer_ids: Vec<CObjectId>,
}

/// PGRP chunk contents. Counts are u16, including each group's member count.
#[binrw]
#[derive(Clone, Debug, Default)]
pub struct PerformanceGroups {
    #[br(temp)]
    #[bw(try_calc = groups.len().try_into())]
    count: u16,
    #[br(count = count)]
    pub groups: Vec<PerformanceGroupResource>,
}

impl PerformanceGroups {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(data);
        let result = reader.read_le()?;
        ensure!(reader.position() == data.len() as u64, "Trailing PGRP bytes");
        Ok(result)
    }
}

#[derive(Clone, Debug)]
pub struct PerformanceGroupState<'a> {
    pub resource: &'a PerformanceGroupResource,
    pub active: bool,
}

/// One AddOnRequestToLayers (active=true) or AddOffRequestToLayers call. Preserve
/// order and duplicates, including an empty list: AddOn also clears its output
/// list of newly constructed Entity IDs even when no members are supplied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PerformanceGroupRequest<'a> {
    pub active: bool,
    pub layer_ids: &'a [CObjectId],
}

#[derive(Clone, Debug)]
pub struct PerformanceGroupManager<'a> {
    pub groups: Vec<PerformanceGroupState<'a>>,
}

impl<'a> PerformanceGroupManager<'a> {
    /// InitializeFromResourceData71003da500; constructor710047afe4.
    pub fn new(resources: &'a [PerformanceGroupResource]) -> Self {
        Self {
            groups: resources
                .iter()
                .map(|resource| PerformanceGroupState { resource, active: resource.active != 0 })
                .collect(),
        }
    }

    /// InitializeLayerStateFromPerformanceGroups71003da70c issues a separate on
    /// request for every active group in manager order. It does not change group
    /// state or guard against a repeated initialization call.
    pub fn initial_layer_requests(&self) -> impl Iterator<Item = PerformanceGroupRequest<'a>> + '_ {
        self.groups
            .iter()
            .filter(|g| g.active)
            .map(|g| PerformanceGroupRequest { active: true, layer_ids: &g.resource.layer_ids })
    }

    /// Activate71003da7a8 / Deactivate71003da890 use the first matching group,
    /// including nil. Missing IDs and an unchanged state produce no call. State
    /// changes before the layer-manager call; member IDs are passed unchanged.
    pub fn set_active(
        &mut self,
        id: CObjectId,
        active: bool,
    ) -> Option<PerformanceGroupRequest<'a>> {
        let group = self.groups.iter_mut().find(|g| g.resource.id == id)?;
        if group.active == active {
            return None;
        }
        group.active = active;
        Some(PerformanceGroupRequest { active, layer_ids: &group.resource.layer_ids })
    }
}

#[cfg(test)]
mod tests {
    use binrw::BinWrite;

    use super::*;

    #[test]
    fn raw_flags_and_names_round_trip_with_bounded_framing() {
        let data = PerformanceGroups {
            groups: vec![PerformanceGroupResource {
                name: CStringFixed { text: vec![0xff, 0, b'g'] },
                active: 0x80,
                layer_ids: vec![CObjectId::default(), CObjectId::default()],
                ..Default::default()
            }],
        };
        let mut encoded = Cursor::new(Vec::new());
        data.write_le(&mut encoded).unwrap();
        let mut bytes = encoded.into_inner();
        let decoded = PerformanceGroups::parse(&bytes).unwrap();
        assert_eq!(decoded.groups[0].name.text, [0xff, 0, b'g']);
        assert_eq!(decoded.groups[0].active, 0x80);
        let mut output = Cursor::new(Vec::new());
        decoded.write_le(&mut output).unwrap();
        assert_eq!(output.into_inner(), bytes);
        assert!(PerformanceGroups::parse(&bytes[..bytes.len() - 1]).is_err());
        bytes.push(0);
        assert!(PerformanceGroups::parse(&bytes).is_err());
    }

    #[test]
    fn first_group_wins_and_initialization_preserves_each_batch() {
        let id = CObjectId::default();
        let resources = [
            PerformanceGroupResource { active: 255, layer_ids: vec![id, id], ..Default::default() },
            PerformanceGroupResource { active: 1, ..Default::default() },
            PerformanceGroupResource::default(),
        ];
        let mut manager = PerformanceGroupManager::new(&resources);
        let first = manager.initial_layer_requests().collect::<Vec<_>>();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].layer_ids, [id, id]);
        assert!(first[1].layer_ids.is_empty());
        assert_eq!(manager.initial_layer_requests().collect::<Vec<_>>(), first);
        assert!(manager.set_active(id, true).is_none());
        assert_eq!(manager.set_active(id, false).unwrap().layer_ids, [id, id]);
        assert!(!manager.groups[0].active);
        assert!(manager.groups[1].active);
        assert!(manager.set_active(id, false).is_none());
        assert_eq!(manager.initial_layer_requests().count(), 1);
        assert!(manager.set_active(id, true).unwrap().active);
    }
}
