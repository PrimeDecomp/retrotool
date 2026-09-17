use std::collections::BTreeMap;

use super::{invalid, Binding, Resource, TextureValueType};
use crate::{
    ir::{
        analysis::Constants,
        texture::{FetchLevel, Kind, SampleLevel},
        Function,
    },
    maxwell::texture::TextureDimension,
    program::{wgsl::unsupported, Diagnostic},
};

pub(in crate::program::wgsl) fn plan(
    function: &Function,
    constants: &Constants,
    bindings: &[Binding],
) -> Result<Vec<Resource>, Diagnostic> {
    let mut supplied = BTreeMap::new();
    for &binding in bindings {
        if supplied.insert(binding.view.texture_index, binding).is_some() {
            return Err(invalid(None, "duplicate texture descriptor binding"));
        }
    }
    let mut resources = Vec::<Resource>::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        let crate::ir::Operation::Texture(operation) = &instruction.operation else {
            continue;
        };
        let binding = *supplied.get(&operation.descriptor).ok_or_else(|| {
            unsupported(instruction.pc, "texture descriptor requires a declared host binding")
        })?;
        let view = binding.view;
        // A sampled 2D resource is also a one-layer array. Conversely, a 2D
        // sample of an array selects its first layer. Keep the actual view
        // type: emission adapts the layer argument, not the supplied resource.
        // Loads retain their separate bounds and dimensionality contract.
        let compatible_sampling = matches!(operation.kind, Kind::Sample { .. })
            && matches!(view.dimension, TextureDimension::D2 | TextureDimension::D2Array)
            && matches!(operation.dimension, TextureDimension::D2 | TextureDimension::D2Array);
        if view.dimension != operation.dimension && !compatible_sampling {
            return Err(invalid(
                Some(instruction.pc),
                &format!(
                    "texture descriptor {} has view dimension {:?}; the operation requires {:?}",
                    operation.descriptor, view.dimension, operation.dimension,
                ),
            ));
        }
        let multisample = matches!(operation.kind, Kind::Load { level: FetchLevel::Sample(_), .. });
        if view.multisample != multisample {
            return Err(invalid(
                Some(instruction.pc),
                "texture view sample-count class does not match the operation",
            ));
        }
        if let Some(sampling) = view.sampling {
            sampling
                .validate()
                .map_err(|error| invalid(Some(instruction.pc), &error.to_string()))?;
        }
        if !view.lod_bias.is_finite() || !(-16.0..16.0).contains(&view.lod_bias) {
            return Err(invalid(
                Some(instruction.pc),
                "texture bias is outside the supported WGSL range",
            ));
        }
        if view.value_type == TextureValueType::Depth
            && !matches!(view.dimension, TextureDimension::D2 | TextureDimension::D2Array)
        {
            return Err(unsupported(instruction.pc, "unsupported depth view dimension"));
        }
        let (uses_sampler, uses_comparison_sampler, uses_implicit_lod) = match operation.kind {
            Kind::Sample { level, comparison } => {
                if binding.sampler_binding.is_none() {
                    return Err(invalid(
                        Some(instruction.pc),
                        "sampled texture requires a sampler binding",
                    ));
                }
                if operation.dimension == TextureDimension::D1 {
                    return Err(unsupported(
                        instruction.pc,
                        "WGSL filtered 1D sampling needs a separate view/provider",
                    ));
                }
                if !matches!(view.value_type, TextureValueType::Float | TextureValueType::Depth) {
                    return Err(unsupported(
                        instruction.pc,
                        "integer filtered sampling needs a separate sampler provider",
                    ));
                }
                if comparison.is_some() && view.value_type != TextureValueType::Depth {
                    return Err(invalid(
                        Some(instruction.pc),
                        "depth comparison requires a depth view",
                    ));
                }
                if comparison.is_some() {
                    match level {
                        SampleLevel::Implicit if view.lod_bias != 0.0 => {
                            return Err(unsupported(
                                instruction.pc,
                                "comparison sampling with implicit bias needs a separate provider",
                            ))
                        }
                        SampleLevel::Explicit(value)
                            if !matches!(constants.word(value), Some(0 | 0x80000000)) =>
                        {
                            return Err(unsupported(
                                instruction.pc,
                                "nonzero or dynamic comparison LOD needs a separate provider",
                            ))
                        }
                        _ => {}
                    }
                } else if view.value_type == TextureValueType::Depth {
                    match level {
                        SampleLevel::Implicit if view.lod_bias != 0.0 => {
                            return Err(unsupported(
                                instruction.pc,
                                "ordinary depth sampling with bias needs a separate provider",
                            ));
                        }
                        SampleLevel::Explicit(value)
                            if !matches!(constants.word(value), Some(0 | 0x80000000)) =>
                        {
                            // WGSL depth LOD is integer. Converting a dynamic
                            // Maxwell floating LOD would lose mip filtering.
                            return Err(unsupported(
                                instruction.pc,
                                "nonzero or dynamic ordinary depth LOD needs a separate provider",
                            ));
                        }
                        _ => {}
                    }
                }
                (true, comparison.is_some(), matches!(level, SampleLevel::Implicit))
            }
            Kind::Load { .. } => (false, false, false),
        };
        if view.value_type == TextureValueType::Depth
            && !uses_comparison_sampler
            && binding.depth_components.is_none()
        {
            return Err(invalid(
                Some(instruction.pc),
                "ordinary depth access requires a logical component mapping",
            ));
        }
        let slot = resources
            .iter()
            .position(|resource| resource.binding.view.texture_index == operation.descriptor)
            .unwrap_or_else(|| {
                resources.push(Resource {
                    binding,
                    uses_sampler: false,
                    comparison_sampler: uses_comparison_sampler,
                    uses_implicit_lod: false,
                    uses_fixed_implicit_lod: false,
                    helper_results_optional: false,
                });
                resources.len() - 1
            });
        let resource = &mut resources[slot];
        if uses_sampler
            && resource.uses_sampler
            && resource.comparison_sampler != uses_comparison_sampler
        {
            return Err(unsupported(
                instruction.pc,
                "one descriptor uses both ordinary and comparison sampling",
            ));
        }
        if uses_sampler {
            resource.comparison_sampler = uses_comparison_sampler;
        }
        resource.uses_sampler |= uses_sampler;
        resource.uses_implicit_lod |= uses_implicit_lod;
        resource.uses_fixed_implicit_lod |= uses_implicit_lod && view.implicit_lod_is_fixed();
        resource.helper_results_optional |= operation.skip_helpers;
    }
    Ok(resources)
}
