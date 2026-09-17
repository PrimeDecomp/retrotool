use super::*;
use crate::program::wgsl::Profile;

#[test]
fn sampled_2d_coordinates_preserve_the_actual_bound_view() {
    for dimension in [D::D2, D::D2Array] {
        for (mode, comparison) in [
            (1, false),
            (2, false),
            (3, false),
            (4, true),
            (6, true),
            (7, false),
            (8, false),
            (9, true),
        ] {
            for profile in [Profile::Checked, Profile::Rendering] {
                let binding = binding(dimension, if comparison { V::Depth } else { V::Float });
                let program = program(&[word(false, mode), EXIT], Stage::Fragment);
                let kernel = program
                    .emit_wgsl_with_textures(Options { profile, ..Default::default() }, &[binding])
                    .unwrap();
                validate(&kernel).unwrap();
                assert_eq!(kernel.textures.len(), 1);
                assert_eq!(kernel.textures[0].binding, binding);
            }
        }
    }
}

#[test]
fn sampling_compatibility_does_not_relax_other_resource_requirements() {
    for (load, mode, dimension) in [
        (true, 2, D::D2Array),
        (true, 8, D::D2),
        (false, 7, D::D3),
        (false, 10, D::D2Array),
        (false, 12, D::D2Array),
    ] {
        let program = program(&[word(load, mode), EXIT], Stage::Fragment);
        let error = program
            .emit_wgsl_with_textures(Options::default(), &[binding(dimension, V::Float)])
            .unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::InvalidInput);
        assert!(error.message.contains("view dimension"));
    }
    let program = program(&[word(false, 8), EXIT], Stage::Fragment);
    let mut binding = binding(D::D2, V::Float);
    binding.view.multisample = true;
    let error = program.emit_wgsl_with_textures(Options::default(), &[binding]).unwrap_err();
    assert!(error.message.contains("sample-count class"));
}
