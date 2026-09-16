//! Conditional MPR graphics states and their runtime composition.

use anyhow::{ensure, Context, Result};

use super::material_features::{BlendState, BlendTarget, FeatureSet, GraphicsState, Technique};

impl BlendTarget {
    /// STargetBlendMode(EBlendMode), 0x71000ef984. The two tables at
    /// 0x7101d0cc44/0x7101d0cd34 contain (preset,equation,source,destination).
    pub fn from_preset(preset: i32) -> Result<Self> {
        const COLOR: [[u32; 3]; 15] = [
            [0, 1, 0],
            [0, 1, 1],
            [0, 6, 7],
            [0, 0, 6],
            [0, 0, 0],
            [0, 1, 7],
            [0, 6, 1],
            [1, 1, 1],
            [2, 1, 1],
            [0, 4, 0],
            [0, 1, 16],
            [4, 1, 1],
            [3, 1, 1],
            [4, 6, 1],
            [3, 6, 1],
        ];
        const ALPHA: [[u32; 3]; 15] = [
            [0, 1, 0],
            [0, 0, 1],
            [0, 1, 7],
            [0, 0, 6],
            [0, 0, 0],
            [0, 1, 7],
            [0, 6, 1],
            [1, 1, 1],
            [2, 1, 1],
            [0, 1, 0],
            [0, 1, 7],
            [4, 1, 1],
            [3, 1, 1],
            [4, 6, 1],
            [3, 6, 1],
        ];
        let &[color_equation, color_source, color_destination] =
            COLOR.get(preset as usize).context("Unknown blend preset")?;
        let [alpha_equation, alpha_source, alpha_destination] = ALPHA[preset as usize];
        Ok(Self {
            preset,
            color_equation,
            color_source,
            color_destination,
            alpha_equation,
            alpha_source,
            alpha_destination,
            // Static initializer 0x71000f2480 loads (1,1,1,1), not BSS zeros.
            constant_color: [1.0; 4],
            logic_op: -1,
        })
    }

    /// IsBlendOff (0x71000eff74) checks the actual factors/equations, not preset.
    pub fn is_blend_off(&self) -> bool {
        self.color_equation == 0
            && self.alpha_equation == 0
            && self.color_source == 1
            && self.alpha_source == 1
            && self.color_destination == 0
            && self.alpha_destination == 0
    }

    /// Exact NVN numeric equations emitted by SetBlendMode (0x71001addf0).
    pub fn nvn_equations(&self) -> [u32; 2] {
        [self.color_equation, self.alpha_equation].map(|v| match v {
            1 => 4,
            2 => 5,
            3 => 2,
            4 => 3,
            _ => 1,
        })
    }

    /// NVN numeric factors in color source/destination, alpha source/destination
    /// order. Unknown engine values fall back to One (NVN2).
    pub fn nvn_factors(&self) -> [u32; 4] {
        const FACTORS: [u32; 19] =
            [1, 2, 3, 4, 9, 10, 5, 6, 7, 8, 11, 0x61, 0x62, 0x63, 0x64, 16, 17, 18, 19];
        [self.color_source, self.color_destination, self.alpha_source, self.alpha_destination]
            .map(|v| FACTORS.get(v as usize).copied().unwrap_or(2))
    }
}

impl BlendState {
    pub fn from_preset(preset: i32) -> Result<Self> {
        Ok(Self { target_mask: u32::MAX, targets: vec![BlendTarget::from_preset(preset)?] })
    }

    /// All-target mask aliases entry0. Otherwise entries use their target index,
    /// not a compact index among the set mask bits. None means preserve target.
    pub fn target(&self, index: u32) -> Result<Option<&BlendTarget>> {
        ensure!(index < 32, "Blend target outside mask");
        if self.target_mask & (1 << index) == 0 {
            return Ok(None);
        }
        Ok(Some(
            self.targets
                .get(if self.target_mask == u32::MAX { 0 } else { index as usize })
                .context("Blend target mask references missing target")?,
        ))
    }
}

impl FeatureSet<'_> {
    /// accumulate_feature_set_option_values (0x710014b370): concatenate one-hot
    /// LOCAL option choices for the top-level features. Composite descendants
    /// and definition-option numbers do not add entries to this array.
    pub fn option_values(&self, permutation: [u64; 2]) -> Result<Vec<bool>> {
        let mut values = Vec::new();
        for (feature, option) in self.features.iter().zip(self.selected_options(permutation)?) {
            let start = values.len();
            let end = start
                .checked_add(feature.option_count as usize)
                .context("Feature option count overflow")?;
            ensure!(end <= 2048, "Feature condition state exceeds engine's 2048-bit capacity");
            values.resize(end, false);
            values[start + option as usize] = true;
        }
        Ok(values)
    }
}

impl GraphicsState {
    /// CGraphicsState::Merge, 0x71000f22e0. Only source fields whose flag is set
    /// replace destination fields; unknown high flag bits are not propagated.
    pub fn merge(&mut self, source: &Self) -> Result<()> {
        macro_rules! field {
            ($bit:expr, $name:ident) => {
                if source.flags & (1 << $bit) != 0 {
                    self.$name = Some(
                        source
                            .$name
                            .as_ref()
                            .context(concat!("Graphics-state flag without ", stringify!($name)))?
                            .clone(),
                    );
                    self.flags |= 1 << $bit;
                }
            };
        }
        field!(0, blend);
        field!(1, cull_mode);
        field!(2, front_face);
        field!(3, depth);
        field!(4, stencil);
        field!(5, fill_mode);
        field!(6, clear_color);
        field!(7, clear_depth);
        field!(8, clear_stencil);
        field!(9, scissors);
        field!(10, color_mask);
        field!(11, stencil_masks);
        field!(12, depth_bias);
        field!(13, alpha_to_coverage);
        field!(14, raster_control);
        field!(15, line_width);
        Ok(())
    }
}

impl Technique<'_> {
    /// BuildGraphicsOverrideState (0x710018a190), also inlined into archive
    /// shader construction (0x7100189908). Empty expressions are unconditional:
    /// IsValid tests instruction count, independently of Evaluate(empty)=false.
    pub fn override_state(&self, permutation: [u64; 2]) -> Result<GraphicsState> {
        let mut state = GraphicsState::default();
        if self.overrides.is_empty() {
            return Ok(state);
        }
        let values = self.features.option_values(permutation)?;
        for entry in &self.overrides {
            if entry.expression.instructions.is_empty()
                || entry.expression.evaluate(|index| {
                    values
                        .get(index as usize)
                        .copied()
                        .context("Graphics-state expression references unavailable option")
                })?
            {
                state.merge(&entry.state)?;
            }
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::material_features::{
        BoolExpression, CompositeFeature, CompositeOption, Feature, FeatureBody, ShaderDefines,
        SimpleFeature, SimpleOption, StateOverride,
    };

    fn expression(instructions: &[[u32; 2]]) -> BoolExpression {
        BoolExpression { instructions: instructions.to_vec() }
    }
    fn feature(options: &[u32]) -> Feature<'static> {
        Feature {
            name: "test",
            data_origin: 0,
            option_count: options.len() as u32,
            body: FeatureBody::Simple(SimpleFeature {
                definition: "FLAG",
                data_ids: vec![],
                flag_expressions: vec![],
                options: options
                    .iter()
                    .map(|&definition_option| SimpleOption {
                        definition_option,
                        defines: ShaderDefines { names: vec![], unknown: 0 },
                        bindings: vec![],
                    })
                    .collect(),
                allowed_options: 0,
                option_map: vec![],
                requirements: vec![],
            }),
        }
    }
    #[test]
    fn conditions_use_top_level_local_options_and_empty_is_unconditional() {
        let composite = Feature {
            name: "composite",
            data_origin: 0,
            option_count: 1,
            body: FeatureBody::Composite(CompositeFeature {
                subset: FeatureSet {
                    features: vec![feature(&[4, 7, 9])],
                    expression: expression(&[]),
                },
                arguments: vec![],
                bind_arguments: vec![],
                requirements: vec![],
                options: vec![CompositeOption {
                    name: "only",
                    permutation: [2, 0],
                    defines: ShaderDefines { names: vec![], unknown: 0 },
                }],
            }),
        };
        let set = FeatureSet {
            features: vec![composite, feature(&[6, 9, 12]), feature(&[4, 7])],
            expression: expression(&[]),
        };
        assert_eq!(set.option_values([10, 0]).unwrap(), [true, false, true, false, false, true]);
        let technique = Technique {
            features: set,
            state: GraphicsState { flags: 4, front_face: Some(1), ..GraphicsState::default() },
            overrides: vec![
                StateOverride {
                    expression: expression(&[]),
                    state: GraphicsState {
                        flags: 2,
                        cull_mode: Some(1),
                        ..GraphicsState::default()
                    },
                },
                StateOverride {
                    expression: expression(&[[u32::MAX, 2]]),
                    state: GraphicsState {
                        flags: 8,
                        depth: Some([0, 4, 0]),
                        ..GraphicsState::default()
                    },
                },
                StateOverride {
                    expression: expression(&[[u32::MAX, 5]]),
                    state: GraphicsState {
                        flags: 2,
                        cull_mode: Some(2),
                        ..GraphicsState::default()
                    },
                },
            ],
        };
        let state = technique.override_state([10, 0]).unwrap();
        assert_eq!(state.flags, 10);
        assert_eq!(state.cull_mode, Some(2));
        assert_eq!(state.depth, Some([0, 4, 0]));
        assert_eq!(state.front_face, None); // Technique defaults are a separate layer.
        assert!(!expression(&[]).evaluate(|_| Ok(false)).unwrap());
    }
    #[test]
    fn merge_preserves_unflagged_fields_and_rejects_missing_flagged_values() {
        let mut state = GraphicsState {
            flags: 6,
            cull_mode: Some(2),
            front_face: Some(1),
            ..GraphicsState::default()
        };
        state
            .merge(&GraphicsState {
                flags: 0xffff0002,
                cull_mode: Some(0),
                front_face: Some(0),
                ..GraphicsState::default()
            })
            .unwrap();
        assert_eq!(state.flags, 6);
        assert_eq!(state.cull_mode, Some(0));
        assert_eq!(state.front_face, Some(1));
        assert!(state.merge(&GraphicsState { flags: 8, ..GraphicsState::default() }).is_err());
    }
    #[test]
    fn blend_masks_alias_only_for_all_targets_and_presets_keep_alpha_factors() {
        let mut state = BlendState::from_preset(2).unwrap();
        assert_eq!(state.target(0).unwrap(), state.target(1).unwrap());
        let alpha = state.target(0).unwrap().unwrap();
        assert_eq!(
            [
                alpha.color_source,
                alpha.color_destination,
                alpha.alpha_source,
                alpha.alpha_destination
            ],
            [6, 7, 1, 7]
        );
        state.target_mask = 2;
        assert!(state.target(0).unwrap().is_none());
        assert!(state.target(1).is_err()); // Absolute target index, not compact mask rank.
        state.targets.push(BlendTarget::from_preset(6).unwrap());
        assert_eq!(state.target(1).unwrap().unwrap().nvn_factors(), [5, 2, 5, 2]);
        assert_eq!(BlendTarget::from_preset(1).unwrap().alpha_source, 0);
        assert!(BlendTarget::from_preset(0).unwrap().is_blend_off());
        assert!(BlendTarget::from_preset(15).is_err());
    }
}
