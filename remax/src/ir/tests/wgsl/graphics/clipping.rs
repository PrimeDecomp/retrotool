use std::collections::BTreeMap;

use super::{
    access, bytes, graphics, header, mov, options,
    render::{render, Resources},
    validate, FragmentFrequency, Gpu, GraphicsProgram, MaxwellCode, StageSource, EXIT,
};
use crate::program::{wgsl::Profile, Diagnostic, Register};

fn program(
    declared: u8,
    enabled: Option<u8>,
    written: bool,
    profile: Profile,
) -> Result<GraphicsProgram, Diagnostic> {
    let mut vertex = vec![access(true, 0, 0x80, 4), access(false, 0, 0x70, 4)];
    if written {
        vertex.extend([
            access(true, 4, 0x90, 4),
            access(false, 4, 0x2c0, 4),
            access(true, 8, 0xa0, 4),
            access(false, 8, 0x2d0, 4),
        ]);
    }
    vertex.push(EXIT);
    let vertex = bytes(&vertex);
    let fragment = bytes(&[
        mov(0, 1f32.to_bits(), 7),
        mov(1, 0, 7),
        mov(2, 0, 7),
        mov(3, 1f32.to_bits(), 7),
        EXIT,
    ]);
    let mut vertex_header = header(true, &[]);
    vertex_header.words[18] = u32::from(declared);
    let mut options = options(FragmentFrequency::Fragment);
    options.vertex.profile = profile;
    options.fragment.profile = profile;
    options.clip_distance_mask = enabled;
    graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex).unwrap(), header: &vertex_header },
        StageSource { code: MaxwellCode::parse(&fragment).unwrap(), header: &header(false, &[]) },
        options,
        &[],
        &[],
    )
}

#[test]
fn clip_distances_are_explicit_raster_outputs_independent_of_fragment_inputs() {
    assert!(program(0x3f, None, true, Profile::Checked)
        .unwrap_err()
        .message
        .contains("explicit enable mask"));
    // Hardware enables do not create exports absent from the shader map.
    for (declared, enabled, effective) in [(0, 255, 0), (1, 2, 0), (0x3f, 255, 0x3f)] {
        let program = program(declared, Some(enabled), true, Profile::Checked).unwrap();
        assert_eq!(program.clip_distance_mask, effective);
        validate(&program);
    }
    for enabled in 0..=255 {
        let program = program(255, Some(enabled), true, Profile::Rendering).unwrap();
        assert_eq!(program.clip_distance_mask, enabled);
        assert_eq!(program.varying_locations, 0);
        let outputs: Vec<_> = program
            .vertex
            .kernel
            .outputs
            .iter()
            .filter_map(|output| match output {
                Register::OutputAttribute(offset) if *offset >= 0x2c0 => Some(*offset),
                _ => None,
            })
            .collect();
        let expected: Vec<_> =
            (0..8).filter(|i| enabled & (1 << i) != 0).map(|i| 0x2c0 + 4 * i).collect();
        assert_eq!(outputs, expected);
        assert!(!program.fragment.source.contains("clip_distances"));
        validate(&program);
    }
}

#[test]
fn legacy_graphics_rejects_clip_exports_instead_of_dropping_them() {
    use crate::wgsl::{graphics as legacy, LoweringOptions};

    let code = bytes(&[EXIT]);
    let mut vertex = header(true, &[]);
    vertex.words[18] = 0x3f;
    let error = legacy::lower_graphics(
        legacy::GraphicsStage { code: MaxwellCode::parse(&code).unwrap(), header: &vertex },
        legacy::GraphicsStage {
            code: MaxwellCode::parse(&code).unwrap(),
            header: &header(false, &[]),
        },
        legacy::GraphicsOptions {
            lowering: LoweringOptions::default(),
            fragment_frequency: legacy::FragmentFrequency::PerFragment,
        },
        legacy::GraphicsBindings::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("clip-distance exports require the semantic"));
}

fn vertices(perspective: bool) -> Vec<BTreeMap<u16, u32>> {
    [(-1.0, -1.0, 1.0), (3.0, -1.0, 2.0), (-1.0, 3.0, 4.0)]
        .into_iter()
        .map(|(x, y, w)| {
            let w = if perspective { w } else { 1.0 };
            let position = [x * w, y * w, 0.5 * w, w];
            // Eight independent half-spaces in homogeneous clip coordinates.
            // Plane6 excludes everything; disabled holes must not observe it.
            let distances = [x * w, -x * w, y * w, -y * w, (x + 0.5) * w, (0.5 - x) * w, -w, w];
            position
                .into_iter()
                .chain(distances)
                .enumerate()
                .map(|(i, value): (usize, f32)| (0x80 + i as u16 * 4, value.to_bits()))
                .collect()
        })
        .collect()
}

#[test]
#[ignore = "requires a native GPU with clip distances and vertex storage writes"]
fn linked_clip_distances_match_independent_half_spaces_on_gpu() {
    let gpu = Gpu::with_features(
        wgpu::Features::CLIP_DISTANCES | wgpu::Features::VERTEX_WRITABLE_STORAGE,
    );
    let mut checks = 0;
    for profile in [Profile::Checked, Profile::Rendering] {
        for perspective in [false, true] {
            let vertices = vertices(perspective);
            for samples in [1, 4] {
                for mask in [0, 1, 2, 4, 8, 16, 32, 64, 128, 0x11, 0x30, 0x81, 0xff] {
                    let program = program(255, Some(mask), true, profile).unwrap();
                    let result = render(&gpu, &program, &vertices, samples, Resources::default());
                    assert_eq!(result.faults, [0, u32::MAX, 0, u32::MAX]);
                    for (index, pixel) in result.colors[&0].iter().enumerate() {
                        let x = (index % 8) as f32 * 0.25 - 0.875;
                        let y = 0.875 - (index / 8) as f32 * 0.25;
                        let distances = [x, -x, y, -y, x + 0.5, 0.5 - x, -1.0, 1.0];
                        let inside = distances
                            .into_iter()
                            .enumerate()
                            .all(|(i, d)| mask & (1 << i) == 0 || d >= 0.0);
                        let expected = if inside { [1.0, 0.0, 0.0, 1.0] } else { [0.0; 4] };
                        assert_eq!(*pixel, expected, "{profile:?} perspective{perspective} {samples}x mask{mask:02x} pixel{index}");
                        checks += 1;
                    }
                }
            }
        }
        // A declared, unwritten enabled output must still fault. Disabling it
        // removes that observation rather than inventing a distance.
        let disabled = program(1, Some(0), false, profile).unwrap();
        assert_eq!(render(&gpu, &disabled, &vertices(false), 1, Resources::default()).faults, [
            0,
            u32::MAX,
            0,
            u32::MAX
        ]);
        let enabled = program(1, Some(1), false, profile).unwrap();
        let result = render(&gpu, &enabled, &vertices(false), 1, Resources::default());
        assert_ne!(result.faults[0], 0);
        assert!(result.colors[&0].iter().all(|pixel| *pixel == [0.0; 4]));
    }
    eprintln!("{checks} pixels checked against independent clip half-spaces");
}
