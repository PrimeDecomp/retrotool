use super::{
    emit::{name, Emitter},
    FaultScope,
};
use crate::{
    ir::{
        analysis::{EntryPrefix, EntryStep},
        BlockId, Edge, Terminator, Type, ValueId,
    },
    program::Diagnostic,
    structured::{Node, Region},
};

impl Emitter<'_> {
    pub(super) fn dispatch(&mut self, prefix: &EntryPrefix) -> Result<(), Diagnostic> {
        let entry = prefix.last();
        self.entry_prefix(prefix)?;
        self.finish_prefix();
        self.line(&format!("var block_id = {}u;", entry.0));
        self.line("loop {");
        self.line("switch block_id {");
        for (index, block) in self.function.blocks.iter().enumerate() {
            let block_id = BlockId(index);
            if block_id != entry && prefix.contains(block_id) {
                continue;
            }
            self.line(&format!("case {index}u: {{"));
            // The entry case executes only the last prefix terminator. All its
            // instructions and its transfer-budget tick have already executed.
            if block_id != entry {
                self.block(block_id)?;
            }
            let pc = block.pc.unwrap_or(0);
            match block.terminator.as_ref().unwrap() {
                Terminator::Jump(edge) => {
                    self.transfer(edge);
                    self.line(&format!("block_id = {}u;", edge.target.0));
                }
                Terminator::Branch { condition, yes, no, .. } => {
                    self.observe(&self.known(*condition), pc);
                    self.line(&format!("if ({}) {{", name(*condition)));
                    self.transfer(yes);
                    self.line(&format!("block_id = {}u;", yes.target.0));
                    self.line("} else {");
                    self.transfer(no);
                    self.line(&format!("block_id = {}u;", no.target.0));
                    self.line("}");
                }
                Terminator::Return(values) => self.output(values, pc),
            }
            self.line("}");
        }
        self.line("default: {");
        self.line("    return remax_fault(32u, 0u);");
        self.line("}");
        self.line("}");
        self.line("}");
        // Naga 0.11 and 29 require a typed return after this loop even though
        // every dynamic exit above already returns. Removal criteria:
        // docs/rendering-compatibility.md#dispatcher-final-return.
        self.line("return remax_fault(16u, 0u);");
        Ok(())
    }

    fn entry_prefix(&mut self, prefix: &EntryPrefix) -> Result<(), Diagnostic> {
        for &step in &prefix.steps {
            let block = step.entry();
            self.block(block)?;
            match step {
                EntryStep::Predicated { guard, body } => {
                    self.predicated_instruction(guard, body)?
                }
                EntryStep::Block(_) if block == prefix.last() => {}
                EntryStep::Block(_) => {
                    let Some(Terminator::Jump(edge)) = &self.function.blocks[block.0].terminator
                    else {
                        unreachable!("verified unconditional prefix edge")
                    };
                    self.transfer(edge);
                }
            }
        }
        Ok(())
    }

    fn predicated_instruction(&mut self, guard: BlockId, body: BlockId) -> Result<(), Diagnostic> {
        let guard = &self.function.blocks[guard.0];
        let Some(Terminator::Branch { condition, yes, no, .. }) = &guard.terminator else {
            unreachable!("verified predicated prefix step")
        };
        self.observe(&self.known(*condition), guard.pc.unwrap_or(0));
        self.line(&format!("if ({}) {{", name(*condition)));
        self.transfer(yes);
        self.block(body)?;
        let Some(Terminator::Jump(join)) = &self.function.blocks[body.0].terminator else {
            unreachable!("verified predicated instruction rejoin")
        };
        self.transfer(join);
        self.line("} else {");
        self.transfer(no);
        self.line("}");
        Ok(())
    }

    pub(super) fn region(&mut self, region: &Region) -> Result<(), Diagnostic> {
        for node in &region.0 {
            match node {
                Node::Block { block, arms } => self.region_block(*block, arms.as_ref())?,
                Node::Loop { body, exit, .. } => {
                    self.line("loop {");
                    self.region(body)?;
                    self.line("}");
                    if exit.is_none() {
                        // No guest exit: only a fault can finish execution.
                        // As for dispatch, validators need a typed final return.
                        // See docs/rendering-compatibility.md#dispatcher-final-return.
                        self.line("return remax_fault(16u, 0u);");
                    }
                }
                Node::Break => self.line("break;"),
                Node::Continue => self.line("continue;"),
            }
        }
        Ok(())
    }

    fn region_block(&mut self, id: BlockId, arms: Option<&[Region; 2]>) -> Result<(), Diagnostic> {
        self.block(id)?;
        if self.analysis.entry_prefix.last() == id {
            self.finish_prefix();
        }
        let block = &self.function.blocks[id.0];
        let pc = block.pc.unwrap_or(0);
        match block.terminator.as_ref().unwrap() {
            Terminator::Jump(edge) => self.transfer(edge),
            Terminator::Branch { condition, yes, no, .. } => {
                let [then_region, else_region] = arms.expect("verified branch shape");
                self.observe(&self.known(*condition), pc);
                self.line(&format!("if ({}) {{", name(*condition)));
                self.transfer(yes);
                self.region(then_region)?;
                self.line("} else {");
                self.transfer(no);
                self.region(else_region)?;
                self.line("}");
            }
            Terminator::Return(values) => self.output(values, pc),
        }
        Ok(())
    }

    fn transfer(&mut self, edge: &Edge) {
        // Snapshot the entire edge before assigning any parameter (parallel copy).
        self.line("{");
        let parameters = &self.function.blocks[edge.target.0].parameters;
        for (index, (value, parameter)) in edge.arguments.iter().zip(parameters).enumerate() {
            self.line(&format!("let edge_v{index} = {};", name(*value)));
            if !self.analysis.availability.is_available(*parameter) {
                self.line(&format!("let edge_k{index} = {};", self.known(*value)));
            }
        }
        for (index, parameter) in parameters.iter().enumerate() {
            self.line(&format!("{} = edge_v{index};", name(*parameter)));
            if !self.analysis.availability.is_available(*parameter) {
                self.line(&format!("{} = edge_k{index};", self.known(*parameter)));
            }
        }
        self.line("}");
    }

    fn output(&mut self, values: &[ValueId], pc: usize) {
        self.line("if (!fragment_discarded) {");
        for value in values {
            self.observe(&self.known(*value), pc);
        }
        self.line("}");
        self.line("{");
        self.line("var result: RemaxResult;");
        self.line("result.discarded = select(0u, 1u, fragment_discarded);");
        if self.kernel.fault_scope == FaultScope::Quad {
            self.line("result.status = fault_status;");
            self.line("result.pc = fault_pc;");
        }
        for (index, &value) in values.iter().enumerate() {
            let value = if self.function.values[value.0].ty == Type::Predicate {
                format!("select(0u, 1u, {})", name(value))
            } else {
                name(value)
            };
            self.line(&format!("result.words[{index}] = {value};"));
        }
        self.line("return result;");
        self.line("}");
    }

    fn block(&mut self, block: BlockId) -> Result<(), Diagnostic> {
        let proven = self.budget_plan.proven_prefix(block);
        if proven != 0 {
            self.line(&format!("steps -= {proven}u;"));
        }
        let instructions = &self.function.blocks[block.0].instructions;
        for (index, instruction) in instructions.iter().enumerate() {
            if index >= proven {
                self.tick(instruction.pc);
            }
            self.instruction(instruction)?;
        }
        if instructions.len() >= proven {
            self.tick(self.function.blocks[block.0].pc.unwrap_or(0));
        }
        Ok(())
    }
}
