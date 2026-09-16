//! Closed-region lowering and definite-value analysis for the parent emitter.
use std::{collections::BTreeSet, fmt::Write, mem};

use anyhow::{ensure, Result};

use super::{Emitter, CONTROL_FLOW_BLOCK_LIMIT};
use crate::maxwell::control::{ReconvergenceRegion, RegionEdge};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Defined {
    registers: BTreeSet<u8>,
    predicates: BTreeSet<u8>,
    attributes: BTreeSet<u16>,
}
impl Defined {
    fn current(e: &Emitter) -> Self {
        Self {
            registers: e.definite.iter().copied().collect(),
            predicates: e.definite_predicates.iter().copied().collect(),
            attributes: e.definite_attributes.iter().copied().collect(),
        }
    }

    fn universe() -> Self {
        Self {
            registers: (0..255).collect(),
            predicates: (0..7).collect(),
            attributes: (0..256).map(|i| i * 4).collect(),
        }
    }

    fn intersect(&mut self, other: &Self) {
        self.registers.retain(|r| other.registers.contains(r));
        self.predicates.retain(|r| other.predicates.contains(r));
        self.attributes.retain(|r| other.attributes.contains(r));
    }

    fn union(&mut self, other: &Self) {
        self.registers.extend(&other.registers);
        self.predicates.extend(&other.predicates);
        self.attributes.extend(&other.attributes);
    }
}
struct BlockValues {
    generated: Defined,
    replaced: BTreeSet<u8>,
    opaque: BTreeSet<u8>,
    replaced_predicates: BTreeSet<u8>,
    registers: Vec<u8>,
    predicates: Vec<u8>,
    attributes: Vec<u16>,
}
fn extend_unique<T: PartialEq>(to: &mut Vec<T>, values: impl IntoIterator<Item = T>) {
    for v in values {
        if !to.contains(&v) {
            to.push(v);
        }
    }
}

impl Emitter {
    pub(super) fn control_region(&mut self, region: &ReconvergenceRegion) -> Result<()> {
        let before_opaque: BTreeSet<_> = self
            .range_values
            .drain(..)
            .map(|(r, _)| r)
            .chain(self.address_pairs.drain(..).map(|p| p.register))
            .collect();
        let before_opaque_predicates: BTreeSet<_> =
            self.address_predicates.drain(..).map(|(p, _)| p).collect();
        self.known_words.clear();
        self.known_predicates.clear();
        let before = Defined::current(self);
        let mut outer_body = mem::take(&mut self.body);
        let mut inputs = mem::take(&mut self.inputs);
        let mut predicates = mem::take(&mut self.predicates);
        let mut attributes = mem::take(&mut self.initial_output_attributes);
        let region_index = self.control_flow_regions;
        self.control_flow_regions += 1;
        let pc = format!("mpr_pc_{region_index}");
        let steps = format!("mpr_steps_{region_index}");
        let end = region.blocks.len();
        let mut blocks = Vec::new();
        let mut values = Vec::new();
        self.in_control_region = true;
        for (index, block) in region.blocks.iter().enumerate() {
            self.definite.clear();
            self.definite_predicates.clear();
            self.definite_attributes.clear();
            self.inputs.clear();
            self.predicates.clear();
            self.initial_output_attributes.clear();
            self.replaced_registers.clear();
            self.replaced_predicates.clear();
            for &i in &block.instructions {
                self.instruction(i).with_context(|| {
                    format!(
                        "control block{index} instruction {:#x}, word {:#018x}",
                        i.offset, i.word
                    )
                })?;
            }
            let opaque = self.range_values.drain(..).map(|(r, _)| r).collect();
            match block.edge {
                RegionEdge::Jump(target) => {
                    writeln!(self.body, "{pc}={}u;", target.unwrap_or(end))?;
                }
                RegionEdge::Branch { predicate, taken, otherwise } => {
                    let guard = self.predicate(predicate)?;
                    writeln!(
                        self.body,
                        "{pc}=select({}u,{}u,{guard});",
                        otherwise.unwrap_or(end),
                        taken.unwrap_or(end)
                    )?;
                }
            }
            blocks.push(mem::take(&mut self.body));
            values.push(BlockValues {
                generated: Defined::current(self),
                replaced: self.replaced_registers.drain(..).collect(),
                opaque,
                replaced_predicates: self.replaced_predicates.drain(..).collect(),
                registers: mem::take(&mut self.inputs),
                predicates: mem::take(&mut self.predicates),
                attributes: mem::take(&mut self.initial_output_attributes),
            });
        }
        self.in_control_region = false;
        // Least fixed point: a register may contain an opaque value on any
        // incoming path. Only an unconditional replacement kills that state.
        // A local MUFU can consume its local captured input; using a producer
        // from another block remains unsupported. Reads before replacement are
        // already recorded in BlockValues::registers by the ordinary emitter.
        let mut opaque_in = vec![BTreeSet::new(); end];
        let mut opaque_out = opaque_in.clone();
        loop {
            let mut changed = false;
            for (index, value) in values.iter().enumerate() {
                let mut incoming = BTreeSet::new();
                if index == region.entry {
                    incoming.extend(&before_opaque);
                }
                for (pred, block) in region.blocks.iter().enumerate() {
                    if block.edge.targets().contains(&Some(index)) {
                        incoming.extend(&opaque_out[pred]);
                    }
                }
                let mut outgoing = incoming.clone();
                outgoing.retain(|r| !value.replaced.contains(r));
                outgoing.extend(&value.opaque);
                changed |= opaque_in[index] != incoming || opaque_out[index] != outgoing;
                opaque_in[index] = incoming;
                opaque_out[index] = outgoing;
            }
            if !changed {
                break;
            }
        }
        // AL2P producers inside a control region are rejected. Incoming opaque
        // predicates can survive paths/loops until an unconditional replacement.
        let mut predicate_in = vec![BTreeSet::new(); end];
        let mut predicate_out = predicate_in.clone();
        loop {
            let mut changed = false;
            for (index, value) in values.iter().enumerate() {
                let mut incoming = BTreeSet::new();
                if index == region.entry {
                    incoming.extend(&before_opaque_predicates);
                }
                for (pred, block) in region.blocks.iter().enumerate() {
                    if block.edge.targets().contains(&Some(index)) {
                        incoming.extend(&predicate_out[pred]);
                    }
                }
                let mut outgoing = incoming.clone();
                outgoing.retain(|p| !value.replaced_predicates.contains(p));
                changed |= predicate_in[index] != incoming || predicate_out[index] != outgoing;
                predicate_in[index] = incoming;
                predicate_out[index] = outgoing;
            }
            if !changed {
                break;
            }
        }
        let mut after_opaque_predicates = BTreeSet::new();
        let mut after_opaque = BTreeSet::new();
        for (index, value) in values.iter().enumerate() {
            for p in &value.predicates {
                ensure!(
                    !predicate_in[index].contains(p),
                    "control block {:#x} reads opaque AL2P predicate P{p} before replacement",
                    region.blocks[index].offset
                );
            }
            for r in &value.registers {
                ensure!(
                    !opaque_in[index].contains(r),
                    "control block {:#x} reads opaque RRO/AL2P register R{r} before replacement",
                    region.blocks[index].offset
                );
            }
            if region.blocks[index].edge.targets().contains(&None) {
                after_opaque.extend(&opaque_out[index]);
                after_opaque_predicates.extend(&predicate_out[index]);
            }
        }
        // Greatest fixed point for values definitely assigned along every path.
        // Loops start at the universe and lose unsupported assumptions; the entry
        // edge is constrained by the enclosing straight-line region's definitions.
        let mut incoming = vec![Defined::universe(); end];
        let mut outgoing = incoming.clone();
        loop {
            let mut changed = false;
            for (index, value) in values.iter().enumerate() {
                let mut known = Defined::universe();
                if index == region.entry {
                    known.intersect(&before);
                }
                for (pred, block) in region.blocks.iter().enumerate() {
                    if block.edge.targets().contains(&Some(index)) {
                        known.intersect(&outgoing[pred]);
                    }
                }
                let mut after = known.clone();
                after.union(&value.generated);
                after.registers.retain(|r| !opaque_out[index].contains(r));
                after.predicates.retain(|p| !predicate_out[index].contains(p));
                changed |= incoming[index] != known || outgoing[index] != after;
                incoming[index] = known;
                outgoing[index] = after;
            }
            if !changed {
                break;
            }
        }
        let mut after = Defined::universe();
        for (index, value) in values.iter().enumerate() {
            let known = &incoming[index];
            extend_unique(
                &mut inputs,
                value.registers.iter().copied().filter(|r| !known.registers.contains(r)),
            );
            extend_unique(
                &mut predicates,
                value.predicates.iter().copied().filter(|r| !known.predicates.contains(r)),
            );
            extend_unique(
                &mut attributes,
                value.attributes.iter().copied().filter(|r| !known.attributes.contains(r)),
            );
            if region.blocks[index].edge.targets().contains(&None) {
                after.intersect(&outgoing[index]);
            }
        }
        self.inputs = inputs;
        self.predicates = predicates;
        self.initial_output_attributes = attributes;
        self.definite = after.registers.into_iter().collect();
        self.definite_predicates = after.predicates.into_iter().collect();
        self.definite_attributes = after.attributes.into_iter().collect();
        self.address_predicates = after_opaque_predicates.into_iter().map(|p| (p, None)).collect();
        self.range_values = after_opaque.into_iter().map(|r| (r, None)).collect();
        writeln!(outer_body,"var {pc}={}u;\nvar {steps}=0u;\nloop {{\nif ({pc}=={end}u) {{ break; }}\nif ({steps}>={CONTROL_FLOW_BLOCK_LIMIT}u) {{ *memory_status|=16u; break; }}\n{steps}+=1u;\nswitch ({pc}) {{",region.entry)?;
        for (index, body) in blocks.iter().enumerate() {
            writeln!(outer_body, "case {index}u: {{\n{body}}}")?;
        }
        writeln!(outer_body, "default: {{ *memory_status|=16u; {pc}={end}u; }}\n}}\n}}")?;
        self.body = outer_body;
        Ok(())
    }
}
use anyhow::Context;
