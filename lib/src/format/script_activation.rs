//! Component activation bits from the original MPR creation and action paths.
//!
//! These operations do not invoke virtual callbacks or dispatch script events.
//! Entity operations must also propagate the appropriate bit to their cached
//! components, including objects in limbo. Layer request/lifetime state is external.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentActivation {
    pub flags: u8,
}

impl ComponentActivation {
    /// CGameObjectComponent constructor710046dd44. Only bit7 survives from
    /// object storage; the active-on-construction input has already become bool.
    pub fn constructed(active_on_construction: bool, old_flags: u8) -> Self {
        Self { flags: (old_flags & 0x80) | (u8::from(active_on_construction) << 3) | 0x70 }
    }

    /// Base Create710046de20 inherits owner activity and layer activity.
    pub fn create(&mut self, owner_flags: u8, layer_active: bool) {
        self.flags = (self.flags & 0xcf) | ((owner_flags & 8) << 1) | (u8::from(layer_active) << 5);
    }

    /// AreaEntitiesLoaded710046e024, after caching eligible script targets.
    /// Registration state and the effective-activity predicate are independent.
    pub fn area_entities_loaded(&mut self) { self.flags = (self.flags & 0xf8) | 3; }

    pub fn is_active(&self) -> bool { self.flags & 0x78 == 0x78 }

    /// SetLayerActive710046b5bc writes this bit to Entity and cached components.
    /// The setter itself does not dispatch activation callbacks.
    pub fn set_layer_active(&mut self, active: bool) {
        self.flags = (self.flags & 0xdf) | (u8::from(active) << 5);
    }

    /// SetTemplateActive710046b64c writes this bit to Entity and cached components.
    pub fn set_template_active(&mut self, active: bool) {
        self.flags = (self.flags & 0xbf) | (u8::from(active) << 6);
    }

    /// Entity ACTV/ICTV writes bit4 to cached components before changing itself.
    pub fn set_owner_active(&mut self, active: bool) {
        self.flags = (self.flags & 0xef) | (u8::from(active) << 4);
    }

    /// Base ActivateComponent/DeactivateComponent actions710046e2c4/710046e314.
    /// `owns_itself` compares the native owner pointer to this component, rather
    /// than testing its type. Returns whether OnActivationStateChange is invoked.
    /// For Entity ACTV/ICTV, update cached children's owner bits first; a callback
    /// on the Entity then notifies those children even if their own bits gate them.
    pub fn component_action(&mut self, active: bool, owns_itself: bool) -> bool {
        let was_active = self.is_active();
        let mask = if owns_itself { 0x18 } else { 8 };
        self.flags = if active { self.flags | mask } else { self.flags & !mask };
        was_active != self.is_active()
    }
}

/// CScriptLayer::IsActive7100476c70. The signed reference count is tested for
/// nonzero, not positivity; balancing Add/Remove requests is the caller's job.
pub fn layer_is_active(active_requests: i32, area_active: bool) -> bool {
    active_requests != 0 && area_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_component_actions_require_all_four_activation_gates() {
        // Original base callbacks: own activity alone cannot bypass a layer or
        // template gate. Entity self-ownership changes both own and owner bits.
        let mut state = ComponentActivation { flags: 0x62 };
        assert!(!state.component_action(true, false));
        assert_eq!(state.flags, 0x6a);
        state.set_owner_active(true);
        assert!(state.is_active());
        assert!(state.component_action(false, false));
        assert_eq!(state.flags, 0x72);
        state.flags = 0xe2;
        assert!(state.component_action(true, true));
        assert_eq!(state.flags, 0xfa);
        assert!(state.component_action(false, true));
        assert_eq!(state.flags, 0xe2);
        state.area_entities_loaded();
        assert_eq!(state.flags, 0xe3);
        assert!(layer_is_active(-1, true));
        assert!(!layer_is_active(0, true));
    }
}
