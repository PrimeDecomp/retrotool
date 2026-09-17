//! Reuse identical scalar expressions along the dominator tree. A replacement
//! has already executed on every path reaching its use; nothing is speculated
//! or hoisted. Keep operand order, numeric controls and unavailable-value origins.
//! Replaced instructions remain Nop checkpoints at their original PCs.
//!
//! The scoped expression table follows the usual SSA CSE approach; see the pinned
//! Mesa reference's nir_opt_cse.c. Memory and lane-dependent operations are excluded.

use std::collections::HashMap;

use petgraph::{
    algo::dominators::simple_fast,
    graph::{DiGraph, NodeIndex},
};

use crate::ir::{float, rewrite::Aliases, Definition, Function, Operation, Type, ValueId};

type Expression = (Type, Operation);

enum Visit {
    Enter(usize),
    Leave(Vec<Expression>),
}

pub(super) fn eliminate(function: &mut Function) {
    let children = dominator_children(function);
    let mut aliases = Aliases::new(function);
    share_literals(function, &mut aliases);
    let mut expressions = HashMap::<Expression, ValueId>::new();
    let mut pending = vec![Visit::Enter(0)];
    while let Some(visit) = pending.pop() {
        let block = match visit {
            Visit::Enter(block) => block,
            Visit::Leave(inserted) => {
                for expression in inserted {
                    expressions.remove(&expression);
                }
                continue;
            }
        };
        let mut inserted = Vec::new();
        for instruction in &mut function.blocks[block].instructions {
            instruction.operation.map_operands(|value| aliases.resolve(value));
            let Some(result) = instruction.result else { continue };
            let replacement = if let Operation::Copy(value) = instruction.operation {
                Some(value)
            } else if reusable(&instruction.operation) {
                let expression = (function.values[result.0].ty, instruction.operation.clone());
                if let Some(&previous) = expressions.get(&expression) {
                    Some(previous)
                } else {
                    expressions.insert(expression.clone(), result);
                    inserted.push(expression);
                    None
                }
            } else {
                None
            };
            if let Some(replacement) = replacement {
                aliases.replace(result, replacement);
                instruction.result = None;
                instruction.operation = Operation::Nop;
            }
        }
        pending.push(Visit::Leave(inserted));
        pending.extend(children[block].iter().rev().copied().map(Visit::Enter));
    }
    aliases.apply(function);
}

fn share_literals(function: &Function, aliases: &mut Aliases) {
    let mut words = HashMap::new();
    let mut predicates = [None; 2];
    for (index, value) in function.values.iter().enumerate() {
        let id = ValueId(index);
        let previous = match value.definition {
            Definition::Word(word) => *words.entry(word).or_insert(id),
            Definition::Predicate(value) => *predicates[usize::from(value)].get_or_insert(id),
            _ => continue,
        };
        if previous != id {
            aliases.replace(id, previous);
        }
    }
}

fn dominator_children(function: &Function) -> Vec<Vec<usize>> {
    let mut graph = DiGraph::<(), ()>::new();
    for _ in &function.blocks {
        graph.add_node(());
    }
    for (index, block) in function.blocks.iter().enumerate() {
        for edge in block.terminator.as_ref().unwrap().edges() {
            graph.add_edge(NodeIndex::new(index), NodeIndex::new(edge.target.0), ());
        }
    }
    let dominators = simple_fast(&graph, NodeIndex::new(0));
    let mut children = vec![Vec::new(); function.blocks.len()];
    for index in 1..function.blocks.len() {
        let parent = dominators.immediate_dominator(NodeIndex::new(index)).unwrap();
        children[parent.index()].push(index);
    }
    children
}

/// Structural equality is necessary, not sufficient: reads and lane-dependent
/// operations retain their execution points even when all operands are equal.
fn reusable(operation: &Operation) -> bool {
    match operation {
        Operation::Copy(_)
        | Operation::AttributeAddress { .. }
        | Operation::Reduce { .. }
        | Operation::Add(..)
        | Operation::Multiply(..)
        | Operation::Negate(_)
        | Operation::NotBits(_)
        | Operation::ReverseBits(_)
        | Operation::CountLeadingZeros(_)
        | Operation::ExtractBits { .. }
        | Operation::Logic { .. }
        | Operation::Shift { .. }
        | Operation::Compare { .. }
        | Operation::PredicateNot(_)
        | Operation::PredicateLogic { .. }
        | Operation::Select { .. }
        | Operation::Component { .. } => true,
        Operation::Float(operation) => match operation {
            float::Operation::Modify { .. }
            | float::Operation::Binary { .. }
            | float::Operation::MinMax { .. }
            | float::Operation::Fma { .. }
            | float::Operation::Approximate { .. }
            | float::Operation::Reduced { .. }
            | float::Operation::Compare { .. }
            | float::Operation::IsNan(_)
            | float::Operation::FromInteger { .. }
            | float::Operation::RoundIntegral { .. } => true,
            // Keep the possible missing-NaN-policy fault at its execution point.
            float::Operation::ToInteger { .. } => false,
        },
        Operation::Nop
        | Operation::Demote
        | Operation::ReadConstant(_)
        | Operation::LoadConstantWord { .. }
        | Operation::ReadAttribute(_)
        | Operation::ReadSampleIndex
        | Operation::Texture(_)
        | Operation::GlobalLoad(_)
        | Operation::LocalLoad(_)
        | Operation::LocalStore(_)
        | Operation::Interpolate { .. } => false,
    }
}
