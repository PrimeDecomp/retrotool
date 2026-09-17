//! Target planning follows semantic analysis and precedes source emission.
//! Resource requirements and execution restrictions belong here; the writer
//! consumes the plan without deciding whether a binding is required.

use super::{
    budget, constant, memory, texture, unsupported, ControlFlow, FaultScope, Kernel, Options,
    Profile,
};
use crate::{
    ir::{
        analysis::Analysis,
        texture::{Kind, SampleLevel},
        BlockId, Function, Operation,
    },
    program::{bindings::ConstantRequirements, Diagnostic, DiagnosticKind, Phase},
    structured::{Shape, Strategy},
};

pub(super) struct Plan {
    pub shape: Shape,
    pub analysis: Analysis,
    pub kernel: Kernel,
    pub constants: constant::Plan,
    pub budget: budget::Plan,
}

pub(super) fn build(
    function: &Function,
    options: Options,
    bindings: &[texture::Binding],
) -> Result<Plan, Diagnostic> {
    if options.step_budget == 0 {
        return Err(Diagnostic::new(
            Phase::Emit,
            None,
            DiagnosticKind::InvalidInput,
            "zero shader execution budget",
        ));
    }
    function.verify()?;
    let analysis = Analysis::new(function);
    let strategy = match options.profile {
        Profile::Checked => Strategy::Acyclic,
        Profile::Rendering => Strategy::NaturalLoops,
    };
    let shape = crate::structured::build(function, &analysis.entry_prefix, strategy)?;
    if function
        .blocks
        .iter()
        .filter_map(|block| block.pc)
        .chain(
            function
                .blocks
                .iter()
                .flat_map(|block| block.instructions.iter().map(|instruction| instruction.pc)),
        )
        .any(|pc| u32::try_from(pc).is_err())
    {
        return Err(Diagnostic::new(
            Phase::Emit,
            None,
            DiagnosticKind::InvalidInput,
            "instruction PC exceeds the WGSL fault-record range",
        ));
    }
    let textures = texture::plan(function, &analysis.constants, bindings)?;
    let constant_requirements = ConstantRequirements::collect(function, &analysis.constants);
    let constant_preconditions = match options.profile {
        Profile::Checked => ConstantRequirements::default(),
        Profile::Rendering => constant_requirements.fixed(),
    };
    let constants = constant::Plan::new(function, &analysis, options.profile);
    let budget = budget::Plan::new(function, &analysis.steps, options);
    let requires_uniform_call = textures
        .iter()
        .any(|resource| resource.uses_implicit_lod && !resource.uses_fixed_implicit_lod);
    // Implicit samples must execute once with entry participation. Binding
    // legalization can remove a derivative dependence (fixed one-mip LOD),
    // but scalar structuring alone cannot establish convergence after branches.
    for (index, block) in function.blocks.iter().enumerate() {
        if analysis.entry_prefix.has_entry_participation(BlockId(index)) {
            continue;
        }
        for instruction in &block.instructions {
            let Operation::Texture(operation) = &instruction.operation else {
                continue;
            };
            let implicit =
                matches!(operation.kind, Kind::Sample { level: SampleLevel::Implicit, .. });
            let resource = textures
                .iter()
                .find(|resource| resource.binding.view.texture_index == operation.descriptor)
                .unwrap();
            if implicit && !resource.uses_fixed_implicit_lod {
                return Err(unsupported(
                    instruction.pc,
                    "implicit sampling after branching or in a loop needs GPU execution analysis",
                ));
            }
        }
    }
    let kernel = Kernel {
        stage: function.stage,
        source: String::new(),
        inputs: vec![],
        outputs: function.outputs.clone(),
        constant_memory: (!constant_requirements.accesses().is_empty())
            .then_some(options.constant_memory),
        constant_requirements,
        constant_preconditions,
        global_memory: memory::plan(function, options.global_memory)?,
        textures,
        fault_scope: if requires_uniform_call { FaultScope::Quad } else { FaultScope::Invocation },
        requires_uniform_call,
        control_flow: if matches!(shape, Shape::Regions(_)) {
            ControlFlow::Regions
        } else {
            ControlFlow::ScalarDispatcher
        },
        step_budget: options.step_budget,
        approximate_arithmetic: false,
        approximate_special_functions: false,
        offset_interpolation: None,
        f2i_nan_behavior: None,
    };
    Ok(Plan { shape, analysis, kernel, constants, budget })
}
