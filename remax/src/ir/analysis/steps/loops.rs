//! A bounded bitset traversal: clear a set bit, continue while the result is
//! nonzero, and carry that exact result to the next iteration. Popcount strictly
//! decreases for nonzero masks; an initially empty mask exits on its first visit.
//! Other recurrences and multi-block loops remain unproven.

use crate::ir::{
    analysis::Constants, BlockId, Definition, Function, Logic, Operation, Relation, Terminator,
    ValueId,
};

pub(super) fn bitset_iterations(
    function: &Function,
    constants: &Constants,
    block: BlockId,
) -> Option<u64> {
    let Terminator::Branch { condition, yes, no, .. } =
        function.blocks[block.0].terminator.as_ref()?
    else {
        return None;
    };
    let (backedge, continue_when_true) = match (yes.target == block, no.target == block) {
        (true, false) => (yes, true),
        (false, true) => (no, false),
        _ => return None,
    };
    let expressions = Expressions { function, constants };
    let next = expressions.nonzero(*condition, continue_when_true)?;
    let Operation::Logic { op: Logic::And, a, b } = *expressions.operation(next)? else {
        return None;
    };
    for (mask, cleared) in [(a, b), (b, a)] {
        let mask = expressions.word(mask);
        let Definition::Parameter { block: owner, index } = function.values[mask.0].definition
        else {
            continue;
        };
        if owner == block
            && expressions.word(backedge.arguments[index]) == next
            && expressions.clears_bit(cleared, mask)
        {
            return Some(32);
        }
    }
    None
}

struct Expressions<'a> {
    function: &'a Function,
    constants: &'a Constants,
}

impl Expressions<'_> {
    fn operation(&self, value: ValueId) -> Option<&Operation> {
        let Definition::Instruction { block, index } = self.function.values[value.0].definition
        else {
            return None;
        };
        Some(&self.function.blocks[block.0].instructions[index].operation)
    }

    /// Follow exact transport only; SSA verification excludes instruction cycles.
    /// A full-width extraction has no effect on the original 32 bits.
    fn word(&self, mut value: ValueId) -> ValueId {
        loop {
            value = match self.operation(value) {
                Some(Operation::Copy(source)) => *source,
                Some(Operation::ExtractBits { value: source, offset, count, .. })
                    if self.constants.word(*offset) == Some(0)
                        && self.constants.word(*count).is_some_and(|count| count >= 32) =>
                {
                    *source
                }
                _ => return value,
            };
        }
    }

    /// Recover the value whose nonzero test selects the backedge. Constant
    /// predicate identities may wrap the test without changing its meaning.
    fn nonzero(&self, mut condition: ValueId, mut when_true: bool) -> Option<ValueId> {
        loop {
            match *self.operation(condition)? {
                Operation::Copy(source) => condition = source,
                Operation::PredicateNot(source) => {
                    condition = source;
                    when_true = !when_true;
                }
                Operation::PredicateLogic { op, a, b } => {
                    let (constant, other) = self
                        .constants
                        .predicate(a)
                        .map(|v| (v, b))
                        .or_else(|| self.constants.predicate(b).map(|v| (v, a)))?;
                    match (op, constant) {
                        (Logic::And, true) | (Logic::Or, false) | (Logic::Xor, false) => {}
                        (Logic::Xor, true) => when_true = !when_true,
                        _ => return None,
                    }
                    condition = other;
                }
                Operation::Compare { op, a, b, .. } => {
                    if !matches!(
                        (op, when_true),
                        (Relation::Equal, false) | (Relation::NotEqual, true)
                    ) {
                        return None;
                    }
                    let value = if self.constants.word(a) == Some(0) {
                        b
                    } else if self.constants.word(b) == Some(0) {
                        a
                    } else {
                        return None;
                    };
                    return Some(self.word(value));
                }
                _ => return None,
            }
        }
    }

    fn clears_bit(&self, value: ValueId, mask: ValueId) -> bool {
        match self.operation(self.word(value)) {
            // mask & (mask - 1) clears the lowest set bit.
            Some(Operation::Add(a, b)) => [(a, b), (b, a)]
                .into_iter()
                .any(|(&a, &b)| self.word(a) == mask && self.constants.word(b) == Some(u32::MAX)),
            // mask & ~(1 << trailing_zeros(mask)); CLZ(reverse(mask)) is the
            // lifted Maxwell spelling. Wrapped/unwrapped shifts agree for every
            // nonzero mask; either spelling leaves an empty mask empty.
            Some(Operation::NotBits(shift)) => {
                let Some(Operation::Shift { a, b, right: false, .. }) =
                    self.operation(self.word(*shift))
                else {
                    return false;
                };
                if self.constants.word(*a) != Some(1) {
                    return false;
                }
                let Some(Operation::CountLeadingZeros(reverse)) = self.operation(self.word(*b))
                else {
                    return false;
                };
                matches!(self.operation(self.word(*reverse)), Some(Operation::ReverseBits(value)) if self.word(*value) == mask)
            }
            _ => false,
        }
    }
}
