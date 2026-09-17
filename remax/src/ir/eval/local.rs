use std::collections::BTreeMap;

use super::Cell;
use crate::{
    ir::memory::local::{Access, Load, Store},
    program::{Diagnostic, DiagnosticKind, Phase, Value},
};

pub(super) struct Memory {
    size: u32,
    /// Missing entries are unavailable. A sparse frame also avoids allocating
    /// unobserved memory merely because the supplied declaration is large.
    bytes: BTreeMap<u32, u8>,
}

impl Memory {
    pub fn new(size: u32) -> Self { Self { size, bytes: BTreeMap::new() } }

    fn address(&self, access: Access, address: Cell, pc: usize) -> Result<u32, Diagnostic> {
        let Value::Word(address) = address.observable(Some(pc))? else {
            unreachable!("verified local address type")
        };
        if !address.is_multiple_of(u32::from(access.bytes))
            || address.checked_add(u32::from(access.bytes)).is_none_or(|end| end > self.size)
        {
            return Err(Diagnostic::new(
                Phase::Interpret,
                Some(pc),
                DiagnosticKind::InvalidInput,
                format!("local access at {address:#x} is misaligned or outside its frame"),
            ));
        }
        Ok(address)
    }

    pub fn load(&self, load: Load, address: Cell, pc: usize) -> Result<Cell, Diagnostic> {
        let start = self.address(load.access, address, pc)? + u32::from(load.part);
        let count = u32::from(load.access.bytes.min(4));
        let mut word = 0u32;
        for byte in 0..count {
            let Some(&value) = self.bytes.get(&(start + byte)) else {
                return Ok(Cell::LocalUnavailable(start + byte));
            };
            word |= u32::from(value) << (byte * 8);
        }
        if load.signed {
            let shift = 32 - count * 8;
            word = ((word << shift) as i32 >> shift) as u32;
        }
        Ok(Cell::Value(Value::Word(word)))
    }

    pub fn store(
        &mut self,
        store: &Store,
        address: Cell,
        values: &[Cell],
        pc: usize,
    ) -> Result<(), Diagnostic> {
        let start = self.address(store.access, address, pc)?;
        for byte in 0..usize::from(store.access.bytes) {
            let value = match values[byte / 4] {
                Cell::Value(Value::Word(word)) => Some((word >> ((byte % 4) * 8)) as u8),
                Cell::Missing(_) | Cell::Unavailable(_) | Cell::LocalUnavailable(_) => None,
                _ => unreachable!("verified local store type"),
            };
            let address = start + byte as u32;
            if let Some(value) = value {
                self.bytes.insert(address, value);
            } else {
                self.bytes.remove(&address);
            }
        }
        Ok(())
    }
}
