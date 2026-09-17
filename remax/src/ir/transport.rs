//! Value transport through copies, selections and simultaneous block arguments.
//! This graph deliberately excludes arithmetic and selection conditions.

use std::collections::BTreeSet;

use super::{Definition, Function, Operation, Type, ValueId};

mod demand;
mod paths;
mod specialize;
pub(crate) use demand::Demand;
pub(crate) use specialize::specialize_uses;

pub(crate) struct Transport {
    sources: Vec<Vec<ValueId>>,
    users: Vec<Vec<ValueId>>,
}

impl Transport {
    pub fn new(function: &Function) -> Self { Self::filtered(function, |_, _| true) }

    pub fn filtered(function: &Function, retain: impl Fn(ValueId, ValueId) -> bool) -> Self {
        let mut sources = vec![Vec::new(); function.values.len()];
        for block in &function.blocks {
            for instruction in &block.instructions {
                let Some(result) = instruction.result else { continue };
                sources[result.0] = match instruction.operation {
                    Operation::Copy(value) => vec![value],
                    Operation::Select { yes, no, .. } => vec![yes, no],
                    _ => Vec::new(),
                };
            }
            for edge in block.terminator.as_ref().unwrap().edges() {
                for (&argument, &parameter) in
                    edge.arguments.iter().zip(&function.blocks[edge.target.0].parameters)
                {
                    sources[parameter.0].push(argument);
                }
            }
        }
        for (index, values) in sources.iter_mut().enumerate() {
            values.retain(|&source| retain(ValueId(index), source));
        }
        let mut users = vec![Vec::new(); sources.len()];
        for (index, operands) in sources.iter().enumerate() {
            for source in operands {
                users[source.0].push(ValueId(index));
            }
        }
        Self { sources, users }
    }

    /// Native registers are untyped during construction. Refine transport from
    /// opaque producers after sealing, then let verification reject mixed paths.
    pub fn refine(&self, function: &mut Function) -> Result<(), ValueId> {
        let mut pending: Vec<_> = function
            .values
            .iter()
            .enumerate()
            .filter(|(_, value)| value.ty.is_opaque())
            .map(|(index, _)| ValueId(index))
            .collect();
        while let Some(source) = pending.pop() {
            let ty = function.values[source.0].ty;
            for &user in &self.users[source.0] {
                let current = function.values[user.0].ty;
                if current == ty {
                    continue;
                }
                if current != Type::Bits32
                    && !(current == Type::Predicate && ty == Type::AttributeAddress)
                {
                    return Err(user);
                }
                function.values[user.0].ty = ty;
                pending.push(user);
            }
        }
        Ok(())
    }

    pub fn origins(&self, value: ValueId) -> BTreeSet<ValueId> {
        let mut pending = vec![value];
        let mut visited = BTreeSet::new();
        let mut origins = BTreeSet::new();
        while let Some(value) = pending.pop() {
            if !visited.insert(value) {
                continue;
            }
            if self.sources[value.0].is_empty() {
                origins.insert(value);
            } else {
                pending.extend(&self.sources[value.0]);
            }
        }
        origins
    }

    /// Prove matching provenance without equating distinct producers. Parameter
    /// inputs are compared on corresponding incoming edges, never as unordered
    /// origin sets. Equal loop-carried networks are checked coinductively.
    pub fn equivalent(&self, function: &Function, a: ValueId, b: ValueId) -> bool {
        let operation = |value: ValueId| match function.values[value.0].definition {
            Definition::Instruction { block, index } => {
                Some(&function.blocks[block.0].instructions[index].operation)
            }
            _ => None,
        };
        let mut pending = vec![(a, b)];
        let mut visited = BTreeSet::new();
        while let Some((a, b)) = pending.pop() {
            if a == b || !visited.insert((a, b)) {
                continue;
            }
            if let Some(Operation::Copy(source)) = operation(a) {
                pending.push((*source, b));
                continue;
            }
            if let Some(Operation::Copy(source)) = operation(b) {
                pending.push((a, *source));
                continue;
            }
            match (&function.values[a.0].definition, &function.values[b.0].definition) {
                (
                    Definition::Parameter { block: a_block, .. },
                    Definition::Parameter { block: b_block, .. },
                ) if a_block == b_block => {
                    pending.extend(
                        self.sources[a.0].iter().copied().zip(self.sources[b.0].iter().copied()),
                    );
                }
                _ => match (operation(a), operation(b)) {
                    (
                        Some(Operation::Select { condition: a_condition, yes: a_yes, no: a_no }),
                        Some(Operation::Select { condition: b_condition, yes: b_yes, no: b_no }),
                    ) if a_condition == b_condition => {
                        pending.extend([(*a_yes, *b_yes), (*a_no, *b_no)]);
                    }
                    _ => return false,
                },
            }
        }
        true
    }
}
