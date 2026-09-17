use super::{
    options,
    render::{render, Resources},
    validate, FragmentFrequency, Gpu, GraphicsProgram,
};
use crate::{
    ir::tests::{
        bytes,
        probe::{
            self,
            vertex::{Transforms, Vertex, IDENTITY_ROWS},
        },
    },
    maxwell::{
        graphics::{self, StageSource},
        MaxwellCode, ShaderProgramHeader,
    },
    program::wgsl::memory::Binding,
    resources::ConstantMemory,
};

mod code;
mod simple_color;
mod simple_texture;

pub(super) use simple_color::render_simple_color;
pub(super) use simple_texture::render_simple_texture;

fn parse_header(words: &[u32]) -> ShaderProgramHeader {
    let bytes = words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
    ShaderProgramHeader::parse(&bytes).unwrap()
}

fn probe_pair() -> GraphicsProgram {
    let vertex_code = bytes(probe::VERTEX);
    let fragment_code = bytes(probe::NONLINEAR);
    let mut options = options(FragmentFrequency::Sample);
    options.vertex.approximate_special_functions = true;
    options.fragment.global_memory = Some(Binding { group: 0, binding: 4 });
    graphics::translate(
        StageSource {
            code: MaxwellCode::parse(&vertex_code).unwrap(),
            header: &parse_header(&probe::VERTEX_HEADER),
        },
        StageSource {
            code: MaxwellCode::parse(&fragment_code).unwrap(),
            header: &parse_header(&probe::FRAGMENT_HEADER),
        },
        options,
        &[],
        &[],
    )
    .unwrap()
}

#[test]
fn original_probe_pair_links_with_its_program_headers() {
    let program = probe_pair();
    let mut attributes = program.attributes.clone();
    attributes.sort_unstable();
    assert_eq!(attributes, [
        0x80, 0x84, 0x88, 0x90, 0xa0, 0xa4, 0xa8, 0xac, 0xb0, 0xb4, 0xb8, 0xbc, 0xc0, 0xc4, 0xc8,
        0xcc,
    ]);
    assert_eq!(program.varying_locations, 2);
    assert_eq!(program.frequency, FragmentFrequency::Sample);
    assert!(program.vertex.kernel.constant_memory.is_some());
    assert!(program.fragment.kernel.constant_memory.is_some());
    assert!(program.fragment.kernel.global_memory.is_some());
    validate(&program);
}

// Identity instances and a projective clip matrix place these vertices at
// (0,0), (16,0), (0,16) on an 8x8 target, with clip W = 1, 2, 4. This exercises
// perspective transport of the original vertex shader's normalized positions.
const POSITIONS: [[f32; 3]; 3] = [[-1., 1., 1.], [6., 2., 2.], [-4., -12., 4.]];

fn normal_numerator(x: f64, y: f64) -> [f64; 3] {
    let weights = [1. - x / 16. - y / 16., x / 16., y / 16.];
    let mut normal = [0.; 3];
    for (position, weight) in POSITIONS.into_iter().zip(weights) {
        let position = position.map(f64::from);
        let length = position.iter().map(|value| value * value).sum::<f64>().sqrt();
        let clip_w = position[2];
        for component in 0..3 {
            normal[component] += weight * position[component] / (length * clip_w);
        }
    }
    normal
}

pub(super) fn render_probe_pair(gpu: &Gpu) -> usize {
    let program = probe_pair();
    let (_, snapshot, cases) = probe::nonlinear();
    let vertex_constants = Transforms {
        clip_columns: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 0.5, 1.], [0., 0., 0., 0.]],
        normal_rows: IDENTITY_ROWS,
    }
    .constants();
    let mut driver = cases[0].constants.read_bytes::<0x640>(0, 0).unwrap();
    let uniforms = cases[0].constants.read_bytes::<28>(3, 0).unwrap();
    let offsets =
        [(0u8, [0., 0.]), (0x4c, [-0.25, 0.25]), (0x88, [-0.5, -0.5]), (0x77, [0.4375, 0.4375])];
    let mut checks = 0;
    for (packed_offset, delta) in offsets {
        driver[0x620..0x630].fill(packed_offset);
        let fragment_constants = ConstantMemory::new(&[(0, &driver), (3, &uniforms)]).unwrap();
        for (record, probe_index) in [0, 1, 2, 0x10001].into_iter().enumerate() {
            let vertices = POSITIONS.map(|position| {
                Vertex { position, instance_rows: IDENTITY_ROWS, probe_index }.attributes()
            });
            let rendered = render(gpu, &program, &vertices, 1, Resources {
                vertex_constants: Some(&vertex_constants),
                fragment_constants: Some(&fragment_constants),
                fragment_memory: Some(&snapshot),
            });
            assert_eq!(rendered.faults, [0, u32::MAX, 0, u32::MAX]);
            for (pixel, actual) in rendered.colors[&0].iter().enumerate() {
                let x = (pixel % 8) as f64 + 0.5 + delta[0];
                let y = (pixel / 8) as f64 + 0.5 + delta[1];
                let expected = probe::reference_color(normal_numerator(x, y), record);
                for channel in 0..4 {
                    assert!(
                        (f64::from(actual[channel]) - expected[channel]).abs()
                            < expected[channel].abs().max(1.) * 3e-5,
                        "original pair: offset {packed_offset:#x}, record {record}, pixel {pixel}, channel {channel}: {actual:?} != {expected:?}"
                    );
                }
            }
            checks += rendered.colors[&0].len();
        }
    }
    checks
}
