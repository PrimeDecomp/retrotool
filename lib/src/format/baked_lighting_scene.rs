//! Scene context selection for MPR baked lighting, separate from probe decoding.
//!
//! The caller supplies the registered context slots and current primary pointer.
//! Volume bounds and modulation color follow different native selection rules.
//! These helpers do not infer loaded areas or scene groups. Tile registration
//! and mapping decisions are modeled separately in baked_lighting::residency.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};

use super::{baked_lighting::ProbeVolumeLayout, CAABox, CColor4f, CObjectId, CVector3f};

/// Effective runtime header and controller-list state of one CGameArea.
#[derive(Clone, Debug)]
pub struct BakedLightingAreaSnapshot {
    pub id: CObjectId,
    pub parent_room_id: CObjectId,
    pub room_type: u16,
    /// Current type4 small-list order, after activation/registration changes.
    /// Serialized RoomController definitions alone do not establish this list.
    pub resident_controller_rooms: Vec<CObjectId>,
}

pub struct BakedLightingAreaScene<'a> {
    /// Universe area records, using SGameAreaMetaData::lighting_bounds(). None
    /// means no active universe; an empty map is a present, empty universe.
    pub area_bounds: Option<&'a BTreeMap<CObjectId, CAABox>>,
    pub loaded_areas: &'a BTreeMap<CObjectId, BakedLightingAreaSnapshot>,
}

impl BakedLightingAreaScene<'_> {
    /// CGameArea::GetZoneLightProbeBounds71003f1248. Only loaded parents can
    /// redirect an area's bounds to a zone; UI rooms use their own metadata.
    /// Cyclic parent chains and nonfinite participating bounds are rejected.
    pub fn zone_bounds(&self, area: &BakedLightingAreaSnapshot) -> Result<CAABox> {
        let Some(bounds) = self.area_bounds else { return Ok(native_inverted_bounds()) };
        let mut zone = None;
        match area.room_type {
            1 => zone = Some(area),
            0 | 7 => {
                let mut parent_id = area.parent_room_id;
                let mut visited = BTreeSet::new();
                while let Some(parent) = self.loaded_areas.get(&parent_id) {
                    ensure!(visited.insert(parent_id), "Cyclic loaded area parent chain");
                    if parent.room_type == 1 {
                        zone = Some(parent);
                        break;
                    }
                    parent_id = parent.parent_room_id;
                }
            }
            3..=5 => {}
            _ => return Ok(native_inverted_bounds()),
        }
        if let Some(zone) = zone {
            let mut result = native_inverted_bounds();
            for room in &zone.resident_controller_rooms {
                if let Some(&bounds) = bounds.get(room) {
                    finite_bounds(bounds)?;
                    if bounds.min.to_array() != [1e12; 3] || bounds.max.to_array() != [-1e12; 3] {
                        // The native zone path excludes the exact inverted
                        // sentinel; it does not discard every reversed box.
                        accumulate_box(&mut result, bounds);
                    }
                }
            }
            Ok(result)
        } else {
            let result = bounds.get(&area.id).copied().unwrap_or_else(native_inverted_bounds);
            finite_bounds(result)?;
            Ok(result)
        }
    }
}

fn native_inverted_bounds() -> CAABox {
    CAABox { min: CVector3f::splat(1e12), max: CVector3f::splat(-1e12) }
}

fn finite_bounds(bounds: CAABox) -> Result<()> {
    ensure!(
        bounds.min.to_array().iter().chain(bounds.max.to_array().iter()).all(|v| v.is_finite()),
        "Nonfinite probe context bounds"
    );
    Ok(())
}

fn accumulate_box(accumulator: &mut CAABox, bounds: CAABox) {
    for point in [bounds.min, bounds.max] {
        let mut min = accumulator.min.to_array();
        let mut max = accumulator.max.to_array();
        for (axis, value) in point.to_array().into_iter().enumerate() {
            // Native subtract/compare/select preserves old bits for equal
            // minima and takes incoming bits for equal maxima.
            if !((value - min[axis]) >= 0.0) {
                min[axis] = value;
            }
            if (value - max[axis]) >= 0.0 {
                max[axis] = value;
            }
        }
        *accumulator = CAABox { min: min.into(), max: max.into() };
    }
}

/// Relevant fields from CBakedLightingContextSceneNode (0x98 bytes).
#[derive(Debug, Clone, Copy)]
pub struct BakedLightingContextSnapshot {
    pub group: u8,
    pub zone_bounds: CAABox,
    pub storage_bounds: CAABox,
    /// The storage's virtual GetWorldToStorage result, if it has probe storage.
    pub world_to_storage: Option<[[f32; 4]; 3]>,
    pub modulation_color: CColor4f,
}

/// Inputs consumed by CBakedLightingProbeTexture::Initialize after selection.
#[derive(Debug, Clone, Copy)]
pub struct BakedProbeVolumeInputs {
    pub bounds: CAABox,
    pub world_to_storage: [[f32; 4]; 3],
}

impl BakedProbeVolumeInputs {
    pub fn layout(&self) -> Result<ProbeVolumeLayout> {
        ProbeVolumeLayout::from_bounds(self.bounds, self.world_to_storage, 0.5)
    }
}

/// Optional developer override, evaluated before scene/area color overrides.
#[derive(Debug, Clone, Copy)]
pub struct BakedLightingDebugColor {
    pub rgb: [i32; 3],
    pub intensity: f32,
}

/// Current native registration order, including vacant slots after removals.
/// RegisterContextNode chooses the first flagged primary while none is set;
/// removing it clears that pointer without electing another existing context.
pub struct BakedLightingSceneInputs<'a> {
    pub primary: Option<&'a BakedLightingContextSnapshot>,
    pub contexts: &'a [Option<BakedLightingContextSnapshot>],
}

impl BakedLightingSceneInputs<'_> {
    /// InitializeProbeTextureFromStoredNodes71001c71b0. A primary with any valid
    /// bounds suppresses the fallback list. Otherwise all stored contexts join
    /// the bounds, in order. Each encountered probe storage replaces the matrix,
    /// even when that context contributes no valid bounds.
    pub fn probe_volume_inputs(&self) -> Result<Option<BakedProbeVolumeInputs>> {
        let mut inputs = BakedProbeVolumeInputs {
            // Native CAABox::skInvertedBox uses +/-1e12, not float extrema.
            bounds: native_inverted_bounds(),
            world_to_storage: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
        };
        if let Some(primary) = self.primary {
            inputs.accumulate(primary)?;
        }
        if invalid_bounds(inputs.bounds) {
            for context in self.contexts.iter().flatten() {
                inputs.accumulate(context)?;
            }
        }
        if invalid_bounds(inputs.bounds) {
            return Ok(None);
        }
        ensure!(
            inputs.world_to_storage.iter().flatten().all(|v| v.is_finite()),
            "Nonfinite selected probe storage transform"
        );
        Ok(Some(inputs))
    }

    /// UpdateLightModulationColor71001c6f4c: developer override, explicit scene
    /// override, first context matching the active group, primary, then white.
    /// Priority and bounds do not participate in this color selection.
    pub fn modulation_color(
        &self,
        active_group: Option<u8>,
        override_color: Option<CColor4f>,
        debug: Option<BakedLightingDebugColor>,
    ) -> CColor4f {
        if let Some(debug) = debug {
            // Native SCVTF -> FMUL -> FDIV; no clamping or alpha modulation.
            let rgb = debug.rgb.map(|v| (debug.intensity * v as f32) / 255.0);
            return CColor4f::new(rgb[0], rgb[1], rgb[2], 1.0);
        }
        if let Some(color) = override_color {
            return color;
        }
        if let Some(group) = active_group {
            if let Some(context) = self.contexts.iter().flatten().find(|c| c.group == group) {
                return context.modulation_color;
            }
        }
        self.primary.map_or(CColor4f::splat(1.0), |c| c.modulation_color)
    }
}

impl BakedProbeVolumeInputs {
    fn accumulate(&mut self, context: &BakedLightingContextSnapshot) -> Result<()> {
        if let Some(transform) = context.world_to_storage {
            self.world_to_storage = transform;
        }
        for bounds in [context.zone_bounds, context.storage_bounds] {
            finite_bounds(bounds)?;
            if invalid_bounds(bounds) {
                continue;
            }
            accumulate_box(&mut self.bounds, bounds);
        }
        Ok(())
    }
}

fn invalid_bounds(bounds: CAABox) -> bool {
    bounds.min.to_array().into_iter().zip(bounds.max.to_array()).any(|(min, max)| (max - min) < 0.0)
}

/// Area manager defaults and setters71003fc248/71003fc2a0. RoomSettings controls
/// primary; transient effects can control secondary. The proxy receives their
/// component-wise product, including alpha, using CColor4f::Modulatef3078.
#[derive(Debug, Clone, Copy)]
pub struct BakedLightingAreaColors {
    pub primary: CColor4f,
    pub secondary: CColor4f,
}

impl Default for BakedLightingAreaColors {
    fn default() -> Self { Self { primary: CColor4f::splat(1.0), secondary: CColor4f::splat(1.0) } }
}

impl BakedLightingAreaColors {
    pub fn modulation_color(&self) -> CColor4f {
        CColor4f::new(
            self.primary.r * self.secondary.r,
            self.primary.g * self.secondary.g,
            self.primary.b * self.secondary.b,
            self.primary.a * self.secondary.a,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(min: f32, max: f32, group: u8) -> BakedLightingContextSnapshot {
        BakedLightingContextSnapshot {
            group,
            zone_bounds: CAABox { min: CVector3f::splat(min), max: CVector3f::splat(max) },
            storage_bounds: CAABox::default(),
            world_to_storage: None,
            modulation_color: CColor4f::splat(group as f32),
        }
    }

    #[test]
    fn zone_bounds_use_loaded_parents_and_resident_controllers() {
        let id = |v| CObjectId::from(uuid::Uuid::from_u128(v));
        let area = BakedLightingAreaSnapshot {
            id: id(1),
            parent_room_id: id(2),
            room_type: 0,
            resident_controller_rooms: Vec::new(),
        };
        let zone = BakedLightingAreaSnapshot {
            id: id(2),
            parent_room_id: id(0),
            room_type: 1,
            resident_controller_rooms: vec![id(3), id(4), id(99)],
        };
        let metadata = BTreeMap::from([
            (id(1), context(-1.0, 1.0, 0).zone_bounds),
            // Non-sentinel reversed boxes still contribute both endpoints.
            (id(3), context(5.0, -5.0, 0).zone_bounds),
            (id(4), native_inverted_bounds()),
            (id(5), context(-100.0, 100.0, 0).zone_bounds),
        ]);
        let mut loaded = BTreeMap::new();
        let result = BakedLightingAreaScene { area_bounds: Some(&metadata), loaded_areas: &loaded }
            .zone_bounds(&area)
            .unwrap();
        assert_eq!(result.min.to_array(), [-1.0; 3]);
        loaded.insert(zone.id, zone);
        let scene = BakedLightingAreaScene { area_bounds: Some(&metadata), loaded_areas: &loaded };
        let result = scene.zone_bounds(&area).unwrap();
        assert_eq!(result.min.to_array(), [-5.0; 3]);
        assert_eq!(result.max.to_array(), [5.0; 3]);
        let ui = BakedLightingAreaSnapshot { room_type: 3, ..area.clone() };
        assert_eq!(scene.zone_bounds(&ui).unwrap().min.to_array(), [-1.0; 3]);
        let no_universe = BakedLightingAreaScene { area_bounds: None, loaded_areas: &loaded };
        assert_eq!(no_universe.zone_bounds(&area).unwrap().min.to_array(), [1e12; 3]);
    }

    #[test]
    fn cyclic_loaded_parents_are_rejected() {
        let id = CObjectId::default();
        let area = BakedLightingAreaSnapshot {
            id,
            parent_room_id: id,
            room_type: 7,
            resident_controller_rooms: Vec::new(),
        };
        let loaded = BTreeMap::from([(id, area.clone())]);
        let bounds = BTreeMap::new();
        let scene = BakedLightingAreaScene { area_bounds: Some(&bounds), loaded_areas: &loaded };
        assert!(scene.zone_bounds(&area).unwrap_err().to_string().contains("Cyclic"));
    }

    #[test]
    fn volume_primary_and_color_group_are_independent() {
        let primary = context(-1.0, 1.0, 2);
        let contexts = [None, Some(context(-20.0, 20.0, 7))];
        let scene = BakedLightingSceneInputs { primary: Some(&primary), contexts: &contexts };
        let volume = scene.probe_volume_inputs().unwrap().unwrap();
        assert_eq!(volume.bounds.min.to_array(), [-1.0; 3]);
        assert_eq!(volume.bounds.max.to_array(), [1.0; 3]);
        assert_eq!(scene.modulation_color(Some(7), None, None).to_array(), [7.0; 4]);
        assert_eq!(scene.modulation_color(Some(8), None, None).to_array(), [2.0; 4]);
    }

    #[test]
    fn fallback_retains_native_bounds_ties_and_last_storage() {
        let mut last = context(1.0, -1.0, 0);
        let transform = [[2.0, 0.0, 0.0, 3.0], [0.0, 2.0, 0.0, 4.0], [0.0, 0.0, 2.0, 5.0]];
        last.world_to_storage = Some(transform);
        let contexts = [Some(context(0.0, 0.0, 0)), Some(context(-0.0, -0.0, 0)), Some(last)];
        let result = BakedLightingSceneInputs { primary: None, contexts: &contexts }
            .probe_volume_inputs()
            .unwrap()
            .unwrap();
        assert_eq!(result.bounds.min.to_array().map(f32::to_bits), [0; 3]);
        assert_eq!(result.bounds.max.to_array().map(f32::to_bits), [0x80000000; 3]);
        assert_eq!(result.world_to_storage, transform);

        let large = context(2e12, 3e12, 0);
        let result = BakedLightingSceneInputs { primary: Some(&large), contexts: &[] }
            .probe_volume_inputs()
            .unwrap()
            .unwrap();
        assert_eq!(result.bounds.min.to_array(), [1e12; 3]);
        assert_eq!(result.bounds.max.to_array(), [3e12; 3]);
    }
}
