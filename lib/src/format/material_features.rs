//! MPR featured-material metadata (MTRL archive v22).
//! The payload is 16 optional technique entries, not a single feature graph.

use anyhow::{bail, ensure, Context, Result};
use serde_derive::Serialize;

use crate::format::{material_archive::Reader, material_feature_semantics::semantic_names};

/// CBoolExpressionEvaluator's postfix instruction stream. Operand is an
/// identifier index for opcode 0xffffffff; operators ignore the operand word.
#[derive(Clone, Debug, Serialize)]
pub struct BoolExpression {
    pub instructions: Vec<[u32; 2]>,
}

impl BoolExpression {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let count = r.count(8)?;
        let instructions = (0..count).map(|_| Ok([r.u32()?, r.u32()?])).collect::<Result<_>>()?;
        Ok(Self { instructions })
    }

    /// Evaluate using engine opcodes. Empty expressions evaluate false. Rejects
    /// stack underflow and unknown opcodes instead of accepting malformed data.
    pub fn evaluate(&self, mut identifier: impl FnMut(u32) -> Result<bool>) -> Result<bool> {
        if self.instructions.is_empty() {
            return Ok(false);
        }
        let mut stack: Vec<bool> = Vec::new();
        for &[opcode, operand] in &self.instructions {
            let value = match opcode {
                0 => true,
                1 => false,
                u32::MAX => identifier(operand)?,
                4 => !stack.pop().context("Boolean expression stack underflow")?,
                2 | 3 | 5 | 6 | 7 => {
                    let rhs = stack.pop().context("Boolean expression stack underflow")?;
                    let lhs = stack.pop().context("Boolean expression stack underflow")?;
                    match opcode {
                        2 => lhs == rhs,
                        3 => lhs != rhs,
                        5 => !lhs || rhs,
                        6 => lhs && rhs,
                        7 => lhs || rhs,
                        _ => unreachable!(),
                    }
                }
                _ => bail!("Unknown boolean-expression opcode {opcode:#x}"),
            };
            stack.push(value);
        }
        ensure!(stack.len() == 1, "Boolean expression leaves multiple results");
        Ok(stack[0])
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ShaderDefines<'a> {
    pub names: Vec<&'a str>,
    pub unknown: u32,
}

impl<'a> ShaderDefines<'a> {
    fn read(r: &mut Reader<'a>) -> Result<Self> {
        let count = r.count(4)?;
        let names = (0..count).map(|_| r.string()).collect::<Result<_>>()?;
        Ok(Self { names, unknown: r.u32()? })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ShaderBinding<'a> {
    pub semantic: &'a str,
    pub binding: &'a str,
}

#[derive(Clone, Debug, Serialize)]
pub struct SimpleOption<'a> {
    /// Option number in the engine feature definition (not its serialized index).
    pub definition_option: u32,
    pub defines: ShaderDefines<'a>,
    pub bindings: Vec<ShaderBinding<'a>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OptionRequirement {
    /// Bits to remove when expression is false: definition options for simple
    /// features, local serialized options for composites.
    pub option_mask: u32,
    pub expression: BoolExpression,
}

fn requirements(r: &mut Reader<'_>) -> Result<Vec<OptionRequirement>> {
    let count = r.count(8)?;
    (0..count)
        .map(|_| {
            Ok(OptionRequirement { option_mask: r.u32()?, expression: BoolExpression::read(r)? })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct SimpleFeature<'a> {
    pub definition: &'a str,
    pub data_ids: Vec<&'a str>,
    pub flag_expressions: Vec<BoolExpression>,
    pub options: Vec<SimpleOption<'a>>,
    /// Definition-option bits initially permitted by this feature instance.
    pub allowed_options: u32,
    /// Definition option -> serialized option index; 0xffffffff means absent.
    pub option_map: Vec<u32>,
    pub requirements: Vec<OptionRequirement>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompositeArgument<'a> {
    pub name: &'a str,
    pub expression: BoolExpression,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompositeOption<'a> {
    pub name: &'a str,
    pub permutation: [u64; 2],
    pub defines: ShaderDefines<'a>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompositeFeature<'a> {
    pub subset: FeatureSet<'a>,
    pub arguments: Vec<CompositeArgument<'a>>,
    pub bind_arguments: Vec<CompositeArgument<'a>>,
    pub options: Vec<CompositeOption<'a>>,
    pub requirements: Vec<OptionRequirement>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum FeatureBody<'a> {
    Simple(SimpleFeature<'a>),
    Composite(CompositeFeature<'a>),
    ExplicitComposite { composite: CompositeFeature<'a>, selector: Box<Feature<'a>> },
}

#[derive(Clone, Debug, Serialize)]
pub struct Feature<'a> {
    pub name: &'a str,
    pub data_origin: u32,
    pub option_count: u32,
    pub body: FeatureBody<'a>,
}

impl<'a> Feature<'a> {
    /// Runtime uses LogBase2(option_count - 1) + 1, with LogBase2(0) == 0.
    /// Even a feature with exactly one option occupies one permutation bit.
    pub fn option_bit_count(&self) -> u32 {
        (32 - self.option_count.saturating_sub(1).leading_zeros()).max(1)
    }

    fn read(r: &mut Reader<'a>, depth: usize) -> Result<Self> {
        ensure!(depth < 128, "Material feature graph nesting exceeds supported depth");
        let kind = r.u32()?;
        let name = r.string()?;
        let data_origin = r.u32()?;
        let option_count = r.u32()?;
        ensure!(option_count != 0, "Feature has no options");
        let body = match kind {
            0 => {
                let definition = std::str::from_utf8(r.take(4)?)?;
                let count = r.count(4)?;
                ensure!(count <= 16, "Too many feature data IDs");
                let data_ids = (0..count)
                    .map(|_| Ok(std::str::from_utf8(r.take(4)?)?))
                    .collect::<Result<_>>()?;
                let count = r.count(4)?;
                let flag_expressions =
                    (0..count).map(|_| BoolExpression::read(r)).collect::<Result<_>>()?;
                let count = r.count(16)?;
                let mut options = Vec::new();
                for _ in 0..count {
                    let definition_option = r.u32()?;
                    let defines = ShaderDefines::read(r)?;
                    let count = r.count(8)?;
                    let bindings = (0..count)
                        .map(|_| Ok(ShaderBinding { semantic: r.string()?, binding: r.string()? }))
                        .collect::<Result<_>>()?;
                    options.push(SimpleOption { definition_option, defines, bindings });
                }
                let allowed_options = r.u32()?;
                let count = r.count(4)?;
                ensure!(count <= 32, "Too many feature option mappings");
                let option_map = (0..count).map(|_| r.u32()).collect::<Result<_>>()?;
                let requirements = requirements(r)?;
                FeatureBody::Simple(SimpleFeature {
                    definition,
                    data_ids,
                    flag_expressions,
                    options,
                    allowed_options,
                    option_map,
                    requirements,
                })
            }
            1 | 2 => {
                let subset = FeatureSet::read(r, depth + 1)?;
                let mut arguments = || -> Result<Vec<CompositeArgument<'a>>> {
                    let count = r.count(8)?;
                    (0..count)
                        .map(|_| {
                            Ok(CompositeArgument {
                                name: r.string()?,
                                expression: BoolExpression::read(r)?,
                            })
                        })
                        .collect()
                };
                let arguments1 = arguments()?;
                let bind_arguments = arguments()?;
                let count = r.count(28)?;
                let options = (0..count)
                    .map(|_| {
                        Ok(CompositeOption {
                            name: r.string()?,
                            permutation: [r.u64()?, r.u64()?],
                            defines: ShaderDefines::read(r)?,
                        })
                    })
                    .collect::<Result<_>>()?;
                let requirements = requirements(r)?;
                let composite = CompositeFeature {
                    subset,
                    arguments: arguments1,
                    bind_arguments,
                    options,
                    requirements,
                };
                if kind == 2 {
                    FeatureBody::ExplicitComposite {
                        composite,
                        selector: Box::new(Feature::read(r, depth + 1)?),
                    }
                } else {
                    FeatureBody::Composite(composite)
                }
            }
            _ => bail!("Unknown feature instance kind {kind}"),
        };
        Ok(Self { name, data_origin, option_count, body })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FeatureSet<'a> {
    pub features: Vec<Feature<'a>>,
    pub expression: BoolExpression,
}

/// One entry in CShader_Featured's packed variable vector. An unused semantic
/// still consumes an entry, normally 0xffff; it must not be dropped during traversal.
#[derive(Clone, Debug, Serialize)]
pub struct FeatureVariable<'a> {
    pub feature_path: Vec<&'a str>,
    pub definition: &'a str,
    pub data_ids: Vec<&'a str>,
    pub data_origin: u32,
    pub definition_option: u32,
    /// Evaluated against all selected options in this set, after composite bind
    /// argument prefixes. Flag i sets bit i%32, matching the native binder.
    pub binding_flags: u32,
    pub semantic: &'static str,
    pub binding: Option<&'a str>,
    pub packed: u16,
}

/// One selected simple feature, including definitions with no shader variables.
#[derive(Clone, Debug, Serialize)]
pub struct FeatureChoice<'a> {
    pub feature_path: Vec<&'a str>,
    pub definition: &'a str,
    pub data_ids: Vec<&'a str>,
    pub data_origin: u32,
    pub definition_option: u32,
}

impl<'a> FeatureSet<'a> {
    fn read(r: &mut Reader<'a>, depth: usize) -> Result<Self> {
        let count = r.count(16)?;
        let features = (0..count).map(|_| Feature::read(r, depth)).collect::<Result<Vec<_>>>()?;
        ensure!(
            features.iter().map(Feature::option_bit_count).sum::<u32>() <= 128,
            "Feature set exceeds 128 permutation bits"
        );
        Ok(Self { features, expression: BoolExpression::read(r)? })
    }

    /// Local serialized option indices in feature order, including one-option
    /// features. Composite options contain a separate permutation for their subset.
    pub fn selected_options(&self, permutation: [u64; 2]) -> Result<Vec<u32>> {
        let bits = u128::from(permutation[0]) | (u128::from(permutation[1]) << 64);
        let mut offset = 0;
        let mut out = Vec::new();
        for feature in &self.features {
            let width = feature.option_bit_count();
            ensure!(offset + width <= 128, "Feature permutation exceeds 128 bits");
            let option = ((bits >> offset) & ((1u128 << width) - 1)) as u32;
            ensure!(
                option < feature.option_count,
                "Invalid option {option} for feature {}",
                feature.name
            );
            out.push(option);
            offset += width;
        }
        Ok(out)
    }

    /// Expand the selected composite subsets into definition choices. Unlike
    /// bind_variables, this also includes flags and other variable-free features.
    pub fn selected_features(&self, permutation: [u64; 2]) -> Result<Vec<FeatureChoice<'a>>> {
        let mut out = Vec::new();
        self.append_choices(permutation, &mut Vec::new(), &mut out, 0)?;
        Ok(out)
    }

    fn append_choices(
        &self,
        permutation: [u64; 2],
        path: &mut Vec<&'a str>,
        out: &mut Vec<FeatureChoice<'a>>,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth < 128, "Material feature graph nesting exceeds supported depth");
        for (feature, option) in self.features.iter().zip(self.selected_options(permutation)?) {
            path.push(feature.name);
            match &feature.body {
                FeatureBody::Simple(simple) => {
                    let option = simple
                        .options
                        .get(option as usize)
                        .context("Missing simple feature option")?;
                    out.push(FeatureChoice {
                        feature_path: path.clone(),
                        definition: simple.definition,
                        data_ids: simple.data_ids.clone(),
                        data_origin: feature.data_origin,
                        definition_option: option.definition_option,
                    });
                }
                FeatureBody::Composite(composite)
                | FeatureBody::ExplicitComposite { composite, .. } => {
                    let option = composite
                        .options
                        .get(option as usize)
                        .context("Missing composite feature option")?;
                    composite.subset.append_choices(option.permutation, path, out, depth + 1)?;
                }
            }
            path.pop();
        }
        Ok(())
    }

    /// Recover semantic/binding names in engine order, and require exact variable
    /// consumption. Also evaluates binding flags using the entire set's one-hot
    /// option vector (including later features), plus composite bind arguments.
    /// Composite options select their own subset permutation.
    pub fn bind_variables(
        &self,
        permutation: [u64; 2],
        variables: &[u16],
    ) -> Result<Vec<FeatureVariable<'a>>> {
        let mut remain = variables;
        let mut out = Vec::new();
        self.append_variables(permutation, &[], &mut remain, &mut Vec::new(), &mut out, 0)?;
        ensure!(remain.is_empty(), "Unconsumed featured shader variables: {}", remain.len());
        Ok(out)
    }

    fn append_variables(
        &self,
        permutation: [u64; 2],
        bind_arguments: &[bool],
        variables: &mut &[u16],
        path: &mut Vec<&'a str>,
        out: &mut Vec<FeatureVariable<'a>>,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth < 128, "Material feature graph nesting exceeds supported depth");
        // CFeatureSet::AccumulateOptionBindVariablesFuncs (0x710014c18c) appends
        // ALL option values before visiting any feature. This differs from the
        // selection path, whose condition vector grows after each choice.
        let mut values = bind_arguments.to_vec();
        values.extend(self.option_values(permutation)?);
        ensure!(values.len() <= 2048, "Feature binding state exceeds engine's 2048-bit capacity");
        let evaluate = |expression: &BoolExpression| {
            expression.evaluate(|index| {
                values
                    .get(index as usize)
                    .copied()
                    .context("Binding expression references unavailable option/argument")
            })
        };
        for (feature, option) in self.features.iter().zip(self.selected_options(permutation)?) {
            path.push(feature.name);
            match &feature.body {
                FeatureBody::Simple(simple) => {
                    // CSimpleFeatureInstance at 0x7100152cb0: empty expressions
                    // do not set a flag. ARM64's 32-bit shift wraps the index.
                    let mut binding_flags = 0;
                    for (i, expression) in simple.flag_expressions.iter().enumerate() {
                        if evaluate(expression)? {
                            binding_flags |= 1 << (i & 31);
                        }
                    }
                    let names = semantic_names(simple.definition).with_context(|| {
                        format!("Unknown feature definition {}", simple.definition)
                    })?;
                    let option = simple
                        .options
                        .get(option as usize)
                        .context("Missing simple feature option")?;
                    ensure!(variables.len() >= names.len(), "Truncated featured shader variables");
                    let (entries, remain) = variables.split_at(names.len());
                    for (&semantic, &packed) in names.iter().zip(entries) {
                        out.push(FeatureVariable {
                            feature_path: path.clone(),
                            definition: simple.definition,
                            data_ids: simple.data_ids.clone(),
                            data_origin: feature.data_origin,
                            definition_option: option.definition_option,
                            binding_flags,
                            semantic,
                            binding: option
                                .bindings
                                .iter()
                                .find(|b| b.semantic == semantic)
                                .map(|b| b.binding),
                            packed,
                        });
                    }
                    *variables = remain;
                }
                FeatureBody::Composite(composite)
                | FeatureBody::ExplicitComposite { composite, .. } => {
                    // The explicit selector chooses an option; variable binding
                    // uses the inherited composite implementation (0x710015c55c).
                    let option = composite
                        .options
                        .get(option as usize)
                        .context("Missing composite feature option")?;
                    // Composite binder 0x710015c55c passes its SECOND argument
                    // list (+0x88) to helper0x710015c2f0, evaluated against the
                    // outer complete vector. Selection uses the first list.
                    let arguments = composite
                        .bind_arguments
                        .iter()
                        .map(|argument| evaluate(&argument.expression))
                        .collect::<Result<Vec<_>>>()?;
                    composite.subset.append_variables(
                        option.permutation,
                        &arguments,
                        variables,
                        path,
                        out,
                        depth + 1,
                    )?;
                }
            }
            path.pop();
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlendTarget {
    pub preset: i32,
    pub color_equation: u32,
    pub color_source: u32,
    pub color_destination: u32,
    pub alpha_equation: u32,
    pub alpha_source: u32,
    pub alpha_destination: u32,
    pub constant_color: [f32; 4],
    pub logic_op: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlendState {
    /// Bits select render targets; 0xffffffff applies targets[0] to every target.
    /// This word is NOT an EBlendMode preset (each target has its own preset).
    pub target_mask: u32,
    pub targets: Vec<BlendTarget>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GraphicsState {
    pub flags: u32,
    pub cull_mode: Option<u8>,
    pub front_face: Option<u8>,
    /// Depth test enabled, comparison function, depth write enabled.
    pub depth: Option<[u32; 3]>,
    pub depth_bias: Option<[f32; 3]>,
    /// Stencil test enabled, comparison, and three EStencilFunction operations.
    pub stencil: Option<[u32; 5]>,
    pub stencil_masks: Option<[u8; 3]>,
    pub alpha_to_coverage: Option<u8>,
    pub blend: Option<BlendState>,
    pub color_mask: Option<[u8; 2]>,
    pub fill_mode: Option<u32>,
    pub clear_color: Option<[f32; 4]>,
    pub clear_depth: Option<f32>,
    pub clear_stencil: Option<u32>,
    pub raster_control: Option<[u32; 2]>,
    pub scissors: Option<Vec<[i32; 4]>>,
    pub line_width: Option<f32>,
}

fn float(r: &mut Reader<'_>) -> Result<f32> { Ok(f32::from_bits(r.u32()?)) }
fn color(r: &mut Reader<'_>) -> Result<[f32; 4]> {
    Ok([float(r)?, float(r)?, float(r)?, float(r)?])
}

impl GraphicsState {
    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let flags = r.u32()?;
        macro_rules! field {
            ($bit:expr,$value:expr) => {
                if flags & (1 << $bit) != 0 {
                    Some($value)
                } else {
                    None
                }
            };
        }
        let cull_mode = field!(1, r.u8()?);
        let front_face = field!(2, r.u8()?);
        let depth = field!(3, [r.u32()?, r.u32()?, r.u32()?]);
        let depth_bias = field!(12, [float(r)?, float(r)?, float(r)?]);
        let stencil = field!(4, [r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
        let stencil_masks = field!(11, [r.u8()?, r.u8()?, r.u8()?]);
        let alpha_to_coverage = field!(13, r.u8()?);
        let blend = if flags & 1 != 0 {
            let target_mask = r.u32()?;
            let count = r.count(48)?;
            let targets = (0..count)
                .map(|_| {
                    Ok(BlendTarget {
                        preset: r.i32()?,
                        color_equation: r.u32()?,
                        color_source: r.u32()?,
                        color_destination: r.u32()?,
                        alpha_equation: r.u32()?,
                        alpha_source: r.u32()?,
                        alpha_destination: r.u32()?,
                        constant_color: color(r)?,
                        logic_op: r.i32()?,
                    })
                })
                .collect::<Result<_>>()?;
            Some(BlendState { target_mask, targets })
        } else {
            None
        };
        let color_mask = field!(10, [r.u8()?, r.u8()?]);
        let fill_mode = field!(5, r.u32()?);
        let clear_color = field!(6, color(r)?);
        let clear_depth = field!(7, float(r)?);
        let clear_stencil = field!(8, r.u32()?);
        let raster_control = field!(14, [r.u32()?, r.u32()?]);
        let scissors = if flags & (1 << 9) != 0 {
            let count = r.count(16)?;
            Some(
                (0..count)
                    .map(|_| Ok([r.i32()?, r.i32()?, r.i32()?, r.i32()?]))
                    .collect::<Result<_>>()?,
            )
        } else {
            None
        };
        let line_width = field!(15, float(r)?);
        Ok(Self {
            flags,
            cull_mode,
            front_face,
            depth,
            depth_bias,
            stencil,
            stencil_masks,
            alpha_to_coverage,
            blend,
            color_mask,
            fill_mode,
            clear_color,
            clear_depth,
            clear_stencil,
            raster_control,
            scissors,
            line_width,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StateOverride {
    pub expression: BoolExpression,
    pub state: GraphicsState,
}

#[derive(Clone, Debug, Serialize)]
pub struct Technique<'a> {
    pub features: FeatureSet<'a>,
    pub state: GraphicsState,
    pub overrides: Vec<StateOverride>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FeaturedMaterial<'a> {
    pub techniques: [Option<Technique<'a>>; 16],
}

impl<'a> FeaturedMaterial<'a> {
    /// Code -> runtime array index, initialized at 0x710014f0f0 and by MPR's
    /// five overrides at 0x7100b3da90 (ThermalHot and four XRay techniques).
    pub fn technique_index(code: u8) -> Option<usize> {
        b"xzgifswmhrayb".iter().position(|&v| v == code)
    }

    pub fn technique(&self, code: u8) -> Option<&Technique<'a>> {
        self.techniques[Self::technique_index(code)?].as_ref()
    }

    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let mut techniques = std::array::from_fn(|_| None);
        for technique in &mut techniques {
            if r.u8()? != 0 {
                let features = FeatureSet::read(&mut r, 0)?;
                let state = GraphicsState::read(&mut r)?;
                let count = r.count(8)?;
                let overrides = (0..count)
                    .map(|_| {
                        Ok(StateOverride {
                            expression: BoolExpression::read(&mut r)?,
                            state: GraphicsState::read(&mut r)?,
                        })
                    })
                    .collect::<Result<_>>()?;
                *technique = Some(Technique { features, state, overrides });
            }
        }
        r.finish()?;
        Ok(Self { techniques })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_expression() -> BoolExpression { BoolExpression { instructions: vec![] } }
    fn words(data: &mut Vec<u8>, values: &[u32]) {
        for v in values {
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn floats(data: &mut Vec<u8>, values: &[f32]) {
        for v in values {
            words(data, &[v.to_bits()]);
        }
    }

    #[test]
    fn boolean_engine_truth_tables_and_malformed_stacks() {
        for (opcode, expected) in [
            (2, [true, false, false, true]),
            (3, [false, true, true, false]),
            (5, [true, true, false, true]),
            (6, [false, false, false, true]),
            (7, [false, true, true, true]),
        ] {
            for (i, &value) in expected.iter().enumerate() {
                let expression = BoolExpression {
                    instructions: vec![[u32::MAX, 10], [u32::MAX, 20], [opcode, 0]],
                };
                assert_eq!(
                    expression
                        .evaluate(|id| Ok(if id == 10 { i & 2 != 0 } else { i & 1 != 0 }))
                        .unwrap(),
                    value
                );
            }
        }
        assert!(!empty_expression().evaluate(|_| unreachable!()).unwrap());
        for (opcode, expected) in [(0, true), (1, false)] {
            let mut expression = BoolExpression { instructions: vec![[opcode, 0]] };
            assert_eq!(expression.evaluate(|_| unreachable!()).unwrap(), expected);
            expression.instructions.push([4, 0]);
            assert_eq!(expression.evaluate(|_| unreachable!()).unwrap(), !expected);
        }
        for instructions in [vec![[6, 0]], vec![[0, 0], [0, 0]], vec![[8, 0]], vec![[4, 0]]] {
            assert!(BoolExpression { instructions }.evaluate(|_| unreachable!()).is_err());
        }
    }

    fn simple(name: &'static str, definition: &'static str, option_count: u32) -> Feature<'static> {
        Feature {
            name,
            data_origin: 0,
            option_count,
            body: FeatureBody::Simple(SimpleFeature {
                definition,
                data_ids: vec![],
                flag_expressions: vec![],
                options: (0..option_count.min(5))
                    .map(|index| SimpleOption {
                        definition_option: index + 10,
                        defines: ShaderDefines { names: vec![], unknown: 0 },
                        bindings: vec![ShaderBinding { semantic: "Value", binding: "uc_value" }],
                    })
                    .collect(),
                allowed_options: 0,
                option_map: vec![],
                requirements: vec![],
            }),
        }
    }

    #[test]
    fn permutation_counts_one_option_features_and_crosses_word_boundary() {
        let mut set = FeatureSet {
            features: vec![simple("flag", "POD_", 1); 63],
            expression: empty_expression(),
        };
        set.features.push(simple("wide", "POD_", 5));
        let options = set.selected_options([1 << 63, 1]).unwrap();
        assert!(options[..63].iter().all(|&v| v == 0));
        assert_eq!(options[63], 3);
        assert_eq!(set.selected_options([0, 2]).unwrap()[63], 4);
        assert!(set.selected_options([1 << 63, 2]).is_err()); // 5 exceeds option count
        assert!(set.selected_options([1, 0]).is_err()); // one-option feature cannot select 1
        set.features = vec![simple("flag", "POD_", 1); 129];
        assert!(set.selected_options([0, 0]).is_err());
    }

    #[test]
    fn selected_features_include_definitions_without_variables() {
        let set = FeatureSet {
            features: vec![simple("flag", "AERO", 2)],
            expression: empty_expression(),
        };
        assert!(set.bind_variables([1, 0], &[]).unwrap().is_empty());
        let choices = set.selected_features([1, 0]).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].feature_path, ["flag"]);
        assert_eq!(choices[0].definition_option, 11);
    }

    #[test]
    fn binding_traversal_keeps_unused_semantics_and_uses_composite_permutation() {
        let composite = CompositeFeature {
            subset: FeatureSet {
                features: vec![simple("value", "POD_", 2)],
                expression: empty_expression(),
            },
            arguments: vec![],
            bind_arguments: vec![],
            options: vec![CompositeOption {
                name: "chosen",
                permutation: [1, 0],
                defines: ShaderDefines { names: vec![], unknown: 0 },
            }],
            requirements: vec![],
        };
        let set = FeatureSet {
            features: vec![simple("layers", "LAYR", 1), Feature {
                name: "outer",
                data_origin: 0,
                option_count: 1,
                body: FeatureBody::ExplicitComposite {
                    composite,
                    selector: Box::new(simple("selector", "POD_", 1)),
                },
            }],
            expression: empty_expression(),
        };
        let choices = set.selected_features([0, 0]).unwrap();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[1].feature_path, ["outer", "value"]);
        assert_eq!(choices[1].definition_option, 11);
        let mut packed = vec![u16::MAX; 9];
        packed.push(8);
        let variables = set.bind_variables([0, 0], &packed).unwrap();
        assert_eq!(variables.len(), 10);
        assert_eq!(variables[2].semantic, "BlendTexture");
        assert_eq!(variables[2].binding, None);
        assert_eq!(variables[9].feature_path, ["outer", "value"]);
        assert_eq!(variables[9].definition_option, 11);
        assert_eq!(variables[9].binding, Some("uc_value"));
        assert_eq!(variables[9].packed, 8);
        assert!(set.bind_variables([0, 0], &packed[..9]).is_err());
        packed.push(u16::MAX);
        assert!(set.bind_variables([0, 0], &packed).is_err());
    }

    #[test]
    fn binding_flags_use_complete_sets_and_second_composite_argument_list() {
        let identifier = |index| BoolExpression { instructions: vec![[u32::MAX, index]] };
        let mut texture = simple("texture", "TEXS", 1);
        let FeatureBody::Simple(body) = &mut texture.body else { unreachable!() };
        body.flag_expressions = vec![empty_expression(); 34];
        // Child vector: bind arguments [true, false], then selected one-hot
        // groups [true], [false, true]. Even the later feature is visible.
        body.flag_expressions[0] = identifier(0);
        body.flag_expressions[1] = identifier(1);
        body.flag_expressions[2] = identifier(4);
        body.flag_expressions[33] = identifier(0); // Native u32 shift wraps to bit1.
        let composite = CompositeFeature {
            subset: FeatureSet {
                features: vec![texture, simple("later child", "AERO", 2)],
                expression: empty_expression(),
            },
            arguments: vec![CompositeArgument {
                name: "selection only",
                expression: empty_expression(),
            }],
            bind_arguments: vec![
                CompositeArgument { name: "later outer option", expression: identifier(2) },
                CompositeArgument { name: "empty", expression: empty_expression() },
            ],
            options: vec![CompositeOption {
                name: "chosen",
                permutation: [2, 0],
                defines: ShaderDefines { names: vec![], unknown: 0 },
            }],
            requirements: vec![],
        };
        for body in [FeatureBody::Composite(composite.clone()), FeatureBody::ExplicitComposite {
            composite,
            selector: Box::new(simple("selector consumes no bindings", "TEXS", 1)),
        }] {
            let set = FeatureSet {
                features: vec![
                    Feature { name: "outer", data_origin: 0, option_count: 1, body },
                    simple("later outer", "AERO", 2),
                ],
                expression: empty_expression(),
            };
            let variables = set.bind_variables([2, 0], &[u16::MAX; 3]).unwrap();
            assert_eq!(variables.len(), 3);
            assert!(variables.iter().all(|v| v.binding_flags == 7));
            // Choosing the other later outer option clears both argument bits.
            assert!(set
                .bind_variables([0, 0], &[u16::MAX; 3])
                .unwrap()
                .iter()
                .all(|v| v.binding_flags == 4));
        }
    }

    #[test]
    fn graphics_state_reads_every_conditional_field_in_wire_order() {
        let mut data = vec![];
        words(&mut data, &[0xffff]);
        data.extend_from_slice(&[2, 1]); // cull, front face
        words(&mut data, &[1, 3, 1]); // depth
        floats(&mut data, &[1.0, 2.0, 3.0]);
        words(&mut data, &[1, 4, 5, 6, 7]); // stencil
        data.extend_from_slice(&[8, 9, 10, 1]); // masks, alpha-to-coverage
        words(&mut data, &[u32::MAX, 1, u32::MAX, 11, 12, 13, 14, 15, 16]);
        floats(&mut data, &[0.25, 0.5, 0.75, 1.0]);
        words(&mut data, &[u32::MAX]); // blend logic op
        data.extend_from_slice(&[2, 15]);
        words(&mut data, &[17]);
        floats(&mut data, &[4.0, 5.0, 6.0, 7.0, 0.5]);
        words(&mut data, &[18, 19, 20, 1, u32::MAX, 22, 23, 24]);
        floats(&mut data, &[2.5]);
        assert_eq!(data.len(), 172);
        let mut r = Reader::new(&data);
        let state = GraphicsState::read(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(state.cull_mode, Some(2));
        assert_eq!(state.front_face, Some(1));
        assert_eq!(state.depth, Some([1, 3, 1]));
        assert_eq!(state.depth_bias, Some([1.0, 2.0, 3.0]));
        assert_eq!(state.stencil, Some([1, 4, 5, 6, 7]));
        assert_eq!(state.stencil_masks, Some([8, 9, 10]));
        assert_eq!(state.alpha_to_coverage, Some(1));
        let blend = state.blend.unwrap();
        assert_eq!(blend.target_mask, u32::MAX);
        assert_eq!(blend.targets[0].alpha_destination, 16);
        assert_eq!(blend.targets[0].constant_color, [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(blend.targets[0].logic_op, -1);
        assert_eq!(state.color_mask, Some([2, 15]));
        assert_eq!(state.fill_mode, Some(17));
        assert_eq!(state.clear_color, Some([4.0, 5.0, 6.0, 7.0]));
        assert_eq!(state.clear_depth, Some(0.5));
        assert_eq!(state.clear_stencil, Some(18));
        assert_eq!(state.raster_control, Some([19, 20]));
        assert_eq!(state.scissors, Some(vec![[-1, 22, 23, 24]]));
        assert_eq!(state.line_width, Some(2.5));
        for end in 0..data.len() {
            assert!(
                GraphicsState::read(&mut Reader::new(&data[..end])).is_err(),
                "accepted truncation at {end}"
            );
        }
    }

    #[test]
    fn material_preserves_absent_technique_slots_and_multiple_overrides() {
        let mut data = vec![];
        for index in 0..16 {
            data.push(u8::from(index == 4 || index == 8));
            if index == 4 {
                words(&mut data, &[0, 0, 0, 2]); // empty graph/expression/state, two overrides
                words(&mut data, &[1, 0, 0, 2]); // true expression, cull flag
                data.push(2);
                words(&mut data, &[1, 1, 0, 4]); // false expression, front-face flag
                data.push(1);
            } else if index == 8 {
                words(&mut data, &[0, 0, 0, 0]);
            }
        }
        let material = FeaturedMaterial::parse(&data).unwrap();
        assert!(material.technique(b'x').is_none());
        assert_eq!(material.technique(b'f').unwrap().overrides.len(), 2);
        assert_eq!(material.technique(b'f').unwrap().overrides[0].state.cull_mode, Some(2));
        assert!(material.technique(b'h').is_some());
        for end in 0..data.len() {
            assert!(FeaturedMaterial::parse(&data[..end]).is_err());
        }
        data.push(0);
        assert!(FeaturedMaterial::parse(&data).is_err());
    }
}
