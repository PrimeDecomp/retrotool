use super::{
    bytes, graphics, header, mov, options,
    render::{render_with_state, RenderState, Resources},
    validate, vertex, vertices, FragmentFrequency, Gpu, GraphicsProgram, MaxwellCode, Options,
    StageSource, EXIT,
};
use crate::{
    maxwell::graphics::ColorOutputRouting,
    program::{wgsl::Profile, Diagnostic},
};

const PRIMARY: [f32; 4] = [0.125, 0.25, 0.375, 0.5];
const SECONDARY: [f32; 4] = [0.75, 0.5, 0.25, 0.125];
const BACKGROUND: [f32; 4] = [0.25, 0.375, 0.5, 0.625];

fn program(options: Options, masks: u32, flags: u32) -> Result<GraphicsProgram, Diagnostic> {
    let vertex_code = bytes(&vertex());
    let mut words: Vec<_> = PRIMARY
        .into_iter()
        .chain(SECONDARY)
        .enumerate()
        .map(|(register, value)| mov(register as u8, value.to_bits(), 7))
        .collect();
    words.extend([mov(8, 0b0101, 7), mov(9, 0.375f32.to_bits(), 7), EXIT]);
    let fragment_code = bytes(&words);
    let mut fragment_header = header(false, &[]);
    fragment_header.mrt_enable = true;
    fragment_header.words[0] |= 1 << 14;
    fragment_header.words[18] = masks;
    fragment_header.words[19] = flags;
    graphics::translate(
        StageSource { code: MaxwellCode::parse(&vertex_code).unwrap(), header: &header(true, &[]) },
        StageSource { code: MaxwellCode::parse(&fragment_code).unwrap(), header: &fragment_header },
        options,
        &[],
        &[],
    )
}

#[test]
fn dual_source_routing_is_explicit_and_preserves_builtin_exports() {
    for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
        let mut options = options(frequency);
        let attachments = program(options, 0xff, 3).unwrap();
        assert_eq!(attachments.color_routing, ColorOutputRouting::Attachments);
        assert!(attachments.fragment.source.contains("@location(1) color1"));
        validate(&attachments);

        options.color_routing = ColorOutputRouting::DualSource;
        let dual = program(options, 0xff, 3).unwrap();
        assert_eq!(dual.color_routing, ColorOutputRouting::DualSource);
        assert_eq!(dual.fragment_outputs, attachments.fragment_outputs);
        assert!(dual.fragment.source.starts_with("enable dual_source_blending;"));
        assert!(!dual.vertex.source.contains("blend_src"));
        validate(&dual);

        // Missing secondary, a target hole and an extra attachment are different
        // native contracts. None may silently become dual-source output.
        for masks in [0, 0xf, 0xf0, 0xf0f, 0xfff] {
            let error = program(options, masks, 0).unwrap_err();
            assert!(error.message.contains("only RGBA exports 0 and 1"));
        }
        assert!(program(options, 0x7f, 0).unwrap_err().message.contains("complete RGBA"));
    }
}

struct BlendCase {
    name: &'static str,
    state: wgpu::BlendState,
    expected: [f32; 4],
}

fn blend_cases() -> Vec<BlendCase> {
    use wgpu::BlendFactor;
    let mut cases = Vec::new();
    for (name, factor, rgba) in [
        ("Src1", BlendFactor::Src1, SECONDARY),
        ("OneMinusSrc1", BlendFactor::OneMinusSrc1, SECONDARY.map(|v| 1. - v)),
        ("Src1Alpha", BlendFactor::Src1Alpha, [SECONDARY[3]; 4]),
        ("OneMinusSrc1Alpha", BlendFactor::OneMinusSrc1Alpha, [1. - SECONDARY[3]; 4]),
    ] {
        // Exercise both sides of each equation; the expectation is ordinary
        // blend arithmetic, independent of the Maxwell lifter/interpreter.
        for source_factor in [true, false] {
            let component = wgpu::BlendComponent {
                src_factor: if source_factor { factor } else { BlendFactor::One },
                dst_factor: if source_factor { BlendFactor::One } else { factor },
                operation: wgpu::BlendOperation::Add,
            };
            cases.push(BlendCase {
                name,
                state: wgpu::BlendState { color: component, alpha: component },
                expected: std::array::from_fn(|i| {
                    if source_factor {
                        PRIMARY[i] * rgba[i] + BACKGROUND[i]
                    } else {
                        PRIMARY[i] + BACKGROUND[i] * rgba[i]
                    }
                }),
            });
        }
    }
    // MPR preset 10 uses secondary RGB but primary alpha. Distinct alpha values
    // make accidental use of secondary alpha observable.
    cases.push(BlendCase {
        name: "MPR preset 10",
        state: wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrc1,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        expected: std::array::from_fn(|i| {
            let opacity = if i == 3 { PRIMARY[3] } else { SECONDARY[i] };
            PRIMARY[i] + BACKGROUND[i] * (1. - opacity)
        }),
    });
    cases
}

#[test]
#[ignore = "requires a native GPU with dual-source blending and vertex storage writes"]
fn linked_dual_source_blending_matches_equations_on_gpu() {
    let gpu = Gpu::with_features(
        wgpu::Features::DUAL_SOURCE_BLENDING | wgpu::Features::VERTEX_WRITABLE_STORAGE,
    );
    let vertices = vertices();
    let mut checks = 0;
    for profile in [Profile::Checked, Profile::Rendering] {
        for frequency in [FragmentFrequency::Fragment, FragmentFrequency::Sample] {
            let mut options = options(frequency);
            options.vertex.profile = profile;
            options.fragment.profile = profile;
            options.color_routing = ColorOutputRouting::DualSource;
            for samples in [1, 4] {
                // At 4x, the native sample mask preserves samples 0/2 and leaves
                // samples 1/3 at the background. Resolve should average them.
                let program = program(options, 0xff, u32::from(samples == 4)).unwrap();
                for case in blend_cases() {
                    let state = RenderState {
                        samples,
                        blend: Some(case.state),
                        clear: wgpu::Color {
                            r: f64::from(BACKGROUND[0]),
                            g: f64::from(BACKGROUND[1]),
                            b: f64::from(BACKGROUND[2]),
                            a: f64::from(BACKGROUND[3]),
                        },
                    };
                    let rendered = render_with_state(
                        &gpu,
                        &program,
                        &vertices,
                        state,
                        Resources::default(),
                        |_| vec![],
                    );
                    assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
                    assert_eq!(rendered.colors.len(), 1);
                    for pixel in &rendered.colors[&0] {
                        for i in 0..4 {
                            let blended = case.expected[i].clamp(0., 1.);
                            let expected = if samples == 4 {
                                (blended + BACKGROUND[i]) * 0.5
                            } else {
                                blended
                            };
                            assert!((pixel[i] - expected).abs() < 2. / 255.,
                                "{} {profile:?} {frequency:?} {samples}x channel {i}: {} != {expected}",
                                case.name, pixel[i]);
                        }
                        checks += 1;
                    }
                }
            }
        }
    }
    eprintln!("{checks} dual-source blended pixels checked");
}
