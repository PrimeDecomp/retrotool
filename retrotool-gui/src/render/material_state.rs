use anyhow::{bail, Result};
use remax::maxwell::graphics::ColorOutputRouting;
use retrolib::format::material_features::{
    BlendState as MprBlendState, BlendTarget, GraphicsState,
};
use wgpu_types::{
    BlendComponent, BlendFactor, BlendOperation, BlendState, ColorWrites, CompareFunction, Face,
    FrontFace,
};

/// Fixed-function fields supported by the current viewer pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialRenderState {
    pub cull_mode: Option<Face>,
    pub front_face: FrontFace,
    pub blend: Option<BlendState>,
    pub depth_compare: CompareFunction,
    pub depth_write: bool,
    pub color_writes: ColorWrites,
    pub alpha_to_coverage: bool,
}

impl Default for MaterialRenderState {
    fn default() -> Self {
        Self {
            cull_mode: Some(Face::Back),
            front_face: FrontFace::Ccw,
            blend: None,
            depth_compare: CompareFunction::GreaterEqual,
            depth_write: true,
            color_writes: ColorWrites::ALL,
            alpha_to_coverage: false,
        }
    }
}

/// Standalone viewer draw context, using the recovered mesh-class blend presets.
/// MPR comparison4 is LessEqual; the viewer maps it to its reversed depth buffer.
pub fn mesh_draw_state(class: u8, two_sided: bool, mirrored: bool) -> Result<GraphicsState> {
    let preset = match class {
        0 | 2 => 0,
        1 => 2,
        3 => 6,
        _ => bail!("Unknown mesh class {class}"),
    };
    Ok(GraphicsState {
        flags: 0xf,
        blend: Some(MprBlendState::from_preset(preset)?),
        cull_mode: Some(if two_sided { 0 } else { 2 }),
        front_face: Some(u8::from(mirrored)),
        depth: Some([1, 4, u32::from(class & 1 == 0)]),
        ..GraphicsState::default()
    })
}

impl MaterialRenderState {
    /// Derive shader output routing from the effective blend equations. Min/max
    /// factors have already been canonicalized because they do not participate.
    pub fn color_routing(&self) -> ColorOutputRouting {
        let dual_source = self.blend.is_some_and(|blend| {
            [blend.color, blend.alpha].iter().any(|component| {
                component.src_factor.ref_second_blend_source()
                    || component.dst_factor.ref_second_blend_source()
            })
        });
        if dual_source {
            ColorOutputRouting::DualSource
        } else {
            ColorOutputRouting::Attachments
        }
    }

    /// Apply supported fields and return explicit diagnostics for unsupported
    /// state. A target omitted by a blend/color mask keeps the existing value.
    pub fn apply(&mut self, state: &GraphicsState) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(cull) = state.cull_mode {
            self.cull_mode = match cull {
                1 => Some(Face::Front),
                2 => Some(Face::Back),
                _ => None,
            };
        }
        if let Some(front) = state.front_face {
            self.front_face = if front == 1 { FrontFace::Cw } else { FrontFace::Ccw };
        }
        if let Some(blend) = &state.blend {
            match blend.target(0).and_then(|target| {
                let Some(target) = target else {
                    return Ok(None);
                };
                // NVN has a shared constant color: different RT0/RT1 colors
                // depend on which cached target changed last. This standalone
                // single-target pipeline cannot reproduce that cache history.
                if blend
                    .target(1)?
                    .is_some_and(|other| other.constant_color != target.constant_color)
                {
                    bail!("Different render-target blend constants require shared NVN cache state");
                }
                Ok(Some(convert_blend(target)?))
            }) {
                Ok(Some(blend)) => self.blend = blend,
                Ok(None) => {}
                Err(e) => warnings.push(e.to_string()),
            }
        }
        if let Some([test, compare, write]) = state.depth {
            self.depth_write = write == 1;
            self.depth_compare = if test != 1 {
                CompareFunction::Always
            } else {
                match compare {
                    0 => CompareFunction::Never,
                    1 => CompareFunction::Always,
                    2 => CompareFunction::NotEqual,
                    3 => CompareFunction::Greater,
                    5 => CompareFunction::Equal,
                    6 => CompareFunction::LessEqual,
                    7 => CompareFunction::Less,
                    _ => CompareFunction::GreaterEqual,
                }
            };
        }
        if let Some([channels, targets]) = state.color_mask {
            if targets & 1 != 0 {
                self.color_writes = ColorWrites::from_bits_truncate(u32::from(channels));
            }
        }
        if let Some(enabled) = state.alpha_to_coverage {
            self.alpha_to_coverage = enabled != 0;
        }
        if state.depth_bias.is_some_and(|v| v.iter().any(|v| *v != 0.0)) {
            warnings.push("Depth bias is not yet applied by the viewer".into());
        }
        if state.stencil.is_some_and(|v| v[0] == 1) {
            warnings.push(
                "Stencil state requires a stencil attachment; viewer approximation retained".into(),
            );
        }
        if state.fill_mode == Some(0) {
            warnings.push("Wireframe fill state is not yet applied".into());
        }
        if state.scissors.as_ref().is_some_and(|r| !r.is_empty()) {
            warnings.push("Material scissor rectangles are not yet applied".into());
        }
        if state.raster_control.is_some() {
            warnings.push("Raster/sample-location controls are not yet applied".into());
        }
        warnings
    }
}

fn convert_blend(target: &BlendTarget) -> Result<Option<BlendState>> {
    if target.logic_op != -1 {
        bail!("Logic-operation blending is not yet supported by the viewer");
    }
    if target.is_blend_off() {
        return Ok(None);
    }
    fn equation(v: u32) -> BlendOperation {
        match v {
            1 => BlendOperation::Min,
            2 => BlendOperation::Max,
            3 => BlendOperation::Subtract,
            4 => BlendOperation::ReverseSubtract,
            _ => BlendOperation::Add,
        }
    }
    fn factor(v: u32, color: [f32; 4], alpha: bool) -> Result<BlendFactor> {
        Ok(match v {
            0 => BlendFactor::Zero,
            1 => BlendFactor::One,
            2 => BlendFactor::Src,
            3 => BlendFactor::OneMinusSrc,
            4 => BlendFactor::Dst,
            5 => BlendFactor::OneMinusDst,
            6 => BlendFactor::SrcAlpha,
            7 => BlendFactor::OneMinusSrcAlpha,
            8 => BlendFactor::DstAlpha,
            9 => BlendFactor::OneMinusDstAlpha,
            10 => BlendFactor::SrcAlphaSaturated,
            11..=14 => {
                let values: &[f32] = if alpha || v >= 13 { &color[3..] } else { &color[..3] };
                let zero = values.iter().all(|&v| v == 0.0);
                let one = values.iter().all(|&v| v == 1.0);
                if !zero && !one {
                    bail!("Nontrivial constant blend factors require a per-draw blend color");
                }
                if one ^ (v == 12 || v == 14) {
                    BlendFactor::One
                } else {
                    BlendFactor::Zero
                }
            }
            15 => BlendFactor::Src1,
            16 => BlendFactor::OneMinusSrc1,
            17 => BlendFactor::Src1Alpha,
            18 => BlendFactor::OneMinusSrc1Alpha,
            _ => BlendFactor::One,
        })
    }
    fn component(
        eq: u32,
        src: u32,
        dst: u32,
        color: [f32; 4],
        alpha: bool,
    ) -> Result<BlendComponent> {
        let operation = equation(eq);
        // Min/max equations ignore the factors; wgpu requires canonical One.
        let (src_factor, dst_factor) =
            if matches!(operation, BlendOperation::Min | BlendOperation::Max) {
                (BlendFactor::One, BlendFactor::One)
            } else {
                (factor(src, color, alpha)?, factor(dst, color, alpha)?)
            };
        Ok(BlendComponent { src_factor, dst_factor, operation })
    }
    Ok(Some(BlendState {
        color: component(
            target.color_equation,
            target.color_source,
            target.color_destination,
            target.constant_color,
            false,
        )?,
        alpha: component(
            target.alpha_equation,
            target.alpha_source,
            target.alpha_destination,
            target.constant_color,
            true,
        )?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_secondary_blend_factors_select_dual_source_routing() {
        for (native, expected) in [
            (15, BlendFactor::Src1),
            (16, BlendFactor::OneMinusSrc1),
            (17, BlendFactor::Src1Alpha),
            (18, BlendFactor::OneMinusSrc1Alpha),
        ] {
            for alpha in [false, true] {
                let mut target = BlendTarget::from_preset(0).unwrap();
                if alpha {
                    target.alpha_destination = native;
                } else {
                    target.color_destination = native;
                }
                let blend = convert_blend(&target).unwrap().unwrap();
                let component = if alpha { blend.alpha } else { blend.color };
                assert_eq!(component.dst_factor, expected);
                let state = MaterialRenderState { blend: Some(blend), ..Default::default() };
                assert_eq!(state.color_routing(), ColorOutputRouting::DualSource);
            }
        }
    }

    #[test]
    fn glass_override_replaces_mesh_blending_and_min_max_ignore_factors() {
        let bound = mesh_draw_state(1, false, false).unwrap();
        let mut state = MaterialRenderState::default();
        assert!(state.apply(&bound).is_empty());
        assert_eq!(state.color_routing(), ColorOutputRouting::Attachments);
        let mut overrides = GraphicsState {
            flags: 1,
            blend: Some(MprBlendState::from_preset(10).unwrap()),
            ..Default::default()
        };
        assert!(state.apply(&overrides).is_empty());
        assert_eq!(state.color_routing(), ColorOutputRouting::DualSource);
        let blend = state.blend.unwrap();
        assert_eq!(blend.color.src_factor, BlendFactor::One);
        assert_eq!(blend.color.dst_factor, BlendFactor::OneMinusSrc1);
        assert_eq!(blend.alpha, BlendComponent::OVER);
        // Preset metadata and ignored factors cannot determine output routing.
        for equation in [1, 2] {
            overrides.blend.as_mut().unwrap().targets[0].color_equation = equation;
            assert!(state.apply(&overrides).is_empty());
            assert_eq!(state.color_routing(), ColorOutputRouting::Attachments);
        }
        // A target mask excluding RT0 preserves the preceding draw state.
        overrides.blend.as_mut().unwrap().target_mask = 0;
        let before = state;
        assert!(state.apply(&overrides).is_empty());
        assert_eq!(state, before);
    }
}
