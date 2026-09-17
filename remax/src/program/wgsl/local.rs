//! Local storage belongs to one kernel invocation. Validity tracks individual
//! bytes so subword writes cannot initialize their untouched neighbors.

use super::emit::{name, Emitter};
use crate::ir::{
    memory::local::{Access, Load, Store},
    Operation,
};

impl Emitter<'_> {
    pub(super) fn local_declarations(&mut self) {
        if !self.function.blocks.iter().flat_map(|block| &block.instructions).any(|instruction| {
            matches!(instruction.operation, Operation::LocalLoad(_) | Operation::LocalStore(_))
        }) {
            return;
        }
        let words = self.function.local_memory_bytes.div_ceil(4).max(1);
        self.line(&format!("var local_words: array<u32, {words}>;"));
        self.line(&format!("var local_known: array<u32, {words}>;"));
    }

    fn local_access(&mut self, access: Access, pc: usize) -> String {
        let address = name(access.address);
        let known = self.known(access.address);
        self.observe(&known, pc);
        let bytes = access.bytes;
        let frame = self.function.local_memory_bytes;
        self.line(&format!("let local_aligned = ({address} & {}u) == 0u;", bytes - 1));
        self.line(&format!(
            "let local_in_bounds = {address} <= {frame}u && {bytes}u <= {frame}u - {address};"
        ));
        self.fault("!local_aligned", "4u", pc);
        self.fault("!local_in_bounds", "2048u", pc);
        format!("{known} && local_aligned && local_in_bounds")
    }

    pub(super) fn local_load(&mut self, load: Load, pc: usize) -> (String, String) {
        let valid_access = self.local_access(load.access, pc);
        self.line("var local_value = 0u;");
        self.line("var local_valid = false;");
        self.line(&format!("if ({valid_access}) {{"));
        let offset = format!("({} + {}u)", name(load.access.address), load.part);
        let count = load.access.bytes.min(4);
        let byte_mask = (1u32 << count) - 1;
        self.line(&format!("let local_word = {offset} / 4u;"));
        self.line(&format!("let local_byte = {offset} % 4u;"));
        self.line(&format!("local_valid = ((local_known[local_word] >> local_byte) & {byte_mask}u) == {byte_mask}u;"));
        self.line("local_value = local_words[local_word] >> (local_byte * 8u);");
        if count < 4 {
            self.line(&format!("local_value &= {}u;", (1u32 << (count * 8)) - 1));
        }
        if load.signed {
            let shift = 32 - count * 8;
            self.line(&format!(
                "local_value = bitcast<u32>(bitcast<i32>(local_value << {shift}u) >> {shift}u);"
            ));
        }
        self.line("}");
        ("local_value".into(), "local_valid".into())
    }

    pub(super) fn local_store(&mut self, store: &Store, pc: usize) {
        self.line("{");
        let valid_access = self.local_access(store.access, pc);
        self.line(&format!("if ({valid_access}) {{"));
        let address = name(store.access.address);
        if store.access.bytes < 4 {
            let value = name(store.values[0]);
            let known = self.known(store.values[0]);
            let bits = u32::from(store.access.bytes) * 8;
            let byte_mask = (1u32 << store.access.bytes) - 1;
            self.line(&format!("let local_word = {address} / 4u;"));
            self.line(&format!("let local_byte = {address} % 4u;"));
            self.line("let local_shift = local_byte * 8u;");
            self.line(&format!("let local_mask = {}u << local_shift;", (1u32 << bits) - 1));
            self.line(&format!("let local_byte_mask = {byte_mask}u << local_byte;"));
            self.line(&format!("local_words[local_word] = (local_words[local_word] & ~local_mask) | (({value} << local_shift) & local_mask);"));
            self.line(&format!("local_known[local_word] = (local_known[local_word] & ~local_byte_mask) | select(0u, local_byte_mask, {known});"));
        } else {
            for (part, &value) in store.values.iter().enumerate() {
                let word = format!("({address} / 4u + {part}u)");
                self.line(&format!("local_words[{word}] = {};", name(value)));
                self.line(&format!(
                    "local_known[{word}] = select(0u, 15u, {});",
                    self.known(value)
                ));
            }
        }
        self.line("}");
        self.line("}");
    }
}
