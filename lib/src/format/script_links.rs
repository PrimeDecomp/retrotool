//! Original MPR linked-component selection over an explicit runtime snapshot.
//!
//! ROOM definitions alone do not establish loaded objects, their versioned IDs,
//! editor-ID aliases, generated-instance ancestry, or owning areas. The caller
//! supplies those facts. This implements the null-predicate iterator used by
//! ColorModifier; event criteria, message queues and scene lifetime are separate.

use std::collections::{BTreeMap, HashSet};

use anyhow::{ensure, Context, Result};

use super::{
    room::SScriptLink, room_lighting::ROOM_SETTINGS_COMPONENT,
    script_activation::ComponentActivation, CObjectId,
};

pub const INVALID_UNIQUE_ID: u32 = u32::MAX;
pub const ENTITY_COMPONENT: u32 = 0x749749f1;
pub const ENTITY_COMPONENT_LINK: u32 = 0x434f4d50; // 'COMP'
pub const COLOR_MODIFIER_LINK: u32 = 0x1edcba75;

/// The target GUID and cached ID are SConnectionTarget. The serialized link
/// constructor initializes cached_unique_id to 0xffffffff, not zero.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeScriptLink {
    pub link: u32,
    pub target: CObjectId,
    pub cached_unique_id: u32,
}

impl From<&SScriptLink> for RuntimeScriptLink {
    fn from(value: &SScriptLink) -> Self {
        Self { link: value.link, target: value.target, cached_unique_id: INVALID_UNIQUE_ID }
    }
}

/// Initial serialized links retained by the base component Create callback.
/// Generated groups may first remap their cached IDs; when constructing that
/// state explicitly, prune the links after remapping instead of resetting caches.
pub fn created_script_links(links: &[SScriptLink]) -> Vec<RuntimeScriptLink> {
    links.iter().filter(|l| l.retained_after_component_create()).map(Into::into).collect()
}

/// Native Create preserves the component's own low four and high two bits,
/// inherits the owner's active bit into bit4 and sets layer-active bit5.
/// This does not advance registration state or run derived Create callbacks.
pub fn component_create_flags(component_flags: u8, owner_flags: u8, layer_active: bool) -> u8 {
    let mut state = ComponentActivation { flags: component_flags };
    state.create(owner_flags, layer_active);
    state.flags
}

/// NGOCUtils::remap_connections_to_uniqueIds searches the newly constructed
/// batch in order, including nil GUIDs. Mode1 replaces existing caches; all
/// other modes fill only ffffffff caches. A missing local target leaves the cache.
#[derive(Clone, Debug)]
pub struct GeneratedIdRemap {
    first_ids: BTreeMap<CObjectId, u32>,
    replace_cached: bool,
}

impl GeneratedIdRemap {
    pub fn new(objects: impl IntoIterator<Item = (CObjectId, u32)>, generation_mode: u32) -> Self {
        let mut first_ids = BTreeMap::new();
        for (guid, id) in objects {
            first_ids.entry(guid).or_insert(id);
        }
        Self { first_ids, replace_cached: generation_mode == 1 }
    }

    /// Shared rule for both serialized connection targets and script-link targets.
    pub fn target_unique_id(&self, guid: CObjectId, cached_unique_id: u32) -> u32 {
        if self.replace_cached || cached_unique_id == INVALID_UNIQUE_ID {
            self.first_ids.get(&guid).copied().unwrap_or(cached_unique_id)
        } else {
            cached_unique_id
        }
    }

    pub fn remap_links(&self, links: &mut [RuntimeScriptLink]) {
        for link in links {
            link.cached_unique_id = self.target_unique_id(link.target, link.cached_unique_id);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedEntityComponent {
    pub unique_id: u32,
    pub interface_type: u32,
    pub component_type: u32,
}

#[derive(Clone, Debug)]
pub struct ScriptComponentSnapshot {
    pub unique_id: u32,
    pub component_type: u32,
    pub interface_type: u32,
    /// ObjectById requires flags & 6 == 2. This is not the Think activation mask.
    pub flags: u8,
    /// Identifies the captured direct owner pointer, independently of slot lookup.
    pub owner_entity: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct ScriptEntitySnapshot {
    pub unique_id: u32,
    pub component_ids: Vec<u32>,
    /// Area reached through the entity's native +0x118 pointer, when known.
    pub area_unique_id: Option<u32>,
    /// Consumer names for generation metadata at +0x2c0/+0x2d0/+0x2d4.
    /// Editor property names and the generation lifecycle remain unresolved.
    pub scope_id: CObjectId,
    pub parent_unique_id: u32,
    pub instance_index: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakedColorWrite {
    pub room_settings_id: u32,
    pub area_unique_id: u32,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct ScriptLinkScene {
    slots: BTreeMap<u16, ScriptComponentSnapshot>,
    entities: BTreeMap<u32, ScriptEntitySnapshot>,
    aliases: BTreeMap<CObjectId, Vec<u32>>,
}

impl ScriptLinkScene {
    /// Base AreaEntitiesLoaded uses this rule for both connections and links.
    /// It caches the FIRST alias without validating its object slot or flags.
    /// Nil GUIDs can match explicit aliases; absent aliases return ffffffff.
    /// A previously cached target is never refreshed here, even if it is stale.
    pub fn area_loaded_target(
        &self,
        target: CObjectId,
        cached_unique_id: u32,
        cache_on_load: bool,
    ) -> u32 {
        if cache_on_load && cached_unique_id == INVALID_UNIQUE_ID {
            self.aliases
                .get(&target)
                .and_then(|ids| ids.first())
                .copied()
                .unwrap_or(INVALID_UNIQUE_ID)
        } else {
            cached_unique_id
        }
    }

    /// Later components replace earlier occupants of the same low-16-bit slot.
    /// Alias order for each GUID must match the runtime multimap's iteration order.
    /// Direct owner snapshots need not themselves be visible in the slot table.
    pub fn new(
        components: impl IntoIterator<Item = ScriptComponentSnapshot>,
        entities: impl IntoIterator<Item = ScriptEntitySnapshot>,
        aliases: impl IntoIterator<Item = (CObjectId, u32)>,
    ) -> Self {
        let mut scene = Self::default();
        for component in components {
            scene.slots.insert(component.unique_id as u16, component);
        }
        for entity in entities {
            scene.entities.insert(entity.unique_id, entity);
        }
        for (guid, id) in aliases {
            scene.aliases.entry(guid).or_default().push(id);
        }
        scene
    }

    pub fn component_by_id(&self, id: u32) -> Option<&ScriptComponentSnapshot> {
        self.component_by_id_including_limbo(id).filter(|c| c.flags & 6 == 2)
    }

    /// Native lookup during creation checks slot/version but not activation or
    /// registration flag bits. It can see components hidden from ordinary links.
    pub fn component_by_id_including_limbo(&self, id: u32) -> Option<&ScriptComponentSnapshot> {
        if id == INVALID_UNIQUE_ID {
            return None;
        }
        self.slots.get(&(id as u16)).filter(|c| c.unique_id == id)
    }

    /// CEntityGOC::CacheComponentVector, called after base Create prunes links.
    /// Each COMP link selects its cached ID or the FIRST alias for its GUID.
    /// A stale first alias is skipped without trying later aliases. This uses
    /// lookup including limbo and does not filter interfaces, flags or ancestry.
    pub fn entity_component_cache(
        &self,
        links: &[RuntimeScriptLink],
    ) -> Vec<CachedEntityComponent> {
        links
            .iter()
            .filter(|l| l.link == ENTITY_COMPONENT_LINK)
            .filter_map(|l| {
                let id = if l.cached_unique_id != INVALID_UNIQUE_ID {
                    l.cached_unique_id
                } else {
                    *self.aliases.get(&l.target)?.first()?
                };
                let component = self.component_by_id_including_limbo(id)?;
                Some(CachedEntityComponent {
                    unique_id: id,
                    interface_type: component.interface_type,
                    component_type: component.component_type,
                })
            })
            .collect()
    }

    fn owner(&self, component: &ScriptComponentSnapshot) -> Option<&ScriptEntitySnapshot> {
        self.entities.get(&component.owner_entity?)
    }

    fn owner_by_component_id(&self, id: u32) -> Option<&ScriptEntitySnapshot> {
        self.owner(self.component_by_id(id)?)
    }

    fn allows_communication(&self, source: &ScriptEntitySnapshot, target: u32) -> Result<bool> {
        if source.parent_unique_id == INVALID_UNIQUE_ID {
            return Ok(true);
        }
        let Some(target) = self.owner_by_component_id(target) else { return Ok(false) };
        if target.parent_unique_id == INVALID_UNIQUE_ID {
            return Ok(true);
        }
        let mut source = source;
        let mut source_seen = HashSet::new();
        while source.parent_unique_id != INVALID_UNIQUE_ID {
            ensure!(source_seen.insert(source.unique_id), "cyclic source generation ancestry");
            let mut candidate = target;
            let mut target_seen = HashSet::new();
            while candidate.parent_unique_id != INVALID_UNIQUE_ID {
                ensure!(
                    target_seen.insert(candidate.unique_id),
                    "cyclic target generation ancestry"
                );
                if source.scope_id == candidate.scope_id {
                    return Ok(source.parent_unique_id == candidate.parent_unique_id
                        && source.instance_index == candidate.instance_index);
                }
                let Some(parent) = self.owner_by_component_id(candidate.parent_unique_id) else {
                    break;
                };
                candidate = parent;
            }
            let Some(parent) = self.owner_by_component_id(source.parent_unique_id) else {
                break;
            };
            source = parent;
        }
        Ok(true)
    }

    /// Preserve authored link order, alias order, component order and duplicates.
    /// Link flags and trailing serialized GUIDs are not consulted by this native
    /// iterator. Communication filtering applies to base targets, not their children.
    /// Cyclic snapshots return an error instead of reproducing a native infinite loop.
    pub fn linked_components(
        &self,
        source_owner: u32,
        links: &[RuntimeScriptLink],
        link_type: u32,
        interface_type: u32,
    ) -> Result<Vec<u32>> {
        let source = self.entities.get(&source_owner).context("missing source owner snapshot")?;
        let mut out = Vec::new();
        for link in links.iter().filter(|l| l.link == link_type) {
            let cached;
            let targets = if link.cached_unique_id != INVALID_UNIQUE_ID {
                cached = [link.cached_unique_id];
                &cached[..]
            } else if link.target.is_nil() {
                &[]
            } else {
                self.aliases.get(&link.target).map(Vec::as_slice).unwrap_or_default()
            };
            for &target in targets {
                if !self.allows_communication(source, target)? {
                    continue;
                }
                let mut pending = std::slice::from_ref(&target).iter();
                let mut expanded = HashSet::new();
                while let Some(&id) = pending.next() {
                    let Some(component) = self.component_by_id(id) else { continue };
                    if component.interface_type == interface_type {
                        out.push(id);
                    } else if component.component_type == ENTITY_COMPONENT {
                        // Native keeps one child iterator. An entity child replaces
                        // it, dropping remaining siblings; there is no recursive stack.
                        ensure!(expanded.insert(id), "cyclic linked entity expansion");
                        pending = self
                            .owner(component)
                            .map(|e| e.component_ids.as_slice())
                            .unwrap_or_default()
                            .iter();
                    }
                }
            }
        }
        Ok(out)
    }

    /// Mode 1 of ApplyNewColor: select RoomSettings by interface, cast by actual
    /// component type, then reach each owning area. Returns ordered primary-color
    /// writes; the caller applies them to its live baked-lighting managers.
    pub fn baked_color_writes(
        &self,
        source_owner: u32,
        links: &[RuntimeScriptLink],
        color: [f32; 4],
    ) -> Result<Vec<BakedColorWrite>> {
        let mut writes = Vec::new();
        for id in self.linked_components(
            source_owner,
            links,
            COLOR_MODIFIER_LINK,
            ROOM_SETTINGS_COMPONENT,
        )? {
            let component = self.component_by_id(id).expect("selected from immutable snapshot");
            if component.component_type != ROOM_SETTINGS_COMPONENT {
                continue;
            }
            let area_unique_id = self
                .owner(component)
                .and_then(|e| e.area_unique_id)
                .context("linked RoomSettings has no captured owning area")?;
            writes.push(BakedColorWrite { room_settings_id: id, area_unique_id, color });
        }
        Ok(writes)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn area_loaded_cache_keeps_first_alias_without_validating_target() {
        let id: CObjectId = Uuid::nil().into();
        let scene = ScriptLinkScene::new([], [], [(id, 0x20001), (id, 0x10001)]);
        assert_eq!(scene.area_loaded_target(id, INVALID_UNIQUE_ID, true), 0x20001);
        assert_eq!(scene.area_loaded_target(id, INVALID_UNIQUE_ID, false), INVALID_UNIQUE_ID);
        assert_eq!(scene.area_loaded_target(id, 0, true), 0);
        assert_eq!(scene.area_loaded_target(id, 0x90001, true), 0x90001);
    }

    const COLOR: [f32; 4] = [0.5, 0.25, 0.75, 1.0];
    fn guid(n: u128) -> CObjectId { Uuid::from_u128(n << 96 | 0x0123456789abcdef01234567).into() }
    fn component(id: u32, kind: u32, owner: u32) -> ScriptComponentSnapshot {
        ScriptComponentSnapshot {
            unique_id: id,
            component_type: kind,
            interface_type: kind,
            flags: 0x7a,
            owner_entity: Some(owner),
        }
    }
    fn entity(id: u32, children: Vec<u32>, area: u32) -> ScriptEntitySnapshot {
        ScriptEntitySnapshot {
            unique_id: id,
            component_ids: children,
            area_unique_id: Some(area),
            scope_id: guid(0),
            parent_unique_id: INVALID_UNIQUE_ID,
            instance_index: 0,
        }
    }
    fn fixture() -> (ScriptLinkScene, RuntimeScriptLink) {
        let scene = ScriptLinkScene::new(
            [
                component(0x10001, ENTITY_COMPONENT, 0x10001),
                component(0x10003, ENTITY_COMPONENT, 0x10003),
                component(0x10004, ROOM_SETTINGS_COMPONENT, 0x10003),
            ],
            [entity(0x10001, vec![], 0), entity(0x10003, vec![0x10004], 1)],
            [(guid(5), 0x10003)],
        );
        (scene, RuntimeScriptLink {
            link: COLOR_MODIFIER_LINK,
            target: guid(5),
            cached_unique_id: INVALID_UNIQUE_ID,
        })
    }
    fn targets(scene: &ScriptLinkScene, links: &[RuntimeScriptLink]) -> Vec<(u32, u32)> {
        scene
            .baked_color_writes(0x10001, links, COLOR)
            .unwrap()
            .into_iter()
            .map(|w| {
                assert_eq!(w.color, COLOR);
                (w.room_settings_id, w.area_unique_id)
            })
            .collect()
    }

    #[test]
    fn native_cached_target_overrides_guid_and_zero_is_valid() {
        // Golden outputs from complete original ApplyNewColor, including the
        // static initializer, linked iterators, type casts and area setters.
        let (mut scene, mut link) = fixture();
        assert_eq!(targets(&scene, &[link]), [(0x10004, 1)]);
        link.target = guid(99);
        link.cached_unique_id = 0x10004;
        assert_eq!(targets(&scene, &[link]), [(0x10004, 1)]);
        scene.slots.insert(0, component(0, ROOM_SETTINGS_COMPONENT, 0x10003));
        link.target = CObjectId::default();
        link.cached_unique_id = 0;
        assert_eq!(targets(&scene, &[link]), [(0, 1)]);
        link.cached_unique_id = INVALID_UNIQUE_ID;
        assert!(targets(&scene, &[link]).is_empty());
    }

    #[test]
    fn native_order_and_duplicates_survive_direct_and_entity_targets() {
        let (mut scene, link) = fixture();
        scene.aliases.insert(guid(5), vec![0x10004, 0x10003]);
        assert_eq!(targets(&scene, &[link, link]), [(0x10004, 1); 4]);
    }

    #[test]
    fn native_slot_versions_visibility_and_concrete_cast() {
        let (mut scene, mut link) = fixture();
        link.cached_unique_id = 0x10004;
        scene.slots.insert(4, component(0x20004, ROOM_SETTINGS_COMPONENT, 0x10003));
        assert!(targets(&scene, &[link]).is_empty());
        scene.slots.insert(4, component(0x10004, ROOM_SETTINGS_COMPONENT, 0x10003));
        for (flags, visible) in [(0, false), (2, true), (4, false), (6, false), (0xfa, true)] {
            scene.slots.get_mut(&4).unwrap().flags = flags;
            assert_eq!(targets(&scene, &[link]).len(), usize::from(visible));
        }
        scene.slots.get_mut(&4).unwrap().component_type = 0xb85d6790;
        assert!(targets(&scene, &[link]).is_empty());
    }

    #[test]
    fn native_generated_scope_matches_parent_and_instance() {
        let (mut scene, link) = fixture();
        scene.entities.get_mut(&0x10001).unwrap().parent_unique_id = 0x30001;
        scene.entities.get_mut(&0x10003).unwrap().parent_unique_id = 0x30001;
        assert_eq!(targets(&scene, &[link]), [(0x10004, 1)]);
        scene.entities.get_mut(&0x10003).unwrap().instance_index = 1;
        assert!(targets(&scene, &[link]).is_empty());
        scene.entities.get_mut(&0x10003).unwrap().instance_index = 0;
        scene.entities.get_mut(&0x10003).unwrap().parent_unique_id = 0x30002;
        assert!(targets(&scene, &[link]).is_empty());
        scene.entities.get_mut(&0x10003).unwrap().scope_id = guid(45);
        assert_eq!(targets(&scene, &[link]), [(0x10004, 1)]);
    }

    #[test]
    fn native_child_communication_is_not_rechecked() {
        let (mut scene, link) = fixture();
        scene.entities.get_mut(&0x10001).unwrap().parent_unique_id = 0x50001;
        let mut other_owner = entity(0x10006, vec![], 0);
        other_owner.parent_unique_id = 0x50002;
        scene.entities.insert(0x10006, other_owner);
        scene.slots.get_mut(&4).unwrap().owner_entity = Some(0x10006);
        assert_eq!(targets(&scene, &[link]), [(0x10004, 0)]);
        let direct = RuntimeScriptLink { cached_unique_id: 0x10004, ..link };
        assert!(targets(&scene, &[direct]).is_empty());
    }

    #[test]
    fn native_nested_entity_replaces_remaining_siblings() {
        let (mut scene, link) = fixture();
        scene.entities.get_mut(&0x10003).unwrap().component_ids = vec![0x10006, 0x10004];
        scene.entities.insert(0x10006, entity(0x10006, vec![0x10007], 0));
        scene.slots.insert(6, component(0x10006, ENTITY_COMPONENT, 0x10006));
        scene.slots.insert(7, component(0x10007, ROOM_SETTINGS_COMPONENT, 0x10006));
        assert_eq!(targets(&scene, &[link]), [(0x10007, 0)]);
        scene.entities.get_mut(&0x10006).unwrap().component_ids = vec![0x10003];
        assert!(scene.baked_color_writes(0x10001, &[link], COLOR).is_err());
    }

    #[test]
    fn native_creation_keeps_runtime_links_and_inherits_activation_bits() {
        let links: Vec<_> = [[0, 0], [1, 1], [255, 0], [0, 255]]
            .into_iter()
            .enumerate()
            .map(|(i, flag_bytes)| SScriptLink { link: i as u32, flag_bytes, ..Default::default() })
            .collect();
        let created = created_script_links(&links);
        assert_eq!(created.iter().map(|l| l.link).collect::<Vec<_>>(), [1, 3]);
        assert!(created.iter().all(|l| l.cached_unique_id == INVALID_UNIQUE_ID));
        // Original Create outputs: low registration/own-active bits survive,
        // bit4 follows the owner and bit5 follows the layer's create-info flag.
        assert_eq!(component_create_flags(0xcf, 8, true), 0xff);
        assert_eq!(component_create_flags(0xff, 0, false), 0xcf);
        assert_eq!(component_create_flags(1, 8, false), 0x11);
    }

    #[test]
    fn native_entity_cache_sees_limbo_and_uses_only_first_alias() {
        let (mut scene, mut link) = fixture();
        link.link = ENTITY_COMPONENT_LINK;
        scene.slots.get_mut(&4).unwrap().flags = 0;
        scene.aliases.insert(guid(5), vec![0x10004, 0x10003]);
        assert!(scene.component_by_id(0x10004).is_none());
        assert_eq!(scene.entity_component_cache(&[link])[0].unique_id, 0x10004);
        scene.aliases.insert(guid(5), vec![0x20004, 0x10004]);
        assert!(scene.entity_component_cache(&[link]).is_empty());
        link.cached_unique_id = 0x10004;
        assert_eq!(scene.entity_component_cache(&[link, link]).len(), 2);
    }

    #[test]
    fn native_generated_remap_prefers_first_local_match_and_preserves_missing_targets() {
        let local = [(guid(1), 0x10001), (guid(1), 0x10002), (CObjectId::default(), 0x10003)];
        for mode in [0, 1, 2, u32::MAX] {
            let remap = GeneratedIdRemap::new(local, mode);
            assert_eq!(remap.target_unique_id(guid(1), INVALID_UNIQUE_ID), 0x10001);
            assert_eq!(remap.target_unique_id(guid(1), 7), if mode == 1 { 0x10001 } else { 7 });
            assert_eq!(remap.target_unique_id(guid(9), 7), 7);
            assert_eq!(remap.target_unique_id(CObjectId::default(), INVALID_UNIQUE_ID), 0x10003);
        }
    }
}
