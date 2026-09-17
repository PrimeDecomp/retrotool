mod known_bits;
pub(in crate::ir::tests) mod steps;

use super::{add, branch, lift, mov, swapping_loop, EXIT};
use crate::{
    ir::{analysis::Analysis, Definition, Terminator, Type, ValueId},
    program::{DiagnosticKind, Inputs, Register, Value},
};

#[test]
fn availability_propagates_through_cycles_from_their_incoming_values() {
    let mut function = swapping_loop();
    function.verify().unwrap();
    let analysis = Analysis::new(&function);
    assert!(analysis.steps.maximum().is_none());
    assert!(
        (0..function.values.len()).all(|index| analysis.availability.is_available(ValueId(index)))
    );

    let missing = function.value(Type::Bits32, Definition::Unavailable(Register::Gpr(7)));
    function.blocks[0].terminator.as_mut().unwrap().edges_mut()[0].arguments[0] = missing;
    function.verify().unwrap();
    let analysis = Analysis::new(&function);
    let parameters = &function.blocks[1].parameters;
    // The two swapped values can now be unavailable on alternate iterations.
    // The literal-initialized loop counter is still established independently.
    assert!(!analysis.availability.is_available(parameters[0]));
    assert!(!analysis.availability.is_available(parameters[1]));
    assert!(analysis.availability.is_available(parameters[2]));
}

#[test]
fn every_incoming_join_value_must_be_established() {
    use Register::{Gpr as R, Predicate as P};
    for initialized in [false, true] {
        let mut words = if initialized { vec![mov(0, 2, 7)] } else { vec![] };
        words.extend([mov(0, 1, 0), add(0, 0, 3), EXIT]);
        let program = lift(&words, &[P(0)], &[R(0)]);
        let analysis = Analysis::new(&program.function);
        for block in &program.function.blocks {
            if let Some(Terminator::Return(values)) = &block.terminator {
                assert_eq!(analysis.availability.is_available(values[0]), initialized);
            }
        }
        let predicate = program
            .function
            .values
            .iter()
            .position(|value| matches!(value.definition, Definition::Input(P(0))))
            .unwrap();
        assert!(!analysis.availability.is_available(ValueId(predicate)));
    }
}

#[test]
fn longest_path_matches_exhaustive_branch_execution_budgets() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(
        &[
            mov(0, 1, 7),
            branch(1, 4, 0),
            add(0, 0, 1),
            add(0, 0, 2),
            EXIT & !(7 << 16) | (1 << 16), // @P1 EXIT
            add(0, 0, 4),
            EXIT,
        ],
        &[P(0), P(1)],
        &[R(0)],
    );
    let maximum = Analysis::new(&program.function).steps.maximum().unwrap();
    let mut measured = 0;
    for first in [false, true] {
        for second in [false, true] {
            let inputs = Inputs {
                registers: [(P(0), Value::Predicate(first)), (P(1), Value::Predicate(second))]
                    .into(),
                ..Default::default()
            };
            let minimum = (0..=maximum)
                .find(|&budget| match program.interpret(&inputs, budget) {
                    Ok(_) => true,
                    Err(error) => {
                        assert_eq!(error.kind, DiagnosticKind::StepLimit);
                        false
                    }
                })
                .expect("computed bound covers the path");
            measured = measured.max(minimum);
        }
    }
    assert_eq!(measured, maximum);
}

#[test]
fn analysis_does_not_infer_termination_from_a_conditional_exit() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(&[mov(0, 1, 7), branch(1, 1, 0), EXIT], &[P(0)], &[R(0)]);
    assert!(Analysis::new(&program.function).steps.maximum().is_none());
    let inputs =
        Inputs { registers: [(P(0), Value::Predicate(true))].into(), ..Default::default() };
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::StepLimit);
}

#[test]
fn constant_loop_facts_reach_a_fixed_point_before_use() {
    let mut function = swapping_loop();
    function.verify().unwrap();
    let constants = Analysis::new(&function).constants;
    for &parameter in &function.blocks[1].parameters {
        assert_eq!(constants.word(parameter), None);
    }

    // Equal inputs to the swap remain equal on every iteration. The counter
    // changes on the backedge, so its initially constant fact must be revoked.
    let edge = function.blocks[0].terminator.as_mut().unwrap().edges_mut()[0].clone();
    function.blocks[0].terminator.as_mut().unwrap().edges_mut()[0].arguments[1] = edge.arguments[0];
    function.verify().unwrap();
    let constants = Analysis::new(&function).constants;
    let parameters = &function.blocks[1].parameters;
    assert_eq!(constants.word(parameters[0]), Some(23));
    assert_eq!(constants.word(parameters[1]), Some(23));
    assert_eq!(constants.word(parameters[2]), None);
}

#[test]
fn constants_preserve_select_laziness_without_inventing_an_incoming_guard() {
    use crate::ir::{ssa::Builder, Operation};
    let mut builder = Builder::new(&[], &[Register::Gpr(0)]);
    let block = builder.block(None);
    let condition = builder.predicate(true);
    let yes = builder.word(12);
    let no = builder.read(block, Register::Gpr(1));
    let result =
        builder.function.append(block, 8, Type::Bits32, Operation::Select { condition, yes, no });
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(vec![result]));
    let mut function = builder.finish().0;
    function.verify().unwrap();
    assert_eq!(Analysis::new(&function).constants.word(result), Some(12));
    function.values[condition.0].definition = Definition::Unavailable(Register::Predicate(0));
    function.verify().unwrap();
    assert_eq!(Analysis::new(&function).constants.word(result), None);
}

#[test]
fn entry_prefix_stops_before_repeating_blocks_and_rejects_other_entries() {
    use crate::ir::{
        analysis::{EntryPrefix, EntryStep},
        convergence::BranchKind,
        BlockId,
    };
    let function = swapping_loop();
    function.verify().unwrap();
    let prefix = EntryPrefix::new(&function);
    assert_eq!(prefix.steps, [EntryStep::Block(BlockId(0))]);
    prefix.verify(&function).unwrap();
    assert!(EntryPrefix {
        steps: vec![EntryStep::Block(BlockId(0)), EntryStep::Block(BlockId(1))]
    }
    .verify(&function)
    .unwrap_err()
    .message
    .contains("another incoming edge"));
    assert!(EntryPrefix {
        steps: vec![EntryStep::Block(BlockId(0)), EntryStep::Block(BlockId(0))]
    }
    .verify(&function)
    .is_err());

    let mut program =
        lift(&[mov(0, 1, 7), mov(0, 2, 0), EXIT], &[Register::Predicate(0)], &[Register::Gpr(0)]);
    // An ordinary diamond has no native participation guarantee.
    for block in &mut program.function.blocks {
        if let Some(Terminator::Branch { kind, .. }) = &mut block.terminator {
            *kind = BranchKind::ControlFlow;
        }
    }
    let prefix = EntryPrefix::new(&program.function);
    prefix.verify(&program.function).unwrap();
    let last = prefix.last();
    let Some(Terminator::Branch { yes, .. }) = &program.function.blocks[last.0].terminator else {
        panic!()
    };
    let mut extended = prefix;
    extended.steps.push(EntryStep::Block(yes.target));
    assert!(extended
        .verify(&program.function)
        .unwrap_err()
        .message
        .contains("unconditional edges"));
}

#[test]
fn predication_rejoins_entry_participation_but_keeps_its_body_conditional() {
    use crate::ir::{
        analysis::{EntryPrefix, EntryStep},
        convergence::BranchKind,
    };
    let mut program =
        lift(&[mov(0, 1, 7), mov(0, 2, 0), mov(0, 3, 8), EXIT], &[Register::Predicate(0)], &[
            Register::Gpr(0),
        ]);
    let prefix = EntryPrefix::new(&program.function);
    prefix.verify(&program.function).unwrap();
    assert!(matches!(
        program.function.blocks[prefix.last().0].terminator,
        Some(Terminator::Return(_))
    ));
    let guards: Vec<_> = prefix
        .steps
        .iter()
        .filter_map(|step| match *step {
            EntryStep::Predicated { guard, body } => Some((guard, body)),
            _ => None,
        })
        .collect();
    assert_eq!(guards.len(), 2);
    for &(guard, body) in &guards {
        assert!(prefix.has_entry_participation(guard));
        assert!(!prefix.has_entry_participation(body));
        assert!(prefix.contains(body));
    }
    let (guard, body) = guards[0];
    let original_pc = program.function.blocks[body.0].pc;
    program.function.blocks[body.0].pc = Some(0xffff);
    assert!(program.function.verify().unwrap_err().message.contains("predicated instruction"));
    program.function.blocks[body.0].pc = original_pc;
    let Some(Terminator::Branch { kind, .. }) = &mut program.function.blocks[guard.0].terminator
    else {
        panic!()
    };
    *kind = BranchKind::ControlFlow;
    program.function.verify().unwrap();
    assert!(prefix
        .verify(&program.function)
        .unwrap_err()
        .message
        .contains("predication provenance"));
}
