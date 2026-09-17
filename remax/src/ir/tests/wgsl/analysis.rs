use super::{compare, gpu::Gpu, validate};
use crate::{
    ir::{
        analysis::Analysis,
        tests::{add, branch, lift, mov, EXIT},
    },
    program::{
        wgsl::{Options, Profile},
        Inputs, Register, Value,
    },
    resources::ConstantMemory,
};

#[test]
fn reused_values_validate_across_dominance_and_loop_boundaries() {
    for (program, _) in crate::ir::tests::optimize::reuse::comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            for step_budget in [7, 64] {
                validate(
                    &program
                        .emit_wgsl(Options { profile, step_budget, ..Default::default() })
                        .unwrap(),
                );
            }
        }
    }
}

#[test]
fn proven_bitset_loops_remove_only_budgets_covering_the_complete_traversal() {
    for (program, _, bound) in crate::ir::tests::analysis::steps::bitset::comparison_cases() {
        let checked =
            program.emit_wgsl(Options { step_budget: bound + 1, ..Default::default() }).unwrap();
        validate(&checked);
        assert!(checked.source.contains("steps == 0u"));
        for step_budget in [bound - 1, bound, bound + 1] {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            validate(&kernel);
            let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
            assert_eq!(body.contains("var steps"), step_budget < bound);
            assert_eq!(body.contains("steps == 0u"), step_budget < bound);
        }
    }
}

#[test]
fn finite_prefixes_are_charged_together_without_erasing_cyclic_checks() {
    for (program, _) in crate::ir::tests::analysis::steps::comparison_cases() {
        for step_budget in [1, 2, 3, 8, 15, 16, 17, 64] {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            validate(&kernel);
            let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
            let bounded = Analysis::new(&program.function).steps.maximum().is_some();
            if !bounded || step_budget < 16 {
                assert!(body.contains(&format!("steps -= {}u;", step_budget.min(3))));
                assert!(body.contains("steps -= min(steps, 1u);"));
            } else {
                assert!(!body.contains("steps"));
            }
            assert!(body.contains("memory_status != 0u"));
        }
    }
}

#[test]
fn rendering_removes_only_proven_redundant_input_flushing() {
    for (program, _) in crate::ir::tests::optimize::float::comparison_cases() {
        for (profile, flushes) in [(Profile::Checked, 4), (Profile::Rendering, 2)] {
            let kernel = program.emit_wgsl(Options { profile, ..Default::default() }).unwrap();
            validate(&kernel);
            let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
            assert_eq!(body.matches("remax_flush(").count(), flushes);
        }
    }
}

#[test]
fn proven_values_and_finite_paths_do_not_emit_validity_or_fuel_checks() {
    let program = lift(&[mov(0, 5, 7), add(0, 0, 2), EXIT], &[], &[Register::Gpr(0)]);
    let maximum = Analysis::new(&program.function).steps.maximum().unwrap() as u32;
    let kernel = program.emit_wgsl(Options { step_budget: maximum, ..Default::default() }).unwrap();
    let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
    assert!(!body.contains(": bool ="));
    assert!(!body.contains("remax_fault("));
    assert!(!body.contains("steps"));
    validate(&kernel);

    let kernel =
        program.emit_wgsl(Options { step_budget: maximum - 1, ..Default::default() }).unwrap();
    assert!(kernel.source.contains("steps -= min(steps, 1u)"));
    validate(&kernel);
}

#[test]
fn available_resource_results_do_not_erase_access_validation() {
    let program = lift(&[0x4c98078c00070000, EXIT], &[], &[Register::Gpr(0)]); // c[3][0]
    let kernel = program.emit_wgsl(Options::default()).unwrap();
    let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
    assert!(kernel.constant_memory.is_some());
    assert!(body.contains("memory_status != 0u"));
    assert!(!body.contains(": bool ="));
    validate(&kernel);
}

pub(super) fn compare_analysis_results(gpu: &Gpu) -> usize {
    use Register::{Gpr as R, Predicate as P};
    let mut count = 0;
    for (program, cases) in crate::ir::tests::optimize::reuse::comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            for step_budget in [1, 2, 3, 5, 7, 16, 32, 64] {
                let kernel = program
                    .emit_wgsl(Options { profile, step_budget, ..Default::default() })
                    .unwrap();
                compare(gpu, &program, &kernel, &cases);
                count += cases.len();
            }
        }
    }
    for (program, cases, bound) in crate::ir::tests::analysis::steps::bitset::comparison_cases() {
        for step_budget in (1..=16).chain([bound - 1, bound, bound + 1]) {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
    }
    for (program, cases) in crate::ir::tests::analysis::steps::comparison_cases() {
        // Includes exhaustion before/after resource faults and branch joins,
        // successful early exits, loop exits and nontermination.
        for step_budget in 1..=24 {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
    }
    for (program, cases) in crate::ir::tests::optimize::float::comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            for step_budget in 1..=6 {
                let kernel = program
                    .emit_wgsl(Options { profile, step_budget, ..Default::default() })
                    .unwrap();
                compare(gpu, &program, &kernel, &cases);
                count += cases.len();
            }
        }
    }
    // The rendering optimizer must preserve each checkpoint, including those
    // formerly represented by a linear block transfer. Compare against the
    // untouched interpreter graph, rather than interpreting optimized IR.
    for (program, mut cases) in crate::ir::tests::optimize::cases() {
        // Rendering requires this guarded fixed read at bind time, regardless
        // of the selected path. Missing-resource behavior is checked before
        // binding in the scalar optimization test.
        for input in &mut cases {
            input.constants = ConstantMemory::new(&[(3, &91u32.to_le_bytes())]).unwrap();
        }
        for step_budget in 1..=40 {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
    }
    let program =
        lift(&[mov(0, 1, 7), branch(1, 4, 0), add(0, 0, 1), add(0, 0, 2), EXIT], &[P(0)], &[R(0)]);
    let maximum = Analysis::new(&program.function).steps.maximum().unwrap() as u32;
    let cases = [
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() },
        Inputs { registers: [(P(0), Value::Predicate(true))].into(), ..Default::default() },
        Inputs::default(),
    ];
    // Exercise every exhaustion PC, both branch lengths, and the transition to
    // a kernel without a runtime counter. Missing predicates must still fault.
    for step_budget in 1..=maximum + 1 {
        let kernel = program.emit_wgsl(Options { step_budget, ..Default::default() }).unwrap();
        compare(gpu, &program, &kernel, &cases);
        count += cases.len();
    }

    // Both sides of the join are established only when the initial write exists.
    // Removing flags must retain faults for the other path and for missing guards.
    for initialized in [false, true] {
        let mut words = if initialized { vec![mov(0, 9, 7)] } else { vec![] };
        words.extend([mov(0, 1, 0), add(0, 0, 3), EXIT]);
        let program = lift(&words, &[P(0)], &[R(0)]);
        let kernel = program.emit_wgsl(Options::default()).unwrap();
        compare(gpu, &program, &kernel, &cases);
        count += cases.len();
    }

    let program = lift(&[0x4c98078c00070000, EXIT], &[], &[R(0)]);
    let kernel = program.emit_wgsl(Options::default()).unwrap();
    let cases = [Inputs::default(), Inputs {
        constants: ConstantMemory::new(&[(3, &91u32.to_le_bytes())]).unwrap(),
        ..Default::default()
    }];
    // A dispatch shares one immutable constant-memory binding across its lanes.
    for input in &cases {
        compare(gpu, &program, &kernel, std::slice::from_ref(input));
    }
    count + cases.len()
}
