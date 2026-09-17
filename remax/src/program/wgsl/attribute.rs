//! Select one logical input before observing its availability. Candidate values
//! are interface declarations, not unconditional resource-read obligations.

use super::emit::{name, Emitter};
use crate::{ir::attribute::Input, program::Diagnostic};

impl Emitter<'_> {
    pub(super) fn attribute(
        &mut self,
        input: Input,
        pc: usize,
        mut read: impl FnMut(&mut Self, u16) -> Result<String, Diagnostic>,
    ) -> Result<String, Diagnostic> {
        let address = match input {
            Input::Direct(offset) => return read(self, offset),
            Input::Indexed(address) => address,
        };
        self.observe(&self.known(address), pc);
        let candidates: Vec<_> = self
            .function
            .indexed_attributes
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(slot, offset)| {
                self.analysis.attributes.permits(address, slot).then_some(offset)
            })
            .collect();
        let address = name(address);
        // The base is a verified generic component. Check the unsigned offset
        // before addition so an overflowing value cannot alias a declared input.
        self.fault(&format!("{address}.offset > 639u - {address}.base"), "2u", pc);
        self.fault(&format!("({address}.offset & 3u) != 0u"), "4u", pc);
        self.line("var attribute_value = 0u;");
        self.line(&format!("switch ({address}.base + {address}.offset) {{"));
        for offset in candidates {
            self.line(&format!("case {offset}u: {{"));
            let value = read(self, offset)?;
            self.line(&format!("attribute_value = {value};"));
            self.line("}");
        }
        self.line("default: {");
        self.fault("true", "32u", pc);
        self.line("}");
        self.line("}");
        Ok("attribute_value".into())
    }
}
