use super::{emit::Emitter, Kernel, Profile};
use crate::{
    ir::{analysis::Analysis, Function, Instruction, Operation},
    program::bindings::ConstantAccess,
};

/// Validation decisions for constant-memory loads. Emission consumes compile-time
/// proofs and upload preconditions without introducing additional assumptions.
pub(super) struct Plan {
    /// Present only when the upload contract validates the full instruction span.
    bound_offsets: Vec<Option<u32>>,
    /// Rendering uploads preserve ConstantMemory's validated packed layout.
    /// This proves header/payload/bitmap structure, not address availability.
    snapshot_layout: bool,
    check_alignment: Vec<bool>,
}

impl Plan {
    pub fn new(function: &Function, analysis: &Analysis, profile: Profile) -> Self {
        let mut bound_offsets = vec![None; function.values.len()];
        let mut check_alignment = vec![true; function.values.len()];
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            let Some(result) = instruction.result else { continue };
            check_alignment[result.0] = match instruction.operation {
                Operation::ReadConstant(_) => false, // Verified word alignment.
                Operation::LoadConstantWord { address, bytes, .. } => {
                    !analysis.bits.get(address).is_aligned(bytes)
                }
                _ => true,
            };
            if profile == Profile::Rendering {
                bound_offsets[result.0] =
                    ConstantAccess::for_instruction(instruction, &analysis.constants)
                        .and_then(|a| a.byte_offset);
            }
        }
        Self { bound_offsets, snapshot_layout: profile == Profile::Rendering, check_alignment }
    }
}

pub(super) fn declarations(kernel: &Kernel) -> String {
    let Some(binding) = kernel.constant_memory else {
        return String::new();
    };
    include_str!("../../wgsl/resources.wgsl")
        .replace("@group(1)", &format!("@group({})", binding.group))
        .replace("__BINDING__", &binding.binding.to_string())
        .replace("mpr_", "remax_")
}

impl Emitter<'_> {
    pub(super) fn load_constant(
        &mut self,
        bank: u8,
        address: String,
        bytes: u8,
        part: u8,
        signed: bool,
        instruction: &Instruction,
    ) -> String {
        let result = instruction.result.expect("verified constant load");
        if let Some(offset) = self.constant_plan.bound_offsets[result.0] {
            return bound_load(bank, offset, bytes, part, signed);
        }
        self.line("var memory_status = 0u;");
        let check_layout = !self.constant_plan.snapshot_layout;
        let check_alignment = self.constant_plan.check_alignment[result.0];
        self.line(&format!(
            "let loaded = remax_ldc_impl({bank}u, {address}, {bytes}u, {check_layout}, {check_alignment}, &memory_status);"
        ));
        self.fault("memory_status != 0u", "memory_status", instruction.pc);
        let word = format!("loaded[{}]", part / 4);
        extend(word, bytes, signed)
    }
}

fn bound_load(bank: u8, offset: u32, bytes: u8, part: u8, signed: bool) -> String {
    // The bank's physical packing remains runtime data. Only the validated byte
    // offset is constant; no uniform contents or payload offsets are frozen.
    let header = u32::from(bank) * 2;
    let word_offset = offset / 4 + u32::from(part) / 4;
    let mut word =
        format!("remax_constant_memory[remax_constant_memory[{header}u] + {word_offset}u]");
    if bytes < 4 {
        let shift = (offset % 4) * 8;
        let mask = (1u32 << (u32::from(bytes) * 8)) - 1;
        word = format!("(({word} >> {shift}u) & {mask}u)");
    }
    extend(word, bytes, signed)
}

fn extend(word: String, bytes: u8, signed: bool) -> String {
    if signed {
        let shift = 32 - u32::from(bytes) * 8;
        format!("bitcast<u32>(bitcast<i32>({word} << {shift}u) >> {shift}u)")
    } else {
        word
    }
}
