//! Native MPR layer request ordering over explicit runtime state.
//!
//! The scene supplies component construction, registration, notifications and
//! deletion queuing through LayerScene. Calls are synchronous so their ordering
//! is preserved; this does not provide a complete script interpreter or streamer.

use anyhow::Result;

use super::{room::Layer, script_activation::layer_is_active, CObjectId};

#[derive(Clone, Debug)]
pub struct ScriptLayerState {
    pub id: CObjectId,
    pub reset_on_activation: bool,
    pub active_requests: i32,
    pub area_active: bool,
    pub loaded: bool,
    pub constructed: bool,
    /// Native +0x10a, set alongside constructed by both construction paths.
    /// Its later consumers are unresolved; destruction does not clear it.
    pub construction_flag: bool,
    /// Runtime IDs of registered ordinary Entities, in native list order.
    pub entity_ids: Vec<u32>,
    /// GGOB keys used to obtain generated Entity IDs through live editor aliases.
    pub generated_object_ids: Vec<CObjectId>,
}

impl ScriptLayerState {
    /// CScriptLayer constructor710047689c starts with no requests, unloaded and
    /// unconstructed, with area_active=true. No entities are instantiated here.
    pub fn from_layer(layer: &Layer) -> Self {
        Self {
            id: layer.header.id,
            reset_on_activation: layer.header.reset_on_activation != 0,
            active_requests: 0,
            area_active: true,
            loaded: false,
            constructed: false,
            construction_flag: false,
            entity_ids: Vec::new(),
            generated_object_ids: layer.generated_objects.iter().map(|g| g.id).collect(),
        }
    }

    pub fn is_active(&self) -> bool { layer_is_active(self.active_requests, self.area_active) }

    /// LoadLayer7100476b98. Construction may be deferred by reset-on-activation;
    /// loaded becomes true even when data is absent or construction is deferred.
    pub fn load(
        &mut self,
        data_available: bool,
        clone_instance_data: u32,
        scene: &mut impl LayerScene,
    ) -> Result<()> {
        if (!self.reset_on_activation || self.is_active()) && data_available && !self.constructed {
            scene
                .construct_components(self, LayerConstruction::Deferred { clone_instance_data })?;
            self.constructed = true;
            self.construction_flag = true;
        }
        self.loaded = true;
        Ok(())
    }

    /// NotifyAreaActiveStateChanged7100477d70. This does not construct or delete
    /// entities. A changed area flag issues the notification with callbacks.
    pub fn set_area_active(&mut self, active: bool, scene: &mut impl LayerScene) -> Result<()> {
        if self.area_active != active {
            self.area_active = active;
            scene.notify_active_state(self, true)?;
        }
        Ok(())
    }

    fn add_request(&mut self, data_available: bool, scene: &mut impl LayerScene) -> Result<bool> {
        let previous = self.active_requests;
        self.active_requests = previous.wrapping_add(1);
        if previous != 0 && self.area_active {
            return Ok(false);
        }
        if self.loaded {
            if !self.reset_on_activation {
                scene.notify_active_state(self, false)?;
            } else if data_available && !self.constructed {
                scene.construct_components(self, LayerConstruction::Immediate)?;
                // Native sets both bytes before calling RegisterAreaEntity or
                // RegisterComponent for the returned component list.
                self.constructed = true;
                self.construction_flag = true;
                self.entity_ids.extend(scene.register_constructed_components(self)?);
            }
        }
        Ok(true)
    }

    fn remove_request(&mut self, scene: &mut impl LayerScene) -> Result<bool> {
        self.active_requests = self.active_requests.wrapping_sub(1);
        if self.active_requests != 0 && self.area_active {
            return Ok(false);
        }
        if self.reset_on_activation {
            scene.destruct_script_objects(self)?;
            if self.constructed {
                self.entity_ids.clear();
                self.constructed = false;
            }
        } else {
            scene.notify_active_state(self, false)?;
        }
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerConstruction {
    /// LoadLayer stores unregistered components for later processing.
    Deferred { clone_instance_data: u32 },
    /// AddActiveRequest uses clone mode0 and registers the new components now.
    Immediate,
}

/// Native call boundaries implemented by the scene. None may silently claim
/// successful construction when its required component providers are unavailable.
/// Actual Entity/child callbacks, generated aliases and queued script messages
/// belong to this scene, rather than being inferred from authored ROOM properties.
pub trait LayerScene {
    /// ConstructComponentFromStream. Save the resulting component list for the
    /// selected registration path. Called before the constructed flags are set.
    fn construct_components(
        &mut self,
        layer: &ScriptLayerState,
        mode: LayerConstruction,
    ) -> Result<()>;
    /// Immediate path only, after flags are set. Register each constructed
    /// component in order and return the IDs of its Entity components.
    fn register_constructed_components(&mut self, layer: &ScriptLayerState) -> Result<Vec<u32>>;
    /// NotifyActiveStateChange71004775b8: if loaded, update ordinary Entities,
    /// then generated aliases, preserving duplicates (nil editor IDs resolve
    /// to no aliases). SetLayerActive propagates
    /// to cached children. With callbacks, notify each Entity immediately after
    /// its bit update; otherwise update bits only. Unloaded layers do nothing.
    fn notify_active_state(&mut self, layer: &ScriptLayerState, callbacks: bool) -> Result<()>;
    /// RefreshEntityActiveState7100477890: ordinary Entities then generated
    /// aliases, using ordinary object lookup and the LACT message. Does not set
    /// layer bits, and has no loaded/active guard. Callback conditions differ
    /// from NotifyActiveStateChange, whose immediate callback gets Invalid().
    fn refresh_entity_active_state(&mut self, layer: &ScriptLayerState) -> Result<()>;
    /// DestructScriptObjects7100476de4 with an unlimited budget: if constructed,
    /// queue deletion for ordinary IDs, then every generated alias; nil editor
    /// IDs resolve to no aliases. The manager
    /// clears its ordinary list/constructed flag afterward. Queuing does not
    /// itself remove component slots or execute the queued deletion messages.
    fn destruct_script_objects(&mut self, layer: &ScriptLayerState) -> Result<()>;
}

/// AddOnRequestToLayers71003dc91c. Process each request against the FIRST matching
/// layer, including nil; preserve request duplicates. Refresh existing layers
/// after all request mutations, then REPLACE the output with newly constructed
/// layers' Entity IDs. Even an empty request clears the output.
pub fn add_on_requests(
    layers: &mut [ScriptLayerState],
    requested: &[CObjectId],
    data_available: bool,
    scene: &mut impl LayerScene,
    new_entity_ids: &mut Vec<u32>,
) -> Result<()> {
    let mut created = Vec::new();
    let mut refresh = Vec::new();
    for id in requested {
        let Some(index) = layers.iter().position(|l| l.id == *id) else { continue };
        let layer = &mut layers[index];
        let was_constructed = layer.constructed;
        if layer.add_request(data_available, scene)? {
            if was_constructed {
                refresh.push(index);
            } else if layer.constructed {
                created.push(index);
            }
        }
    }
    for index in refresh {
        scene.refresh_entity_active_state(&layers[index])?;
    }
    new_entity_ids.clear();
    for index in created {
        new_entity_ids.extend_from_slice(&layers[index].entity_ids);
    }
    Ok(())
}

/// AddOffRequestToLayers71003dce84. First matching layer per requested occurrence;
/// refreshes run after all mutations and only for layers still constructed after
/// their RemoveActiveRequest call. The new-Entity output is not touched.
pub fn add_off_requests(
    layers: &mut [ScriptLayerState],
    requested: &[CObjectId],
    scene: &mut impl LayerScene,
) -> Result<()> {
    let mut refresh = Vec::new();
    for id in requested {
        let Some(index) = layers.iter().position(|l| l.id == *id) else { continue };
        let layer = &mut layers[index];
        if layer.remove_request(scene)? && layer.constructed {
            refresh.push(index);
        }
    }
    for index in refresh {
        scene.refresh_entity_active_state(&layers[index])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Scene {
        calls: Vec<(&'static str, i32, bool)>,
    }

    impl LayerScene for Scene {
        fn construct_components(
            &mut self,
            layer: &ScriptLayerState,
            _: LayerConstruction,
        ) -> Result<()> {
            assert!(!layer.constructed);
            self.calls.push(("construct", layer.active_requests, layer.constructed));
            Ok(())
        }

        fn register_constructed_components(
            &mut self,
            layer: &ScriptLayerState,
        ) -> Result<Vec<u32>> {
            assert!(layer.constructed && layer.construction_flag);
            self.calls.push(("register", layer.active_requests, layer.constructed));
            Ok(vec![0x10001, 0x10002])
        }

        fn notify_active_state(&mut self, layer: &ScriptLayerState, _: bool) -> Result<()> {
            self.calls.push(("notify", layer.active_requests, layer.constructed));
            Ok(())
        }

        fn refresh_entity_active_state(&mut self, layer: &ScriptLayerState) -> Result<()> {
            self.calls.push(("refresh", layer.active_requests, layer.constructed));
            Ok(())
        }

        fn destruct_script_objects(&mut self, layer: &ScriptLayerState) -> Result<()> {
            self.calls.push(("destruct", layer.active_requests, layer.constructed));
            Ok(())
        }
    }

    fn layer(reset: bool) -> ScriptLayerState {
        ScriptLayerState {
            id: CObjectId::default(),
            reset_on_activation: reset,
            active_requests: 0,
            area_active: false,
            loaded: true,
            constructed: !reset,
            construction_flag: false,
            entity_ids: vec![],
            generated_object_ids: vec![],
        }
    }

    #[test]
    fn duplicate_requests_change_first_layer_before_batch_refresh() {
        let mut layers = [layer(false), layer(false)];
        let id = layers[0].id;
        let mut scene = Scene::default();
        let mut output = vec![123];
        add_on_requests(&mut layers, &[id, id], true, &mut scene, &mut output).unwrap();
        assert_eq!(layers[0].active_requests, 2);
        assert_eq!(layers[1].active_requests, 0);
        assert_eq!(scene.calls, [
            ("notify", 1, true),
            ("notify", 2, true),
            ("refresh", 2, true),
            ("refresh", 2, true)
        ]);
        assert!(output.is_empty());
        layers[0].area_active = true;
        layers[0].active_requests = -1;
        scene.calls.clear();
        add_on_requests(&mut layers, &[id], true, &mut scene, &mut output).unwrap();
        assert_eq!(layers[0].active_requests, 0);
        assert!(scene.calls.is_empty()); // Previous nonzero count triggers the early return.
    }

    #[test]
    fn reset_construction_registers_after_flags_and_deletion_preserves_loaded_state() {
        let mut layers = [layer(true)];
        let id = layers[0].id;
        let mut scene = Scene::default();
        let mut output = vec![123];
        add_on_requests(&mut layers, &[id], true, &mut scene, &mut output).unwrap();
        assert_eq!(scene.calls, [("construct", 1, false), ("register", 1, true)]);
        assert_eq!(output, [0x10001, 0x10002]);
        add_off_requests(&mut layers, &[id], &mut scene).unwrap();
        assert!(layers[0].loaded && layers[0].construction_flag);
        assert!(!layers[0].constructed);
        assert!(layers[0].entity_ids.is_empty());
        assert_eq!(output, [0x10001, 0x10002]); // Off requests do not replace this output.
        assert_eq!(scene.calls.last(), Some(&("destruct", 0, true)));
        add_on_requests(&mut layers, &[], true, &mut scene, &mut output).unwrap();
        assert!(output.is_empty());
    }
}
