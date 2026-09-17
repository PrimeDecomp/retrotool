//! Global-memory lowering for frozen snapshots. The binding consumes exactly
//! `resources::memory::Snapshot::words()`, unchanged throughout an invocation
//! and its derivative quad. Writes and inter-invocation races are not modeled.

use super::emit::{name, Emitter};
use crate::{
    ir::{memory::Load, Function, Operation},
    program::{Diagnostic, DiagnosticKind, Phase},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub group: u32,
    pub binding: u32,
}

pub(super) fn plan(
    function: &Function,
    binding: Option<Binding>,
) -> Result<Option<Binding>, Diagnostic> {
    let mut used = false;
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        let Operation::GlobalLoad(load) = &instruction.operation else {
            continue;
        };
        if load.native_u128 {
            return Err(Diagnostic::new(
                Phase::Emit,
                Some(instruction.pc),
                DiagnosticKind::Unsupported,
                "LDG .U.128 has no established WGSL memory model",
            ));
        }
        if binding.is_none() {
            return Err(Diagnostic::new(
                Phase::Emit,
                Some(instruction.pc),
                DiagnosticKind::MissingInput,
                "global reads require an immutable memory-snapshot binding",
            ));
        }
        used = true;
    }
    Ok(if used { binding } else { None })
}

pub(super) fn declarations(binding: Option<Binding>) -> String {
    binding
        .map(|binding| {
            include_str!("memory.wgsl")
                .replace("__GROUP__", &binding.group.to_string())
                .replace("__BINDING__", &binding.binding.to_string())
        })
        .unwrap_or_default()
}

impl Emitter<'_> {
    pub(super) fn global_load(&mut self, load: &Load, pc: usize) -> String {
        let [low, high] = load.address.map(name);
        self.line("var memory_status = 0u;");
        self.line(&format!(
            "let global_words = remax_global_load(vec2<u32>({low}, {high}), {}u, &memory_status);",
            load.bytes,
        ));
        self.fault("memory_status != 0u", "memory_status", pc);
        if load.signed {
            let shift = 32 - u32::from(load.bytes) * 8;
            let extended =
                format!("bitcast<u32>(bitcast<i32>(global_words.x << {shift}u) >> {shift}u)");
            format!("vec4<u32>({extended}, 0u, 0u, 0u)")
        } else {
            "global_words".into()
        }
    }
}
