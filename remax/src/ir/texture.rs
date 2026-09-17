use super::ValueId;
use crate::{
    maxwell::texture::TextureDimension,
    program::{texture as api, Diagnostic, Stage},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SampleLevel {
    Implicit,
    Explicit(ValueId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FetchLevel {
    Mip(ValueId),
    Sample(ValueId),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Sample { level: SampleLevel, comparison: Option<ValueId> },
    Load { level: FetchLevel, offset: Option<[ValueId; 2]> },
}

/// One access and one RGBA result, independent of the destination register mask.
/// An implicit sample retains its execution dependence; this is not permission
/// to move, duplicate or speculate the access through a branch or loop.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Operation {
    pub descriptor: u16,
    pub dimension: TextureDimension,
    pub coordinates: Vec<ValueId>,
    pub layer: Option<ValueId>,
    pub kind: Kind,
    /// Maxwell NODEP: this access need not supply helper-invocation results.
    pub skip_helpers: bool,
}

impl Operation {
    pub fn operands(&self) -> Vec<ValueId> {
        let mut result = self.coordinates.clone();
        result.extend(self.layer);
        match self.kind {
            Kind::Sample { level, comparison } => {
                if let SampleLevel::Explicit(value) = level {
                    result.push(value);
                }
                result.extend(comparison);
            }
            Kind::Load { level, offset } => {
                let (FetchLevel::Mip(value) | FetchLevel::Sample(value)) = level;
                result.push(value);
                result.extend(offset.into_iter().flatten());
            }
        }
        result
    }

    pub fn map_operands(&mut self, mut f: impl FnMut(ValueId) -> ValueId) {
        for value in &mut self.coordinates {
            *value = f(*value);
        }
        self.layer = self.layer.map(&mut f);
        match &mut self.kind {
            Kind::Sample { level, comparison } => {
                if let SampleLevel::Explicit(value) = level {
                    *value = f(*value);
                }
                *comparison = comparison.map(&mut f);
            }
            Kind::Load { level, offset } => {
                let (FetchLevel::Mip(value) | FetchLevel::Sample(value)) = level;
                *value = f(*value);
                if let Some(offset) = offset {
                    for value in offset {
                        *value = f(*value);
                    }
                }
            }
        }
    }

    pub fn valid(&self, stage: Stage) -> bool {
        use TextureDimension::*;
        let count = match self.dimension {
            D1 => 1,
            D2 | D2Array => 2,
            D3 | Cube => 3,
        };
        if self.descriptor >= 0x2000
            || self.coordinates.len() != count
            || (self.dimension == D2Array) != self.layer.is_some()
        {
            return false;
        }
        match self.kind {
            Kind::Sample { level, comparison } => {
                (!matches!(level, SampleLevel::Implicit) || stage == Stage::Fragment)
                    && (comparison.is_none() || matches!(self.dimension, D2 | D2Array))
            }
            Kind::Load { level, offset } => {
                self.dimension != Cube
                    && (!matches!(level, FetchLevel::Sample(_))
                        || (self.dimension == D2 && offset.is_none()))
                    && (offset.is_none() || self.dimension == D2)
            }
        }
    }

    pub fn request(
        &self,
        mut read: impl FnMut(ValueId) -> Result<u32, Diagnostic>,
    ) -> Result<api::Operation, Diagnostic> {
        use TextureDimension::*;
        let values = self.coordinates.iter().map(|&v| read(v)).collect::<Result<Vec<_>, _>>()?;
        let coordinates = match self.dimension {
            D1 => api::Coordinates::D1(values[0]),
            D2 => api::Coordinates::D2([values[0], values[1]]),
            D2Array => api::Coordinates::D2Array {
                xy: [values[0], values[1]],
                layer: read(self.layer.expect("verified layer"))? as u16,
            },
            D3 => api::Coordinates::D3([values[0], values[1], values[2]]),
            Cube => api::Coordinates::Cube([values[0], values[1], values[2]]),
        };
        Ok(match self.kind {
            Kind::Sample { level, comparison } => api::Operation::Sample {
                coordinates,
                level: match level {
                    SampleLevel::Implicit => api::SampleLevel::Implicit,
                    SampleLevel::Explicit(value) => api::SampleLevel::Explicit(read(value)?),
                },
                comparison: comparison.map(read).transpose()?,
            },
            Kind::Load { level, offset } => api::Operation::Load {
                coordinates,
                level: match level {
                    FetchLevel::Mip(value) => api::FetchLevel::Mip(read(value)?),
                    FetchLevel::Sample(value) => api::FetchLevel::Sample(read(value)?),
                },
                offset: offset.map(|v| Ok([read(v[0])? as i32, read(v[1])? as i32])).transpose()?,
            },
        })
    }
}
