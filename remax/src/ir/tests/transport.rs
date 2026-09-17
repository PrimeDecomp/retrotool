use Register::{Gpr as R, Predicate as P};

use super::{
    Builder, Definition, Edge, Function, Inputs, Operation, Program, Register, Terminator, Type,
    Value,
};
use crate::ir::{
    convergence::BranchKind,
    transport::{specialize_uses, Demand},
    ValueId,
};

fn rewrite(function: &Function, retained: &[ValueId]) -> Function {
    function.verify().unwrap();
    let mut result = function.clone();
    specialize_uses(&mut result);
    Demand::analyze(&result, retained).unwrap().erase_unobserved(&mut result);
    result.verify().unwrap();
    result
}

fn compare(original: Function, rewritten: Function) {
    let original = Program { function: original };
    let rewritten = Program { function: rewritten };
    for p in [None, Some(false), Some(true)] {
        for q in [None, Some(false), Some(true)] {
            for word in [None, Some(0), Some(0x7fc12345), Some(u32::MAX)] {
                let mut inputs = Inputs::default();
                for (register, value) in [(P(0), p), (P(1), q)] {
                    if let Some(value) = value {
                        inputs.registers.insert(register, Value::Predicate(value));
                    }
                }
                if let Some(word) = word {
                    inputs.registers.insert(R(0), Value::Word(word));
                }
                for budget in 0..32 {
                    assert_eq!(
                        original.interpret(&inputs, budget),
                        rewritten.interpret(&inputs, budget),
                        "budget {budget}, inputs {inputs:?}"
                    );
                }
            }
        }
    }
}

fn diamonds(distinct_predicates: bool) -> (Function, ValueId) {
    let mut builder = Builder::new(&[P(0), P(1), R(0)], &[R(0)]);
    let blocks = [0, 8, 16, 24, 32].map(|pc| builder.block(Some(pc)));
    let p = builder.read(blocks[0], P(0));
    let q = builder.read(blocks[0], if distinct_predicates { P(1) } else { P(0) });
    let eleven = builder.word(11);
    let twenty_two = builder.word(22);
    builder.write(blocks[1], R(0), eleven);
    builder.write(blocks[3], R(0), twenty_two);
    let intermediate = builder.read(blocks[2], R(0));
    let output = builder.read(blocks[4], R(0));
    builder.function.blocks[0].terminator = Some(Terminator::Branch {
        condition: p,
        yes: Edge::new(blocks[1], vec![]),
        no: Edge::new(blocks[2], vec![]),
        kind: BranchKind::ControlFlow,
    });
    builder.function.blocks[1].terminator = Some(Terminator::Jump(Edge::new(blocks[2], vec![])));
    builder.function.blocks[2].terminator = Some(Terminator::Branch {
        condition: q,
        yes: Edge::new(blocks[4], vec![]),
        no: Edge::new(blocks[3], vec![]),
        kind: BranchKind::ControlFlow,
    });
    builder.function.blocks[3].terminator = Some(Terminator::Jump(Edge::new(blocks[4], vec![])));
    builder.function.blocks[4].terminator = Some(Terminator::Return(vec![output]));
    let (function, remap) = builder.finish();
    (function, remap.get(intermediate))
}

#[test]
fn correlated_phi_choices_preserve_missing_inputs_and_every_execution_budget() {
    for distinct in [false, true] {
        let (original, intermediate) = diamonds(distinct);
        let initial = original
            .values
            .iter()
            .position(|v| matches!(v.definition, Definition::Input(R(0))))
            .unwrap();
        let demand = Demand::analyze(&original, &[]).unwrap();
        assert_eq!(demand.retains(intermediate, ValueId(initial)), distinct);
        compare(original.clone(), rewrite(&original, &[]));
        // Frontend metadata can keep a value that has no semantic consumer.
        let demand = Demand::analyze(&original, &[intermediate]).unwrap();
        assert!(demand.retains(intermediate, ValueId(initial)));
        compare(original.clone(), rewrite(&original, &[intermediate]));
    }
}

#[test]
fn selects_follow_boolean_identities_but_preserve_additional_uses() {
    for expose_intermediate in [false, true] {
        let mut builder = Builder::new(&[P(0), R(0)], &[R(0)]);
        let entry = builder.block(Some(0));
        let p = builder.read(entry, P(0));
        let initial = builder.read(entry, R(0));
        let a = builder.word(11);
        let b = builder.word(22);
        let first = builder.function.append(entry, 8, Type::Bits32, Operation::Select {
            condition: p,
            yes: a,
            no: initial,
        });
        let copied = builder.function.append(entry, 16, Type::Predicate, Operation::Copy(p));
        let inverted =
            builder.function.append(entry, 24, Type::Predicate, Operation::PredicateNot(copied));
        let result = builder.function.append(entry, 32, Type::Bits32, Operation::Select {
            condition: inverted,
            yes: b,
            no: first,
        });
        if expose_intermediate {
            // Arithmetic still observes all its operands for type refinement,
            // even when its own result has no subsequent consumer.
            builder.function.append(entry, 40, Type::Bits32, Operation::Add(first, a));
        }
        builder.function.blocks[0].terminator = Some(Terminator::Return(vec![result]));
        let (function, remap) = builder.finish();
        let demand = Demand::analyze(&function, &[]).unwrap();
        assert_eq!(demand.retains(remap.get(first), remap.get(initial)), expose_intermediate);
        compare(function.clone(), rewrite(&function, &[]));
    }
}

#[test]
fn unobserved_select_still_observes_its_condition() {
    let mut builder = Builder::new(&[P(0), R(0)], &[R(0)]);
    let entry = builder.block(Some(0));
    let condition = builder.read(entry, P(0));
    let value = builder.read(entry, R(0));
    let output = builder.word(77);
    builder.function.append(entry, 8, Type::Bits32, Operation::Select {
        condition,
        yes: value,
        no: value,
    });
    builder.function.blocks[0].terminator = Some(Terminator::Return(vec![output]));
    let (function, _) = builder.finish();
    compare(function.clone(), rewrite(&function, &[]));
}

#[test]
fn repeating_predicates_do_not_receive_acyclic_path_facts() {
    let function = super::swapping_loop();
    function.verify().unwrap();
    assert!(Demand::analyze(&function, &[]).is_none());
    let mut rewritten = function.clone();
    specialize_uses(&mut rewritten);
    assert_eq!(format!("{function:?}"), format!("{rewritten:?}"));
}

#[test]
fn each_consumer_uses_its_own_conditions_without_moving_select_faults() {
    for (distinct, uncertain_selector) in [(false, false), (true, false), (false, true)] {
        let mut builder = Builder::new(&[P(0), P(1), R(0)], &[R(0)]);
        let [entry, yes, no, exit] = [0, 8, 16, 24].map(|pc| builder.block(Some(pc)));
        let p = builder.read(entry, P(0));
        let branch = builder.read(entry, if distinct { P(1) } else { P(0) });
        let input = builder.read(entry, R(0));
        let eleven = builder.word(11);
        let mut selected = builder.function.append(entry, 4, Type::Bits32, Operation::Select {
            condition: p,
            yes: input,
            no: eleven,
        });
        if uncertain_selector {
            // Under P0 both alternatives carry `input`, but an unavailable P1
            // must still propagate through the outer selection to its consumer.
            let q = builder.read(entry, P(1));
            selected = builder.function.append(entry, 6, Type::Bits32, Operation::Select {
                condition: q,
                yes: input,
                no: selected,
            });
        }
        for block in [yes, no] {
            let result =
                builder.function.append(block, 8, Type::Bits32, Operation::Add(selected, eleven));
            builder.write(block, R(0), result);
            builder.function.blocks[block.0].terminator =
                Some(Terminator::Jump(Edge::new(exit, vec![])));
        }
        let result = builder.read(exit, R(0));
        builder.function.blocks[entry.0].terminator = Some(Terminator::Branch {
            condition: branch,
            yes: Edge::new(yes, vec![]),
            no: Edge::new(no, vec![]),
            kind: BranchKind::ControlFlow,
        });
        builder.function.blocks[exit.0].terminator = Some(Terminator::Return(vec![result]));
        let (function, remap) = builder.finish();
        let rewritten = rewrite(&function, &[]);
        for (block, expected) in [(yes, input), (no, eleven)] {
            let Operation::Add(operand, _) = rewritten.blocks[block.0].instructions[0].operation
            else {
                panic!("consumer changed");
            };
            assert_eq!(
                operand,
                remap.get(if distinct || uncertain_selector { selected } else { expected })
            );
        }
        if uncertain_selector {
            let mut invalid = rewritten.clone();
            invalid.blocks[yes.0].instructions[0].operation =
                Operation::Add(remap.get(input), remap.get(eleven));
            invalid.verify().unwrap();
            let inputs = Inputs {
                registers: [(P(0), Value::Predicate(true)), (R(0), Value::Word(42))].into(),
                ..Default::default()
            };
            // Type and dominance checks alone cannot catch the lost P1
            // availability dependency. The interpreter provides that oracle.
            assert_ne!(
                Program { function: function.clone() }.interpret(&inputs, 32),
                Program { function: invalid }.interpret(&inputs, 32)
            );
        }
        compare(function, rewritten);
    }
}

#[test]
fn a_conditionally_executed_origin_is_not_substituted_without_dominance() {
    let mut builder = Builder::new(&[P(0), R(0)], &[R(0)]);
    let [entry, first, skip, join, use_value, exit] =
        [0, 8, 16, 24, 32, 40].map(|pc| builder.block(Some(pc)));
    let p = builder.read(entry, P(0));
    let input = builder.read(entry, R(0));
    let eleven = builder.word(11);
    let origin = builder.function.append(first, 8, Type::Bits32, Operation::Add(input, eleven));
    builder.write(first, R(1), origin);
    builder.write(skip, R(1), eleven);
    let choice = builder.read(join, R(1));
    let output =
        builder.function.append(use_value, 32, Type::Bits32, Operation::Add(choice, eleven));
    builder.write(use_value, R(0), output);
    builder.write(join, R(0), eleven);
    let result = builder.read(exit, R(0));
    for (block, yes, no) in [(entry, first, skip), (join, use_value, exit)] {
        builder.function.blocks[block.0].terminator = Some(Terminator::Branch {
            condition: p,
            yes: Edge::new(yes, vec![]),
            no: Edge::new(no, vec![]),
            kind: BranchKind::ControlFlow,
        });
    }
    for (block, target) in [(first, join), (skip, join), (use_value, exit)] {
        builder.function.blocks[block.0].terminator =
            Some(Terminator::Jump(Edge::new(target, vec![])));
    }
    builder.function.blocks[exit.0].terminator = Some(Terminator::Return(vec![result]));
    let (function, remap) = builder.finish();
    let rewritten = rewrite(&function, &[]);
    assert_eq!(
        rewritten.blocks[use_value.0].instructions[0].operation,
        Operation::Add(remap.get(choice), remap.get(eleven))
    );
    let mut invalid = rewritten.clone();
    invalid.blocks[use_value.0].instructions[0].operation =
        Operation::Add(remap.get(origin), remap.get(eleven));
    assert!(invalid.verify().unwrap_err().message.contains("does not dominate"));
    compare(function, rewritten);
}
