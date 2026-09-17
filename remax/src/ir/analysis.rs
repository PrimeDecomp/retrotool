//! Binding-independent facts about a verified function. These do not establish
//! resource contents, numeric fidelity, or participation of neighboring lanes.

use std::collections::VecDeque;

use super::{Definition, Function, ValueId};

mod constants;
pub(crate) use constants::Constants;
mod float;
pub(crate) use float::FloatFacts;
mod bits;
#[cfg(test)]
pub(crate) use bits::Bits;
pub(crate) use bits::KnownBits;
mod execution;
pub(crate) use execution::{EntryPrefix, Step as EntryStep};
mod attributes;
pub(crate) use attributes::AttributeInputs;
mod local;
pub(crate) use local::LocalSlots;
mod steps;
pub(crate) use steps::StepBounds;

pub(crate) struct Analysis {
    pub availability: Availability,
    pub constants: Constants,
    pub bits: KnownBits,
    pub attributes: AttributeInputs,
    pub entry_prefix: EntryPrefix,
    pub steps: StepBounds,
}

impl Analysis {
    /// The caller must verify SSA, types and reachability before analysis.
    pub fn new(function: &Function) -> Self {
        let dependencies = Dependencies::new(function);
        let constants = Constants::analyze(function, &dependencies);
        let bits = KnownBits::analyze(function, &dependencies, &constants);
        let attributes = AttributeInputs::analyze(function, &dependencies, &bits);
        let steps = StepBounds::new(function, &constants);
        Self {
            availability: Availability::new(function, &dependencies),
            constants,
            bits,
            attributes,
            entry_prefix: EntryPrefix::new(function),
            steps,
        }
    }
}

/// Value dependencies include incoming edge arguments for block parameters.
/// This is a dataflow graph; control/effect dependencies require separate analysis.
struct Dependencies {
    operands: Vec<Vec<ValueId>>,
    users: Vec<Vec<ValueId>>,
}

impl Dependencies {
    fn new(function: &Function) -> Self {
        let mut operands = vec![Vec::new(); function.values.len()];
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Some(result) = instruction.result {
                    operands[result.0] = instruction.operation.operands();
                }
            }
            for edge in block.terminator.as_ref().unwrap().edges() {
                for (&argument, &parameter) in
                    edge.arguments.iter().zip(&function.blocks[edge.target.0].parameters)
                {
                    operands[parameter.0].push(argument);
                }
            }
        }
        let mut users = vec![Vec::new(); function.values.len()];
        for (index, sources) in operands.iter().enumerate() {
            for &source in sources {
                users[source.0].push(ValueId(index));
            }
        }
        Self { operands, users }
    }

    /// Solve a finite-height, monotone value lattice. Joins only discard facts
    /// as new paths become known; users are revisited whenever an operand changes.
    fn solve<T: Copy + Eq>(
        &self,
        bottom: T,
        join: impl Fn(T, T) -> T,
        evaluate: impl Fn(&[T], ValueId, &[ValueId]) -> T,
    ) -> Vec<T> {
        let mut facts = vec![bottom; self.operands.len()];
        let mut queued = vec![true; facts.len()];
        let mut pending = (0..facts.len()).map(ValueId).collect::<VecDeque<_>>();
        while let Some(value) = pending.pop_front() {
            queued[value.0] = false;
            let fact = join(facts[value.0], evaluate(&facts, value, &self.operands[value.0]));
            if fact == facts[value.0] {
                continue;
            }
            facts[value.0] = fact;
            for &user in &self.users[value.0] {
                if !queued[user.0] {
                    queued[user.0] = true;
                    pending.push_back(user);
                }
            }
        }
        facts
    }
}

/// Whether a value may carry unavailable incoming register contents. This is
/// separate from checking an executed resource read: a load can have an available
/// result only after its binding/range/input obligations have been satisfied.
/// Neither a resource access nor its checks may be removed using this fact alone.
pub(crate) struct Availability {
    may_be_unavailable: Vec<bool>,
}

impl Availability {
    fn new(function: &Function, dependencies: &Dependencies) -> Self {
        // Both SELECT arms participate conservatively. Emitted dynamic validity
        // retains lazy choice even when this analysis cannot establish the result.
        // Propagate possible unavailability from its actual sources. Starting
        // with every loop parameter unknown would miss inductively known cycles
        // such as a counter initialized to a literal and incremented on the backedge.
        let may_be_unavailable = dependencies.solve(
            false,
            |a, b| a || b,
            |facts, value, operands| {
                if let Definition::Instruction { block, index } =
                    function.values[value.0].definition
                {
                    if matches!(
                        function.blocks[block.0].instructions[index].operation,
                        super::Operation::LocalLoad(_)
                    ) {
                        // Address availability says nothing about earlier writes.
                        // Until memory promotion/dataflow proves otherwise, keep
                        // the load's byte-validity result dynamic in both profiles.
                        return true;
                    }
                }
                matches!(
                    function.values[value.0].definition,
                    Definition::Input(_)
                        | Definition::Unavailable(_)
                        | Definition::UnavailableLocal(_)
                ) || operands.iter().any(|operand| facts[operand.0])
            },
        );
        Self { may_be_unavailable }
    }

    pub fn is_available(&self, value: ValueId) -> bool { !self.may_be_unavailable[value.0] }
}
