//! Mesh-scoped material providers stored in a model's HEAD.
//!
//! Records have no size prefix. Parsing can advance only through known types;
//! an unknown source leaves it and all following bytes untouched.

pub mod animated_uv;
pub mod wind;

use anyhow::{ensure, Result};

use self::{animated_uv::ModelAnimatedUvData, wind::ModelWindData};

#[derive(Clone, Debug)]
pub enum ModelDataSource {
    AnimatedUv(ModelAnimatedUvData),
    Wind(ModelWindData),
}

#[derive(Debug)]
pub struct ModelDataSources<'a> {
    /// Known records in their authored order.
    pub sources: Vec<ModelDataSource>,
    /// Includes the first unknown source, if any.
    pub unread_sources: u32,
    /// Begins with the next source's FourCC, or the model's custom-data tail
    /// when all records were read.
    pub remaining: &'a [u8],
}

impl<'a> ModelDataSources<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut reader = Reader { remaining: bytes };
        let mut unread_sources = reader.word()?;
        ensure!(
            unread_sources as usize <= reader.remaining.len() / 4,
            "Truncated model data-source tags"
        );
        let mut sources = Vec::new();
        while unread_sources != 0 {
            let start = reader.remaining;
            let (source, remaining) = match reader.take(4)? {
                b"ANUV" => {
                    let (data, tail) = ModelAnimatedUvData::parse(reader.remaining)?;
                    (ModelDataSource::AnimatedUv(data), tail)
                }
                b"WIND" => {
                    let (data, tail) = ModelWindData::parse(reader.remaining)?;
                    (ModelDataSource::Wind(data), tail)
                }
                _ => {
                    reader.remaining = start;
                    break;
                }
            };
            sources.push(source);
            reader.remaining = remaining;
            unread_sources -= 1;
        }
        Ok(Self { sources, unread_sources, remaining: reader.remaining })
    }
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        ensure!(size <= self.remaining.len(), "Truncated model data source");
        let (bytes, tail) = self.remaining.split_at(size);
        self.remaining = tail;
        Ok(bytes)
    }

    fn word(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into()?)) }

    fn float(&mut self) -> Result<f32> { Ok(f32::from_bits(self.word()?)) }

    fn vector(&mut self) -> Result<[f32; 3]> { Ok([self.float()?, self.float()?, self.float()?]) }

    fn count(&mut self, stride: usize) -> Result<usize> {
        let count = self.word()? as usize;
        ensure!(count <= self.remaining.len() / stride, "Invalid model data-source count");
        Ok(count)
    }
}

#[cfg(test)]
mod tests;
