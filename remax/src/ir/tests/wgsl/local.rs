use super::{compare, gpu::Gpu, validate};
use crate::{
    ir::tests::local_memory::comparison_cases,
    program::wgsl::{Options, Profile},
};

#[test]
fn local_memory_validates_in_both_profiles() {
    for (program, _) in comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            validate(&program.emit_wgsl(Options { profile, ..Default::default() }).unwrap());
        }
    }
}

pub(super) fn compare_local_memory(gpu: &Gpu) -> usize {
    let mut count = 0;
    for (program, cases) in comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = program.emit_wgsl(Options { profile, ..Default::default() }).unwrap();
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
    }
    for (program, cases) in crate::ir::tests::local_memory::promotion::cases() {
        // Include exhaustion before/after removed stores, in both branch paths
        // and loop iterations. Optimizing storage must not optimize away fuel.
        for step_budget in [1, 3, 7, 12, 23, 41, 160] {
            let kernel = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            assert!(!kernel.source.contains("var local_words:"));
            compare(gpu, &program, &kernel, &cases);
            count += cases.len();
        }
        for step_budget in [7, 512] {
            let checked = program.emit_wgsl(Options { step_budget, ..Default::default() }).unwrap();
            let rendering = program
                .emit_wgsl(Options {
                    profile: Profile::Rendering,
                    step_budget,
                    ..Default::default()
                })
                .unwrap();
            let checked = gpu.run(&checked, &cases);
            let rendering = gpu.run(&rendering, &cases);
            let outputs = program.function.outputs.len().max(1);
            for (checked, rendering) in checked.iter().zip(&rendering) {
                // Invalid result words are not observable; status bits and the
                // first fault PC must nevertheless agree exactly.
                assert_eq!(checked[outputs..], rendering[outputs..]);
                if checked[outputs] == 0 {
                    assert_eq!(checked, rendering);
                }
            }
            count += cases.len() * 2;
        }
    }
    count
}

#[test]
fn rendering_promotes_proven_storage_while_checked_retains_the_memory_path() {
    for (program, _) in crate::ir::tests::local_memory::promotion::cases() {
        let checked = program.emit_wgsl(Options::default()).unwrap();
        assert!(checked.source.contains("var local_words:"));
        let rendering = program
            .emit_wgsl(Options { profile: Profile::Rendering, ..Default::default() })
            .unwrap();
        assert!(!rendering.source.contains("var local_words:"));
        assert!(!rendering.source.contains("local_known"));
        validate(&rendering);
    }
}
