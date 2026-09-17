//! Constant-buffer requirements and binding validation. Reads retain their
//! original PCs; collecting a guarded read does not claim that it always executes.
//! A preflight report alone does not authorize removal of shader checks. The
//! backend separately records the preconditions its emitted code relies on.

use super::{Diagnostic, DiagnosticKind, Phase, Program};
use crate::{
    ir::{analysis::Constants, Function, Instruction, Operation},
    resources::ConstantMemory,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConstantAccess {
    /// Byte offset in the original instruction stream.
    pub pc: usize,
    pub bank: u8,
    /// None means no fixed address has been established. Uniform contents are
    /// never treated as program constants by this analysis.
    pub byte_offset: Option<u32>,
    /// Full validated span, including both halves of a multiword load.
    pub byte_count: u8,
}

#[derive(Clone, Debug, Default)]
pub struct ConstantRequirements {
    accesses: Vec<ConstantAccess>,
}

#[derive(Clone, Debug, Default)]
pub struct ConstantBindingReport {
    pub validated_accesses: usize,
    /// Addresses needing range analysis, specialization, or an execution check.
    pub dynamic_accesses: Vec<ConstantAccess>,
    /// Potential fixed-address reads that these bindings cannot satisfy.
    pub failures: Vec<Diagnostic>,
}

impl ConstantBindingReport {
    /// Every possible constant-buffer access is covered by the supplied bytes.
    /// This says nothing about other resources, termination or lane participation.
    pub fn is_complete(&self) -> bool {
        self.dynamic_accesses.is_empty() && self.failures.is_empty()
    }
}

impl ConstantRequirements {
    pub(crate) fn collect(function: &Function, constants: &Constants) -> Self {
        let mut accesses = Vec::new();
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            if let Some(access) = ConstantAccess::for_instruction(instruction, constants) {
                accesses.push(access);
            }
        }
        // A native multiword load has multiple SSA results but one full-span
        // obligation. Identical accesses in distinct sync contexts share it too.
        accesses.sort_unstable();
        accesses.dedup();
        Self { accesses }
    }

    pub fn accesses(&self) -> &[ConstantAccess] { &self.accesses }

    pub(crate) fn fixed(&self) -> Self {
        Self {
            accesses: self.accesses.iter().copied().filter(|a| a.byte_offset.is_some()).collect(),
        }
    }

    /// Validate a binding contract and return its upload words. Call again for
    /// every replacement snapshot; values and packing may change freely while
    /// these byte spans remain established. The uploaded buffer must preserve
    /// the returned words for the duration of the draw.
    /// ConstantMemory also guarantees the packed header, payload and validity
    /// bitmap layout. Rendering kernels rely on that invariant even when this
    /// contract lists no fixed-address reads. Do not substitute a handwritten
    /// packed buffer or mutate its metadata after binding.
    ///
    /// Every listed access must have a fixed address. A guarded access is still
    /// a precondition: use the checked shader profile to diagnose it only when
    /// executed instead. An empty contract imposes no additional requirements.
    pub fn bind<'a>(&self, memory: &'a ConstantMemory) -> Result<&'a [u32], Diagnostic> {
        let report = self.check(memory);
        if let Some(failure) = report.failures.into_iter().next() {
            return Err(failure);
        }
        if let Some(access) = report.dynamic_accesses.first() {
            return Err(Diagnostic::new(
                Phase::Bind,
                Some(access.pc),
                DiagnosticKind::Unsupported,
                "dynamic constant address cannot be validated before execution",
            ));
        }
        Ok(memory.words())
    }

    /// Check all potential fixed-address reads without executing the shader.
    /// A failure may belong to an untaken path; dynamic addresses remain unresolved.
    /// Recheck after changing bindings or byte validity. Changing established
    /// uniform values does not specialize the program or change these requirements.
    pub fn check(&self, memory: &ConstantMemory) -> ConstantBindingReport {
        let mut report = ConstantBindingReport::default();
        for &access in &self.accesses {
            let Some(offset) = access.byte_offset else {
                report.dynamic_accesses.push(access);
                continue;
            };
            let error = if !offset.is_multiple_of(u32::from(access.byte_count)) {
                Some((
                    DiagnosticKind::InvalidInput,
                    format!(
                        "unaligned {}-byte constant read c[{}][{offset:#x}]",
                        access.byte_count, access.bank,
                    ),
                ))
            } else {
                memory
                    .validate_range(access.bank, offset, usize::from(access.byte_count))
                    .err()
                    .map(|error| (DiagnosticKind::MissingInput, error.to_string()))
            };
            if let Some((kind, message)) = error {
                report.failures.push(Diagnostic::new(Phase::Bind, Some(access.pc), kind, message));
            } else {
                report.validated_accesses += 1;
            }
        }
        report
    }
}

impl ConstantAccess {
    pub(crate) fn for_instruction(
        instruction: &Instruction,
        constants: &Constants,
    ) -> Option<Self> {
        let (bank, byte_offset, byte_count) = match instruction.operation {
            Operation::ReadConstant(address) => {
                (address.bank, Some(u32::from(address.byte_offset)), 4)
            }
            Operation::LoadConstantWord { bank, address, bytes, .. } => {
                (bank, constants.word(address), bytes)
            }
            _ => return None,
        };
        Some(Self { pc: instruction.pc, bank, byte_offset, byte_count })
    }
}

impl Program {
    /// Describe potential constant-buffer reads without selecting a backend or
    /// specializing bound values. The result is reusable across uniform updates.
    pub fn constant_requirements(&self) -> ConstantRequirements {
        ConstantRequirements::collect(&self.function, &Constants::new(&self.function))
    }
}
