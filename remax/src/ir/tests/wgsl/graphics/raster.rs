use super::{
    ipa, mov, options, perspective_fragment,
    render::{render, Resources},
    translate, validate, vertices, ConstantMemory, FragmentFrequency, Gpu, EXIT,
};

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn graphics_entry_points_execute_on_native_gpu() {
    let gpu = Gpu::with_features(wgpu::Features::VERTEX_WRITABLE_STORAGE);
    let vertices = vertices();
    let mut checks = 0;
    for (byte, delta) in
        [(0, [0f32, 0f32]), (0x4c, [-0.25, 0.25]), (0x88, [-0.5, -0.5]), (0x77, [0.4375, 0.4375])]
    {
        let program = translate(
            &perspective_fragment(byte),
            &[(0x80, 2), (0x84, 3), (0x88, 1)],
            options(FragmentFrequency::Fragment),
        );
        let rendered = render(&gpu, &program, &vertices, 1, Resources::default());
        assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
        for (i, pixel) in rendered.colors[&0].iter().enumerate() {
            let x = (i % 8) as f32 + 0.5;
            let y = (i / 8) as f32 + 0.5;
            let [offset_x, offset_y] = [x + delta[0], y + delta[1]];
            let numerator = 0.25 + offset_x / 64. + offset_y / 32.;
            let reciprocal_w = 1. - offset_x / 32. - 3. * offset_y / 64.;
            let expected = [numerator / reciprocal_w, 0.125 + x / 32. - y / 64., 0.375, 1.];
            for j in 0..4 {
                assert!(
                    (pixel[j] - expected[j]).abs() < 2e-5,
                    "{byte:x} {i} {pixel:?} {expected:?}"
                );
            }
        }
        checks += 64;
    }
    // At sample frequency, default reciprocal-W is still centered, rather than
    // the per-sample builtin value. Verify after a real four-sample resolve.
    let program = translate(
        &[
            ipa(0, 0x7c, 0, 0),
            ipa(1, 0x7c, 0, 0),
            ipa(2, 0x7c, 0, 0),
            mov(3, 1f32.to_bits(), 7),
            EXIT,
        ],
        &[],
        options(FragmentFrequency::Sample),
    );
    validate(&program);
    let rendered = render(&gpu, &program, &vertices, 4, Resources::default());
    assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
    for (i, pixel) in rendered.colors[&0].iter().enumerate() {
        let reciprocal_w = 1. - ((i % 8) as f32 + 0.5) / 32. - 3. * ((i / 8) as f32 + 0.5) / 64.;
        assert!((pixel[0] - reciprocal_w).abs() < 1.5 / 255., "{i} {pixel:?} {reciprocal_w}");
        assert_eq!(pixel[3], 1.);
    }
    checks += 64;
    // Faults in different stages invalidate the draw and retain original PCs.
    let program = translate(
        &perspective_fragment(0),
        &[(0x80, 2), (0x84, 3), (0x88, 1)],
        options(FragmentFrequency::Fragment),
    );
    let mut invalid = vertices.clone();
    invalid[1].remove(&0x80);
    let rendered = render(&gpu, &program, &invalid, 1, Resources::default());
    assert_ne!(rendered.faults[0], 0);
    assert_eq!(rendered.faults[1], 8);
    let program = translate(
        &[ipa(0, 0x80, 1, 0), mov(1, 0, 7), mov(2, 0, 7), mov(3, 1f32.to_bits(), 7), EXIT],
        &[(0x80, 2)],
        options(FragmentFrequency::Fragment),
    );
    // Unknown multiplier taints the result; it becomes observable at EXIT.
    let rendered = render(&gpu, &program, &vertices, 1, Resources::default());
    assert_eq!(rendered.faults, [0, u32::MAX, 32, crate::ir::tests::offset(4) as u32]);
    checks += nonlinear_probe(&gpu);
    checks += super::original::render_probe_pair(&gpu);
    checks += super::original::render_simple_color(&gpu);
    checks += super::original::render_simple_texture(&gpu);
    checks += super::outputs::render_exports(&gpu);
    checks += super::defaults::render_defaults(&gpu);
    println!("{checks} linked graphics pixels plus vertex/fragment fault cases");
}

fn nonlinear_probe(gpu: &Gpu) -> usize {
    use crate::{ir::tests::probe, program::wgsl::memory::Binding};
    let (_, snapshot, cases) = probe::nonlinear();
    let mut options = options(FragmentFrequency::Sample);
    options.fragment.global_memory = Some(Binding { group: 0, binding: 4 });
    let program =
        translate(probe::NONLINEAR, &[(0x80, 2), (0x84, 2), (0x88, 2), (0x90, 1)], options);
    validate(&program);
    let original = &cases[0].constants;
    let mut driver = original.read_bytes::<0x640>(0, 0).unwrap();
    let uniforms = original.read_bytes::<28>(3, 0).unwrap();
    let mut checks = 0;
    for (byte, delta) in
        [(0u8, [0f64, 0f64]), (0x4c, [-0.25, 0.25]), (0x88, [-0.5, -0.5]), (0x77, [0.4375, 0.4375])]
    {
        driver[0x620..0x630].fill(byte);
        let constants = ConstantMemory::new(&[(0, &driver), (3, &uniforms)]).unwrap();
        for (record, index) in [0u32, 1, 2, 0x10001].into_iter().enumerate() {
            let mut vertices = vertices();
            for (vertex, (x, y, clip_w)) in
                vertices.iter_mut().zip([(0f32, 0f32, 1f32), (16., 0., 2.), (0., 16., 4.)])
            {
                for (attribute, numerator) in [
                    (0x90, 1. + 0.5 * x - 0.25 * y),
                    (0x94, 2. - 0.25 * x + 0.5 * y),
                    (0x98, 3. + x + 0.25 * y),
                ] {
                    vertex.insert(attribute, (numerator * clip_w).to_bits());
                }
                vertex.insert(0xa0, index);
            }
            let rendered = render(gpu, &program, &vertices, 1, Resources {
                fragment_constants: Some(&constants),
                fragment_memory: Some(&snapshot),
                ..Default::default()
            });
            assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
            for (pixel, result) in rendered.colors[&0].iter().enumerate() {
                let x = (pixel % 8) as f64 + 0.5 + delta[0];
                let y = (pixel / 8) as f64 + 0.5 + delta[1];
                let normal = [1. + 0.5 * x - 0.25 * y, 2. - 0.25 * x + 0.5 * y, 3. + x + 0.25 * y];
                let expected = probe::reference_color(normal, record);
                for channel in 0..3 {
                    let expected = expected[channel];
                    assert!(
                        (f64::from(result[channel]) - expected).abs()
                            < expected.abs().max(1.) * 3e-5,
                        "probe {byte:x} {record} {pixel} {result:?} {channel} {expected}"
                    );
                }
                assert_eq!(result[3], 1.);
            }
            checks += rendered.colors[&0].len();
        }
    }
    checks
}
