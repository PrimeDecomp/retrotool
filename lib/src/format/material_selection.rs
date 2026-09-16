//! Feature selection and cooked dynamic-key layout, traced from MPR's loaders.
//! The caller supplies definition-specific decisions from its material instance
//! and rendering context. `material_predicates::MaterialSelectionState` supplies
//! the recovered predicates for a resolved snapshot, without creating scene data.

use anyhow::{ensure, Context, Result};
use serde_derive::Serialize;

use crate::format::material_features::{
    BoolExpression, Feature, FeatureBody, FeatureSet, OptionRequirement,
};

#[derive(Debug, Serialize)]
pub struct DynamicFeature<'s, 'a> {
    /// Index before applying the EXPT mask. Mask bit 1 means skip this entry.
    pub index: usize,
    pub feature_path: Vec<&'a str>,
    pub bit_offset: Option<u32>,
    pub bit_count: u32,
    pub feature: &'s Feature<'a>,
}

impl<'a> FeatureSet<'a> {
    /// CFeatureSet constructor 0x710014bb70: only top-level origin 1 features
    /// contribute. An ordinary composite then flattens ALL descendants, in order,
    /// while an explicit composite contributes its selector instead of its subset.
    pub fn dynamic_features(&self, skip_mask: u32) -> Result<Vec<DynamicFeature<'_, 'a>>> {
        fn append<'s, 'a>(
            feature: &'s Feature<'a>,
            path: &mut Vec<&'a str>,
            out: &mut Vec<DynamicFeature<'s, 'a>>,
            depth: usize,
        ) -> Result<()> {
            ensure!(depth < 128, "Dynamic feature nesting exceeds supported depth");
            path.push(feature.name);
            match &feature.body {
                FeatureBody::Simple(_) => out.push(DynamicFeature {
                    index: out.len(),
                    feature_path: path.clone(),
                    bit_offset: None,
                    bit_count: feature.option_bit_count(),
                    feature,
                }),
                FeatureBody::Composite(c) => {
                    for child in &c.subset.features {
                        append(child, path, out, depth + 1)?;
                    }
                }
                FeatureBody::ExplicitComposite { selector, .. } => {
                    append(selector, path, out, depth + 1)?
                }
            }
            path.pop();
            Ok(())
        }
        let mut out = Vec::new();
        for feature in &self.features {
            if feature.data_origin == 1 {
                append(feature, &mut Vec::new(), &mut out, 0)?;
            }
        }
        let mut offset = 0;
        for entry in &mut out {
            // ARM64's 32-bit mask test wraps the shift index at 32.
            if skip_mask & (1 << (entry.index & 31)) == 0 {
                ensure!(offset + entry.bit_count <= 64, "Dynamic runtime key exceeds 64 bits");
                entry.bit_offset = Some(offset);
                offset += entry.bit_count;
            }
        }
        Ok(out)
    }

    /// Decode active local option indices in the same order as dynamic_features.
    /// Skipped entries produce None. Rejects out-of-range options and unused high bits.
    pub fn dynamic_options(&self, key: u64, skip_mask: u32) -> Result<Vec<Option<u32>>> {
        let layout = self.dynamic_features(skip_mask)?;
        let mut used = 0;
        let mut values = Vec::new();
        for entry in layout {
            values.push(if let Some(offset) = entry.bit_offset {
                let value = ((key >> offset) & ((1u64 << entry.bit_count) - 1)) as u32;
                ensure!(
                    value < entry.feature.option_count,
                    "Invalid dynamic option for {}",
                    entry.feature.name
                );
                used = offset + entry.bit_count;
                Some(value)
            } else {
                None
            });
        }
        ensure!(used == 64 || key >> used == 0, "Unused high bits in dynamic runtime key");
        Ok(values)
    }

    /// SelectDynamicRuntimeId 0x710014c078 ignores requirements. The callback
    /// receives a SIMPLE feature plus an allowed DEFINITION-option mask, and
    /// returns a DEFINITION option (not the packed local index), or None on failure.
    /// Pass the returned key to RuntimeMaterial::find_permutation for this technique.
    pub fn select_dynamic_runtime_id(
        &self,
        skip_mask: u32,
        mut selector: impl FnMut(&Feature<'a>, u32) -> Result<Option<u32>>,
    ) -> Result<Option<u64>> {
        let mut key = 0;
        for entry in self.dynamic_features(skip_mask)? {
            if let Some(offset) = entry.bit_offset {
                let Some(option) = select_feature(entry.feature, &[], false, &mut selector, 0)?
                else {
                    return Ok(None);
                };
                key |= u64::from(option) << offset;
            }
        }
        Ok(Some(key))
    }

    /// Full uncooked selection (0x710014be1c), including preceding one-hot option
    /// values, requirements, composite arguments and explicit selectors. The
    /// feature-set expression is NOT evaluated by this engine selection path.
    /// Definition-specific material/scene predicates are supplied by the caller.
    pub fn select_permutation(
        &self,
        mut selector: impl FnMut(&Feature<'a>, u32) -> Result<Option<u32>>,
    ) -> Result<Option<[u64; 2]>> {
        select_set(self, &mut Vec::new(), true, &mut selector, 0)
    }

    /// Same full-permutation path without requirements or composite arguments.
    pub fn select_permutation_ignoring_requirements(
        &self,
        mut selector: impl FnMut(&Feature<'a>, u32) -> Result<Option<u32>>,
    ) -> Result<Option<[u64; 2]>> {
        select_set(self, &mut Vec::new(), false, &mut selector, 0)
    }
}

fn evaluate(expression: &BoolExpression, values: &[bool]) -> Result<bool> {
    expression.evaluate(|index| {
        values
            .get(index as usize)
            .copied()
            .context("Feature expression references unavailable option/argument")
    })
}

fn filter_options(
    mut mask: u32,
    requirements: &[OptionRequirement],
    values: &[bool],
    simple: bool,
) -> Result<u32> {
    for requirement in requirements {
        if (!simple || mask & requirement.option_mask != 0)
            && !requirement.expression.instructions.is_empty()
            && !evaluate(&requirement.expression, values)?
        {
            mask &= !requirement.option_mask;
        }
    }
    Ok(mask)
}

fn select_feature<'a>(
    feature: &Feature<'a>,
    values: &[bool],
    requirements: bool,
    selector: &mut impl FnMut(&Feature<'a>, u32) -> Result<Option<u32>>,
    depth: usize,
) -> Result<Option<u32>> {
    ensure!(depth < 128, "Feature selection nesting exceeds supported depth");
    ensure!(feature.option_count != 0, "Feature has no options");
    let selected = match &feature.body {
        FeatureBody::ExplicitComposite { selector: explicit, .. } => {
            // Tail call to selector's virtual method, even when the parent has one option.
            select_feature(explicit, values, requirements, selector, depth + 1)?
        }
        _ if feature.option_count == 1 => Some(0),
        FeatureBody::Simple(simple) => {
            let mask = if requirements {
                filter_options(simple.allowed_options, &simple.requirements, values, true)?
            } else {
                simple.allowed_options
            };
            // IgnoreRequirements always calls the definition for multi-option features.
            let definition = if requirements && mask.count_ones() == 1 {
                Some(mask.trailing_zeros())
            } else {
                selector(feature, mask)?
            };
            definition
                .and_then(|d| simple.option_map.get(d as usize).copied())
                .filter(|&v| v != u32::MAX)
        }
        FeatureBody::Composite(composite) => {
            ensure!(
                composite.options.len() == feature.option_count as usize,
                "Composite option count mismatch"
            );
            let all =
                if feature.option_count >= 32 { u32::MAX } else { (1 << feature.option_count) - 1 };
            let mask = if requirements {
                filter_options(all, &composite.requirements, values, false)?
            } else {
                all
            };
            if requirements && mask.count_ones() == 1 {
                Some(mask.trailing_zeros())
            } else {
                let mut arguments = Vec::new();
                if requirements {
                    for argument in &composite.arguments {
                        arguments.push(evaluate(&argument.expression, values)?);
                    }
                }
                let Some(permutation) = select_set(
                    &composite.subset,
                    &mut arguments,
                    requirements,
                    selector,
                    depth + 1,
                )?
                else {
                    return Ok(None);
                };
                // The last option is an unconditional fallback, even if its mask
                // bit was removed. The engine compares only the earlier options.
                let fallback = feature.option_count - 1;
                Some(
                    composite.options[..fallback as usize]
                        .iter()
                        .enumerate()
                        .find(|(i, o)| mask & (1 << (i & 31)) != 0 && o.permutation == permutation)
                        .map(|(i, _)| i as u32)
                        .unwrap_or(fallback),
                )
            }
        }
    };
    if let Some(option) = selected {
        ensure!(
            option < feature.option_count,
            "Selected local option outside feature {}",
            feature.name
        );
    }
    Ok(selected)
}

fn select_set<'a>(
    set: &FeatureSet<'a>,
    values: &mut Vec<bool>,
    requirements: bool,
    selector: &mut impl FnMut(&Feature<'a>, u32) -> Result<Option<u32>>,
    depth: usize,
) -> Result<Option<[u64; 2]>> {
    let mut permutation = [0u64; 2];
    let mut offset = 0;
    for feature in &set.features {
        let Some(option) = select_feature(feature, values, requirements, selector, depth)? else {
            return Ok(None);
        };
        if requirements {
            let start = values.len();
            let end = start
                .checked_add(feature.option_count as usize)
                .context("Feature option count overflow")?;
            ensure!(end <= 2048, "Feature condition state exceeds engine's 2048-bit capacity");
            values.resize(end, false);
            values[start + option as usize] = true;
        }
        let width = feature.option_bit_count();
        ensure!(offset + width <= 128, "Feature permutation exceeds 128 bits");
        if offset < 64 {
            permutation[0] |= u64::from(option) << offset;
            if offset + width > 64 {
                // Faithful AppendValue quirk at 0x710014e064: no high spill
                // insertion; retain only existing low endBit%64 bits of word 1.
                permutation[1] &= (1u64 << (offset + width - 64)) - 1;
            }
        } else {
            permutation[1] |= u64::from(option) << (offset - 64);
        }
        offset += width;
    }
    Ok(Some(permutation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::material_features::{
        CompositeArgument, CompositeFeature, CompositeOption, ShaderDefines, SimpleFeature,
        SimpleOption,
    };

    fn expr(instructions: &[[u32; 2]]) -> BoolExpression {
        BoolExpression { instructions: instructions.to_vec() }
    }
    fn set(features: Vec<Feature<'static>>) -> FeatureSet<'static> {
        FeatureSet { features, expression: expr(&[]) }
    }
    fn simple(name: &'static str, origin: u32, definitions: &[u32]) -> Feature<'static> {
        let mut option_map = vec![u32::MAX; 1 + *definitions.iter().max().unwrap() as usize];
        for (i, &d) in definitions.iter().enumerate() {
            option_map[d as usize] = i as u32;
        }
        Feature {
            name,
            data_origin: origin,
            option_count: definitions.len() as u32,
            body: FeatureBody::Simple(SimpleFeature {
                definition: "TEST",
                data_ids: vec![],
                flag_expressions: vec![],
                allowed_options: definitions.iter().fold(0, |m, d| m | (1 << d)),
                option_map,
                options: definitions
                    .iter()
                    .map(|&definition_option| SimpleOption {
                        definition_option,
                        defines: ShaderDefines { names: vec![], unknown: 0 },
                        bindings: vec![],
                    })
                    .collect(),
                requirements: vec![],
            }),
        }
    }
    fn composite(
        children: Vec<Feature<'static>>,
        permutations: &[[u64; 2]],
    ) -> CompositeFeature<'static> {
        CompositeFeature {
            subset: set(children),
            arguments: vec![],
            bind_arguments: vec![],
            requirements: vec![],
            options: permutations
                .iter()
                .map(|&permutation| CompositeOption {
                    name: "option",
                    permutation,
                    defines: ShaderDefines { names: vec![], unknown: 0 },
                })
                .collect(),
        }
    }

    #[test]
    fn requirements_use_prior_one_hot_values_and_definition_masks() {
        let mut dependent = simple("dependent", 0, &[3, 1]);
        if let FeatureBody::Simple(s) = &mut dependent.body {
            s.requirements.push(OptionRequirement {
                option_mask: 1 << 3,
                expression: expr(&[[u32::MAX, 0]]),
            });
        }
        let features = set(vec![simple("first", 0, &[0, 1]), dependent]);
        let mut calls = 0;
        let permutation = features
            .select_permutation(|f, mask| {
                calls += 1;
                assert_eq!(f.name, "first");
                assert_eq!(mask, 3);
                Ok(Some(1))
            })
            .unwrap();
        // First local1 makes one-hot [false,true]; dependent is forced to definition1/local1.
        assert_eq!(permutation, Some([3, 0]));
        assert_eq!(calls, 1);
        assert_eq!(
            features
                .select_permutation_ignoring_requirements(|f, mask| {
                    Ok(Some(if f.name == "first" {
                        1
                    } else {
                        assert_eq!(mask, 10);
                        3
                    }))
                })
                .unwrap(),
            Some([1, 0])
        );
    }

    #[test]
    fn composite_arguments_seed_nested_conditions() {
        let mut child = simple("child", 0, &[0, 1]);
        if let FeatureBody::Simple(s) = &mut child.body {
            s.requirements.push(OptionRequirement {
                option_mask: 1,
                expression: expr(&[[u32::MAX, 0], [4, 0]]),
            });
        }
        let mut c = composite(vec![child], &[[1, 0], [0, 0]]);
        c.arguments
            .push(CompositeArgument { name: "argument", expression: expr(&[[u32::MAX, 1]]) });
        let features = set(vec![simple("first", 0, &[0, 1]), Feature {
            name: "composite",
            data_origin: 0,
            option_count: 2,
            body: FeatureBody::Composite(c),
        }]);
        assert_eq!(
            features
                .select_permutation(|f, _| {
                    assert_eq!(f.name, "first");
                    Ok(Some(1))
                })
                .unwrap(),
            Some([1, 0])
        );
        assert_eq!(features.select_permutation(|_, _| Ok(Some(0))).unwrap(), Some([2, 0]));
    }

    #[test]
    fn composite_last_option_is_unconditional_fallback() {
        let mut c = composite(vec![simple("child", 0, &[0, 1, 2])], &[[0, 0], [1, 0], [999, 0]]);
        c.requirements.push(OptionRequirement { option_mask: 4, expression: expr(&[[1, 0]]) });
        let features = set(vec![Feature {
            name: "composite",
            data_origin: 0,
            option_count: 3,
            body: FeatureBody::Composite(c),
        }]);
        assert_eq!(features.select_permutation(|_, _| Ok(Some(2))).unwrap(), Some([2, 0]));
        assert_eq!(features.select_permutation(|_, _| Ok(None)).unwrap(), None);
    }

    #[test]
    fn dynamic_layout_flattens_children_and_uses_explicit_selector() {
        let mut c = composite(vec![simple("ignored subset", 1, &[0, 1, 2])], &[[0, 0], [1, 0]]);
        c.requirements.push(OptionRequirement { option_mask: 3, expression: expr(&[[1, 0]]) });
        let explicit = Feature {
            name: "explicit",
            data_origin: 0,
            option_count: 2,
            body: FeatureBody::ExplicitComposite {
                composite: c,
                selector: Box::new(simple("selector", 0, &[0, 2])),
            },
        };
        assert_eq!(
            set(vec![explicit.clone()])
                .select_permutation(|f, _| {
                    assert_eq!(f.name, "selector");
                    Ok(Some(2))
                })
                .unwrap(),
            Some([1, 0])
        );
        let features = set(vec![
            Feature {
                name: "static parent",
                data_origin: 0,
                option_count: 1,
                body: FeatureBody::Composite(composite(vec![simple("excluded", 1, &[0, 1])], &[[
                    0, 0,
                ]])),
            },
            Feature {
                name: "dynamic parent",
                data_origin: 1,
                option_count: 1,
                body: FeatureBody::Composite(composite(
                    vec![simple("single", 0, &[0]), explicit],
                    &[[0, 0]],
                )),
            },
            simple("last", 1, &[0, 3, 6]),
        ]);
        let layout = features.dynamic_features(2).unwrap();
        assert_eq!(layout.len(), 3);
        assert_eq!(layout[1].feature_path, ["dynamic parent", "explicit", "selector"]);
        assert_eq!(layout.iter().map(|e| e.bit_offset).collect::<Vec<_>>(), [
            Some(0),
            None,
            Some(1)
        ]);
        assert_eq!(
            features
                .select_dynamic_runtime_id(2, |f, mask| {
                    assert_eq!(f.name, "last");
                    assert_eq!(mask, 73);
                    Ok(Some(6))
                })
                .unwrap(),
            Some(4)
        );
        assert_eq!(features.dynamic_options(4, 2).unwrap(), [Some(0), None, Some(2)]);
        assert!(features.dynamic_options(5, 2).is_err());
        assert!(features.dynamic_options(8, 2).is_err());
        assert_eq!(
            features.select_dynamic_runtime_id(u32::MAX, |_, _| unreachable!()).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn selection_failure_and_unavailable_conditions_are_explicit() {
        let mut feature = simple("test", 1, &[0, 3]);
        assert_eq!(
            set(vec![feature.clone()]).select_permutation(|_, _| Ok(Some(2))).unwrap(),
            None
        );
        if let FeatureBody::Simple(s) = &mut feature.body {
            s.requirements
                .push(OptionRequirement { option_mask: 8, expression: expr(&[[u32::MAX, 999]]) });
        }
        assert!(set(vec![feature.clone()]).select_permutation(|_, _| Ok(Some(0))).is_err());
        assert_eq!(
            set(vec![feature]).select_dynamic_runtime_id(0, |_, _| Ok(Some(3))).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn material_state_drives_mapped_local_indices() {
        use crate::format::{
            cmdl::EMaterialDataId,
            material_predicates::{MaterialSelectionState, SelectionValue},
        };
        let mut flag = simple("instanced", 1, &[1, 0]);
        if let FeatureBody::Simple(s) = &mut flag.body {
            s.definition = "MFIN";
        }
        let mut light = simple("lighting", 1, &[2, 0, 3]);
        if let FeatureBody::Simple(s) = &mut light.body {
            s.definition = "DDLT";
            s.data_ids = vec!["DLLD"];
        }
        let features = set(vec![flag, light]);
        let mut state = MaterialSelectionState::default();
        state.set(EMaterialDataId::DLLD, SelectionValue::DirectionalLights {
            unshadowed: 1,
            shadowed: 0,
        });
        assert_eq!(
            features.select_dynamic_runtime_id(0, |f, m| state.select_feature(f, m)).unwrap(),
            Some(1)
        );
        assert_eq!(
            features.select_permutation(|f, m| state.select_feature(f, m)).unwrap(),
            Some([1, 0])
        );
        state.flags = 1 << 7;
        state.set(EMaterialDataId::DLLD, SelectionValue::DirectionalLights {
            unshadowed: 0,
            shadowed: 1,
        });
        assert_eq!(
            features.select_dynamic_runtime_id(0, |f, m| state.select_feature(f, m)).unwrap(),
            Some(4)
        );
        assert_eq!(
            features.select_permutation(|f, m| state.select_feature(f, m)).unwrap(),
            Some([4, 0])
        );
    }

    #[test]
    fn permutation_builder_matches_confirmed_cross_word_quirk() {
        let mut features = vec![simple("one", 1, &[0]); 63];
        features.push(simple("cross", 1, &[0, 1, 2, 3]));
        let features = set(features);
        assert_eq!(features.select_permutation(|_, _| Ok(Some(3))).unwrap(), Some([1 << 63, 0]));
        assert!(features.dynamic_features(0).is_err()); // 65 bits cannot fit a runtime key.
    }
}
