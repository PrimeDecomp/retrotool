//! A predicated resource read must neither speculate faults nor lose the other
//! lanes before a subsequent unconditional implicit sample.

use super::{
    link_profile, mip_texture, prefix, render_with_bind_groups, set_varyings, single_pixel,
    validate, vertices, ConstantMemory, FragmentFrequency, Gpu, Resources, EXIT,
};
use crate::{
    ir::tests::{add, branch, isetp, mov, offset},
    program::wgsl::{ControlFlow, Profile},
    resources::ConstantBank,
};

fn words(loop_tail: bool) -> Vec<u64> {
    let mut words = prefix();
    // The top two lanes of the test quad take the read; the bottom two skip it.
    // Both banks contain the same coordinates. Sampling must still see a full
    // quad and choose mip one, whose independent expected color is green.
    words.splice(4..4, [
        mov(13, 96, 7),
        isetp(0, 7, 12, 13, 1, 7, 7), // unsigned R12 < R13
        0xef94004000000c06,           // @P0 LDC R6,c4[R12]
    ]);
    if loop_tail {
        let start = words.len();
        words.extend([
            mov(13, 0, 7),
            mov(14, 2, 7),
            add(13, 13, 1),
            isetp(0, 7, 13, 14, 2, 7, 7),
            branch(start + 4, start + 2, 8), // @!P0 backedge
        ]);
    }
    words.push(EXIT);
    words
}

fn memory(unknown: Option<[u32; 2]>) -> ConstantMemory {
    let data = (0..64)
        .flat_map(|index| ((index % 8) as f32 / 8. + 0.0625).to_le_bytes())
        .collect::<Vec<_>>();
    let mut known = vec![true; data.len()];
    if let Some([x, y]) = unknown {
        let offset = (4 * (x + y * 8)) as usize;
        known[offset..offset + 4].fill(false);
    }
    ConstantMemory::from_banks(&[
        ConstantBank { bank: 3, bytes: &data, known: None },
        ConstantBank { bank: 4, bytes: &data, known: Some(&known) },
    ])
    .unwrap()
}

#[test]
fn implicit_sample_after_predication_validates_in_both_control_shapes() {
    for profile in [Profile::Checked, Profile::Rendering] {
        for loop_tail in [false, true] {
            let program =
                link_profile(&words(loop_tail), FragmentFrequency::Fragment, 0., 65_536, profile);
            assert_eq!(
                program.fragment.kernel.control_flow,
                if loop_tail && profile == Profile::Checked {
                    ControlFlow::ScalarDispatcher
                } else {
                    ControlFlow::Regions
                }
            );
            validate(&program);
            let source = &program.fragment.kernel.source;
            if loop_tail && profile == Profile::Checked {
                assert!(
                    source.find("textureSample").unwrap() < source.find("switch block_id").unwrap()
                );
            }
        }
    }
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn predicated_reads_preserve_implicit_sampling_and_helper_faults_on_native_gpu() {
    let gpu =
        Gpu::with_features(wgpu::Features::VERTEX_WRITABLE_STORAGE | wgpu::Features::SUBGROUP);
    let view = mip_texture(&gpu);
    let sampler = gpu.device.create_sampler(&Default::default());
    let mut draws = 0;
    for profile in [Profile::Checked, Profile::Rendering] {
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
                let result = draw(&full, &memory(None));
                assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
                assert!(result.colors[&0].iter().all(|pixel| *pixel == [0., 1., 0., 1.]));
                draws += 1;
                for covered in 0..4 {
                    let pixel = [2 + covered % 2, 2 + covered / 2];
                    let mut small = single_pixel(pixel);
                    set_varyings(&mut small);
                    for faulty in 0..4 {
                        let unknown = [2 + faulty % 2, 2 + faulty / 2];
                        let result = draw(&small, &memory(Some(unknown)));
                        if faulty < 2 {
                            assert_eq!(result.faults, [0, u32::MAX, 8, offset(6) as u32],
                                "{profile:?}, loop {loop_tail}, {frequency:?}, covered {covered}, faulty {faulty}");
                            assert!(result.colors[&0].iter().all(|pixel| *pixel == [0.; 4]));
                        } else {
                            // Poisoned bytes on the skipped path must not fault.
                            assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
                            let color = result.colors[&0][(pixel[0] + pixel[1] * 8) as usize];
                            assert!(color[3] > 0.);
                            assert_eq!(color, [0., color[3], 0., color[3]]);
                        }
                        draws += 1;
                    }
                }
            }
        }
    }
    println!("Checked {draws} predication/rejoin draws with independent mip expectations and conditional helper faults");
}
