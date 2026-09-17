use super::{
    bytes, graphics, header, mov, options,
    render::{render, Resources},
    validate, vertex, vertices, FragmentFrequency, Gpu, GraphicsProgram, MaxwellCode, StageSource,
    EXIT,
};
use crate::program::Register;

fn translate(
    words: &[u64],
    color_masks: u32,
    flags: u32,
    frequency: FragmentFrequency,
) -> GraphicsProgram {
    let vertex_code = bytes(&vertex());
    let fragment_code = bytes(words);
    let mut fragment_header = header(false, &[]);
    fragment_header.mrt_enable = true;
    fragment_header.words[0] |= 1 << 14;
    fragment_header.words[18] = color_masks;
    fragment_header.words[19] = flags;
    graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex_code).unwrap(), header: &header(true, &[]) },
        StageSource { code: MaxwellCode::parse(&fragment_code).unwrap(), header: &fragment_header },
        options(frequency),
        &[],
        &[],
    )
    .unwrap()
}

fn color_words() -> Vec<u64> {
    [0.25f32, 0.5, 0.75, 1., 1., 0.75, 0.5, 0.25]
        .into_iter()
        .enumerate()
        .map(|(register, value)| mov(register as u8, value.to_bits(), 7))
        .collect()
}

#[test]
fn graphics_outputs_preserve_target_holes_and_unobserved_mask_registers() {
    let mut words = color_words();
    words.push(mov(9, 0.375f32.to_bits(), 7));
    words.push(EXIT);
    // Targets 1 and 6 consume R0..R7. R8 is uninitialized but not exported;
    // depth comes from R9. The target holes consume no registers or locations.
    let program = translate(&words, (15 << 4) | (15 << 24), 2, FragmentFrequency::Fragment);
    let expected: Vec<_> = (0..8).chain([9]).map(Register::Gpr).collect();
    assert_eq!(program.fragment.kernel.outputs, expected);
    assert!(program.fragment.source.contains("@location(1) color1"));
    assert!(program.fragment.source.contains("@location(6) color6"));
    assert!(!program.fragment.source.contains("@location(0) color0"));
    validate(&program);

    for (words, flags, expected) in [
        (vec![mov(1, 0.25f32.to_bits(), 7), EXIT], 2, vec![Register::Gpr(1)]),
        (vec![mov(0, 5, 7), EXIT], 1, vec![Register::Gpr(0)]),
        (vec![EXIT], 0, vec![]),
    ] {
        for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
            let program = translate(&words, 0, flags, frequency);
            assert_eq!(program.fragment.kernel.outputs, expected);
            validate(&program);
        }
    }
}

pub(super) fn render_exports(gpu: &Gpu) -> usize {
    let vertices = vertices();
    let mut words = color_words();
    words.push(mov(9, 0.375f32.to_bits(), 7));
    words.push(EXIT);
    let program = translate(&words, (15 << 4) | (15 << 24), 2, FragmentFrequency::Fragment);
    let rendered = render(gpu, &program, &vertices, 1, Resources::default());
    assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
    assert_eq!(rendered.colors.len(), 2);
    for (target, expected) in [(1, [0.25, 0.5, 0.75, 1.]), (6, [1., 0.75, 0.5, 0.25])] {
        assert!(rendered.colors[&target].iter().all(|pixel| *pixel == expected));
    }
    assert!(rendered.depth.unwrap().iter().all(|&depth| depth == 0.375));
    let mut checks = 3 * 64;

    // A pure depth shader exports R1; R0 is not required. With no exports at
    // all, the same pass writes the triangle's fixed-function depth of 0.5.
    for (words, flags, expected_depth) in [
        (vec![mov(1, 0.25f32.to_bits(), 7), EXIT], 2, 0.25),
        (vec![EXIT], 0, 0.5),
        (vec![mov(0, 0, 7), EXIT], 1, 1.),
    ] {
        let program = translate(&words, 0, flags, FragmentFrequency::Fragment);
        let rendered = render(gpu, &program, &vertices, 1, Resources::default());
        assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
        assert!(rendered.colors.is_empty());
        assert!(rendered.depth.unwrap().iter().all(|&depth| depth == expected_depth));
        checks += 64;
    }

    // A native mask can suppress the current sample. Verify both invocation
    // frequencies after a real four-sample resolve on two color attachments.
    let mut words = color_words();
    words.push(mov(8, 0b0101, 7));
    words.push(EXIT);
    for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
        let program = translate(&words, 0xff, 1, frequency);
        let rendered = render(gpu, &program, &vertices, 4, Resources::default());
        assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
        for (target, expected) in [(0, [0.125, 0.25, 0.375, 0.5]), (1, [0.5, 0.375, 0.25, 0.125])] {
            for pixel in &rendered.colors[&target] {
                for component in 0..4 {
                    assert!((pixel[component] - expected[component]).abs() < 1.5 / 255.);
                }
            }
            checks += 64;
        }
    }
    checks
}
