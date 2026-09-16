//! ColorModifier properties and startup selection from the original MPR loader.
//! Prepared curves and playback reproduce the color calculations. Script links,
//! activation and scheduler ownership remain external; parsing does not apply color.

use anyhow::{ensure, Result};

use super::{
    maya_spline::{ColorGradient, MayaSpline},
    maya_spline_runtime::RuntimeMayaSpline,
    room_lighting::ScriptProperty,
    script_activation::ComponentActivation,
    time_playback::{PlaybackResult, TimePlaybackManager},
};

pub const COLOR_MODIFIER_COMPONENT: u32 = 0xb85d6790;
pub const COLOR_MODIFIER_REACHED_START_EVENT: u32 = 0x6832bfc0;
pub const COLOR_MODIFIER_REACHED_END_EVENT: u32 = 0x84507813;

/// Descriptive consumer names, not recovered editor labels. Native offsets are
/// relative to SLdrColorModifierOptions at SLdrColorModifier+0x148.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorModifierOptions {
    pub initial_time: f32,
    pub random_time_range: f32,
    pub initially_playing: bool,
    pub apply_from_start: bool,
    pub apply_gradient_from_start: bool,
    pub apply_end_color: bool,
    pub apply_gradient_end: bool,
    pub apply_initial_time: bool,
    pub looping: bool,
    pub use_gradient: bool,
    /// True uses Think's delta; false reads the alternate state-manager clock.
    pub use_think_delta: bool,
}

impl Default for ColorModifierOptions {
    fn default() -> Self {
        Self {
            initial_time: 0.0,
            random_time_range: 0.0,
            initially_playing: false,
            apply_from_start: false,
            apply_gradient_from_start: false,
            apply_end_color: false,
            apply_gradient_end: false,
            apply_initial_time: false,
            looping: false,
            use_gradient: false,
            use_think_delta: true,
        }
    }
}

impl ColorModifierOptions {
    fn apply(&mut self, data: &[u8]) -> Result<()> {
        for p in ScriptProperty::parse_list(data)? {
            match p.id {
                0x28376d3f => self.initial_time = p.float()?,
                0xb1437b20 => self.random_time_range = p.float()?,
                0xe4c9cd3e => self.initially_playing = p.boolean()?,
                0x47a9b8f4 => self.apply_from_start = p.boolean()?,
                0xcb60e6ae => self.apply_gradient_from_start = p.boolean()?,
                0xe000181c => self.apply_end_color = p.boolean()?,
                0xf9159caf => self.apply_gradient_end = p.boolean()?,
                0xff17f0bb => self.apply_initial_time = p.boolean()?,
                0xcb494abc => self.looping = p.boolean()?,
                0x68b9c38e => self.use_gradient = p.boolean()?,
                0x83737173 => self.use_think_delta = p.boolean()?,
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ColorModifierProperties {
    pub initial_color: [f32; 4],
    pub end_color: [f32; 4],
    pub initial_intensity: f32,
    pub end_intensity: f32,
    pub gradient: ColorGradient,
    pub timing: MayaSpline,
    pub options: ColorModifierOptions,
    /// Serialized CRC; absent and unknown values resolve to runtime mode zero.
    pub mode: Option<u32>,
    pub properties: Vec<ScriptProperty>,
}

impl Default for ColorModifierProperties {
    fn default() -> Self {
        Self {
            initial_color: [1.0; 4],
            end_color: [1.0; 4],
            initial_intensity: 1.0,
            end_intensity: 1.0,
            gradient: ColorGradient::default(),
            timing: MayaSpline::default(),
            options: ColorModifierOptions::default(),
            mode: None,
            properties: Vec::new(),
        }
    }
}

/// Work chosen by AreaEntitiesLoaded after its base-class callback. End-color
/// actions leave the playback time untouched; AtTime calls SetTime and evaluates
/// the timing spline followed by either the endpoint lerp or the RGBA gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorModifierStartup {
    None,
    EndColor,
    GradientAtEnd,
    AtTime(f32),
}

/// Prepared curves and endpoint colors. Playback, activation and applying the
/// result to linked objects remain the caller's responsibility.
#[derive(Clone, Debug)]
pub struct PreparedColorModifier {
    timing: RuntimeMayaSpline,
    gradient: Option<[RuntimeMayaSpline; 4]>,
    endpoints: [[f32; 4]; 2],
    use_think_delta: bool,
    initially_playing: bool,
    looping: bool,
    mode: u32,
}

/// One explicit Think call's calculation. The caller applies color to the
/// appropriate linked objects and queues the optional script event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorModifierFrame {
    pub color: [f32; 4],
    pub mode: u32,
    pub playback_result: PlaybackResult,
    pub script_event: Option<u32>,
}

/// CheckToSendEvents emits only endpoint events, not either loop result.
pub fn color_modifier_playback_event(result: PlaybackResult) -> Option<u32> {
    match result {
        PlaybackResult::ReachedStart => Some(COLOR_MODIFIER_REACHED_START_EVENT),
        PlaybackResult::ReachedEnd => Some(COLOR_MODIFIER_REACHED_END_EVENT),
        _ => None,
    }
}

/// Predicate used for the existing Think functor. This does not establish that
/// the component is loaded, a functor exists, or a scheduler actually calls it.
pub fn color_modifier_think_enabled(
    clock: &TimePlaybackManager,
    activation_flags: u8,
    use_external_time: bool,
) -> bool {
    ComponentActivation { flags: activation_flags }.is_active()
        && clock.playing()
        && !use_external_time
}

#[cfg(test)]
mod playback_tests {
    use super::*;
    use crate::format::time_playback::TimePlaybackState;

    #[test]
    fn original_think_sequence_reaches_end_then_still_colors_when_called_stopped() {
        // A serialized native fixture, with golden full Think results. Alpha
        // follows the timing curve while RGB remains white.
        let raw = "03008ebd68d910000000803f0000803f0000803f00000000ef8fd8342d0003000000000000000000000002020000803f0000803f00000000c03f0000803f0202000000000000803f000001fd28bfb709000100f4b8a947010001";
        let bytes: Vec<_> = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
            .collect();
        let color = ColorModifierProperties::parse(&bytes).unwrap().prepare().unwrap();
        let mut clock = TimePlaybackManager::from_state(TimePlaybackState {
            time: 0.75,
            min_time: 0.0,
            max_time: 1.5,
            flags: 0x41,
        })
        .unwrap();
        for (delta, time, alpha, event) in [
            (0x3c888889, 0x3f444444, 0x3e6eeef0, None),
            (0x3d088889, 0x3f4ccccd, 0x3e4ccccc, None),
            (0x3f43d70a, 0x3fc00000, 0, Some(COLOR_MODIFIER_REACHED_END_EVENT)),
            (0xbf900000, 0x3fc00000, 0, None),
        ] {
            let frame = color.think(&mut clock, 1.0, f32::from_bits(delta), 99.0).unwrap().unwrap();
            assert_eq!(clock.time().to_bits(), time);
            assert_eq!(frame.color.map(f32::to_bits), [0x3f800000, 0x3f800000, 0x3f800000, alpha]);
            assert_eq!(frame.script_event, event);
        }
        assert!(!clock.playing());
    }
}

impl PreparedColorModifier {
    pub fn max_time(&self) -> f32 { self.timing.max_time() }

    pub fn playback_clock(&self) -> Result<TimePlaybackManager> {
        TimePlaybackManager::new(self.initially_playing, true, self.looping, 0.0, self.max_time())
    }

    /// Evaluate an actual Think invocation, selecting the configured delta and
    /// multiplying by runtime speed. A stopped clock still produces a color if
    /// called; scheduler gating is separate. Empty timing curves skip all work.
    pub fn think(
        &self,
        clock: &mut TimePlaybackManager,
        speed: f32,
        think_delta: f32,
        alternate_delta: f32,
    ) -> Result<Option<ColorModifierFrame>> {
        if self.timing.knots().is_empty() {
            return Ok(None);
        }
        let delta = if self.use_think_delta { think_delta } else { alternate_delta };
        let playback_result = clock.update(speed * delta)?;
        let script_event = color_modifier_playback_event(playback_result);
        // Native calls SetTime with its current time after event selection. It
        // changes no finite state, but keep the operation explicit and ordered.
        clock.set_time(clock.time())?;
        Ok(Some(ColorModifierFrame {
            color: self.color_at_time(clock.time())?,
            mode: self.mode,
            playback_result,
            script_event,
        }))
    }

    pub fn color_at_time(&self, time: f32) -> Result<[f32; 4]> {
        let value = snap_spline_value(self.timing.evaluate(time)?);
        if let Some(channels) = &self.gradient {
            let mut result = [0.0; 4];
            for (i, channel) in channels.iter().enumerate() {
                result[i] = channel.evaluate(value)?;
            }
            Ok(result)
        } else {
            let [initial, end] = self.endpoints;
            Ok(std::array::from_fn(|i| (1.0 - value) * initial[i] + end[i] * value))
        }
    }

    /// Compute a selected initial write without dispatching it or changing time.
    pub fn startup_color(&self, action: ColorModifierStartup) -> Result<Option<[f32; 4]>> {
        Ok(match action {
            ColorModifierStartup::None => None,
            ColorModifierStartup::EndColor => Some(self.endpoints[1]),
            ColorModifierStartup::AtTime(time) => Some(self.color_at_time(time)?),
            ColorModifierStartup::GradientAtEnd => {
                let channels = self
                    .gradient
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("ColorModifier has no prepared gradient"))?;
                let mut result = [0.0; 4];
                for (i, channel) in channels.iter().enumerate() {
                    result[i] = channel.evaluate(1.0)?;
                }
                Some(result)
            }
        })
    }
}

fn snap_spline_value(value: f32) -> f32 {
    let value = if value.abs() < 1e-5 { 0.0 } else { value };
    if (value - 1.0).abs() < 1e-5 {
        1.0
    } else {
        value
    }
}

impl ColorModifierProperties {
    pub fn prepare(&self) -> Result<PreparedColorModifier> {
        let timing = RuntimeMayaSpline::prepare(&self.timing)?;
        let gradient = if self.options.use_gradient {
            ensure!(
                self.gradient.channels.len() == 4,
                "ColorModifier requires four gradient channels"
            );
            let channels = self
                .gradient
                .channels
                .iter()
                .map(RuntimeMayaSpline::prepare)
                .collect::<Result<Vec<_>>>()?;
            Some(channels.try_into().unwrap())
        } else {
            None
        };
        Ok(PreparedColorModifier {
            timing,
            gradient,
            endpoints: self.scaled_endpoints(),
            use_think_delta: self.options.use_think_delta,
            initially_playing: self.options.initially_playing,
            looping: self.options.looping,
            mode: self.runtime_mode(),
        })
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for p in &properties {
            match p.id {
                0x8133a24b => result.initial_color = p.words::<4>()?.map(f32::from_bits),
                0xd968bd8e => result.end_color = p.words::<4>()?.map(f32::from_bits),
                0x627d5359 => result.initial_intensity = p.float()?,
                0xc1fd4866 => result.end_intensity = p.float()?,
                0xdbe10078 => result.gradient = ColorGradient::parse(&p.data)?,
                0x34d88fef => result.timing = MayaSpline::parse(&p.data)?,
                0xb7bf28fd => result.options.apply(&p.data)?,
                0xa3fbf12b => result.mode = Some(p.words::<1>()?[0]),
                _ => {}
            }
        }
        result.properties = properties;
        Ok(result)
    }

    /// LdrToEnum_ColorModifierMode710089ef20, table7101d16c54. Mode1 applies
    /// baked-lighting modulation to linked RoomSettings components.
    pub fn runtime_mode(&self) -> u32 {
        match self.mode {
            Some(0xd2fd45bc) => 1,
            Some(0x209ea819) => 2,
            Some(0x586d50a4) => 3,
            _ => 0,
        }
    }

    /// CColor4f::ScaleRGB71000f30bc: intensity affects RGB only, preserving alpha.
    pub fn scaled_endpoints(&self) -> [[f32; 4]; 2] {
        [(self.initial_color, self.initial_intensity), (self.end_color, self.end_intensity)].map(
            |(mut color, intensity)| {
                for component in &mut color[..3] {
                    *component *= intensity;
                }
                color
            },
        )
    }

    /// Caller supplies an evaluated timing-spline value. Native snaps values
    /// within 1e-5 of zero/one and permits extrapolation; there is no [0,1] clamp.
    /// Gradient evaluation is a separate path and must not use endpoint colors.
    pub fn endpoint_color_for_spline_value(&self, value: f32) -> Result<[f32; 4]> {
        ensure!(!self.options.use_gradient, "ColorModifier uses an RGBA gradient");
        ensure!(value.is_finite(), "Nonfinite ColorModifier spline value");
        let value = snap_spline_value(value);
        let [initial, end] = self.scaled_endpoints();
        Ok(std::array::from_fn(|i| (1.0 - value) * initial[i] + end[i] * value))
    }

    /// AreaEntitiesLoaded7100734d74. Supply activation bits AFTER the base
    /// callback and the native playback maximum (after spline knot repair).
    /// sample_range is called once only when needed, with the positive upper
    /// bound for CRandom16::RangeF(0,bound). Its result must be in that range.
    pub fn startup_action(
        &self,
        activation_flags: u8,
        max_time: f32,
        mut sample_range: impl FnMut(f32) -> Result<f32>,
    ) -> Result<ColorModifierStartup> {
        if activation_flags & 0x58 != 0x58 {
            return Ok(ColorModifierStartup::None);
        }
        let options = &self.options;
        let from_start = if options.use_gradient {
            if options.apply_gradient_end {
                return Ok(ColorModifierStartup::GradientAtEnd);
            }
            options.apply_gradient_from_start
        } else {
            if options.apply_end_color {
                return Ok(ColorModifierStartup::EndColor);
            }
            options.apply_from_start
        };
        if !from_start && !options.apply_initial_time {
            return Ok(ColorModifierStartup::None);
        }
        ensure!(
            max_time.is_finite()
                && options.initial_time.is_finite()
                && options.random_time_range.is_finite(),
            "Nonfinite ColorModifier startup time"
        );
        // Subtract-and-select preserves the native ordering, including signed zero.
        let positive = |value: f32| if 0.0 - value >= 0.0 { 0.0 } else { value };
        let minimum = |a: f32, b: f32| if a - b >= 0.0 { b } else { a };
        let start = if from_start { 0.0 } else { options.initial_time };
        let remaining = if from_start { max_time } else { max_time - start };
        let bound = minimum(positive(options.random_time_range), remaining);
        let offset = if bound > 0.0 {
            let value = sample_range(bound)?;
            ensure!(
                value.is_finite() && value >= 0.0 && value <= bound,
                "Invalid ColorModifier random offset"
            );
            value
        } else {
            bound
        };
        let time = if from_start { offset } else { start + offset };
        Ok(ColorModifierStartup::AtTime(minimum(positive(time), max_time)))
    }
}
