use std::collections::BTreeMap;

use super::{
    bytes, code, graphics, options, parse_header, render, validate, ConstantMemory,
    FragmentFrequency, Gpu, GraphicsProgram, MaxwellCode, Resources, StageSource,
};

fn program() -> GraphicsProgram {
    let vertex = bytes(code::VERTEX);
    let fragment = bytes(code::FRAGMENT);
    graphics::translate(
        StageSource {
            code: MaxwellCode::parse(&vertex).unwrap(),
            header: &parse_header(&code::VERTEX_HEADER),
        },
        StageSource {
            code: MaxwellCode::parse(&fragment).unwrap(),
            header: &parse_header(&code::FRAGMENT_HEADER),
        },
        options(FragmentFrequency::Sample),
        &[],
        &[],
    )
    .unwrap()
}

#[test]
fn original_simple_color_links_both_render_targets() {
    let program = program();
    assert_eq!(program.fragment_outputs.colors[0], [Some(0), Some(1), Some(2), Some(3)]);
    assert_eq!(program.fragment_outputs.colors[1], [Some(4), Some(5), Some(6), Some(7)]);
    assert_eq!(program.fragment.kernel.outputs.len(), 8);
    validate(&program);
}

pub(in crate::ir::tests::wgsl::graphics) fn render_simple_color(gpu: &Gpu) -> usize {
    let program = program();
    // Original VS transforms position by c3[0..64] and passes input color.
    // This projective matrix gives the triangle clip W = 1, 2, 4.
    let clip_columns = [[1f32, 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 0.5, 1.], [0., 0., 0., 0.]];
    let transform =
        clip_columns.into_iter().flatten().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
    let vertex_constants = ConstantMemory::new(&[(3, &transform)]).unwrap();
    let vertices = [
        ([-1f32, 1., 1.], [0f32, 0., 1., 0.25]),
        ([6., 2., 2.], [1., 0., 0., 0.5]),
        ([-4., -12., 4.], [0., 1., 0., 1.]),
    ]
    .map(|(position, color)| {
        let mut attributes = BTreeMap::new();
        for (component, value) in position.into_iter().enumerate() {
            attributes.insert(0x80 + component as u16 * 4, value.to_bits());
        }
        for (component, value) in color.into_iter().enumerate() {
            attributes.insert(0x90 + component as u16 * 4, value.to_bits());
        }
        attributes
    });
    let mut driver = vec![0; 0x640];
    let mut checks = 0;
    for (offset, delta) in
        [(0u8, [0., 0.]), (0x4c, [-0.25, 0.25]), (0x88, [-0.5, -0.5]), (0x77, [0.4375, 0.4375])]
    {
        driver[0x620..0x630].fill(offset);
        let fragment_constants =
            ConstantMemory::new(&[(0, &driver), (1, code::FRAGMENT_CONSTANTS)]).unwrap();
        let rendered = render(gpu, &program, &vertices, 1, Resources {
            vertex_constants: Some(&vertex_constants),
            fragment_constants: Some(&fragment_constants),
            ..Default::default()
        });
        assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
        for pixel in 0..64 {
            let x = (pixel % 8) as f64 + 0.5 + delta[0];
            let y = (pixel / 8) as f64 + 0.5 + delta[1];
            let weights = [1. - x / 16. - y / 16., x / 16., y / 16.];
            let reciprocal_w = weights[0] + weights[1] / 2. + weights[2] / 4.;
            let color = [weights[1] / 2., weights[2] / 4., weights[0], 0.25]
                .map(|value| value / reciprocal_w);
            // Target 0 retains RGBA; target 1 contains Rec.709 luminance in R.
            let luminance = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
            for (target, expected) in [(0, color), (1, [luminance, 0., 0., 0.])] {
                let actual = rendered.colors[&target][pixel];
                for component in 0..4 {
                    assert!(
                        (f64::from(actual[component]) - expected[component]).abs() < 3e-5,
                        "SimpleColor offset {offset:#x}, target {target}, pixel {pixel}: {actual:?} != {expected:?}"
                    );
                }
                checks += 1;
            }
        }
    }
    checks
}
