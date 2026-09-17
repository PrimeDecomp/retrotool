//! Sampling participation must survive faults before a divergent tail, and
//! helpers must still report faults after taking different loop counts/exits.

use super::{
    link_profile, prefix, render_with_bind_groups, set_varyings, single_pixel, validate, vertices,
    ConstantMemory, FragmentFrequency, Gpu, Resources, EXIT,
};
use crate::{
    ir::tests::{
        add, branch,
        control::{setup, sync},
        isetp, mov, offset,
    },
    program::wgsl::{ControlFlow, Profile},
    resources::ConstantBank,
};

fn words() -> Vec<u64> {
    let mut words = prefix();
    assert_eq!(words.len(), 7);
    words.extend([
        mov(13, 0, 7),
        setup(8, 14),
        isetp(0, 7, 13, 12, 6, 7, 7), // unsigned counter >= per-pixel address
        sync(0),
        add(13, 13, 1),
        branch(12, 9, 7),
        0,                  // unreachable padding
        0xef94004000070c0f, // LDC R15,c4[R12], independent tail fault
        0x5cb8000000d70a00, // I2F.F32.U32 R0,R13, expose the loop count
        mov(14, (1.0f32 / 256.).to_bits(), 7),
        0x5c68000000e70000, // FMUL R0,R0,R14, fit the MSAA UNORM target
        EXIT,
    ]);
    words
}

fn constants(before: Option<[u32; 2]>, after: Option<[u32; 2]>) -> ConstantMemory {
    let coordinates = (0..64)
        .flat_map(|index| ((index % 8) as f32 / 8. + 0.0625).to_le_bytes())
        .collect::<Vec<_>>();
    let validity = |fault: Option<[u32; 2]>| {
        let mut known = vec![true; 256];
        if let Some([x, y]) = fault {
            let offset = (4 * (x + 8 * y)) as usize;
            known[offset..offset + 4].fill(false);
        }
        known
    };
    ConstantMemory::from_banks(&[
        ConstantBank { bank: 3, bytes: &coordinates, known: Some(&validity(before)) },
        ConstantBank { bank: 4, bytes: &[0; 256], known: Some(&validity(after)) },
    ])
    .unwrap()
}

pub(super) fn compare(gpu: &Gpu, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> usize {
    [Profile::Checked, Profile::Rendering]
        .into_iter()
        .map(|profile| compare_profile(gpu, view, sampler, profile))
        .sum()
}

fn compare_profile(
    gpu: &Gpu,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    profile: Profile,
) -> usize {
    let mut draws = 0;
    for (frequency, samples) in [(FragmentFrequency::Fragment, 1), (FragmentFrequency::Sample, 4)] {
        for budget in [65_536, 64] {
            let program = link_profile(&words(), frequency, 0., budget, profile);
            assert_eq!(program.fragment.kernel.control_flow, match profile {
                Profile::Checked => ControlFlow::ScalarDispatcher,
                Profile::Rendering => ControlFlow::Regions,
            });
            validate(&program);
            let draw = |vertices: &[_], memory: &ConstantMemory| {
                render_with_bind_groups(
                    gpu,
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
                                        resource: wgpu::BindingResource::TextureView(view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(sampler),
                                    },
                                ],
                            }),
                        )]
                    },
                )
            };
            if budget == 64 {
                let mut small = single_pixel([2, 2]);
                set_varyings(&mut small);
                let rendered = draw(&small, &constants(None, None));
                assert_eq!(&rendered.faults[..3], [0, u32::MAX, 16]);
                assert!(
                    (9..=12).any(|index| rendered.faults[3] as usize == offset(index)),
                    "fuel must expire in the tail: {:?}",
                    rendered.faults
                );
                assert!(rendered.colors[&0].iter().all(|pixel| *pixel == [0.; 4]));
                draws += 1;
                // A helper faults in the charged prefix while its neighbors
                // exhaust their budgets in the tail. Preserve the earlier PC
                // and report both kinds across the quad, including the helper.
                let rendered = draw(&small, &constants(Some([3, 3]), None));
                assert_eq!(rendered.faults, [0, u32::MAX, 8 | 16, offset(2) as u32]);
                assert!(rendered.colors[&0].iter().all(|pixel| *pixel == [0.; 4]));
                draws += 1;
                continue;
            }
            let mut full = vertices();
            set_varyings(&mut full);
            let rendered = draw(&full, &constants(None, None));
            assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
            for (index, pixel) in rendered.colors[&0].iter().enumerate() {
                check_color(*pixel, index, samples);
            }
            draws += 1;
            for covered in 0..4 {
                let pixel = [2 + covered % 2, 2 + covered / 2];
                let mut small = single_pixel(pixel);
                set_varyings(&mut small);
                let rendered = draw(&small, &constants(None, None));
                assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
                let index = (pixel[0] + 8 * pixel[1]) as usize;
                let color = rendered.colors[&0][index];
                assert!(color[3] > 0.);
                check_color(color, index, samples);
                draws += 1;
                for faulty in 0..4 {
                    let fault = Some([2 + faulty % 2, 2 + faulty / 2]);
                    for (before, after, pc) in [
                        (fault, None, offset(2)),
                        (None, fault, offset(14)),
                        (fault, fault, offset(2)),
                    ] {
                        let rendered = draw(&small, &constants(before, after));
                        assert_eq!(rendered.faults, [0, u32::MAX, 8, pc as u32],
                            "{frequency:?}, covered {covered}, faulty {faulty}, before {before:?}, after {after:?}");
                        assert!(rendered.colors[&0].iter().all(|pixel| *pixel == [0.; 4]));
                        draws += 1;
                    }
                }
            }
        }
    }
    draws
}

fn check_color(actual: [f32; 4], pixel_index: usize, samples: u32) {
    let alpha = actual[3];
    let expected = [pixel_index as f32 / 64. * alpha, alpha, 0., alpha];
    // The multisample harness resolves RGBA8. Account for attachment and resolve
    // quantization, including the alpha used to measure partial coverage.
    let tolerance = if samples == 1 { 0. } else { 1. / 255. };
    for component in 0..4 {
        assert!(
            (actual[component] - expected[component]).abs() <= tolerance,
            "pixel {pixel_index}, {samples} samples: {actual:?} != {expected:?}"
        );
    }
}
