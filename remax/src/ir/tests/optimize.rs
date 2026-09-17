use super::{
    add, branch, control, isetp, lift, mov, swapping_loop, Inputs, Program, Register, Value, EXIT,
};

pub(in crate::ir::tests) mod float;
pub(in crate::ir::tests) mod reuse;

pub(super) fn cases() -> Vec<(Program, Vec<Inputs>)> {
    use Register::{Gpr as R, Predicate as P};
    let predicates = || {
        [None, Some(false), Some(true)]
            .map(|value| Inputs {
                registers: value.map(|value| (P(0), Value::Predicate(value))).into_iter().collect(),
                ..Default::default()
            })
            .to_vec()
    };
    let loops = || {
        [None, Some(0), Some(1), Some(3)]
            .map(|value| Inputs {
                registers: value.map(|value| (R(2), Value::Word(value))).into_iter().collect(),
                ..Default::default()
            })
            .to_vec()
    };
    vec![
        (lift(&[mov(0, 1, 7), add(0, 0, 2), add(0, 0, 3), EXIT], &[], &[R(0)]), vec![
            Inputs::default(),
        ]),
        (
            lift(&[mov(0, 1, 7), branch(1, 4, 0), add(0, 0, 1), add(0, 0, 2), EXIT], &[P(0)], &[
                R(0),
            ]),
            predicates(),
        ),
        (
            lift(
                &[mov(0, 7, 7), 0x4c98078c00000000, add(0, 0, 1), EXIT], // @P0 MOV c[3][0]
                &[P(0)],
                &[R(0)],
            ),
            predicates(),
        ),
        (
            lift(
                &[mov(0, 0, 7), add(0, 0, 1), isetp(0, 7, 0, 2, 1, 7, 7), branch(3, 1, 0), EXIT],
                &[R(2)],
                &[R(0)],
            ),
            loops(),
        ),
        (control::sync_loop(), loops()),
        (
            lift(
                &[
                    mov(0, 10, 7),
                    control::setup(1, 8),
                    control::setup(2, 6),
                    control::sync(0),
                    add(0, 0, 1),
                    control::sync(7),
                    add(0, 0, 2),
                    control::sync(7),
                    add(0, 0, 4),
                    EXIT,
                ],
                &[P(0)],
                &[R(0)],
            ),
            predicates(),
        ),
        (Program { function: swapping_loop() }, vec![Inputs::default()]),
    ]
}

#[test]
fn coalescing_preserves_execution_and_each_exhaustion_location() {
    let mut removed_blocks = 0;
    for (program, inputs) in cases() {
        let function = crate::ir::optimize::for_rendering(&program.function).unwrap();
        removed_blocks += program.function.blocks.len() - function.blocks.len();
        let optimized = Program { function };
        // Includes successful paths, missing guards/constants, loops and their
        // simultaneous phi transfers, predication and nested synchronization.
        for inputs in &inputs {
            for budget in 0..100 {
                assert_eq!(
                    optimized.interpret(inputs, budget),
                    program.interpret(inputs, budget),
                    "budget {budget}, inputs {inputs:?}"
                );
            }
        }
    }
    assert!(removed_blocks > 10);
}
