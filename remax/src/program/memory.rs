//! Executed global-memory reads for interpreter models and trace replay.
//! Cache modes remain visible; a mutable provider must honor their semantics.

use std::fmt;

use super::Stage;
use crate::resources::memory::Snapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cache {
    Default,
    Cg,
    Ci,
    Cv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    pub address: u64,
    pub bytes: u8,
    pub cache: Cache,
    /// Native LDG .U.128; its distinction from .128 remains unresolved.
    pub native_u128: bool,
    pub stage: Stage,
    /// Byte offset within MaxwellCode, including scheduling words.
    pub pc: usize,
    /// Zero-based execution count at this PC within one interpretation.
    pub occurrence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable(String),
    InvalidAccess(String),
    Unsupported(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(s) | Self::InvalidAccess(s) | Self::Unsupported(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for Error {}

pub trait Provider {
    /// Return the first `request.bytes` little-endian bytes in the array.
    /// The interpreter performs sign extension and register placement. Reject
    /// unavailable data or unresolved native modifiers; padding is unobserved.
    /// One call corresponds to one executed LDG, even for a discarded result.
    fn read(&mut self, request: &Read) -> Result<[u8; 16], Error>;
}
impl<F> Provider for F
where F: FnMut(&Read) -> Result<[u8; 16], Error>
{
    fn read(&mut self, request: &Read) -> Result<[u8; 16], Error> { self(request) }
}

impl Provider for &Snapshot {
    /// A frozen address space makes cache policies equivalent for returned
    /// bytes. It does not model races, volatile observations or synchronization.
    /// Natural alignment is required; misaligned native behavior is not inferred.
    fn read(&mut self, request: &Read) -> Result<[u8; 16], Error> {
        if request.native_u128 {
            return Err(Error::Unsupported(
                "LDG .U.128 requires an established result model".into(),
            ));
        }
        if !matches!(request.bytes, 1 | 2 | 4 | 8 | 16)
            || !request.address.is_multiple_of(u64::from(request.bytes))
        {
            return Err(Error::InvalidAccess("invalid or misaligned global read".into()));
        }
        let mut bytes = [0; 16];
        Snapshot::read(self, request.address, &mut bytes[..request.bytes as usize])
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        Ok(bytes)
    }
}

impl Provider for Snapshot {
    fn read(&mut self, request: &Read) -> Result<[u8; 16], Error> {
        Provider::read(&mut &*self, request)
    }
}
