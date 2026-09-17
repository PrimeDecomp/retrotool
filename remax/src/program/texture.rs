//! Texture requests for interpreter comparisons. The provider owns the bound
//! descriptor, view, sampler, result conversion and invocation/quad context.
//! Coordinates alone cannot determine an implicit-LOD sample.

use std::fmt;

use super::Stage;

/// Component words retain their bits: f32 coordinates for sampling, integer
/// texel coordinates for loads. Array layers have been unpacked as unsigned16.
/// This is the instruction's coordinate shape, not the bound view dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coordinates {
    D1(u32),
    D2([u32; 2]),
    D2Array { xy: [u32; 2], layer: u16 },
    D3([u32; 3]),
    Cube([u32; 3]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleLevel {
    /// Requires the actual participating quad and its coordinates, or a
    /// recorded result. The interpreter does not substitute a zero LOD.
    Implicit,
    /// f32 LOD bits; LZ is represented by positive zero.
    Explicit(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchLevel {
    Mip(u32),
    Sample(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Sample {
        coordinates: Coordinates,
        level: SampleLevel,
        /// f32 reference bits for depth comparison.
        comparison: Option<u32>,
    },
    Load {
        coordinates: Coordinates,
        level: FetchLevel,
        /// Signed texel offsets, unpacked from the instruction's operand.
        offset: Option<[i32; 2]>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// Bound descriptor-table offset in words, not an NVN API binding slot.
    pub descriptor_word_offset: u16,
    pub stage: Stage,
    pub pc: usize,
    /// Zero-based execution count at this PC in one invocation. Useful for
    /// trace replay; it is not a convergence token or a quad identity.
    pub occurrence: u64,
    /// The instruction permits skipping helper invocations (Maxwell NODEP).
    /// A scalar comparison must establish that this invocation is live;
    /// suppressed helper-lane register effects are not modeled.
    pub skip_helpers: bool,
    pub operation: Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Logical RGBA after descriptor conversion/swizzle, before TEXS/TLDS
    /// output masking. Words can hold float, signed or unsigned result bits.
    Texel([u32; 4]),
    /// f32 comparison result. The instruction expands it to (d,d,d,1).
    Comparison(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable(String),
    InvalidResource(String),
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(s) | Self::InvalidResource(s) | Self::Unsupported(s) => {
                f.write_str(s)
            }
        }
    }
}
impl std::error::Error for Error {}

pub trait Provider {
    /// Resolve one executed access. Validate the actual resource type, view and
    /// sampler, and return an error if required state or execution context is
    /// unavailable. Results are never inferred from the destination registers.
    /// If `skip_helpers` is set, establish a live invocation or reject the
    /// request; do not invent a value for a suppressed helper access.
    /// For filtered 2D/array sampling, a 2D view has one layer, a 2D request
    /// selects layer zero, and an array layer clamps to the view's last layer.
    /// Loads have a separate addressing contract.
    fn read(&mut self, request: &Request) -> Result<Response, Error>;
}

impl<F> Provider for F
where F: FnMut(&Request) -> Result<Response, Error>
{
    fn read(&mut self, request: &Request) -> Result<Response, Error> { self(request) }
}
