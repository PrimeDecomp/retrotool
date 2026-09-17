use super::{compare, gpu::Gpu, options, validate};
use crate::{ir::tests::indexed_attributes::comparison_cases, program::wgsl::Profile};

#[test]
fn indexed_attributes_validate_in_both_profiles() {
    for (program, _) in comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = program.emit_wgsl(super::Options { profile, ..options() }).unwrap();
            validate(&kernel);
        }
    }
}

pub(super) fn compare_indexed_attributes(gpu: &Gpu) -> usize {
    let mut comparisons = 0;
    for (program, cases) in comparison_cases() {
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = program.emit_wgsl(super::Options { profile, ..options() }).unwrap();
            compare(gpu, &program, &kernel, &cases);
            comparisons += cases.len();
        }
    }
    comparisons
}
