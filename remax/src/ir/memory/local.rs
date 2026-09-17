//! Invocation-private, byte-addressed storage. Effects remain ordered within
//! blocks; a local load is not a pure function of its address. Cache policies do
//! not change the values of this isolated frame. Shared/global aliases are absent.

use crate::ir::ValueId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Access {
    pub address: ValueId,
    pub bytes: u8,
}

impl Access {
    pub fn valid(self) -> bool { matches!(self.bytes, 1 | 2 | 4 | 8 | 16) }
}

/// One word of an access. Every component checks the entire native span, but
/// only the selected word's bytes determine its value availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Load {
    pub access: Access,
    pub part: u8,
    pub signed: bool,
}

impl Load {
    pub fn valid(self) -> bool {
        self.access.valid()
            && self.part < self.access.bytes
            && self.part.is_multiple_of(4)
            && (!self.signed || self.access.bytes <= 2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Store {
    pub access: Access,
    /// Captured before the write. A subword store takes the low bytes of word0.
    pub values: Vec<ValueId>,
}

impl Store {
    pub fn valid(&self) -> bool {
        self.access.valid() && self.values.len() == usize::from(self.access.bytes.div_ceil(4))
    }
}
