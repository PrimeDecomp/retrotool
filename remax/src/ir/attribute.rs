//! Logical attribute selection after native address recovery. An indexed input
//! consumes an opaque address token; its representation is never a native GPR
//! encoding and cannot be observed by arithmetic or shader outputs.

use super::ValueId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Input {
    Direct(u16),
    Indexed(ValueId),
}

impl Input {
    pub fn operand(self) -> Option<ValueId> {
        match self {
            Self::Direct(_) => None,
            Self::Indexed(value) => Some(value),
        }
    }

    pub fn map_operand(&mut self, mut map: impl FnMut(ValueId) -> ValueId) {
        if let Self::Indexed(value) = self {
            *value = map(*value);
        }
    }
}

/// A symbolic address, retaining addition operands so invalid offsets cannot
/// wrap into the supported domain. Resolution happens only at a consuming read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Address {
    pub base: u16,
    pub offset: u32,
}

impl Address {
    pub fn resolve(self) -> Option<u16> {
        let address = u32::from(self.base).checked_add(self.offset)?;
        ((0x80..0x280).contains(&address) && address.is_multiple_of(4)).then_some(address as u16)
    }
}
