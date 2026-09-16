//! Original MPR spline preparation and uncached evaluation.
//!
//! Preparation repairs close knots and computes the native normalized tangents.
//! Authored serialization remains available in MayaSpline. Evaluation follows
//! EvaluateAt with a null SCachedValues pointer, as used by ColorModifier.

use anyhow::{ensure, Result};

use super::{maya_spline::MayaSpline, retro_math::sqrt_f};

const TANGENT_EPSILON: f32 = f32::from_bits(0x343b_673f);
const MIN_TIME_DELTA: f32 = f32::from_bits(0x38d1_b717);
const VERTICAL_SLOPE: f32 = f32::from_bits(0x4aae_da54);

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeMayaSplineKnot {
    pub time: f32,
    pub value: f32,
    ///0 linear,1 flat,2 smooth,3 step,4 adaptive,5 authored tangent.
    /// Adaptive tangents are resolved to1 or2 during preparation.
    pub tangent_types: [u8; 2],
    /// Incoming and outgoing normalized tangents, including native near-vertical
    /// handling. These are runtime vectors, not optional serialized payloads.
    pub tangents: [[f32; 2]; 2],
}

#[derive(Clone, Debug)]
pub struct RuntimeMayaSpline {
    knots: Vec<RuntimeMayaSplineKnot>,
    min_amplitude: f32,
    max_amplitude: f32,
    pre_infinity: u8,
    post_infinity: u8,
    clamp_mode: u8,
}

impl RuntimeMayaSpline {
    pub fn prepare(source: &MayaSpline) -> Result<Self> {
        ensure!(
            source.knots.len() <= 32768,
            "Maya spline exceeds native signed-short segment index"
        );
        ensure!(
            source.pre_infinity <= 4 && source.post_infinity <= 4,
            "Unknown Maya spline infinity mode"
        );
        ensure!(
            source.min_amplitude.is_finite() && source.max_amplitude.is_finite(),
            "Nonfinite Maya spline amplitude bounds"
        );
        let mut knots = Vec::with_capacity(source.knots.len());
        for knot in &source.knots {
            ensure!(knot.time.is_finite() && knot.value.is_finite(), "Nonfinite Maya spline knot");
            let mut tangents = [[0.0; 2]; 2];
            for i in 0..2 {
                ensure!(
                    (knot.tangent_types[i] == 5) == knot.tangents[i].is_some(),
                    "Maya spline tangent payload does not match type"
                );
                if let Some(tangent) = knot.tangents[i] {
                    ensure!(tangent.iter().all(|v| v.is_finite()), "Nonfinite Maya spline tangent");
                    tangents[i] = tangent;
                }
            }
            knots.push(RuntimeMayaSplineKnot {
                time: knot.time,
                value: knot.value,
                tangent_types: knot.tangent_types,
                tangents,
            });
        }
        // Constructor7100099ecc: retain the first knot in a close run, set its
        // outgoing tangent to step, and remove following intermediate knots.
        let mut i = 0;
        while i + 1 < knots.len() {
            if knots[i + 1].time - knots[i].time < 0.002 {
                knots[i].tangent_types[1] = 3;
                while i + 2 < knots.len() && knots[i + 2].time - knots[i + 1].time < 0.002 {
                    knots.remove(i + 1);
                }
            }
            i += 1;
        }
        ensure!(
            knots.windows(2).all(|k| k[0].time <= k[1].time),
            "Unordered Maya spline knots after repair"
        );
        for i in 0..knots.len() {
            let previous = i.checked_sub(1).map(|p| [knots[p].time, knots[p].value]);
            let next = knots.get(i + 1).map(|n| [n.time, n.value]);
            prepare_tangents(&mut knots[i], previous, next);
        }
        Ok(Self {
            knots,
            min_amplitude: source.min_amplitude,
            max_amplitude: source.max_amplitude,
            pre_infinity: source.pre_infinity,
            post_infinity: source.post_infinity,
            clamp_mode: source.clamp_mode,
        })
    }

    pub fn knots(&self) -> &[RuntimeMayaSplineKnot] { &self.knots }

    pub fn max_time(&self) -> f32 { self.knots.last().map_or(0.0, |k| k.time) }

    pub fn evaluate(&self, time: f32) -> Result<f32> {
        ensure!(time.is_finite(), "Nonfinite Maya spline evaluation time");
        let value = self.clamp(self.unclamped(time, 0)?);
        ensure!(value.is_finite(), "Maya spline evaluation produced a nonfinite result");
        Ok(value)
    }

    fn unclamped(&self, time: f32, depth: usize) -> Result<f32> {
        ensure!(depth < 16, "Maya spline infinity mapping does not converge");
        let Some(first) = self.knots.first() else {
            return Ok(0.0);
        };
        let last = self.knots.last().unwrap();
        if time < first.time {
            return if self.pre_infinity == 0 {
                Ok(first.value)
            } else {
                self.infinity(time, true, depth)
            };
        }
        if last.time < time {
            return if self.post_infinity == 0 {
                Ok(last.value)
            } else {
                self.infinity(time, false, depth)
            };
        }
        // Match native binary-search tie selection, including duplicate times.
        // Exact nonfirst knots evaluate the preceding segment in the null-cache
        // path; a step segment therefore keeps its old value at that exact time.
        let mut low = 0i32;
        let mut high = self.knots.len() as i32 - 1;
        while low <= high {
            let mid = (low + high) / 2;
            let knot_time = self.knots[mid as usize].time;
            if time < knot_time {
                high = mid - 1;
            } else if time > knot_time {
                low = mid + 1;
            } else {
                low = mid;
                break;
            }
        }
        if low == 0 {
            return Ok(first.value);
        }
        let i = low as usize - 1;
        let a = &self.knots[i];
        let b = &self.knots[i + 1];
        if a.tangent_types[1] == 3 {
            return Ok(a.value);
        }
        let third = f32::from_bits(0x3eaa_aaab);
        let p0 = [a.time, a.value];
        let p1 = [a.time + a.tangents[1][0] * third, a.value + a.tangents[1][1] * third];
        let p2 = [b.time - b.tangents[0][0] * third, b.value - b.tangents[0][1] * third];
        let p3 = [b.time, b.value];
        let slope0 = if (p1[0] - p0[0]).abs() >= TANGENT_EPSILON {
            (p1[1] - p0[1]) / (p1[0] - p0[0])
        } else {
            VERTICAL_SLOPE
        };
        let slope1 = if (p3[0] - p2[0]).abs() >= TANGENT_EPSILON {
            (p3[1] - p2[1]) / (p3[0] - p2[0])
        } else {
            VERTICAL_SLOPE
        };
        let h = p3[0] - p0[0];
        let v = p3[1] - p0[1];
        let inverse_square = 1.0 / (h * h);
        let d0 = h * slope0;
        let d1 = h * slope1;
        let c0 = (inverse_square * (((d0 + d1) - v) - v)) / h;
        let c1 = inverse_square * ((((v + (v + v)) - d0) - d0) - d1);
        let t = time - p0[0];
        Ok(p0[1] + t * (slope0 + t * (t * c0 + c1)))
    }

    fn infinity(&self, time: f32, before: bool, depth: usize) -> Result<f32> {
        let first = self.knots.first().unwrap();
        let last = self.knots.last().unwrap();
        let duration = last.time - first.time;
        if duration.abs() < 1e-5 {
            return Ok(first.value);
        }
        let mode = if before { self.pre_infinity } else { self.post_infinity };
        if mode == 1 {
            let (knot, tangent) =
                if before { (first, first.tangents[0]) } else { (last, last.tangents[1]) };
            if tangent[0].abs() < TANGENT_EPSILON {
                return Ok(knot.value);
            }
            return Ok(if before {
                knot.value - ((knot.time - time) * tangent[1]) / tangent[0]
            } else {
                knot.value + ((time - knot.time) * tangent[1]) / tangent[0]
            });
        }
        let reference = if time > last.time { last.time } else { first.time };
        let ratio = (time - reference) / duration;
        ensure!(ratio.is_finite(), "Maya spline infinity ratio overflow");
        // The native modf operates on an exactly promoted f32 ratio.
        let integral = f64::from(ratio).trunc();
        let fraction = ((f64::from(ratio) - integral) as f32).abs();
        let remainder = duration * fraction;
        let cycles = (integral as f32).abs() + 1.0;
        let mapped = if mode == 4 {
            let even = (cycles % 2.0).abs() < 1e-5;
            if before == even {
                last.time - remainder
            } else {
                first.time + remainder
            }
        } else if before {
            last.time - remainder
        } else {
            first.time + remainder
        };
        // Native applies amplitude handling to the wrapped sample before adding
        // cycle-relative offsets; the outer EvaluateAt then applies it again.
        let value = self.clamp(self.unclamped(mapped, depth + 1)?);
        Ok(if mode == 3 {
            let offset = cycles * (last.value - first.value);
            if before {
                value - offset
            } else {
                value + offset
            }
        } else {
            value
        })
    }

    fn clamp(&self, value: f32) -> f32 {
        let min = self.min_amplitude;
        let max = self.max_amplitude;
        match self.clamp_mode {
            1 => {
                let value = if min - value >= 0.0 { min } else { value };
                if value - max >= 0.0 {
                    max
                } else {
                    value
                }
            }
            2 => {
                let span = max - min;
                if span <= 0.0 {
                    return min;
                }
                if max + f32::EPSILON < value {
                    let count = ((value - max) / span) as i32;
                    value - span * count.wrapping_add(1) as f32
                } else if value < min - f32::EPSILON {
                    let count = ((value - min) / span) as i32;
                    value + span * count.wrapping_abs().wrapping_add(1) as f32
                } else {
                    value
                }
            }
            _ => value,
        }
    }
}

fn prepare_tangents(
    knot: &mut RuntimeMayaSplineKnot,
    previous: Option<[f32; 2]>,
    next: Option<[f32; 2]>,
) {
    let point = [knot.time, knot.value];
    for side in 0..2 {
        let neighbor = if side == 0 { previous } else { next };
        let other = if side == 0 { next } else { previous };
        if knot.tangent_types[side] == 4 {
            let flat = neighbor.is_some_and(|n| {
                let difference = (n[1] - point[1]).abs();
                let other_difference = other.map_or(difference, |o| (o[1] - point[1]).abs());
                !(difference > 0.05 && other_difference > 0.05)
            });
            knot.tangent_types[side] = if flat { 1 } else { 2 };
        }
        match knot.tangent_types[side] {
            0 => {
                knot.tangents[side] = neighbor.map_or([1.0, 0.0], |n| {
                    if side == 0 {
                        [point[0] - n[0], point[1] - n[1]]
                    } else {
                        [n[0] - point[0], n[1] - point[1]]
                    }
                })
            }
            1 => {
                let dx = if side == 0 {
                    previous
                        .map_or_else(|| next.map_or(0.0, |n| n[0] - point[0]), |p| point[0] - p[0])
                } else {
                    next.map_or_else(
                        || previous.map_or(0.0, |p| point[0] - p[0]),
                        |n| n[0] - point[0],
                    )
                };
                knot.tangents[side] = [dx, 0.0];
            }
            3 => knot.tangents[side] = [1.0, 0.0],
            _ => {}
        }
    }
    if knot.tangent_types.contains(&2) {
        let smooth = match (previous, next) {
            (None, None) => [[1.0, 0.0]; 2],
            (None, Some(n)) => [[n[0] - point[0], n[1] - point[1]]; 2],
            (Some(p), None) => [[point[0] - p[0], point[1] - p[1]]; 2],
            (Some(p), Some(n)) => {
                let dx = n[0] - p[0];
                let dy = n[1] - p[1];
                let slope = if dx >= MIN_TIME_DELTA {
                    dy / dx
                } else if dy > 0.0 {
                    VERTICAL_SLOPE
                } else {
                    -VERTICAL_SLOPE
                };
                // Native uses the next interval for the incoming vector and the
                // previous interval for the outgoing vector before normalizing.
                [n[0] - point[0], point[0] - p[0]].map(|dt| {
                    if dt < MIN_TIME_DELTA {
                        [0.0, slope]
                    } else {
                        [dt, slope * dt]
                    }
                })
            }
        };
        for (i, tangent) in smooth.into_iter().enumerate() {
            if knot.tangent_types[i] == 2 {
                knot.tangents[i] = tangent;
            }
        }
    }
    for tangent in &mut knot.tangents {
        if tangent[0] < 0.0 {
            tangent[0] = 0.0;
        }
        let magnitude = sqrt_f(tangent[0] * tangent[0] + tangent[1] * tangent[1]);
        if magnitude.abs() >= TANGENT_EPSILON {
            let inverse = 1.0 / magnitude;
            tangent[0] = inverse * tangent[0];
            tangent[1] = inverse * tangent[1];
        }
        if tangent[0].abs() < TANGENT_EPSILON && tangent[1].abs() >= TANGENT_EPSILON {
            let y = f32::from_bits(0x440f_3d4c);
            *tangent = [MIN_TIME_DELTA, if tangent[1] < 0.0 { -y } else { y }];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Golden outputs from original loader/evaluator and SDK modulo functions.
    #[test]
    fn original_curve_vectors_cover_boundaries_wrapping_and_repair() {
        {
            // tangents-2-2-infinity-3-clamp-2
            let raw = "04000000000040bf000080bf0202cdcccc3d0000803e02020000e03fcdcc8c3e020200006040000000400202000000c000000040030302";
            let bytes: Vec<_> = (0..raw.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
                .collect();
            let spline = RuntimeMayaSpline::prepare(&MayaSpline::parse(&bytes).unwrap()).unwrap();
            assert_eq!(spline.knots().len(), 4);
            for (time, expected) in [
                (0x00000000u32, 0x3e35fe30u32),
                (0x3ea00000u32, 0x3ea46a38u32),
                (0x3fe00000u32, 0x3e8cccc9u32),
                (0x405fffffu32, 0x40000000u32),
                (0x40600000u32, 0xbffffffeu32),
                (0x40600001u32, 0xbffffffcu32),
                (0x40b40000u32, 0xbf4f58e8u32),
                (0xbea66666u32, 0xbe8bbe78u32),
                (0xc0a00000u32, 0x00000000u32),
            ] {
                assert_eq!(spline.evaluate(f32::from_bits(time)).unwrap().to_bits(), expected);
            }
        }
        {
            // tangents-3-3-infinity-0-clamp-0
            let raw = "04000000000040bf000080bf0303cdcccc3d0000803e03030000e03fcdcc8c3e030300006040000000400303000000c000000040000000";
            let bytes: Vec<_> = (0..raw.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
                .collect();
            let spline = RuntimeMayaSpline::prepare(&MayaSpline::parse(&bytes).unwrap()).unwrap();
            assert_eq!(spline.knots().len(), 4);
            for (time, expected) in [
                (0x00000000u32, 0xbf800000u32),
                (0x3ea00000u32, 0x3e800000u32),
                (0x3fe00000u32, 0x3e800000u32),
                (0x405fffffu32, 0x3e8ccccdu32),
                (0x40600000u32, 0x3e8ccccdu32),
                (0x40600001u32, 0x40000000u32),
                (0x40b40000u32, 0x40000000u32),
                (0xbea66666u32, 0xbf800000u32),
                (0xc0a00000u32, 0xbf800000u32),
            ] {
                assert_eq!(spline.evaluate(f32::from_bits(time)).unwrap().to_bits(), expected);
            }
        }
        {
            // tangents-4-4-infinity-4-clamp-1
            let raw = "04000000000040bf000080bf0404cdcccc3d0000803e04040000e03fcdcc8c3e040400006040000000400404000000c000000040040401";
            let bytes: Vec<_> = (0..raw.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
                .collect();
            let spline = RuntimeMayaSpline::prepare(&MayaSpline::parse(&bytes).unwrap()).unwrap();
            assert_eq!(spline.knots().len(), 4);
            for (time, expected) in [
                (0x00000000u32, 0x3e5ea6e0u32),
                (0x3ea00000u32, 0x3e80950du32),
                (0x3fe00000u32, 0x3e8ccccdu32),
                (0x405fffffu32, 0x3ffffffdu32),
                (0x40600000u32, 0x40000000u32),
                (0x40600001u32, 0x3ffffffdu32),
                (0x40b40000u32, 0x3e8b1df8u32),
                (0xbea66666u32, 0xbe600000u32),
                (0xc0a00000u32, 0x40000000u32),
            ] {
                assert_eq!(spline.evaluate(f32::from_bits(time)).unwrap().to_bits(), expected);
            }
        }
        {
            // tangents-5-5-infinity-1-clamp-0
            let raw = "04000000000040bf000080bf050519f7b53f8b175fc0b83a1c3e9d19ca3fcdcccc3d0000803e0505fd943d3f7000f8bf6f85d3bf065386c00000e03fcdcc8c3e0505f1ad3dbf91c51f3facb3d33eb290dabf000060400000004005054cd498bf06ac14c0bc80bfbfc28e4340000000c000000040010100";
            let bytes: Vec<_> = (0..raw.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
                .collect();
            let spline = RuntimeMayaSpline::prepare(&MayaSpline::parse(&bytes).unwrap()).unwrap();
            assert_eq!(spline.knots().len(), 4);
            for (time, expected) in [
                (0x00000000u32, 0x3f0366ccu32),
                (0x3ea00000u32, 0xc9817593u32),
                (0x3fe00000u32, 0x3f89999au32),
                (0x405fffffu32, 0x40711356u32),
                (0x40600000u32, 0x400f135fu32),
                (0x40600001u32, 0x40576d2au32),
                (0x40b40000u32, 0x4b39c7fbu32),
                (0xbea66666u32, 0x3f805398u32),
                (0xc0a00000u32, 0x4116bceau32),
            ] {
                assert_eq!(spline.evaluate(f32::from_bits(time)).unwrap().to_bits(), expected);
            }
        }
        {
            // close-knots-21
            let raw = "03000000c7d6a03d2a6092c00501f4dbd33f3fcd01c0350ba13d1f668cc00200bb23a53d4f4455c00103000000c000000040030001";
            let bytes: Vec<_> = (0..raw.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
                .collect();
            let spline = RuntimeMayaSpline::prepare(&MayaSpline::parse(&bytes).unwrap()).unwrap();
            assert_eq!(spline.knots().len(), 2);
            for (time, expected) in [
                (0x00000000u32, 0xc0000000u32),
                (0x3d9c89d3u32, 0xc0000000u32),
                (0x3da0d6c7u32, 0xc0000000u32),
                (0x3da2fd41u32, 0xc0000000u32),
                (0x3da523bbu32, 0xc0000000u32),
                (0x3da970afu32, 0xc0000000u32),
                (0x3f8a523cu32, 0xc0000000u32),
            ] {
                assert_eq!(spline.evaluate(f32::from_bits(time)).unwrap().to_bits(), expected);
            }
        }
    }
}
