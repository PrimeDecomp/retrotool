use std::fmt::Write;

use super::{budget, constant, memory, texture, FaultScope, Input, Kernel, Options, Profile};
use crate::{
    ir::{analysis::Analysis, float::Reduction, Definition, Function, Type, ValueId},
    program::Diagnostic,
    structured::Shape,
};

pub(super) fn name(value: ValueId) -> String { format!("v{}", value.0) }

pub(super) fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::Bits32 => "u32",
        Type::Predicate => "bool",
        Type::Words4 => "vec4<u32>",
        Type::Reduced(Reduction::Exp2) => "RemaxReducedExp2",
        Type::Reduced(Reduction::SinCos) => "RemaxReducedSinCos",
        Type::AttributeAddress => "RemaxAttributeAddress",
    }
}

fn zero(ty: Type) -> &'static str {
    match ty {
        Type::Bits32 => "0u",
        Type::Predicate => "false",
        Type::Words4 => "vec4<u32>(0u)",
        Type::Reduced(Reduction::Exp2) => "RemaxReducedExp2(0u)",
        Type::Reduced(Reduction::SinCos) => "RemaxReducedSinCos(0u)",
        Type::AttributeAddress => "RemaxAttributeAddress(0u, 0u)",
    }
}

pub(super) struct Emitter<'a> {
    pub(super) function: &'a Function,
    pub(super) options: Options,
    pub(super) analysis: Analysis,
    pub(super) kernel: Kernel,
    pub(super) constant_plan: constant::Plan,
    pub(super) budget_plan: budget::Plan,
    // Reporting scope is a kernel contract. Deferral ends once entry
    // participation is no longer needed by a subsequent operation.
    defer_faults: bool,
    body: String,
}

pub(super) fn emit(
    function: &Function,
    options: Options,
    bindings: &[texture::Binding],
) -> Result<Kernel, Diagnostic> {
    let optimized;
    let function = if options.profile == Profile::Rendering {
        optimized = crate::ir::optimize::for_rendering(function)?;
        &optimized
    } else {
        function
    };
    let plan = super::plan::build(function, options, bindings)?;
    let defer_faults = plan.kernel.fault_scope == FaultScope::Quad;
    let mut emitter = Emitter {
        function,
        options,
        analysis: plan.analysis,
        kernel: plan.kernel,
        constant_plan: plan.constants,
        budget_plan: plan.budget,
        defer_faults,
        body: String::new(),
    };
    emitter.declarations();
    match plan.shape {
        Shape::Regions(region) => emitter.region(&region)?,
        Shape::Dispatch(prefix) => emitter.dispatch(&prefix)?,
    }
    emitter.kernel.source =
        module_source(&emitter.kernel, &emitter.body, emitter.budget_plan.needs_counter)?;
    Ok(emitter.kernel)
}

fn module_source(kernel: &Kernel, body: &str, needs_budget: bool) -> Result<String, Diagnostic> {
    let input_count = kernel.inputs.len().max(1);
    let output_count = kernel.outputs.len().max(1);
    let budget =
        if needs_budget { format!("var steps = {}u;", kernel.step_budget) } else { String::new() };
    let mut source = format!(
        r#"
struct RemaxReducedExp2 {{
    argument: u32,
}}
struct RemaxReducedSinCos {{
    argument: u32,
}}
struct RemaxAttributeAddress {{
    base: u32,
    offset: u32,
}}
struct RemaxResult {{
    words: array<u32, {output_count}>,
    status: u32,
    pc: u32,
    discarded: u32,
}}
fn remax_fault(status: u32, pc: u32) -> RemaxResult {{
    var result: RemaxResult;
    result.status = status;
    result.pc = pc;
    return result;
}}
"#
    );
    source.push_str(include_str!("bits.wgsl"));
    source.push_str(&include_str!("../../wgsl/integer.wgsl").replace("mpr_", "remax_"));
    source.push_str(include_str!("multiply.wgsl"));
    source.push_str(&constant::declarations(kernel));
    source.push_str(&memory::declarations(kernel.global_memory));

    let buffer_bindings = kernel
        .constant_memory
        .map(|binding| (binding.group, binding.binding))
        .into_iter()
        .chain(kernel.global_memory.map(|binding| (binding.group, binding.binding)))
        .collect::<Vec<_>>();
    source.push_str(&texture::declarations(&kernel.textures, &buffer_bindings)?);
    writeln!(
        source,
        r#"
fn remax_main(
    input_words: array<u32, {input_count}>,
    input_valid: array<u32, {input_count}>,
) -> RemaxResult {{
    {budget}
    var fault_status = 0u;
    var fault_pc = 0u;
    var fragment_discarded = false;
{body}
}}
"#
    )
    .unwrap();
    Ok(source)
}

impl Emitter<'_> {
    pub(super) fn line(&mut self, source: &str) { writeln!(self.body, "{source}").unwrap(); }

    pub(super) fn observe(&mut self, valid: &str, pc: usize) {
        if valid == "true" {
            return;
        }
        self.fault(&format!("!({valid})"), "32u", pc);
    }

    pub(super) fn fault(&mut self, condition: &str, code: &str, pc: usize) {
        if self.defer_faults {
            self.line(&format!("if (({condition}) && fault_status == 0u) {{"));
            self.line(&format!("    fault_status = {code};"));
            self.line(&format!("    fault_pc = {pc}u;"));
        } else {
            self.line(&format!("if ({condition}) {{"));
            self.line(&format!("    return remax_fault({code}, {pc}u);"));
        }
        self.line("}");
    }

    pub(super) fn finish_prefix(&mut self) {
        if self.defer_faults {
            self.line("if (fault_status != 0u) {");
            self.line("    return remax_fault(fault_status, fault_pc);");
            self.line("}");
            self.defer_faults = false;
        }
    }

    pub(super) fn tick(&mut self, pc: usize) {
        if !self.budget_plan.needs_counter {
            return;
        }
        self.fault("steps == 0u", "16u", pc);
        self.line("steps -= min(steps, 1u);");
    }

    pub(super) fn known(&self, value: ValueId) -> String {
        if self.analysis.availability.is_available(value) {
            "true".into()
        } else {
            format!("k{}", value.0)
        }
    }

    pub(super) fn all_known(&self, values: &[ValueId]) -> String {
        let required = values
            .iter()
            .filter(|&&value| !self.analysis.availability.is_available(value))
            .map(|&value| self.known(value))
            .collect::<Vec<_>>();
        if required.is_empty() {
            "true".into()
        } else {
            required.join(" && ")
        }
    }

    fn input(&mut self, input: Input) -> usize {
        if let Some(slot) = self.kernel.inputs.iter().position(|&existing| existing == input) {
            return slot;
        }
        self.kernel.inputs.push(input);
        self.kernel.inputs.len() - 1
    }

    fn declarations(&mut self) {
        self.local_declarations();
        for (index, value) in self.function.values.iter().enumerate() {
            let (binding, expression, valid) = match value.definition {
                Definition::Word(word) => ("let", format!("{word}u"), "true".into()),
                Definition::Predicate(predicate) => ("let", predicate.to_string(), "true".into()),
                Definition::Input(register) => {
                    let slot = self.input(Input::Register(register));
                    let expression = if value.ty == Type::Predicate {
                        format!("input_words[{slot}] != 0u")
                    } else {
                        format!("input_words[{slot}]")
                    };
                    ("let", expression, format!("input_valid[{slot}] != 0u"))
                }
                Definition::Unavailable(_) | Definition::UnavailableLocal(_) => {
                    ("let", zero(value.ty).into(), "false".into())
                }
                _ => ("var", zero(value.ty).into(), "false".into()),
            };
            let shader_type = type_name(value.ty);
            self.line(&format!("{binding} v{index}: {shader_type} = {expression};"));
            if !self.analysis.availability.is_available(ValueId(index)) {
                self.line(&format!("{binding} k{index}: bool = {valid};"));
            }
        }
    }

    pub(super) fn read_input(&mut self, input: Input, pc: usize) -> String {
        let slot = self.input(input);
        self.observe(&format!("input_valid[{slot}] != 0u"), pc);
        format!("input_words[{slot}]")
    }
}
