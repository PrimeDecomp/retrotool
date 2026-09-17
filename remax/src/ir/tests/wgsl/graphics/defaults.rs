//! Output-map defaults are graphics state, not initial register availability.

use super::{
    access, bytes, graphics, header, ipa, mov, options,
    render::{render, Resources},
    validate, vertices, FragmentFrequency, Gpu, GraphicsProgram, MaxwellCode, Options,
    ShaderProgramHeader, StageSource, EXIT,
};
use crate::{
    maxwell::graphics::GenericAttributeDefaults,
    program::{wgsl::Profile, Diagnostic, Register},
};

const ATTRIBUTES: [u16; 4] = [0x80, 0x84, 0x88, 0x8c];

fn omit(header: &mut ShaderProgramHeader, attributes: &[u16]) {
    for &attribute in attributes {
        let bit = 400 + usize::from(attribute / 4);
        header.words[bit / 32] &= !(1 << (bit % 32));
    }
}

fn vertex(write_w: bool) -> Vec<u64> {
    let mut code = vec![access(true, 0, 0x80, 4), access(false, 0, 0x70, 4)];
    if write_w {
        code.extend([mov(4, 0.625f32.to_bits(), 7), access(false, 4, 0x8c, 1)]);
    }
    code.push(EXIT);
    code
}

fn fragment(mode: u32, location: u8, indexed: bool) -> Vec<u64> {
    let reciprocal_location = if location == 2 { 2 } else { 0 };
    let mut code = vec![
        mov(6, (0x4c << 8) | (0x4c << 20), 7),
        ipa(5, 0x7c, 0, reciprocal_location),
        0x5080000000470505,
    ];
    let ipa_mode = match mode {
        1 => 2, // constant
        2 => 1, // perspective
        3 => 0, // screen linear
        _ => unreachable!(),
    };
    for (register, attribute) in ATTRIBUTES.into_iter().enumerate() {
        if indexed {
            assert_eq!((mode, location), (2, 0));
            // AL2P R10,P0,RZ,attribute; IPA.PERSPECTIVE Rdst,a[R10],R5,P0.
            code.push(0xefa000000007ff0a | (u64::from(attribute) << 20));
            code.push(0xe040004000570a00 | (0x80 << 28) | register as u64);
        } else {
            code.push(ipa(register as u8, attribute, ipa_mode, location));
        }
    }
    // Keep the result below one: the multisample harness uses an UNORM target.
    // Centroid/sample numerators use a centered reciprocal multiplier here, so
    // individual samples straddle the expected resolved value.
    code.extend([mov(7, 0.5f32.to_bits(), 7), 0x5c68000000770303, EXIT]); // FMUL R3,R3,R7
    code
}

fn link(
    vertex_code: &[u64],
    vertex_header: &ShaderProgramHeader,
    fragment_code: &[u64],
    fragment_header: &ShaderProgramHeader,
    settings: Options,
) -> Result<GraphicsProgram, Diagnostic> {
    graphics::translate(
        StageSource {
            code: MaxwellCode::parse(&bytes(vertex_code)).unwrap(),
            header: vertex_header,
        },
        StageSource {
            code: MaxwellCode::parse(&bytes(fragment_code)).unwrap(),
            header: fragment_header,
        },
        settings,
        &[],
        &[],
    )
}

fn program(
    defaults: GenericAttributeDefaults,
    partial: bool,
    mode: u32,
    location: u8,
    indexed: bool,
    mut settings: Options,
) -> GraphicsProgram {
    let mut vertex_header = header(true, &[]);
    omit(&mut vertex_header, &ATTRIBUTES[..if partial { 3 } else { 4 }]);
    settings.generic_attribute_defaults = Some(defaults);
    link(
        &vertex(partial),
        &vertex_header,
        &fragment(mode, location, indexed),
        &header(false, &ATTRIBUTES.map(|offset| (offset, mode))),
        settings,
    )
    .unwrap()
}

#[test]
fn generic_defaults_require_known_state_and_both_stage_maps() {
    let mut vertex_header = header(true, &[]);
    omit(&mut vertex_header, &ATTRIBUTES[..3]);
    let vertex_code = vertex(true);
    let fragment_code = fragment(2, 0, false);
    let mut fragment_header = header(false, &ATTRIBUTES.map(|offset| (offset, 2)));
    let mut settings = options(FragmentFrequency::Fragment);
    assert!(link(&vertex_code, &vertex_header, &fragment_code, &fragment_header, settings)
        .unwrap_err()
        .message
        .contains("vertex output 0x80 is absent"));
    settings.generic_attribute_defaults = Some(GenericAttributeDefaults::ZeroOne);
    let linked =
        link(&vertex_code, &vertex_header, &fragment_code, &fragment_header, settings).unwrap();
    assert_eq!(
        linked.vertex.kernel.outputs,
        [0x70, 0x74, 0x78, 0x7c, 0x8c].map(Register::OutputAttribute)
    );
    validate(&linked);

    // Defaults cannot repair an undeclared fragment input or position output.
    fragment_header.words[6] &= !3;
    assert!(link(&vertex_code, &vertex_header, &fragment_code, &fragment_header, settings)
        .unwrap_err()
        .message
        .contains("pixel input 0x80 is absent"));
    fragment_header.words[6] |= 2;
    omit(&mut vertex_header, &[0x70]);
    assert!(link(&vertex_code, &vertex_header, &fragment_code, &fragment_header, settings)
        .unwrap_err()
        .message
        .contains("vertex output 0x70 is absent"));
}

#[test]
fn direct_and_indexed_defaults_validate_in_both_profiles() {
    for profile in [Profile::Checked, Profile::Rendering] {
        let mut settings = options(FragmentFrequency::Fragment);
        settings.vertex.profile = profile;
        settings.fragment.profile = profile;
        for defaults in [GenericAttributeDefaults::Zero, GenericAttributeDefaults::ZeroOne] {
            for partial in [false, true] {
                for (mode, location, indexed) in cases() {
                    validate(&program(defaults, partial, mode, location, indexed, settings));
                }
            }
        }
    }
}

fn cases() -> [(u32, u8, bool); 6] {
    [(1, 0, false), (2, 0, false), (2, 1, false), (2, 2, false), (3, 0, false), (2, 0, true)]
}

pub(super) fn render_defaults(gpu: &Gpu) -> usize {
    let vertices = vertices();
    let mut pixels = 0;
    for profile in [Profile::Checked, Profile::Rendering] {
        for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
            let mut settings = options(frequency);
            settings.vertex.profile = profile;
            settings.fragment.profile = profile;
            let samples = if frequency == FragmentFrequency::Sample { 4 } else { 1 };
            for defaults in [GenericAttributeDefaults::Zero, GenericAttributeDefaults::ZeroOne] {
                for partial in [false, true] {
                    for (mode, location, indexed) in cases() {
                        let program = program(defaults, partial, mode, location, indexed, settings);
                        let rendered =
                            render(gpu, &program, &vertices, samples, Resources::default());
                        assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
                        let w = if partial {
                            0.625
                        } else if defaults == GenericAttributeDefaults::ZeroOne {
                            1.
                        } else {
                            0.
                        };
                        for &pixel in &rendered.colors[&0] {
                            assert_eq!(&pixel[..3], &[0.; 3]);
                            assert!(
                                (pixel[3] - w * 0.5).abs() < 1. / 255.,
                                "{profile:?} {frequency:?} {defaults:?} partial={partial} \
                                 mode={mode} location={location} indexed={indexed}: {pixel:?}",
                            );
                        }
                        pixels += rendered.colors[&0].len();
                    }
                }
            }
            // A declared output without a write remains unknown even with defaults.
            let mut vertex_header = header(true, &[]);
            omit(&mut vertex_header, &ATTRIBUTES[..3]);
            settings.generic_attribute_defaults = Some(GenericAttributeDefaults::ZeroOne);
            let program = link(
                &vertex(false),
                &vertex_header,
                &fragment(2, 0, false),
                &header(false, &ATTRIBUTES.map(|offset| (offset, 2))),
                settings,
            )
            .unwrap();
            let rendered = render(gpu, &program, &vertices, samples, Resources::default());
            assert_eq!(rendered.faults, [32, crate::ir::tests::offset(2) as u32, 0, u32::MAX]);
        }
    }
    pixels
}
