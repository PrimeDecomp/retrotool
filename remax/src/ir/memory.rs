use super::ValueId;
use crate::program::memory::Cache;

pub(crate) mod local;

/// One global access, retaining its cache/modifier contract and guarded
/// execution point. The low/high address words are evaluated before any result
/// components overwrite machine registers. No buffer binding is inferred here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Load {
    pub address: [ValueId; 2],
    pub bytes: u8,
    pub signed: bool,
    pub cache: Cache,
    pub native_u128: bool,
}

impl Load {
    pub fn valid(&self) -> bool {
        matches!(self.bytes, 1 | 2 | 4 | 8 | 16)
            && (!self.signed || self.bytes <= 2)
            && (!self.native_u128 || self.bytes == 16)
    }
}
