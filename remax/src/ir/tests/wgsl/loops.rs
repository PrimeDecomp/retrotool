//! Compare control-flow lowering with the untouched interpreter graph. Nested
//! loops, several latches, nontermination and fallback graphs exercise paths
//! that are not established by the single-loop material fixture.

use super::{compare, gpu::Gpu, validate};
use crate::{
    ir::{
        analysis::EntryPrefix,
        tests::{add, branch, isetp, lift, mov, swapping_loop, EXIT},
        BlockId,
    },
    program::{
        wgsl::{ControlFlow, Options, Profile},
        Inputs, Program, Register, Value,
    },
    structured::{self, Node, Region, Shape, Strategy},
};

fn inputs(registers: &[(Register, Value)]) -> Inputs {
    Inputs { registers: registers.iter().copied().collect(), ..Default::default() }
}

fn cases() -> Vec<(Program, Vec<Inputs>, ControlFlow)> {
    use Register::{Gpr as R, Predicate as P};
    let nested = lift(
        &[
            mov(0, 0, 7),
            mov(1, 0, 7),
            mov(2, 0, 7),
            add(0, 0, 1),
            add(2, 2, 1),
            isetp(0, 7, 2, 4, 1, 7, 7),
            branch(6, 3, 0),
            add(1, 1, 1),
            isetp(0, 7, 1, 3, 1, 7, 7),
            branch(9, 2, 0),
            EXIT,
        ],
        &[R(3), R(4)],
        &[R(0)],
    );
    let mut nested_inputs = vec![Inputs::default()];
    for (outer, inner) in [(0, 0), (1, 2), (2, 3), (3, 1)] {
        let input = inputs(&[(R(3), Value::Word(outer)), (R(4), Value::Word(inner))]);
        assert_eq!(nested.interpret(&input, 512).unwrap().into_outputs().unwrap(), [Value::Word(
            outer.max(1) * inner.max(1)
        )]);
        nested_inputs.push(input);
    }
    let latches = lift(
        &[
            mov(0, 0, 7),
            add(0, 0, 1),
            isetp(0, 7, 0, 2, 6, 7, 7),
            branch(3, 8, 0),
            branch(4, 1, 1),
            add(0, 0, 1),
            branch(6, 1, 7),
            0,
            EXIT,
        ],
        &[R(2), P(1)],
        &[R(0)],
    );
    let mut latch_inputs = vec![Inputs::default()];
    for limit in [1, 2, 3] {
        for shortcut in [false, true] {
            let input = inputs(&[(R(2), Value::Word(limit)), (P(1), Value::Predicate(shortcut))]);
            assert_eq!(latches.interpret(&input, 512).unwrap().into_outputs().unwrap(), [
                Value::Word(if shortcut { limit } else { limit | 1 })
            ]);
            latch_inputs.push(input);
        }
    }
    let infinite = lift(&[mov(0, 0, 7), add(0, 0, 1), branch(2, 1, 7)], &[], &[R(0)]);
    let irreducible = lift(
        &[
            mov(0, 0, 7),
            branch(1, 4, 0),
            add(0, 0, 1),
            branch(3, 6, 7),
            add(0, 0, 2),
            branch(5, 6, 7),
            branch(6, 2, 1),
            branch(7, 4, 2),
            EXIT,
        ],
        &[P(0), P(1), P(2)],
        &[R(0)],
    );
    let multiple_exits = lift(
        &[
            mov(0, 0, 7),
            add(0, 0, 1),
            branch(2, 6, 0),
            branch(3, 7, 1),
            branch(4, 1, 7),
            0,
            EXIT,
            mov(0, 9, 7),
            EXIT,
        ],
        &[P(0), P(1)],
        &[R(0)],
    );
    let predicates: Vec<_> = (0..8)
        .map(|mask| {
            inputs(
                &(0..3)
                    .map(|index| (P(index), Value::Predicate(mask & (1 << index) != 0)))
                    .collect::<Vec<_>>(),
            )
        })
        .chain([Inputs::default()])
        .collect();
    vec![
        (nested, nested_inputs, ControlFlow::Regions),
        (latches, latch_inputs, ControlFlow::Regions),
        (infinite, vec![Inputs::default()], ControlFlow::Regions),
        (Program { function: swapping_loop() }, vec![Inputs::default()], ControlFlow::Regions),
        (irreducible, predicates.clone(), ControlFlow::ScalarDispatcher),
        (multiple_exits, predicates, ControlFlow::ScalarDispatcher),
    ]
}

#[test]
fn natural_loops_validate_and_unsupported_graphs_keep_dispatch() {
    for (program, _, expected) in cases() {
        let kernel = program
            .emit_wgsl(Options { profile: Profile::Rendering, ..Default::default() })
            .unwrap();
        assert_eq!(kernel.control_flow, expected);
        validate(&kernel);
        let checked = program.emit_wgsl(Options::default()).unwrap();
        assert_eq!(checked.control_flow, ControlFlow::ScalarDispatcher);
    }
}

#[test]
fn nonterminating_loop_keeps_the_validator_required_final_return() {
    let program = lift(&[mov(0, 0, 7), add(0, 0, 1), branch(2, 1, 7)], &[], &[Register::Gpr(0)]);
    let kernel =
        program.emit_wgsl(Options { profile: Profile::Rendering, ..Default::default() }).unwrap();
    validate(&kernel);
    let mut source = kernel.source;
    let sentinel = "return remax_fault(16u, 0u);";
    let at = source.rfind(sentinel).unwrap();
    source.replace_range(at..at + sentinel.len(), "");
    let old = naga::front::wgsl::parse_str(&source).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all()
    )
    .validate(&old)
    .is_err());
    let current = naga_next::front::wgsl::parse_str(&source).unwrap();
    assert!(naga_next::valid::Validator::new(
        naga_next::valid::ValidationFlags::all(),
        naga_next::valid::Capabilities::all()
    )
    .validate(&current)
    .is_err());
}

#[test]
fn loop_verifier_rejects_changed_headers_exits_and_transfer_kinds() {
    let function = swapping_loop();
    function.verify().unwrap();
    let make_shape = || {
        structured::build(&function, &EntryPrefix::new(&function), Strategy::NaturalLoops).unwrap()
    };
    for change_header in [false, true] {
        let mut shape = make_shape();
        let Shape::Regions(region) = &mut shape else { panic!("expected loop regions") };
        let node = region.0.iter_mut().find(|node| matches!(node, Node::Loop { .. })).unwrap();
        let Node::Loop { header, exit, .. } = node else { unreachable!() };
        if change_header {
            *header = BlockId(0);
        } else {
            *exit = Some(BlockId(0));
        }
        assert!(shape.verify(&function).is_err());
    }
    fn change_continue(region: &mut Region) -> bool {
        for node in &mut region.0 {
            let changed = match node {
                Node::Continue => {
                    *node = Node::Break;
                    true
                }
                Node::Loop { body, .. } => change_continue(body),
                Node::Block { arms: Some(arms), .. } => arms.iter_mut().any(change_continue),
                _ => false,
            };
            if changed {
                return true;
            }
        }
        false
    }
    let mut shape = make_shape();
    let Shape::Regions(region) = &mut shape else { panic!("expected loop regions") };
    assert!(change_continue(region));
    assert!(shape.verify(&function).is_err());
    assert!(Shape::Regions(Region(vec![Node::Continue])).verify(&function).is_err());
    let invented_backedge =
        Shape::Regions(Region(vec![Node::Block { block: BlockId(0), arms: None }, Node::Loop {
            header: BlockId(1),
            body: Region(vec![Node::Continue]),
            exit: None,
        }]));
    assert!(invented_backedge.verify(&function).is_err());
}

pub(super) fn compare_loops(gpu: &Gpu) -> usize {
    let mut count = 0;
    for (program, cases, expected) in cases() {
        for step_budget in (1..=48).chain([64, 128, 512]) {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(kernel.control_flow, expected);
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
    }
    count
}
