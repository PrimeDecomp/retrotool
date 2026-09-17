//! Exact scalar constants, without specializing bindings or pruning CFG edges.
//! Pending facts let a loop invariant be established from its incoming value;
//! disagreement on a later backedge permanently makes that value nonconstant.

use super::Dependencies;
use crate::{
    ir::{integer, Definition, Function, Operation, Type, ValueId},
    program::Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fact {
    Pending,
    Constant(Value),
    Varying,
}

impl Fact {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Pending, value) | (value, Self::Pending) => value,
            (Self::Constant(a), Self::Constant(b)) if a == b => self,
            _ => Self::Varying,
        }
    }
}

pub(crate) struct Constants {
    facts: Vec<Fact>,
}

impl Constants {
    pub fn new(function: &Function) -> Self {
        Self::analyze(function, &Dependencies::new(function))
    }

    pub(super) fn analyze(function: &Function, dependencies: &Dependencies) -> Self {
        let facts = dependencies.solve(Fact::Pending, Fact::join, |facts, value, operands| {
            Self::evaluate(facts, function, value, operands)
        });
        Self { facts }
    }

    pub fn predicate(&self, value: ValueId) -> Option<bool> {
        match self.facts[value.0] {
            Fact::Constant(Value::Predicate(value)) => Some(value),
            _ => None,
        }
    }

    pub fn word(&self, value: ValueId) -> Option<u32> {
        match self.facts[value.0] {
            Fact::Constant(Value::Word(word)) => Some(word),
            _ => None,
        }
    }

    fn evaluate(
        facts: &[Fact],
        function: &Function,
        value: ValueId,
        dependencies: &[ValueId],
    ) -> Fact {
        let value = &function.values[value.0];
        if !matches!(value.ty, Type::Bits32 | Type::Predicate) {
            return Fact::Varying;
        }
        match value.definition {
            Definition::Word(word) => Fact::Constant(Value::Word(word)),
            Definition::Predicate(predicate) => Fact::Constant(Value::Predicate(predicate)),
            Definition::Input(_) | Definition::Unavailable(_) | Definition::UnavailableLocal(_) => {
                Fact::Varying
            }
            Definition::Parameter { .. } => {
                dependencies.iter().fold(Fact::Pending, |fact, value| fact.join(facts[value.0]))
            }
            Definition::Instruction { block, index } => {
                let operation = &function.blocks[block.0].instructions[index].operation;
                if let Operation::Select { condition, yes, no } = *operation {
                    return match facts[condition.0] {
                        Fact::Constant(Value::Predicate(condition)) => {
                            facts[if condition { yes } else { no }.0]
                        }
                        Fact::Pending => Fact::Pending,
                        _ => Fact::Varying,
                    };
                }
                let mut args = Vec::with_capacity(dependencies.len());
                let mut pending = false;
                for &operand in dependencies {
                    match facts[operand.0] {
                        Fact::Varying => return Fact::Varying,
                        Fact::Pending => pending = true,
                        Fact::Constant(value) => args.push(value),
                    }
                }
                if pending {
                    return Fact::Pending;
                }
                integer::evaluate(operation, &args).map_or(Fact::Varying, Fact::Constant)
            }
        }
    }
}
