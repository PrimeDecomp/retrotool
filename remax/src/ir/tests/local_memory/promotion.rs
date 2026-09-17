use super::{access, add, branch, input, isetp, lift, loop_program, mov, EXIT, P, R};
use crate::{
    ir::{analysis::Analysis, optimize::promote_local_memory, Operation},
    program::{Diagnostic, DiagnosticKind, Inputs, Program, Value},
};

pub(crate) fn cases() -> Vec<(Program, Vec<Inputs>)> {
    let conditional = lift(
        &[access(true, 4, 255, 0, 4) & !(7 << 16), access(false, 0, 255, 0, 4), EXIT],
        16,
        &[R(4), P(0)],
        &[R(0)],
    );
    let paths = vec![
        input(&[(R(4), Value::Word(17)), (P(0), Value::Predicate(true))]),
        input(&[(R(4), Value::Word(19)), (P(0), Value::Predicate(false))]),
        input(&[(P(0), Value::Predicate(true))]),
        input(&[(R(4), Value::Word(23))]),
    ];
    let overwrite = lift(
        &[
            mov(4, 31, 7),
            access(true, 4, 255, 0, 4),
            access(true, 5, 255, 0, 4) & !(7 << 16),
            access(false, 0, 255, 0, 4),
            EXIT,
        ],
        16,
        &[R(5), P(0)],
        &[R(0)],
    );
    let overwrites = vec![
        input(&[(P(0), Value::Predicate(false))]),
        input(&[(P(0), Value::Predicate(true))]),
        input(&[(R(5), Value::Word(0xabcdef01)), (P(0), Value::Predicate(true))]),
    ];
    let loop_inputs: Vec<_> = (1..8).map(|count| input(&[(R(3), Value::Word(count))])).collect();
    vec![
        (conditional, paths),
        (overwrite, overwrites),
        (loop_program(), loop_inputs.clone()),
        (swapping_slots(), loop_inputs),
    ]
}

fn swapping_slots() -> Program {
    lift(
        &[
            mov(4, 11, 7),
            mov(5, 29, 7),
            access(true, 4, 255, 0, 4),
            access(true, 5, 255, 4, 4),
            mov(1, 0, 7),
            access(false, 4, 255, 0, 4),
            access(false, 5, 255, 4, 4),
            access(true, 5, 255, 0, 4),
            access(true, 4, 255, 4, 4),
            add(1, 1, 1),
            isetp(0, 7, 1, 3, 1, 7, 7),
            branch(11, 5, 0),
            access(false, 0, 255, 0, 4),
            access(false, 1, 255, 4, 4),
            EXIT,
        ],
        16,
        &[R(3)],
        &[R(0), R(1)],
    )
}

#[test]
fn promoted_slots_transfer_simultaneously_on_a_loop_backedge() {
    let original = swapping_slots();
    let promoted =
        Program { function: promote_local_memory(&original.function).unwrap().into_owned() };
    for count in 1..8 {
        let inputs = input(&[(R(3), Value::Word(count))]);
        let expected = if count % 2 == 0 { [11, 29] } else { [29, 11] };
        assert_eq!(
            promoted.interpret(&inputs, 512).unwrap().into_outputs().expect("live invocation"),
            expected.map(Value::Word)
        );
        compare_execution(&original, &promoted, &inputs, 512);
    }
}

fn compare_execution(original: &Program, promoted: &Program, inputs: &Inputs, budget: u64) {
    match (original.interpret(inputs, budget), promoted.interpret(inputs, budget)) {
        (Ok(original), Ok(promoted)) => assert_eq!(original, promoted),
        (Err(original), Err(promoted)) => {
            assert_eq!(original.pc, promoted.pc, "fault moved across an execution point");
            assert_eq!(fault_class(&original), fault_class(&promoted));
        }
        results => panic!("promotion changed execution: {results:?}"),
    }
}

fn fault_class(diagnostic: &Diagnostic) -> DiagnosticKind {
    // Scalar diagnostics can now name the original missing register rather
    // than its spill byte. Both are the same unavailable-value GPU fault.
    match diagnostic.kind {
        DiagnosticKind::MissingInput => DiagnosticKind::UnavailableValue,
        kind => kind,
    }
}

fn memory_count(program: &Program) -> (usize, usize) {
    program.function.blocks.iter().flat_map(|block| &block.instructions).fold(
        (0, 0),
        |(loads, stores), instruction| match instruction.operation {
            Operation::LocalLoad(_) => (loads + 1, stores),
            Operation::LocalStore(_) => (loads, stores + 1),
            _ => (loads, stores),
        },
    )
}

#[test]
fn local_promotion_uses_ssa_for_joins_and_loops_without_changing_budget_faults() {
    for (original, inputs) in cases() {
        let promoted =
            Program { function: promote_local_memory(&original.function).unwrap().into_owned() };
        assert_eq!(memory_count(&promoted), (0, 0));
        assert_eq!(
            Analysis::new(&original.function).steps.maximum(),
            Analysis::new(&promoted.function).steps.maximum()
        );
        for input in inputs {
            for budget in 1..=160 {
                compare_execution(&original, &promoted, &input, budget);
            }
        }
    }
}

#[test]
fn partial_aliases_keep_their_word_in_memory_and_promote_disjoint_wide_components() {
    let original = lift(
        &[
            access(true, 4, 255, 0, 6),
            access(true, 8, 255, 1, 0),
            access(false, 0, 255, 0, 6),
            EXIT,
        ],
        16,
        &[R(4), R(5), R(6), R(7), R(8)],
        &[R(0), R(1), R(2), R(3)],
    );
    let promoted =
        Program { function: promote_local_memory(&original.function).unwrap().into_owned() };
    assert_eq!(memory_count(&promoted), (1, 2));
    let mut inputs = input(&[
        (R(4), Value::Word(0x12345678)),
        (R(5), Value::Word(0xabcdef01)),
        (R(6), Value::Word(0x7fc01234)),
        (R(7), Value::Word(0x80000000)),
        (R(8), Value::Word(0xcc)),
    ]);
    compare_execution(&original, &promoted, &inputs, 100);
    inputs.registers.remove(&R(6));
    compare_execution(&original, &promoted, &inputs, 100);
    inputs.registers.remove(&R(8));
    compare_execution(&original, &promoted, &inputs, 100);
}

#[test]
fn unresolved_aliases_and_invalid_accesses_keep_their_effects() {
    let dynamic = lift(
        &[mov(4, 7, 7), access(true, 4, 255, 0, 4), access(false, 0, 8, 0, 4), EXIT],
        16,
        &[R(8)],
        &[R(0)],
    );
    assert!(matches!(
        promote_local_memory(&dynamic.function).unwrap(),
        std::borrow::Cow::Borrowed(_)
    ));
    let invalid = lift(
        &[mov(4, 7, 7), access(true, 4, 255, 0, 4), access(false, 0, 255, 16, 4), EXIT],
        16,
        &[],
        &[R(0)],
    );
    let promoted =
        Program { function: promote_local_memory(&invalid.function).unwrap().into_owned() };
    assert_eq!(memory_count(&promoted), (1, 0));
    for budget in 1..20 {
        compare_execution(&invalid, &promoted, &Inputs::default(), budget);
    }
}
