//! MPR CTimePlaybackManager, including its asymmetric endpoint and loop rules.
//!
//! Times are engine seconds. Update quantizes with floor(seconds*60000+0.5)
//! in double precision. This is not a modulo clock or a truncating integer cast.

use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PlaybackResult {
    None = 0,
    ReachedStart = 1,
    ReachedEnd = 2,
    LoopedForward = 3,
    LoopedBackward = 4,
}

/// Native fields at +0, +4, +8 and +0xc. The three padding bytes are omitted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimePlaybackState {
    pub time: f32,
    pub min_time: f32,
    pub max_time: f32,
    /// Low six bits: direction (1 forward, other values update backward).
    /// Bit6: playing. Bit7: looping. Unknown direction values are preserved.
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimePlaybackManager {
    state: TimePlaybackState,
}

impl Default for TimePlaybackManager {
    fn default() -> Self {
        Self { state: TimePlaybackState { time: 0.0, min_time: 0.0, max_time: 0.0, flags: 1 } }
    }
}

impl TimePlaybackManager {
    /// The native constructor initializes time to zero, even for a nonzero min.
    pub fn new(
        playing: bool,
        forward: bool,
        looping: bool,
        min_time: f32,
        max_time: f32,
    ) -> Result<Self> {
        Self::from_state(TimePlaybackState {
            time: 0.0,
            min_time,
            max_time,
            flags: u8::from(forward) | (u8::from(playing) << 6) | (u8::from(looping) << 7),
        })
    }

    /// Restore explicit engine state. Reversed ranges and times outside the
    /// range are preserved: SetTime and single-wrap updates can produce these.
    pub fn from_state(state: TimePlaybackState) -> Result<Self> {
        ensure!(
            [state.time, state.min_time, state.max_time].iter().all(|v| v.is_finite()),
            "Nonfinite playback state"
        );
        Ok(Self { state })
    }

    pub fn state(&self) -> TimePlaybackState { self.state }

    pub fn time(&self) -> f32 { self.state.time }

    pub fn playing(&self) -> bool { self.state.flags & 0x40 != 0 }

    pub fn looping(&self) -> bool { self.state.flags & 0x80 != 0 }

    pub fn forward(&self) -> bool { self.state.flags & 0x3f == 1 }

    pub fn set_playing(&mut self, playing: bool) {
        self.state.flags = (self.state.flags & !0x40) | (u8::from(playing) << 6);
    }

    pub fn set_forward(&mut self, forward: bool) {
        self.state.flags = (self.state.flags & 0xc0) | u8::from(forward);
    }

    pub fn reverse_direction(&mut self) { self.set_forward(!self.forward()); }

    pub fn update(&mut self, delta: f32) -> Result<PlaybackResult> {
        // Native does not inspect delta when stopped.
        if !self.playing() {
            return Ok(PlaybackResult::None);
        }
        ensure!(delta.is_finite(), "Nonfinite playback delta");
        let mut time = if self.forward() { self.time() + delta } else { self.time() - delta };
        let mut result = PlaybackResult::None;
        let mut flags = self.state.flags;
        if self.looping() {
            // Test before rounding, and wrap at most once. Backward uses zero,
            // while the wrap distance uses the full stored max-minus-min span.
            if self.forward() {
                if time > self.state.max_time {
                    time -= self.state.max_time - self.state.min_time;
                    result = PlaybackResult::LoopedForward;
                }
            } else if time < 0.0 {
                time += self.state.max_time - self.state.min_time;
                result = PlaybackResult::LoopedBackward;
            }
            time = quantize(time);
        } else {
            time = quantize(time);
            if self.forward() {
                if time >= self.state.max_time {
                    time = self.state.max_time;
                    flags &= !0x40;
                    result = PlaybackResult::ReachedEnd;
                }
            } else if time <= 0.0 {
                time = 0.0;
                flags &= !0x40;
                result = PlaybackResult::ReachedStart;
            }
        }
        ensure!(time.is_finite(), "Playback update produced a nonfinite time");
        self.state.time = time;
        self.state.flags = flags;
        Ok(result)
    }

    /// SetTime does not clamp or quantize. Only a crossing from strictly inside
    /// an endpoint reports that endpoint. Looping prevents automatic stopping.
    pub fn set_time(&mut self, time: f32) -> Result<PlaybackResult> {
        ensure!(time.is_finite(), "Nonfinite playback time");
        let mut result = PlaybackResult::None;
        if self.time() > self.state.min_time && time <= self.state.min_time {
            if self.state.flags & !0x40 == 0 {
                self.state.flags = 0;
            }
            result = PlaybackResult::ReachedStart;
        }
        if self.time() < self.state.max_time && time >= self.state.max_time {
            if self.state.flags & !0x40 == 1 {
                self.state.flags = 1;
            }
            result = PlaybackResult::ReachedEnd;
        }
        self.state.time = time;
        Ok(result)
    }

    /// Looping phase uses a signed truncation to i32, including its saturation,
    /// then subtracts that integer. Negative phases keep a negative fraction.
    pub fn calculate_time_for_phase(&self, phase: f32) -> Result<f32> {
        ensure!(phase.is_finite(), "Nonfinite playback phase");
        let fraction = if self.looping() {
            phase - (phase as i32) as f32
        } else {
            let value = if 0.0 - phase >= 0.0 { 0.0 } else { phase };
            if value + -1.0 >= 0.0 {
                1.0
            } else {
                value
            }
        };
        let time = self.state.min_time + fraction * (self.state.max_time - self.state.min_time);
        ensure!(time.is_finite(), "Playback phase produced a nonfinite time");
        Ok(time)
    }

    pub fn set_phase(&mut self, phase: f32) -> Result<PlaybackResult> {
        self.set_time(self.calculate_time_for_phase(phase)?)
    }
}

fn quantize(time: f32) -> f32 {
    // Original doubles at7101d12378 and7101d12380. FRINTM is floor; Ghidra's
    // recovered cast-to-long presentation of this instruction is misleading.
    let ticks = (f64::from(time) * 60000.0 + 0.5).floor();
    (ticks * f64::from_bits(0x3ef1_79ec_9cbd_821e)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_clock_vectors_preserve_rounding_crossings_and_single_wrap() {
        // Original executable snapshots; op0 Update, op1 SetTime, op2 SetPhase.
        for (time, min, flags, op, input, result, output, output_flags) in [
            (0.0, 0.0, 193, 0, 0xb70bcf65, 0, 0xb78bcf65, 193),
            (0.0, 0.0, 193, 0, 0x40000000, 3, 0x3f800000, 193),
            (0.0, 0.0, 65, 0, 0x3f800000, 2, 0x3f800000, 1),
            (0.0, 0.0, 193, 0, 0x3f800000, 0, 0x3f800000, 193),
            (0.5, 0.25, 64, 0, 0x3f000000, 1, 0x00000000, 0),
            (0.5, 0.0, 65, 1, 0xbf800000, 1, 0xbf800000, 65),
            (1.0, 0.0, 65, 1, 0x40000000, 0, 0x40000000, 65),
            (0.0, 0.0, 193, 2, 0xbfc00000, 0, 0xbf000000, 193),
            (0.0, 0.0, 193, 2, 0x3f800000, 0, 0x00000000, 193),
        ] {
            let mut clock = TimePlaybackManager::from_state(TimePlaybackState {
                time,
                min_time: min,
                max_time: 1.0,
                flags,
            })
            .unwrap();
            let input = f32::from_bits(input);
            let actual = match op {
                0 => clock.update(input),
                1 => clock.set_time(input),
                2 => clock.set_phase(input),
                _ => unreachable!(),
            }
            .unwrap();
            assert_eq!(actual as u32, result);
            assert_eq!(clock.time().to_bits(), output);
            assert_eq!(clock.state().flags, output_flags);
        }
    }
}
