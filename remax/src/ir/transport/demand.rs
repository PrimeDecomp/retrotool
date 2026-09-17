//! Conditions under which a transported value can reach a consumer. This is
//! scalar value analysis, not a proof of GPU lane participation. It never moves
//! an instruction, removes a resource read, or changes an execution checkpoint.

use std::collections::{BTreeSet, VecDeque};

use super::paths::{join, Facts, Paths};
use crate::ir::{Definition, Function, Operation, Type, Value, ValueId};

/// Live transport relations, including all operands of non-transport operations
/// and frontend obligations. Multiple uses of the same relation are merged
/// conservatively: one possible use retains every occurrence.
/// Results belong to this exact graph and its value IDs; only type refinement
/// may intervene between analysis and rewriting.
pub(crate) struct Demand {
    live: BTreeSet<(ValueId, ValueId)>,
}

impl Demand {
    /// Requires sealed, structurally valid SSA; native-register transport types
    /// may still need refinement. Cyclic functions retain their original graph:
    /// a predicate's SSA ID alone cannot identify its dynamic loop iteration.
    pub fn analyze(function: &Function, retained: &[ValueId]) -> Option<Self> {
        let paths = Paths::analyze(function)?;
        let mut roots = paths.roots;
        // Metadata is retained without path refinement. Its external verifier
        // may require correspondence between complete provenance networks.
        roots.extend(retained.iter().map(|&v| (v, Some(Facts::default()))));
        let mut required = vec![None; function.values.len()];
        let mut pending = VecDeque::new();
        for (value, facts) in roots {
            if join(&mut required[value.0], facts.as_ref()) {
                pending.push_back(value);
            }
        }
        let mut live = BTreeSet::new();
        while let Some(value) = pending.pop_front() {
            let facts = required[value.0].as_ref().unwrap().clone();
            for source in &paths.sources[value.0] {
                let Some(when) = source.when.as_ref().and_then(|when| facts.together(when)) else {
                    continue;
                };
                live.insert((value, source.value));
                if join(&mut required[source.value.0], Some(&when)) {
                    pending.push_back(source.value);
                }
            }
        }
        Some(Self { live })
    }

    pub fn retains(&self, result: ValueId, source: ValueId) -> bool {
        self.live.contains(&(result, source))
    }

    /// Supply arbitrary ordinary values only on transport alternatives proven
    /// unobservable. This does not assign native bits to an opaque token. Live
    /// alternatives keep their exact values and unavailable-input behavior.
    /// Opaque destinations are untouched: we cannot construct arbitrary tokens.
    pub fn erase_unobserved(&self, function: &mut Function) {
        let mut defaults = [None, None];
        let values = &mut function.values;
        let mut replacement = |result: ValueId, source: &mut ValueId| {
            if !self.retains(result, *source) {
                let ty = values[result.0].ty;
                let (slot, definition) = match ty {
                    Type::Bits32 => (0, Definition::Word(0)),
                    Type::Predicate => (1, Definition::Predicate(false)),
                    _ => return,
                };
                *source = *defaults[slot].get_or_insert_with(|| {
                    let id = ValueId(values.len());
                    values.push(Value { ty, definition });
                    id
                });
            }
        };
        let parameters: Vec<_> = function.blocks.iter().map(|b| b.parameters.clone()).collect();
        for block in &mut function.blocks {
            for instruction in &mut block.instructions {
                let Some(result) = instruction.result else { continue };
                match &mut instruction.operation {
                    Operation::Copy(source) => replacement(result, source),
                    Operation::Select { yes, no, .. } => {
                        replacement(result, yes);
                        replacement(result, no);
                    }
                    _ => {}
                }
            }
            for edge in block.terminator.as_mut().unwrap().edges_mut() {
                for (&parameter, argument) in
                    parameters[edge.target.0].iter().zip(&mut edge.arguments)
                {
                    replacement(parameter, argument);
                }
            }
        }
    }
}
