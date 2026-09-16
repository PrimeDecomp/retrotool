//! Serialized CMayaSpline and CColorGradient data used by ROOM properties.
//!
//! These retain the authored knots. RuntimeMayaSpline separately prepares and
//! evaluates them, including the native close-knot repair and tangent processing.

use anyhow::{ensure, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct MayaSplineKnot {
    pub time: f32,
    pub value: f32,
    pub tangent_types: [u8; 2],
    /// Present only for tangent type 5, in the same order as tangent_types.
    pub tangents: [Option<[f32; 2]>; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct MayaSpline {
    pub knots: Vec<MayaSplineKnot>,
    pub min_amplitude: f32,
    pub max_amplitude: f32,
    pub pre_infinity: u8,
    pub post_infinity: u8,
    pub clamp_mode: u8,
}

impl Default for MayaSpline {
    fn default() -> Self {
        Self {
            knots: Vec::new(),
            min_amplitude: -f32::MAX,
            max_amplitude: f32::MAX,
            pre_infinity: 0,
            post_infinity: 0,
            clamp_mode: 0,
        }
    }
}

fn take<const N: usize>(data: &mut &[u8]) -> Result<[u8; N]> {
    ensure!(data.len() >= N, "Truncated Maya spline");
    let (value, rest) = data.split_at(N);
    *data = rest;
    Ok(value.try_into().unwrap())
}

fn float(data: &mut &[u8]) -> Result<f32> { Ok(f32::from_le_bytes(take(data)?)) }

impl MayaSpline {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut remaining = data;
        let result = Self::read(&mut remaining)?;
        ensure!(remaining.is_empty(), "Trailing Maya spline bytes");
        Ok(result)
    }

    fn read(data: &mut &[u8]) -> Result<Self> {
        let count = u32::from_le_bytes(take(data)?) as usize;
        ensure!(data.len() >= 11 && count <= (data.len() - 11) / 10, "Truncated Maya spline knots");
        let mut knots = Vec::with_capacity(count);
        for _ in 0..count {
            let time = float(data)?;
            let value = float(data)?;
            let tangent_types = take::<2>(data)?;
            let mut tangents = [None; 2];
            for (i, kind) in tangent_types.into_iter().enumerate() {
                if kind == 5 {
                    tangents[i] = Some([float(data)?, float(data)?]);
                }
            }
            knots.push(MayaSplineKnot { time, value, tangent_types, tangents });
        }
        Ok(Self {
            knots,
            min_amplitude: float(data)?,
            max_amplitude: float(data)?,
            pre_infinity: take::<1>(data)?[0],
            post_infinity: take::<1>(data)?[0],
            clamp_mode: take::<1>(data)?[0],
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = u32::try_from(self.knots.len())?.to_le_bytes().to_vec();
        for knot in &self.knots {
            bytes.extend(knot.time.to_le_bytes());
            bytes.extend(knot.value.to_le_bytes());
            bytes.extend(knot.tangent_types);
            for (kind, tangent) in knot.tangent_types.into_iter().zip(knot.tangents) {
                ensure!(
                    (kind == 5) == tangent.is_some(),
                    "Maya spline tangent payload does not match type"
                );
                if let Some(tangent) = tangent {
                    bytes.extend(tangent.into_iter().flat_map(f32::to_le_bytes));
                }
            }
        }
        bytes.extend(self.min_amplitude.to_le_bytes());
        bytes.extend(self.max_amplitude.to_le_bytes());
        bytes.extend([self.pre_infinity, self.post_infinity, self.clamp_mode]);
        Ok(bytes)
    }
}

/// CColorGradient is a count followed by up to four splines. Native evaluation
/// expects four channels in RGBA order; shorter serialized lists are retained.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorGradient {
    pub channels: Vec<MayaSpline>,
}

impl Default for ColorGradient {
    fn default() -> Self { Self { channels: vec![MayaSpline::default(); 4] } }
}

impl ColorGradient {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut remaining = data;
        let count = u32::from_le_bytes(take(&mut remaining)?) as usize;
        ensure!(count <= 4, "Color gradient exceeds native four-channel storage");
        let channels =
            (0..count).map(|_| MayaSpline::read(&mut remaining)).collect::<Result<_>>()?;
        ensure!(remaining.is_empty(), "Trailing color gradient bytes");
        Ok(Self { channels })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        ensure!(self.channels.len() <= 4, "Color gradient exceeds native four-channel storage");
        let mut bytes = (self.channels.len() as u32).to_le_bytes().to_vec();
        for channel in &self.channels {
            bytes.extend(channel.to_bytes()?);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_splines_do_not_allocate_unbounded_counts_or_lose_tangents() {
        assert!(MayaSpline::parse(&[255; 4]).is_err());
        assert!(ColorGradient::parse(&5u32.to_le_bytes()).is_err());
        let mut spline = MayaSpline::default();
        spline.knots.push(MayaSplineKnot {
            time: 0.0,
            value: 0.5,
            tangent_types: [5, 0],
            tangents: [None, None],
        });
        assert!(spline.to_bytes().is_err());
        spline.knots[0].tangents[0] = Some([1.0, -0.0]);
        let bytes = spline.to_bytes().unwrap();
        assert_eq!(MayaSpline::parse(&bytes).unwrap().to_bytes().unwrap(), bytes);
        for length in 0..bytes.len() {
            assert!(MayaSpline::parse(&bytes[..length]).is_err());
        }
    }
}
