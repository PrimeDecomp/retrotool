//! ROOM lighting and context properties with their native defaults.
//!
//! Property payloads use a u16 count followed by (u32 ID, u16 byte length, bytes).
//! The original ordered properties remain available, including shadow, gobo and
//! baking data whose runtime interpretation is not implemented here.

use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use binrw::BinReaderExt;

use super::CObjectId;

pub const LIGHT_STATIC_COMPONENT: u32 = 0x2be8bc19;
pub const ROOM_SETTINGS_COMPONENT: u32 = 0x160e9af9;
pub const ROOM_CONTROLLER_COMPONENT: u32 = 0x83cc17aa;
pub const BAKED_LIGHTING_PRIORITY_MODIFIER_COMPONENT: u32 = 0x9feab9b3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptProperty {
    pub id: u32,
    pub data: Vec<u8>,
}

impl ScriptProperty {
    pub fn parse_list(mut data: &[u8]) -> Result<Vec<Self>> {
        ensure!(data.len() >= 2, "Truncated script property count");
        let count = u16::from_le_bytes(data[..2].try_into().unwrap()) as usize;
        data = &data[2..];
        ensure!(count <= data.len() / 6, "Truncated script property headers");
        let mut properties = Vec::with_capacity(count);
        for _ in 0..count {
            ensure!(data.len() >= 6, "Truncated script property header");
            let id = u32::from_le_bytes(data[..4].try_into().unwrap());
            let size = u16::from_le_bytes(data[4..6].try_into().unwrap()) as usize;
            data = &data[6..];
            ensure!(size <= data.len(), "Truncated script property {id:08x}");
            properties.push(Self { id, data: data[..size].to_vec() });
            data = &data[size..];
        }
        ensure!(data.is_empty(), "Trailing script property bytes");
        Ok(properties)
    }

    pub(crate) fn words<const N: usize>(&self) -> Result<[u32; N]> {
        ensure!(self.data.len() == N * 4, "Wrong size for property {:08x}", self.id);
        Ok(std::array::from_fn(|i| {
            u32::from_le_bytes(self.data[i * 4..i * 4 + 4].try_into().unwrap())
        }))
    }

    pub(crate) fn float(&self) -> Result<f32> { Ok(f32::from_bits(self.words::<1>()?[0])) }

    pub(crate) fn boolean(&self) -> Result<bool> {
        ensure!(self.data.len() == 1, "Wrong size for property {:08x}", self.id);
        Ok(self.data[0] != 0)
    }
}

/// SLdrRoomSettings contains only nested SLdrRoomRenderSettings. Its separate
/// SetBakedLightingModulationColor runtime method has no serialized color field.
#[derive(Clone, Debug, Default)]
pub struct RoomSettingsProperties {
    /// Nested property71a12a41, native default false. Consumer meaning unresolved.
    pub unknown_render_flag: bool,
    pub properties: Vec<ScriptProperty>,
}

impl RoomSettingsProperties {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for property in &properties {
            if property.id == 0xc028d01f {
                for nested in ScriptProperty::parse_list(&property.data)? {
                    if nested.id == 0x71a12a41 {
                        result.unknown_render_flag = nested.boolean()?;
                    }
                }
            }
        }
        result.properties = properties;
        Ok(result)
    }
}

/// SLdrRoomController::Load7100818ff4. A definition references a room resource;
/// membership in the owning area's resident controller list is separate state.
#[derive(Clone, Debug, Default)]
pub struct RoomControllerProperties {
    pub room_id: CObjectId,
    pub properties: Vec<ScriptProperty>,
}

impl RoomControllerProperties {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for property in &properties {
            if property.id == 0x30a4d63d {
                ensure!(property.data.len() == 16, "Wrong size for RoomController room ID");
                result.room_id = Cursor::new(&property.data).read_le()?;
            }
        }
        result.properties = properties;
        Ok(result)
    }
}

/// SLdrBakedLightingPriorityModifierMP1::Load7100f9118c. Active EntityLoaded and
/// activation-change callbacks set the owning area's context priority. Inactive
/// callbacks do nothing; there is no corresponding restore on deactivation.
#[derive(Clone, Debug)]
pub struct BakedLightingPriorityModifierProperties {
    pub priority: i32,
    pub properties: Vec<ScriptProperty>,
}

impl Default for BakedLightingPriorityModifierProperties {
    fn default() -> Self { Self { priority: 1, properties: Vec::new() } }
}

impl BakedLightingPriorityModifierProperties {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for property in &properties {
            if property.id == 0x8582d268 {
                result.priority = property.words::<1>()?[0] as i32;
            }
        }
        result.properties = properties;
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightDistanceProperties {
    /// Serialized enum CRC, when supplied. It differs from the runtime enum.
    pub mode: Option<u32>,
    pub inner_radius: f32,
    pub outer_radius: f32,
}

impl Default for LightDistanceProperties {
    fn default() -> Self { Self { mode: None, inner_radius: 0.25, outer_radius: 1.0 } }
}

impl LightDistanceProperties {
    /// LdrToEnum_LightDistanceAttenuationType7100abc2f0; unknown CRCs become3.
    pub fn runtime_mode(&self) -> u8 {
        match self.mode {
            Some(0xf75d18cf) => 1,
            Some(0x5e3252a0) => 2,
            _ => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticLightProperties {
    /// 0 ambient, 1 directional, 2 point, 3 spot; 4 area and5/6 square variants
    /// are excluded from the runtime light builder. Preserve other raw values.
    pub light_type: i32,
    pub color: [f32; 4],
    pub intensity: f32,
    pub distance: LightDistanceProperties,
    /// Full cone angles in degrees, before native half-angle conversion.
    pub inner_angle: f32,
    pub outer_angle: f32,
    /// SLdrLightFlags bytes0..4; byte4 becomes scene-node fill-ambient flag bit2.
    /// Other flag meanings require the shadow/light consumer, not their order.
    pub light_flags: [bool; 5],
    pub runtime_enabled: bool,
    /// Becomes scene-node flag bit0, used to distinguish baked contributions.
    pub baked: bool,
    pub unknown_16a: bool,
    /// Serialized RenderTargetScene CRC; absent and unknown values resolve to0.
    pub render_scene: Option<u32>,
    pub properties: Vec<ScriptProperty>,
}

impl Default for StaticLightProperties {
    fn default() -> Self {
        Self {
            light_type: 0,
            color: [1.0; 4],
            intensity: 0.0,
            distance: LightDistanceProperties::default(),
            inner_angle: 0.0,
            outer_angle: 45.0,
            light_flags: [false; 5],
            runtime_enabled: false,
            baked: true,
            unknown_16a: false,
            render_scene: None,
            properties: Vec::new(),
        }
    }
}

impl StaticLightProperties {
    /// Parse the bytes after ComponentProperties.component_type, without a
    /// component ID prefix. Sparse and repeated properties follow native order.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let properties = ScriptProperty::parse_list(data)?;
        let mut result = Self::default();
        for property in &properties {
            match property.id {
                0x52d4c579 => result.light_type = property.words::<1>()?[0] as i32,
                0x6c1d6c28 => result.runtime_enabled = property.boolean()?,
                0xf10ea1b7 => result.baked = property.boolean()?,
                0x163bbc26 => result.unknown_16a = property.boolean()?,
                0x7c3cd2ce => result.render_scene = Some(property.words::<1>()?[0]),
                0x8b76d48e | 0xdd21d666 | 0xebae52b2 | 0x664b1a7c | 0xf939c307 => {
                    let nested = ScriptProperty::parse_list(&property.data)
                        .with_context(|| format!("Light property {:08x}", property.id))?;
                    for child in nested {
                        match (property.id, child.id) {
                            (0x8b76d48e, 0x8f421907) => {
                                result.color = child.words::<4>()?.map(f32::from_bits)
                            }
                            (0xdd21d666, 0xdaf8bddf) => result.intensity = child.float()?,
                            (0xebae52b2, 0x174c8abc) => {
                                result.distance.mode = Some(child.words::<1>()?[0])
                            }
                            (0xebae52b2, 0xe5ceaf7c) => {
                                result.distance.inner_radius = child.float()?
                            }
                            (0xebae52b2, 0x68ac20b3) => {
                                result.distance.outer_radius = child.float()?
                            }
                            (0x664b1a7c, 0x635dfcc7) => result.inner_angle = child.float()?,
                            (0x664b1a7c, 0xb0a71764) => result.outer_angle = child.float()?,
                            (0xf939c307, id) => {
                                let index = match id {
                                    0xa103d675 => Some(0),
                                    0x56608a5c => Some(1),
                                    0xeeb94af2 => Some(2),
                                    0x67241f0c => Some(3),
                                    0x3f1e6850 => Some(4),
                                    _ => None,
                                };
                                if let Some(index) = index {
                                    result.light_flags[index] = child.boolean()?;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        result.properties = properties;
        Ok(result)
    }

    /// RebuildBuildLightProxy710078dc74 checks this independently of entity
    /// activation. Baking-only light types must not become additional live lights.
    pub fn requests_runtime_proxy(&self) -> bool {
        self.runtime_enabled && (0..4).contains(&self.light_type)
    }

    /// LdrToEnum_RenderTargetScene71008a1490; preserves unknown serialized CRCs
    /// in render_scene while returning the native fallback value here.
    pub fn runtime_render_scene(&self) -> u32 {
        match self.render_scene {
            Some(0x90130085) => 1,
            Some(0x986d766d) => 2,
            _ => 0,
        }
    }

    /// CLight constructor/SetLightIntensity use ScaleRGB; alpha is unchanged.
    /// This is a finite host-f32 operation, not a GPU lighting equation.
    pub fn intensity_scaled_color(&self) -> [f32; 4] {
        [
            self.color[0] * self.intensity,
            self.color[1] * self.intensity,
            self.color[2] * self.intensity,
            self.color[3],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(properties: &[(u32, &[u8])]) -> Vec<u8> {
        let mut data = (properties.len() as u16).to_le_bytes().to_vec();
        for &(id, payload) in properties {
            data.extend(id.to_le_bytes());
            data.extend((payload.len() as u16).to_le_bytes());
            data.extend(payload);
        }
        data
    }

    #[test]
    fn room_context_properties_preserve_native_duplicate_order() -> Result<()> {
        let nested_true = list(&[(0x71a12a41, &[255])]);
        let nested_empty = list(&[]);
        let nested_false = list(&[(0x71a12a41, &[0])]);
        let p = RoomSettingsProperties::parse(&list(&[
            (0xc028d01f, &nested_true),
            (0xc028d01f, &nested_empty),
        ]))?;
        assert!(p.unknown_render_flag);
        let p = RoomSettingsProperties::parse(&list(&[
            (0xc028d01f, &nested_true),
            (0xc028d01f, &nested_false),
        ]))?;
        assert!(!p.unknown_render_flag);
        let first = [0x12; 16];
        let last = [0x34; 16];
        let p = RoomControllerProperties::parse(&list(&[
            (0x30a4d63d, &first),
            (0x12345678, b"unknown"),
            (0x30a4d63d, &last),
        ]))?;
        let expected: CObjectId = Cursor::new(last).read_le()?;
        assert_eq!(p.room_id, expected);
        assert_eq!(p.properties.len(), 3);
        assert!(RoomControllerProperties::parse(&list(&[(0x30a4d63d, &[0; 15])])).is_err());
        Ok(())
    }

    #[test]
    fn baked_priority_defaults_and_signed_duplicate_values() -> Result<()> {
        assert_eq!(BakedLightingPriorityModifierProperties::parse(&[0, 0])?.priority, 1);
        let bytes = list(&[
            (0x8582d268, &8i32.to_le_bytes()),
            (0x12345678, &[1, 2, 3]),
            (0x8582d268, &(-4i32).to_le_bytes()),
        ]);
        let property = BakedLightingPriorityModifierProperties::parse(&bytes)?;
        assert_eq!(property.priority, -4);
        assert_eq!(property.properties.len(), 3);
        assert_eq!(property.properties[1].data, [1, 2, 3]);
        assert!(
            BakedLightingPriorityModifierProperties::parse(&list(&[(0x8582d268, &[1])])).is_err()
        );
        Ok(())
    }

    #[test]
    fn sparse_duplicates_preserve_other_fields_and_raw_properties() -> Result<()> {
        let first = list(&[(0xe5ceaf7c, &2f32.to_le_bytes())]);
        let second = list(&[(0x68ac20b3, &7f32.to_le_bytes()), (0x12345678, &[9, 8, 7])]);
        let bytes = list(&[
            (0x6c1d6c28, &[255]),
            (0x52d4c579, &2i32.to_le_bytes()),
            (0xebae52b2, &first),
            (0xebae52b2, &second),
            (0xf10ea1b7, &[0]),
            (0x87654321, &[1, 2, 3]),
        ]);
        let properties = StaticLightProperties::parse(&bytes)?;
        assert_eq!(properties.distance.inner_radius, 2.0);
        assert_eq!(properties.distance.outer_radius, 7.0);
        assert!(properties.requests_runtime_proxy());
        assert!(!properties.baked);
        assert_eq!(properties.properties[3].data, second);
        assert_eq!(properties.properties[5].data, [1, 2, 3]);
        let default = StaticLightProperties::parse(&[0, 0])?;
        assert!(!default.requests_runtime_proxy());
        assert!(default.baked);
        assert_eq!(default.intensity, 0.0);
        Ok(())
    }

    #[test]
    fn rejects_truncated_or_wrong_sized_known_properties() {
        let data = list(&[(0x8b76d48e, &list(&[(0x8f421907, &[0; 16])]))]);
        for end in 0..data.len() {
            assert!(StaticLightProperties::parse(&data[..end]).is_err());
        }
        assert!(StaticLightProperties::parse(&list(&[(0x6c1d6c28, &[0, 1])])).is_err());
        assert!(StaticLightProperties::parse(&list(&[(0x52d4c579, &[2])])).is_err());
        assert!(StaticLightProperties::parse(&[0, 0, 1]).is_err());
    }
}
