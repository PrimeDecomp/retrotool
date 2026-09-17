//! Whole-word local storage that can be represented by SSA values. Any access
//! with an unknown address may alias the frame; subword accesses exclude their
//! containing words. Invalid accesses remain executable diagnostic operations.

use std::collections::{BTreeMap, BTreeSet};

use super::{Availability, Constants, Dependencies};
use crate::ir::{memory::local::Access, Function, Operation, ValueId};

#[derive(Default)]
pub(crate) struct LocalSlots {
    addresses: BTreeMap<ValueId, u32>,
    words: BTreeSet<u32>,
}

impl LocalSlots {
    pub fn analyze(function: &Function) -> Self {
        let accesses = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match &instruction.operation {
                Operation::LocalLoad(load) => Some(load.access),
                Operation::LocalStore(store) => Some(store.access),
                _ => None,
            })
            .collect::<Vec<_>>();
        if accesses.is_empty() {
            return Self::default();
        }
        let dependencies = Dependencies::new(function);
        let constants = Constants::analyze(function, &dependencies);
        let availability = Availability::new(function, &dependencies);
        let mut result = Self::default();
        let mut excluded = BTreeSet::new();
        for access in accesses {
            let Some(address) = constants
                .word(access.address)
                .filter(|_| availability.is_available(access.address))
            else {
                // No address-range or pointer-disjointness proof yet. This
                // includes reads: deleting a store must not change their value.
                return Self::default();
            };
            let bytes = u32::from(access.bytes);
            let valid = address.is_multiple_of(bytes)
                && address.checked_add(bytes).is_some_and(|end| end <= function.local_memory_bytes);
            let first_word = address / 4 * 4;
            let last_word = address.saturating_add(bytes - 1) / 4 * 4;
            if valid && bytes >= 4 {
                result.addresses.insert(access.address, address);
                result.words.extend((first_word..=last_word).step_by(4));
            } else {
                excluded.extend((first_word..=last_word).step_by(4));
            }
        }
        result.words.retain(|word| !excluded.contains(word));
        result
    }

    pub fn is_empty(&self) -> bool { self.words.is_empty() }

    pub fn word(&self, access: Access, part: u8) -> Option<u32> {
        if access.bytes < 4 {
            return None;
        }
        let address = self.addresses.get(&access.address)?.checked_add(u32::from(part))?;
        self.words.contains(&address).then_some(address)
    }
}
