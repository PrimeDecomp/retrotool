//! Independently expected coverage and mip selection after Maxwell KIL.

use super::{
    constants, link_profile, mip_texture, prefix, render_with_bind_groups, set_varyings,
    single_pixel, validate, vertices, ConstantMemory, FragmentFrequency, Gpu, Resources, EXIT,
};
use crate::{
    ir::tests::{add, branch, isetp, mov, offset},
    program::wgsl::Profile,
};

fn words(loop_tail: bool) -> Vec<u64> {
    let mut words = prefix();
    // Kill rows 0..3 before the indirect constant read and implicit sample.
    // In the central quad, the top two lanes become helpers for the bottom two.
    words.splice(2..2, [
        mov(13, 96, 7),
        isetp(0, 7, 12, 13, 1, 7, 7),
        0xe33000000000000f, // @P0 KIL
    ]);
    if loop_tail {
        let start = words.len();
        words.extend([
            mov(13, 0, 7),
            mov(14, 2, 7),
            add(13, 13, 1),
            isetp(0, 7, 13, 14, 2, 7, 7),
            branch(start + 4, start + 2, 8),
        ]);
    }
    words.push(EXIT);
    words
}

#[test]
fn demotion_before_implicit_sampling_validates_in_both_profiles() {
    for profile in [Profile::Checked, Profile::Rendering] {
        for loop_tail in [false, true] {
            let program =
                link_profile(&words(loop_tail), FragmentFrequency::Fragment, 0., 65_536, profile);
            validate(&program);
            // Helper execution and diagnostic atomics precede host discard.
            assert!(!program.fragment.kernel.source.contains("discard;"));
            let source = &program.fragment.source;
            assert!(
                source.find("atomicOr(&remax_draw_faults[2]").unwrap()
                    < source.find("discard;").unwrap()
            );
        }
    }
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn demotion_preserves_derivatives_and_reports_helper_faults_on_native_gpu() {
    let gpu =
        Gpu::with_features(wgpu::Features::VERTEX_WRITABLE_STORAGE | wgpu::Features::SUBGROUP);
    let view = mip_texture(&gpu);
    let sampler = gpu.device.create_sampler(&Default::default());
    let mut draws = 0;
    for profile in [Profile::Checked, Profile::Rendering] {
        // A killed invocation never exports its unwritten color registers.
        let mut options = super::super::options(FragmentFrequency::Fragment);
        options.fragment.profile = profile;
        let killed = super::super::translate(&[0xe33000000007000f, EXIT], &[], options);
        let result =
            super::super::render::render(&gpu, &killed, &vertices(), 1, Resources::default());
        assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
        assert!(result.colors[&0].iter().all(|color| *color == [0.; 4]));
        draws += 1;
        for loop_tail in [false, true] {
            for (frequency, samples) in
                [(FragmentFrequency::Fragment, 1), (FragmentFrequency::Sample, 4)]
            {
                let program = link_profile(&words(loop_tail), frequency, 0., 65_536, profile);
                let draw = |vertices: &[_], memory: &ConstantMemory| {
                    render_with_bind_groups(
                        &gpu,
                        &program,
                        vertices,
                        samples,
                        Resources { fragment_constants: Some(memory), ..Default::default() },
                        |pipeline| {
                            vec![(
                                1,
                                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    label: None,
                                    layout: &pipeline.get_bind_group_layout(1),
                                    entries: &[
                                        wgpu::BindGroupEntry {
                                            binding: 0,
                                            resource: wgpu::BindingResource::TextureView(&view),
                                        },
                                        wgpu::BindGroupEntry {
                                            binding: 1,
                                            resource: wgpu::BindingResource::Sampler(&sampler),
                                        },
                                    ],
                                }),
                            )]
                        },
                    )
                };
                let mut full = vertices();
                set_varyings(&mut full);
                let result = draw(&full, &constants(None));
                assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
                for (index, color) in result.colors[&0].iter().enumerate() {
                    assert_eq!(*color, if index / 8 < 3 { [0.; 4] } else { [0., 1., 0., 1.] });
                }
                draws += 1;
                for covered in 0..4 {
                    let pixel = [2 + covered % 2, 2 + covered / 2];
                    let mut small = single_pixel(pixel);
                    set_varyings(&mut small);
                    for faulty in [None, Some(0), Some(1), Some(2), Some(3)] {
                        let memory = constants(faulty.map(|lane| [2 + lane % 2, 2 + lane / 2]));
                        let result = draw(&small, &memory);
                        if faulty.is_some() {
                            assert_eq!(result.faults, [0, u32::MAX, 8, offset(5) as u32],
                                "{profile:?}, loop {loop_tail}, {frequency:?}, covered {covered}, faulty {faulty:?}");
                            assert!(result.colors[&0].iter().all(|color| *color == [0.; 4]));
                        } else {
                            assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
                            let color = result.colors[&0][(pixel[0] + pixel[1] * 8) as usize];
                            if covered < 2 {
                                assert_eq!(color, [0.; 4]);
                            } else {
                                assert!(color[3] > 0.);
                                assert_eq!(color, [0., color[3], 0., color[3]]);
                            }
                        }
                        draws += 1;
                    }
                }
            }
        }
    }
    println!("Checked {draws} demotion draws: independent coverage/mip expectations, post-KIL helper faults, both profiles/control shapes/frequencies");
}
