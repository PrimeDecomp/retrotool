use std::{
    fmt::Debug,
    io::{Cursor, Read, Seek},
    marker::PhantomData,
    path::Path,
};

use anyhow::{anyhow, bail, ensure, Result};
use binrw::{binrw, BinReaderExt, Endian};
use zerocopy::ByteOrder;

use crate::{
    format::{
        rfrm::FormDescriptor, script_load_units::LoadUnit,
        script_performance_groups::PerformanceGroups, slice_chunks, CAABox, CColor4f, CObjectId,
        CStringFixed, CVector3f, CVector4f, FourCC, TaggedVec,
    },
    util::templates::{
        load_templates, EnumTemplate, HexU32, PropertyListTemplate, PropertyTemplateType,
        StructTemplate, TemplateDatabase, TypeTemplate, TypeTemplateType, TypedefProperty,
    },
};

// Room
pub const K_FORM_ROOM: FourCC = FourCC(*b"ROOM");

// Header
pub const K_FORM_HEAD: FourCC = FourCC(*b"HEAD");
// Room header
pub const K_CHUNK_RMHD: FourCC = FourCC(*b"RMHD");
// Performance group data
pub const K_CHUNK_PGRP: FourCC = FourCC(*b"PGRP");
// Generated object map
pub const K_CHUNK_LGEN: FourCC = FourCC(*b"LGEN");
// Docks
pub const K_CHUNK_DOCK: FourCC = FourCC(*b"DOCK");
// Baked lighting
pub const K_CHUNK_BLIT: FourCC = FourCC(*b"BLIT");
// Load unit count
pub const K_CHUNK_LUNS: FourCC = FourCC(*b"LUNS");
// Load unit
pub const K_FORM_LUNT: FourCC = FourCC(*b"LUNT");
// Load unit header
pub const K_CHUNK_LUHD: FourCC = FourCC(*b"LUHD");
// Load unit resource
pub const K_CHUNK_LRES: FourCC = FourCC(*b"LRES");
// Load unit layers
pub const K_CHUNK_LLYR: FourCC = FourCC(*b"LLYR");

// String pool
pub const K_CHUNK_STRP: FourCC = FourCC(*b"STRP");

// Script data
pub const K_FORM_SDTA: FourCC = FourCC(*b"SDTA");
// Script data header
pub const K_CHUNK_SDHR: FourCC = FourCC(*b"SDHR");
// Script data entity
pub const K_CHUNK_SDEN: FourCC = FourCC(*b"SDEN");
// Game object component instance data
pub const K_CHUNK_IDTA: FourCC = FourCC(*b"IDTA");

// Layers
pub const K_FORM_LYRS: FourCC = FourCC(*b"LYRS");
// Layer
pub const K_FORM_LAYR: FourCC = FourCC(*b"LAYR");
// Layer header
pub const K_CHUNK_LHED: FourCC = FourCC(*b"LHED");
// GSRP
pub const K_FORM_GSRP: FourCC = FourCC(*b"GSRP");
// SRIP
pub const K_FORM_SRIP: FourCC = FourCC(*b"SRIP");
// Game object component
pub const K_CHUNK_COMP: FourCC = FourCC(*b"COMP");
// A generated-object definition inside GSRP; not an automatically live entity.
pub const K_CHUNK_GGOB: FourCC = FourCC(*b"GGOB");

#[binrw]
#[derive(Clone, Debug)]
pub struct SGameAreaHeader {
    pub parent_room_id: CObjectId,
    /// NGameArea::ERoomType (0..9 in MPR); preserved as a raw value.
    pub room_type: u16,
    pub unk2: u16,
    pub unk3: u8,
    pub id_b: CObjectId,
    pub id_c: CObjectId,
    pub id_d: CObjectId,
    pub id_e: CObjectId,
    pub path_find_area_id: CObjectId,
    // TODO ProductionWorkStages
}

/// Version 25 ROOM resource metadata from the PAK META table, separate from
/// RMHD and UINF. Native get_meta_data_from_stream71003e0010 returns these
/// fields as NGameArea::SMetaData; the serialized bounds remain unmodified here.
#[binrw]
#[derive(Clone, Debug)]
#[brw(magic = 0x19u16)]
pub struct SGameAreaMetaData {
    pub room_type: u16,
    /// Observed values: MPR1, MPRT and DFLT.
    pub game: FourCC,
    pub parent_room_id: CObjectId,
    pub unk1: u16,
    /// Native converts each byte to a bool; preserve original bytes on disk.
    pub flags: [u8; 3],
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub persistence_states: Vec<SGameAreaPersistenceState>,
    pub bounds: CAABox,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub load_units: Vec<SScriptLoadUnitMetaData>,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub environment_vars: Vec<SEnvironmentVar>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub unknown_records: Vec<SGameAreaMetaDataUnknownRecord>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub data: Vec<u8>,
}

impl SGameAreaMetaData {
    /// Bounds copied into CUniverseInfo::CAreaInfo+0x40 and later accumulated
    /// for baked lighting. The native loader discards abs(volume) < 1e-5, using
    /// two successive f32 multiplies and the engine's +/-1e12 inverted box.
    /// This is an individual area's input, not a zone's union of loaded areas.
    pub fn lighting_bounds(&self) -> CAABox {
        let width = self.bounds.max.x - self.bounds.min.x;
        let height = self.bounds.max.y - self.bounds.min.y;
        let depth = self.bounds.max.z - self.bounds.min.z;
        if ((width * height) * depth).abs() < 1e-5 {
            CAABox { min: CVector3f::splat(1e12), max: CVector3f::splat(-1e12) }
        } else {
            self.bounds
        }
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn metadata_bounds_threshold_matches_native_loader() {
        // Synthetic version25 metadata with empty counted collections. Native
        // full-loader fixtures confirm the strict threshold and inverted bits.
        let mut bytes = vec![0; 71];
        bytes[0] = 25;
        bytes[4..8].copy_from_slice(b"MPR1");
        let threshold = 1e-5_f32.to_bits();
        for (depth_bits, discarded) in [(threshold - 1, true), (threshold, false), (0, true)] {
            let bounds = [0.0, 0.0, 0.0, 1.0, 1.0, f32::from_bits(depth_bits)];
            for (word, value) in bytes[33..57].chunks_exact_mut(4).zip(bounds) {
                word.copy_from_slice(&value.to_le_bytes());
            }
            let metadata: SGameAreaMetaData = Cursor::new(&bytes).read_le().unwrap();
            let bounds = metadata.lighting_bounds();
            if discarded {
                assert_eq!(bounds.min.to_array().map(f32::to_bits), [0x5368d4a5; 3]);
                assert_eq!(bounds.max.to_array().map(f32::to_bits), [0xd368d4a5; 3]);
            } else {
                assert_eq!(bounds.max.z.to_bits(), depth_bits);
            }
        }
        bytes[0] = 24;
        assert!(Cursor::new(bytes).read_le::<SGameAreaMetaData>().is_err());
    }
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SGameAreaPersistenceState {
    pub id: CObjectId,
    pub name: CStringFixed,
    pub id2: CObjectId,
    pub id3: CObjectId,
    pub flags: [u8; 2],
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub environment_vars: Vec<SGameAreaPersistenceStateEnvVar>,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SGameAreaPersistenceStateEnvVar {
    pub name: CStringFixed,
    pub value: u32,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SScriptLoadUnitMetaData {
    pub id: CObjectId,
    /// Persisted load-unit environment key; nil falls back to id.
    pub state_variable_id: CObjectId,
    /// Authored flags, matching LUHD. Runtime overrides are separate.
    pub flags: u16,
    /// Empty or a serialized LoadUnitLogic prefix condition.
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub data: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SEnvironmentVar {
    pub id: CObjectId,
    pub name: CStringFixed,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub data: Vec<u8>,
    /// Copied to the native current (+2c) and default (+30) signed values.
    pub initial_value: i32,
    pub flags: u32,
    #[br(if(flags & 1 != 0))]
    pub flag0_value: Option<u32>,
    #[br(if(flags & 2 != 0))]
    pub flag1_value: Option<u32>,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SGameAreaMetaDataUnknownRecord {
    pub id: CObjectId,
    pub unk: u32,
    pub name: CStringFixed,
}

#[binrw]
#[derive(Copy, Clone, Debug, Default)]
/// Atlas UV offset in x/y and uniform scale in z. The fourth word is preserved;
/// native SAtlasLookup::Transform does not use it.
pub struct SAtlasLookup(pub CVector4f);

impl SAtlasLookup {
    pub fn is_valid(&self) -> bool { self.0.z >= 0.0 }

    pub fn transform(&self, uv: [f32; 2]) -> [f32; 2] {
        [uv[0] * self.0.z + self.0.x, uv[1] * self.0.z + self.0.y]
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLightingLightMap {
    pub txtr_id: CObjectId,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub ids: Vec<CObjectId>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub atlas_lookups: Vec<SAtlasLookup>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLightingLightProbe {
    pub ltpb_id: CObjectId,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct BakedLighting {
    #[bw(calc = if light_map.is_some() { 1 } else { 0 } | if light_probe.is_some() { 2 } else { 0 })]
    pub flags: u32,
    #[br(if(flags & 1 != 0))]
    pub light_map: Option<BakedLightingLightMap>,
    #[br(if(flags & 2 != 0))]
    pub light_probe: Option<BakedLightingLightProbe>,
}

#[binrw]
#[derive(Clone, Debug)]
// name?
pub struct ScriptDataPairs {
    a: u32,
    b: u32,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ScriptDataHeader {
    pub properties_count: u32,
    pub instance_data_count: u32,
    pub data_len: u32,
    #[br(count = data_len)]
    pub ids: Vec<CObjectId>,
    #[br(count = data_len)]
    pub pairs: Vec<ScriptDataPairs>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct ComponentProperties {
    pub component_type: u32,
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<u8>,
}

impl ComponentProperties {
    pub fn entity(&self) -> Result<Option<EntityProperties>> {
        if self.component_type == super::script_links::ENTITY_COMPONENT {
            EntityProperties::parse(&self.data).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn color_modifier(&self) -> Result<Option<super::room_color::ColorModifierProperties>> {
        if self.component_type == super::room_color::COLOR_MODIFIER_COMPONENT {
            super::room_color::ColorModifierProperties::parse(&self.data).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn room_settings(&self) -> Result<Option<super::room_lighting::RoomSettingsProperties>> {
        if self.component_type == super::room_lighting::ROOM_SETTINGS_COMPONENT {
            super::room_lighting::RoomSettingsProperties::parse(&self.data).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn room_controller(
        &self,
    ) -> Result<Option<super::room_lighting::RoomControllerProperties>> {
        if self.component_type == super::room_lighting::ROOM_CONTROLLER_COMPONENT {
            super::room_lighting::RoomControllerProperties::parse(&self.data).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Decode a LightStatic definition with native defaults. This does not
    /// resolve entity activation, layer visibility, transforms or baked lighting.
    pub fn static_light(&self) -> Result<Option<super::room_lighting::StaticLightProperties>> {
        if self.component_type == super::room_lighting::LIGHT_STATIC_COMPONENT {
            super::room_lighting::StaticLightProperties::parse(&self.data).map(Some)
        } else {
            Ok(None)
        }
    }
}

/// SLdrEntityProperties: two bytes and three vectors, without a property list.
/// Native Load71004697c8 normalizes both bytes; retain their exact wire values.
#[binrw]
#[derive(Clone, Debug)]
pub struct EntityProperties {
    pub active: u8,
    /// Copied through SGameObjectInfo+0x4c to Entity+0x110 bit4; meaning unresolved.
    pub unknown_flag: u8,
    pub position: CVector3f,
    pub rotation: CVector3f,
    pub scale: CVector3f,
}

impl EntityProperties {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() == 38, "Entity properties must contain exactly 38 bytes");
        Ok(Cursor::new(data).read_le()?)
    }

    /// LoadEntityGOC710046d320 ANDs the property with the generation/load request.
    /// This establishes own activity only; Create still applies owner/layer gates.
    pub fn starts_active(&self, requested_active: bool) -> bool {
        self.active != 0 && requested_active
    }
}

#[cfg(test)]
mod entity_property_tests {
    use binrw::BinWrite;

    use super::*;

    #[test]
    fn atomic_entity_record_preserves_bool_bytes_and_float_bits() {
        let mut raw = vec![2, 255];
        let words = [0x80000000u32, 0x7fc12345, 0, 0, 0, 0, 0x3f800000, 0x3f800000, 0x3f800000];
        for word in words {
            raw.extend_from_slice(&word.to_le_bytes());
        }
        let entity = EntityProperties::parse(&raw).unwrap();
        assert!(entity.starts_active(true));
        assert!(!entity.starts_active(false));
        assert_eq!(entity.position.y.to_bits(), 0x7fc12345);
        let mut written = Cursor::new(Vec::new());
        entity.write_le(&mut written).unwrap();
        assert_eq!(written.into_inner(), raw);
        assert!(EntityProperties::parse(&raw[..37]).is_err());
        raw.push(0);
        assert!(EntityProperties::parse(&raw).is_err());
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct PooledString {
    a: u32,
    b: u32,
    #[br(count = if a == u32::MAX { b } else { 0 })]
    bytes: Vec<u8>,
}

impl PooledString {
    pub fn get(&self, pool: Option<&StringPool>) -> Option<String> {
        if self.a == u32::MAX {
            String::from_utf8(self.bytes.clone()).ok()
        } else if let Some(data) =
            pool.and_then(|pool| pool.pool_data.get(self.a as usize..(self.a + self.b) as usize))
        {
            String::from_utf8(data.to_vec()).ok()
        } else {
            None
        }
    }
}

#[binrw]
#[derive(Clone, Debug, Default)]
/// Optional typedef payload shared by connections and links. The native loader
/// constructs the u16-sized typedef data and skips the u32-sized extra data.
/// Both byte strings are preserved; the historical name describes its size pass.
#[bw(assert(type_id != &0 || (typedef_data.is_empty() && extra_data.is_empty())))]
#[bw(assert(typedef_data.len() <= u16::MAX as usize && extra_data.len() <= u32::MAX as usize))]
pub struct SizeofAllocations {
    pub type_id: u32,
    #[br(if(type_id != 0), map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(if(*type_id != 0), map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub typedef_data: Vec<u8>,
    #[br(if(type_id != 0), map = |v: TaggedVec<u32, _>| v.data)]
    #[bw(if(*type_id != 0), map = |v| TaggedVec::<u32, _>::new(v.clone()))]
    pub extra_data: Vec<u8>,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SConnection {
    pub event: u32,
    pub action: u32,
    pub target: CObjectId,
    /// Stored at runtime+0x1c; meaning not yet established.
    pub unknown_u16: u16,
    pub event_criteria_sldr: SizeofAllocations,
    pub action_payload_sldr: SizeofAllocations,
    /// First two bytes become boolean bits0/1 at runtime+0x40. The third is
    /// consumed without storage by the native constructor.
    pub flag_bytes: [u8; 3],
    /// Consumed without storage by the native constructor; meaning unresolved.
    pub trailing_id: CObjectId,
}

impl SConnection {
    /// Base AreaEntitiesLoaded caches the first editor-ID alias when this flag
    /// is true and the current target cache is still ffffffff.
    pub fn cache_target_on_area_load(&self) -> bool { self.flag_bytes[0] != 0 }
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct SScriptLink {
    pub link: u32,
    pub target: CObjectId,
    pub data: SizeofAllocations,
    /// Become boolean bits0/1 at runtime+0x28. Preserve authored byte values.
    pub flag_bytes: [u8; 2],
    /// Consumed without storage by the native constructor; meaning unresolved.
    pub trailing_id: CObjectId,
}

impl SScriptLink {
    /// Base AreaEntitiesLoaded710046e024 uses the first serialized flag to cache
    /// an unresolved target. Existing caches, including stale ones, are retained.
    pub fn cache_target_on_area_load(&self) -> bool { self.flag_bytes[0] != 0 }

    /// Base CGameObjectComponent::Create removes links with a false second
    /// flag. Earlier constructors may still consume them; preserve them on disk.
    pub fn retained_after_component_create(&self) -> bool { self.flag_bytes[1] != 0 }
}

#[binrw]
#[derive(Clone, Debug)]
#[bw(assert(connections.len() <= u16::MAX as usize && links.len() <= u16::MAX as usize))]
pub struct SGOComponentInstanceData {
    pub id: CObjectId,
    pub string: PooledString,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub connections: Vec<SConnection>,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub links: Vec<SScriptLink>,
    /// Native SGOComponentInstanceData copies the remaining IDTA bytes to its
    /// trailing storage (pointer+0x78, size+0x80). Their interpretation varies.
    #[br(parse_with = binrw::until_eof)]
    pub trailing_data: Vec<u8>,
}

#[cfg(test)]
mod script_record_tests {
    use binrw::BinWriterExt;

    use super::*;

    #[test]
    fn idta_retains_link_fields_and_trailing_storage() {
        // Real serialized ColorModifier link; its original constructor was
        // executed separately. IDTA may retain bytes after its counted links.
        let link =
            "75badc1ecb627088b5c726429d7c702107c56edf000000000101838e4ef5f99c3a409b19132efb19895b";
        let mut raw = vec![0; 26]; // editor GUID, pooled string, connection count
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.extend(
            (0..link.len()).step_by(2).map(|i| u8::from_str_radix(&link[i..i + 2], 16).unwrap()),
        );
        let tail = [
            0x3f, 0xcc, 0x58, 0xf6, 0x8a, 0xca, 0x8e, 0x4f, 0xad, 0x43, 0xda, 0xc4, 0x75, 0xed,
            0x3b, 0x37,
        ];
        raw.extend_from_slice(&tail);
        let instance: SGOComponentInstanceData = Cursor::new(&raw).read_le().unwrap();
        assert!(instance.connections.is_empty());
        assert_eq!(instance.links.len(), 1);
        assert_eq!(instance.links[0].link, 0x1edcba75);
        assert_eq!(instance.links[0].target.to_string(), "887062cb-c7b5-4226-9d7c-702107c56edf");
        assert_eq!(instance.links[0].flag_bytes, [1, 1]);
        assert_eq!(instance.trailing_data, tail);
        let mut writer = Cursor::new(Vec::new());
        writer.write_le(&instance).unwrap();
        assert_eq!(writer.into_inner(), raw);
    }

    #[test]
    fn typedef_writer_rejects_payloads_that_cannot_be_encoded() {
        let mut writer = Cursor::new(Vec::new());
        let mut payload =
            SizeofAllocations { type_id: 0, typedef_data: vec![1], extra_data: vec![] };
        assert!(writer.write_le(&payload).is_err());
        payload.type_id = 1;
        payload.typedef_data.resize(65536, 0);
        assert!(writer.write_le(&payload).is_err());
    }

    #[test]
    fn layer_parser_retains_generated_definitions() {
        // A real three-component GGOB payload in a minimal valid layer wrapper.
        let raw = "bbc5d601f4392b03b50a455d1c11a8f50300a03ecae800000000000000009de815d50100000001000000f14997740200000002000000";
        let group: Vec<_> = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
            .collect();
        fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut out = id.to_vec();
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
            out.extend_from_slice(&[0; 12]);
            out.extend_from_slice(data);
            out
        }
        fn form(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut out = b"RFRM".to_vec();
            out.extend_from_slice(&(data.len() as u64).to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(id);
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(data);
            out
        }
        let mut header = vec![0; 43]; // empty name/IDs/dependencies; one GGOB
        header[22..24].copy_from_slice(&1u16.to_le_bytes());
        let mut layer = chunk(b"LHED", &header);
        layer.extend(form(b"GSRP", &chunk(b"GGOB", &group)));
        layer.extend(form(b"SRIP", &chunk(b"COMP", &[])));
        let layers =
            slice_layers::<zerocopy::LittleEndian>(&form(b"LAYR", &layer), Endian::Little).unwrap();
        assert_eq!(layers[0].header.generated_object_count, 1);
        assert!(layers[0].components.is_empty());
        assert_eq!(layers[0].generated_objects.len(), 1);
        let g = &layers[0].generated_objects[0];
        assert_eq!(g.components.len(), 3);
        assert_eq!(g.components[2].component_type, 0x749749f1);
        let mut writer = Cursor::new(Vec::new());
        writer.write_le(g).unwrap();
        assert_eq!(writer.into_inner(), group);
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct LayerHeader {
    #[br(try_map = CStringFixed::into_string)]
    #[bw(map = CStringFixed::from_string)]
    pub name: String,
    pub id: CObjectId,
    /// Number of ordinary Entity components; used to reserve the runtime list.
    pub entity_count: u16,
    /// Number of GGOB definitions in the following GSRP form.
    pub generated_object_count: u16,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    /// Load-unit membership used by CLayerManager::GetAllLayersForLoadUnits.
    pub load_unit_ids: Vec<CObjectId>,
    /// Compared by CLayerManager::GetLayersForReloadSets, including nil GUIDs.
    pub reload_set_id: CObjectId,
    /// CScriptLayer::EResetOnActivation; native normalizes this byte to a bool.
    pub reset_on_activation: u8,
}

#[binrw]
#[derive(Clone, Debug, Default)]
pub struct GameObjectComponent {
    pub component_type: u32,
    pub property_index: u32,
    pub instance_index: u32,
}

/// GGOB: a named group of component definitions available for later generation.
/// CScriptLayer::ParseGeneratedData retains the group ID and stream span.
#[binrw]
#[derive(Clone, Debug)]
#[bw(assert(components.len() <= u16::MAX as usize))]
pub struct GeneratedGameObject {
    pub id: CObjectId,
    #[br(map = |v: TaggedVec<u16, _>| v.data)]
    #[bw(map = |v| TaggedVec::<u16, _>::new(v.clone()))]
    pub components: Vec<GameObjectComponent>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct GameObjectComponents {
    #[br(parse_with = binrw::until_eof)]
    pub data: Vec<GameObjectComponent>,
}

#[binrw]
#[derive(Clone, Debug)]
pub struct StringPool {
    pub unk1: u32,
    pub num_strings: u32,
    #[br(if(num_strings > 0))]
    pub pool_len: u32,
    #[br(if(num_strings > 0), count = pool_len)]
    pub pool_data: Vec<u8>,
    // Unused StringPool struct
    pub unk2: u32,
    pub unk3: u32,
    pub unk_pool_len: u32,
    #[br(count = unk_pool_len)]
    pub unk_pool_data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RoomData<O: ByteOrder> {
    pub string_pool: Option<StringPool>,
    pub room_header: SGameAreaHeader,
    pub baked_lighting: BakedLighting,
    pub load_units: Vec<LoadUnit>,
    pub performance_groups: PerformanceGroups,
    pub component_properties: Vec<ComponentProperties>,
    pub constructed_properties: Vec<ConstructedProperty>,
    pub instance_data: Vec<SGOComponentInstanceData>,
    pub layers: Vec<Layer>,
    _marker: PhantomData<O>,
}

/// Property with an ID.
#[derive(Debug, Clone)]
pub struct ConstructedProperty {
    pub id: u32,
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub enum ConstructedPropertyValue {
    Unknown(Vec<u8>),
    Enum(Box<ConstructedEnumValue>),
    PropertyList(Box<ConstructedPropertyList>),
    Struct(Box<ConstructedStruct>),
    Typedef(Box<ConstructedTypedef>),
    List(Vec<ConstructedPropertyValue>),
    Id(CObjectId),
    Color(CColor4f),
    Vector(CVector3f),
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    String(String),
}

#[derive(Debug, Clone)]
pub struct ConstructedPropertyList {
    pub name: String,
    pub properties: Vec<ConstructedProperty>,
}

#[derive(Debug, Clone)]
pub struct ConstructedTypedef {
    pub id: u32,
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub struct ConstructedEnumValue {
    pub value: u32,
    pub enum_name: String,
    pub enum_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConstructedStruct {
    pub name: String,
    pub elements: Vec<ConstructedElement>,
}

/// Flat value for struct elements.
#[derive(Debug, Clone)]
pub struct ConstructedElement {
    pub name: Option<String>,
    pub value: ConstructedPropertyValue,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub header: LayerHeader,
    pub components: Vec<GameObjectComponent>,
    pub generated_objects: Vec<GeneratedGameObject>,
}

impl<O> RoomData<O>
where O: ByteOrder + 'static
{
    pub fn slice(data: &[u8]) -> Result<Self> {
        let (room_desc, room_data, _) = FormDescriptor::<O>::slice(data)?;
        ensure!(room_desc.id == K_FORM_ROOM);
        ensure!(room_desc.reader_version.get() == 147);
        ensure!(room_desc.writer_version.get() == 160);

        let mut string_pool: Option<StringPool> = None;
        let mut room_header: Option<SGameAreaHeader> = None;
        let mut baked_lighting: Option<BakedLighting> = None;
        let mut load_unit_count: Option<u16> = None;
        let mut load_units = Vec::new();
        let mut performance_groups = None;
        let mut component_properties: Vec<ComponentProperties> = vec![];
        let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
        let mut layers: Vec<Layer> = vec![];
        slice_chunks::<O, _, _>(
            room_data,
            |chunk, data| {
                let mut reader = Cursor::new(data);
                match chunk.id {
                    K_CHUNK_STRP => string_pool = Some(reader.read_type(Endian::Little)?),
                    id => bail!("Unknown ROOM chunk: {id:?}"),
                }
                Ok(())
            },
            |form, data| {
                match form.id {
                    K_FORM_HEAD => {
                        slice_chunks::<O, _, _>(
                            data,
                            |chunk, data| {
                                let mut reader = Cursor::new(data);
                                match chunk.id {
                                    K_CHUNK_RMHD => {
                                        room_header = Some(reader.read_type(Endian::Little)?)
                                    }
                                    K_CHUNK_BLIT => {
                                        baked_lighting = Some(reader.read_type(Endian::Little)?)
                                    }
                                    K_CHUNK_LUNS => {
                                        ensure!(load_unit_count.is_none(), "Duplicate LUNS chunk");
                                        ensure!(data.len() == 2, "Invalid LUNS size");
                                        load_unit_count = Some(reader.read_le()?);
                                    }
                                    K_CHUNK_PGRP => {
                                        ensure!(
                                            performance_groups.is_none(),
                                            "Duplicate PGRP chunk"
                                        );
                                        performance_groups = Some(PerformanceGroups::parse(data)?);
                                    }
                                    K_CHUNK_LGEN | K_CHUNK_DOCK => {
                                        // TODO
                                    }
                                    id => bail!("Unknown HEAD chunk: {id:?}"),
                                }
                                Ok(())
                            },
                            |form, data| {
                                match form.id {
                                    K_FORM_LUNT => {
                                        load_units.push(LoadUnit::slice_data::<O>(data)?);
                                    }
                                    id => bail!("Unknown HEAD form: {id:?}"),
                                }
                                Ok(())
                            },
                        )?;
                    }
                    K_FORM_SDTA => {
                        (component_properties, instance_data) =
                            slice_script_data::<O>(data, Endian::Little)?
                    }
                    K_FORM_LYRS => layers = slice_layers::<O>(data, Endian::Little)?,
                    id => bail!("Unknown ROOM form: {id:?}"),
                }
                Ok(())
            },
        )?;

        let load_unit_count = load_unit_count.ok_or_else(|| anyhow!("Missing LUNS chunk"))?;
        ensure!(load_units.len() == usize::from(load_unit_count), "LUNS/LUNT count mismatch");

        let db = match load_templates(Path::new("lib/templates/mp1r")) {
            Ok(db) => Some(db),
            Err(e) => {
                log::error!("Failed to load templates: {:?}", e);
                None
            }
        };

        let mut constructed_properties = Vec::with_capacity(component_properties.len());
        for props in &component_properties {
            let (name, type_tmpl) = db
                .as_ref()
                .map(|db| db.find_object(props.component_type))
                .map_or((None, None), |v| v);
            let mut reader = Cursor::new(&*props.data);
            let value = if let Some(type_tmpl) = type_tmpl {
                match parse_type(
                    &mut reader,
                    Endian::Little,
                    type_tmpl,
                    db.as_ref().unwrap(),
                    string_pool.as_ref(),
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("Failed to parse type {}: {:?}", type_tmpl.name, e);
                        ConstructedPropertyValue::Unknown(props.data.clone())
                    }
                }
            } else {
                ConstructedPropertyValue::Unknown(props.data.clone())
            };
            constructed_properties.push(ConstructedProperty {
                id: props.component_type,
                name: name.cloned(),
                value,
            });
        }

        let room_header = room_header.ok_or_else(|| anyhow!("Missing RMHD chunk"))?;
        let baked_lighting = baked_lighting.ok_or_else(|| anyhow!("Missing BLIT chunk"))?;
        Ok(Self {
            string_pool,
            room_header,
            baked_lighting,
            load_units,
            performance_groups: performance_groups.ok_or_else(|| anyhow!("Missing PGRP chunk"))?,
            component_properties,
            constructed_properties,
            instance_data,
            layers,
            _marker: PhantomData,
        })
    }
}

#[binrw]
#[derive(Clone, Debug)]
pub struct CDataEnumBitField {
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub ints: Vec<u32>,
    #[br(map = |v: TaggedVec<u32, _>| v.data)]
    pub bools: Vec<u8>,
    pub enum_id: CObjectId,
    pub unk_id: CObjectId,
}

fn parse_type<R>(
    reader: &mut R,
    e: Endian,
    tmpl: &TypeTemplate,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
) -> Result<ConstructedPropertyValue>
where
    R: Read + Seek,
{
    Ok(match &tmpl.template {
        TypeTemplateType::PropertyList(plist_tmpl) => {
            parse_property_list(reader, e, db, string_pool, tmpl, plist_tmpl)?
        }
        TypeTemplateType::Struct(struct_tmpl) => {
            parse_struct(reader, e, db, string_pool, tmpl, struct_tmpl)?
        }
        TypeTemplateType::Enum(enum_tmpl) => parse_enum(reader, e, db, tmpl, enum_tmpl)?,
    })
}

fn parse_property<R>(
    reader: &mut R,
    e: Endian,
    tmpl: &PropertyTemplateType,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
) -> Result<Option<ConstructedPropertyValue>>
where
    R: Read + Seek,
{
    Ok(match tmpl {
        PropertyTemplateType::Unknown => None,
        PropertyTemplateType::Enum(enum_prop) => match db.find_enum(&enum_prop.enum_name) {
            Some(tmpl) => match &tmpl.template {
                TypeTemplateType::Enum(enum_tmpl) => {
                    Some(parse_enum(reader, e, db, tmpl, enum_tmpl)?)
                }
                _ => {
                    log::warn!("Wrong type for enum template {}", enum_prop.enum_name);
                    None
                }
            },
            None => Some(ConstructedPropertyValue::Enum(Box::new(ConstructedEnumValue {
                value: reader.read_type::<u32>(e)?,
                enum_name: enum_prop.enum_name.clone(),
                enum_value: None,
            }))),
        },
        PropertyTemplateType::Struct(struct_prop) => {
            match db.find_struct(&struct_prop.struct_name) {
                Some(tmpl) => match &tmpl.template {
                    TypeTemplateType::PropertyList(plist_tmpl) => {
                        Some(parse_property_list(reader, e, db, string_pool, tmpl, plist_tmpl)?)
                    }
                    TypeTemplateType::Struct(struct_tmpl) => {
                        Some(parse_struct(reader, e, db, string_pool, tmpl, struct_tmpl)?)
                    }
                    _ => {
                        log::warn!("Wrong type for struct template {}", struct_prop.struct_name);
                        None
                    }
                },
                None => None,
            }
        }
        PropertyTemplateType::Typedef(typedef_prop) => {
            Some(parse_typedef_interface(reader, e, db, string_pool, typedef_prop)?)
        }
        PropertyTemplateType::List(list_prop) => {
            Some(ConstructedPropertyValue::List(parse_list(reader, e, |reader, e| {
                parse_property(reader, e, &list_prop.element, db, string_pool)
                    // TODO better error handling
                    .map(|v| v.unwrap_or(ConstructedPropertyValue::Unknown(vec![])))
            })?))
        }
        PropertyTemplateType::Id => Some(ConstructedPropertyValue::Id(reader.read_type(e)?)),
        PropertyTemplateType::Color => Some(ConstructedPropertyValue::Color(reader.read_type(e)?)),
        PropertyTemplateType::Vector => {
            Some(ConstructedPropertyValue::Vector(reader.read_type(e)?))
        }
        PropertyTemplateType::Bool => {
            let v = reader.read_type::<u8>(e)?;
            if v > 1 {
                Some(ConstructedPropertyValue::U8(v))
            } else {
                Some(ConstructedPropertyValue::Bool(v != 0))
            }
        }
        PropertyTemplateType::I8 => Some(ConstructedPropertyValue::I8(reader.read_type(e)?)),
        PropertyTemplateType::I16 => Some(ConstructedPropertyValue::I16(reader.read_type(e)?)),
        PropertyTemplateType::I32 => Some(ConstructedPropertyValue::I32(reader.read_type(e)?)),
        PropertyTemplateType::I64 => Some(ConstructedPropertyValue::I64(reader.read_type(e)?)),
        PropertyTemplateType::U8 => Some(ConstructedPropertyValue::U8(reader.read_type(e)?)),
        PropertyTemplateType::U16 => Some(ConstructedPropertyValue::U16(reader.read_type(e)?)),
        PropertyTemplateType::U32 => Some(ConstructedPropertyValue::U32(reader.read_type(e)?)),
        PropertyTemplateType::U64 => Some(ConstructedPropertyValue::U64(reader.read_type(e)?)),
        PropertyTemplateType::F32 => Some(ConstructedPropertyValue::F32(reader.read_type(e)?)),
        PropertyTemplateType::F64 => Some(ConstructedPropertyValue::F64(reader.read_type(e)?)),
        PropertyTemplateType::PooledString => {
            let ps: PooledString = reader.read_type(e)?;
            ps.get(string_pool).map(ConstructedPropertyValue::String)
        }
    })
}

fn parse_property_list<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    outer: &TypeTemplate,
    tmpl: &PropertyListTemplate,
) -> Result<ConstructedPropertyValue> {
    let num_properties = reader.read_type::<u16>(e)?;
    let mut properties = Vec::with_capacity(num_properties as usize);
    for _ in 0..num_properties {
        let id = reader.read_type::<u32>(e)?;
        let size = reader.read_type::<u16>(e)?;
        let mut data = vec![0; size as usize];
        reader.read_exact(&mut data)?;
        let mut inner = Cursor::new(&*data);
        let (name, value) = match tmpl.properties.get(&HexU32(id)) {
            Some(prop_tmpl) => (
                prop_tmpl.name.clone(),
                parse_property(&mut inner, e, &prop_tmpl.template, db, string_pool)?,
            ),
            None => (None, None),
        };
        let value = value.unwrap_or(ConstructedPropertyValue::Unknown(data));
        properties.push(ConstructedProperty { id, name, value });
    }
    Ok(ConstructedPropertyValue::PropertyList(Box::new(ConstructedPropertyList {
        name: outer.name.to_string(),
        properties,
    })))
}

fn parse_typedef_interface<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    prop: &TypedefProperty,
) -> Result<ConstructedPropertyValue> {
    let (id, size) = {
        let kind: u32 = reader.read_type(e)?;
        let size: u16 = reader.read_type(e)?;
        (kind, size)
    };
    let mut data = vec![0u8; size as usize];
    reader.read_exact(&mut data)?;
    let mut inner = Cursor::new(&*data);
    let (name, type_tmpl) = db.find_typedef(id);
    let name = name.cloned();
    let value = match type_tmpl {
        Some(type_tmpl) => {
            if !prop.supported_types.contains(&type_tmpl.name) {
                log::warn!("Unsupported typedef type: {:?}", type_tmpl.name);
            }
            parse_type(&mut inner, e, type_tmpl, db, string_pool)?
        }
        None => ConstructedPropertyValue::Unknown(data),
    };
    Ok(ConstructedPropertyValue::Typedef(Box::new(ConstructedTypedef { id, name, value })))
}

fn parse_struct<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    db: &TemplateDatabase,
    string_pool: Option<&StringPool>,
    outer: &TypeTemplate,
    tmpl: &StructTemplate,
) -> Result<ConstructedPropertyValue> {
    let mut elements = Vec::with_capacity(tmpl.elements.len());
    for prop_tmpl in &tmpl.elements {
        if let Some(elem) = parse_property(reader, e, &prop_tmpl.template, db, string_pool)? {
            elements.push(ConstructedElement { name: prop_tmpl.name.clone(), value: elem });
        }
    }
    Ok(ConstructedPropertyValue::Struct(Box::new(ConstructedStruct {
        name: outer.name.clone(),
        elements,
    })))
}

fn parse_enum<R>(
    reader: &mut R,
    e: Endian,
    _db: &TemplateDatabase,
    outer: &TypeTemplate,
    tmpl: &EnumTemplate,
) -> Result<ConstructedPropertyValue>
where
    R: Read + Seek,
{
    let value = reader.read_type::<u32>(e)?;
    let element = tmpl.values.iter().find(|v| value == v.value.0);
    let value = ConstructedEnumValue {
        value,
        enum_name: outer.name.clone(),
        enum_value: element.and_then(|e| e.name.clone()),
    };
    Ok(ConstructedPropertyValue::Enum(Box::new(value)))
}

#[inline]
fn parse_list<R, T, Cb>(reader: &mut R, e: Endian, mut cb: Cb) -> Result<Vec<T>>
where
    R: Read + Seek,
    Cb: FnMut(&mut R, Endian) -> Result<T>,
{
    let num_elements = reader.read_type::<u32>(e)?;
    let mut elements = Vec::with_capacity(num_elements as usize);
    for _ in 0..num_elements {
        elements.push(cb(reader, e)?);
    }
    Ok(elements)
}

fn slice_script_data<O>(
    data: &[u8],
    e: Endian,
) -> Result<(Vec<ComponentProperties>, Vec<SGOComponentInstanceData>)>
where
    O: ByteOrder + 'static,
{
    let mut sdhr: Option<ScriptDataHeader> = None;
    let mut component_properties: Vec<ComponentProperties> = vec![];
    let mut instance_data: Vec<SGOComponentInstanceData> = vec![];
    slice_chunks::<O, _, _>(
        data,
        |chunk, data| {
            match chunk.id {
                K_CHUNK_SDHR => {
                    sdhr = Some(Cursor::new(data).read_type(e)?);
                    // println!("SDHR: {sdhr:?}");
                }
                K_CHUNK_SDEN => {
                    let comp: ComponentProperties = Cursor::new(data).read_type(e)?;
                    // if let ComponentType::Unknown(value) = comp.component_type {
                    //     log::warn!("Unknown component type: {value:#X}");
                    // }
                    component_properties.push(comp);
                }
                K_CHUNK_IDTA => {
                    instance_data.push(Cursor::new(data).read_type(e)?);
                }
                id => bail!("Unknown SDTA chunk: {id:?}"),
            }
            Ok(())
        },
        |form, _data| bail!("Unknown SDTA form: {:?}", form.id),
    )?;
    let sdhr = sdhr.ok_or_else(|| anyhow!("Missing SDHR chunk"))?;
    ensure!(sdhr.properties_count as usize == component_properties.len());
    ensure!(sdhr.instance_data_count as usize == instance_data.len());
    Ok((component_properties, instance_data))
}

fn slice_layers<O>(data: &[u8], e: Endian) -> Result<Vec<Layer>>
where O: ByteOrder + 'static {
    let mut layers = vec![];
    slice_chunks::<O, _, _>(
        data,
        |chunk, _data| bail!("Unknown LYRS chunk: {:?}", chunk.id),
        |form, data| {
            match form.id {
                K_FORM_LAYR => {
                    let mut header: Option<LayerHeader> = None;
                    let mut components: Vec<GameObjectComponent> = vec![];
                    let mut generated_objects = Vec::new();
                    slice_chunks::<O, _, _>(
                        data,
                        |chunk, _data| {
                            match chunk.id {
                                K_CHUNK_LHED => {
                                    header = Some(Cursor::new(_data).read_type(e)?);
                                }
                                id => bail!("Unknown LAYR chunk: {id:?}"),
                            }
                            Ok(())
                        },
                        |form, data| {
                            match form.id {
                                K_FORM_GSRP => {
                                    slice_chunks::<O, _, _>(
                                        data,
                                        |chunk, bytes| {
                                            ensure!(
                                                chunk.id == K_CHUNK_GGOB,
                                                "Unknown GSRP chunk: {:?}",
                                                chunk.id
                                            );
                                            let mut reader = Cursor::new(bytes);
                                            let generated: GeneratedGameObject =
                                                reader.read_type(e)?;
                                            ensure!(
                                                reader.position() as usize == bytes.len(),
                                                "Trailing GGOB data"
                                            );
                                            generated_objects.push(generated);
                                            Ok(())
                                        },
                                        |form, _| bail!("Unknown GSRP form: {:?}", form.id),
                                    )?;
                                }
                                K_FORM_SRIP => {
                                    slice_chunks::<O, _, _>(
                                        data,
                                        |chunk, _data| {
                                            match chunk.id {
                                                K_CHUNK_COMP => {
                                                    let comp: GameObjectComponents =
                                                        Cursor::new(_data).read_type(e)?;
                                                    components = comp.data;
                                                }
                                                id => bail!("Unknown SRIP chunk: {id:?}"),
                                            }
                                            Ok(())
                                        },
                                        |form, _data| bail!("Unknown SRIP form: {:?}", form.id),
                                    )?;
                                }
                                id => bail!("Unknown LAYR form: {id:?}"),
                            }
                            Ok(())
                        },
                    )?;
                    let header = header.ok_or_else(|| anyhow!("Missing LHED chunk"))?;
                    ensure!(
                        generated_objects.len() == header.generated_object_count as usize,
                        "GSRP count differs from LHED"
                    );
                    layers.push(Layer { header, components, generated_objects });
                }
                id => bail!("Unknown LYRS form: {id:?}"),
            }
            Ok(())
        },
    )?;
    Ok(layers)
}
