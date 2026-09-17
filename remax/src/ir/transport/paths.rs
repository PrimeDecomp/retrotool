//! Conservative scalar path conditions shared by transport analyses. Predicate
//! identities are valid only in acyclic graphs; lane participation is separate.

use std::collections::{BTreeMap, VecDeque};

use crate::ir::{BlockId, Definition, Function, Operation, Terminator, ValueId};

const MAX_FACTS: usize = 32;

/// Necessary predicate facts. Dropping a fact loses precision, never safety.
/// A bounded conjunction avoids enumerating paths through predication diamonds.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Facts(BTreeMap<ValueId, bool>);

impl Facts {
    fn intersect(&mut self, other: &Self) {
        self.0.retain(|key, value| other.0.get(key) == Some(value));
    }

    fn constrain(&mut self, value: ValueId, expected: bool) -> bool {
        if let Some(&known) = self.0.get(&value) {
            return known == expected;
        }
        if self.0.len() < MAX_FACTS {
            self.0.insert(value, expected);
        }
        true
    }

    pub(super) fn together(&self, other: &Self) -> Option<Self> {
        let mut result = self.clone();
        for (&value, &expected) in &other.0 {
            if !result.constrain(value, expected) {
                return None;
            }
        }
        Some(result)
    }
}

pub(super) struct Source {
    pub(super) value: ValueId,
    pub(super) when: Option<Facts>,
}

pub(super) struct Paths {
    entries: Vec<Option<Facts>>,
    selection_conditions: Vec<Option<ValueId>>,
    pub(super) sources: Vec<Vec<Source>>,
    pub(super) roots: Vec<(ValueId, Option<Facts>)>,
}

impl Paths {
    pub(super) fn analyze(function: &Function) -> Option<Self> {
        let order = acyclic_order(function)?;
        let mut entries = vec![None; function.blocks.len()];
        entries[0] = Some(Facts::default());
        let mut sources: Vec<Vec<Source>> =
            (0..function.values.len()).map(|_| Vec::new()).collect();
        let mut selection_conditions = vec![None; function.values.len()];
        let mut roots = Vec::new();
        for block_id in order {
            let block = &function.blocks[block_id.0];
            let facts = entries[block_id.0].clone();
            for instruction in &block.instructions {
                match instruction.operation {
                    Operation::Copy(value) => sources[instruction.result.unwrap().0]
                        .push(Source { value, when: facts.clone() }),
                    Operation::Select { condition, yes, no } => {
                        roots.push((condition, facts.clone()));
                        let (origin, _) = predicate_origin(function, condition);
                        if !matches!(function.values[origin.0].definition, Definition::Predicate(_))
                        {
                            selection_conditions[instruction.result.unwrap().0] = Some(origin);
                        }
                        for (value, expected) in [(yes, true), (no, false)] {
                            sources[instruction.result.unwrap().0].push(Source {
                                value,
                                when: constrained(function, facts.clone(), condition, expected),
                            });
                        }
                    }
                    _ => roots.extend(
                        instruction.operation.operands().into_iter().map(|v| (v, facts.clone())),
                    ),
                }
            }
            let term = block.terminator.as_ref().unwrap();
            match term {
                Terminator::Branch { condition, .. } => roots.push((*condition, facts.clone())),
                Terminator::Return(values) => {
                    roots.extend(values.iter().map(|&v| (v, facts.clone())));
                }
                Terminator::Jump(_) => {}
            }
            for (index, edge) in term.edges().into_iter().enumerate() {
                let when = match term {
                    Terminator::Branch { condition, .. } => {
                        constrained(function, facts.clone(), *condition, index == 0)
                    }
                    _ => facts.clone(),
                };
                join(&mut entries[edge.target.0], when.as_ref());
                for (&value, &parameter) in
                    edge.arguments.iter().zip(&function.blocks[edge.target.0].parameters)
                {
                    sources[parameter.0].push(Source { value, when: when.clone() });
                }
            }
        }
        Some(Self { entries, selection_conditions, sources, roots })
    }

    /// Every feasible transport alternative must originate at this exact value.
    /// Intersecting conditions when paths merge can only add possible origins.
    pub(super) fn unique_origin(&self, value: ValueId, block: BlockId) -> Option<ValueId> {
        let facts = self.entries[block.0].as_ref()?;
        let mut required = BTreeMap::from([(value, Some(facts.clone()))]);
        let mut pending = VecDeque::from([value]);
        let mut origin = None;
        while let Some(value) = pending.pop_front() {
            if self.sources[value.0].is_empty() {
                if origin.is_some_and(|previous| previous != value) {
                    return None;
                }
                origin = Some(value);
                continue;
            }
            let facts = required[&value].as_ref().unwrap().clone();
            // Select propagates an unavailable condition even when both arms
            // carry the same value. Branch facts establish that a condition was
            // observed; hypothetical select-arm constraints alone do not.
            if self.selection_conditions[value.0]
                .is_some_and(|condition| !facts.0.contains_key(&condition))
            {
                return None;
            }
            for source in &self.sources[value.0] {
                let Some(when) = source.when.as_ref().and_then(|when| facts.together(when)) else {
                    continue;
                };
                if join(required.entry(source.value).or_default(), Some(&when)) {
                    pending.push_back(source.value);
                }
            }
        }
        origin
    }
}

/// None is unreachable; an empty conjunction is reachable with no known facts.
/// Merging possible uses retains only conditions necessary for every use.
pub(super) fn join(destination: &mut Option<Facts>, source: Option<&Facts>) -> bool {
    let Some(source) = source else { return false };
    let Some(current) = destination else {
        *destination = Some(source.clone());
        return true;
    };
    let before = current.0.len();
    current.intersect(source);
    before != current.0.len()
}

fn constrained(
    function: &Function,
    facts: Option<Facts>,
    condition: ValueId,
    expected: bool,
) -> Option<Facts> {
    let mut facts = facts?;
    let (condition, inverted) = predicate_origin(function, condition);
    let expected = expected ^ inverted;
    if let Definition::Predicate(value) = function.values[condition.0].definition {
        return (value == expected).then_some(facts);
    }
    facts.constrain(condition, expected).then_some(facts)
}

fn predicate_origin(function: &Function, mut condition: ValueId) -> (ValueId, bool) {
    let mut inverted = false;
    // Only identities are followed. Native register names are deliberately not
    // involved: another write to P0 creates a different SSA predicate.
    while let Definition::Instruction { block, index } = function.values[condition.0].definition {
        match function.blocks[block.0].instructions[index].operation {
            Operation::Copy(source) => condition = source,
            Operation::PredicateNot(source) => {
                condition = source;
                inverted = !inverted;
            }
            _ => break,
        }
    }
    (condition, inverted)
}

fn acyclic_order(function: &Function) -> Option<Vec<BlockId>> {
    let mut incoming = vec![0; function.blocks.len()];
    for block in &function.blocks {
        for edge in block.terminator.as_ref().unwrap().edges() {
            incoming[edge.target.0] += 1;
        }
    }
    let mut pending: VecDeque<_> = incoming
        .iter()
        .enumerate()
        .filter(|(_, count)| **count == 0)
        .map(|(index, _)| BlockId(index))
        .collect();
    let mut order = Vec::new();
    while let Some(block) = pending.pop_front() {
        order.push(block);
        for edge in function.blocks[block.0].terminator.as_ref().unwrap().edges() {
            incoming[edge.target.0] -= 1;
            if incoming[edge.target.0] == 0 {
                pending.push_back(edge.target);
            }
        }
    }
    (order.len() == function.blocks.len()).then_some(order)
}
