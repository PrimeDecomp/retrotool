use super::*;
use crate::{ir::tests::wgsl::gpu::Gpu, program::wgsl::Profile};

/// Both coordinate shapes access one descriptor. The first sample reads R7/R10;
/// the second additionally reads the low sixteen bits of R6 as the array layer.
fn mixed_program() -> Program {
    let ordinary = (word(false, 2) & !(255 << 8)) | (7 << 8);
    let layered = (word(false, 8) & !255 & !(255 << 28)) | 12 | (14 << 28);
    Program::lift(MaxwellCode::parse(&bytes(&[ordinary, layered, EXIT])).unwrap(), &Signature {
        stage: Stage::Fragment,
        inputs: [6, 7, 10].map(Register::Gpr).to_vec(),
        outputs: [0, 1, 4, 5, 12, 13, 14, 15].map(Register::Gpr).to_vec(),
        local_memory_bytes: 0,
        indexed_attributes: vec![],
    })
    .unwrap()
}

fn image(layers: u32) -> Image {
    // Exact half-float encodings of integers 1..4, 17..20 and 33..36.
    let texels: [u16; 12] = [
        0x3c00, 0x4000, 0x4200, 0x4400, 0x4c40, 0x4c80, 0x4cc0, 0x4d00, 0x5020, 0x5040, 0x5060,
        0x5080,
    ];
    Image {
        dimension: if layers == 0 { D::D2 } else { D::D2Array },
        size: [2, 2, layers.max(1)],
        format: wgpu::TextureFormat::R16Float,
        texel_bytes: 2,
        levels: vec![texels[..layers.max(1) as usize * 4]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect()],
        samples: 1,
    }
}

#[derive(Debug)]
struct Capture {
    layers: u32,
    linear: bool,
    wrap: u32,
    x: f32,
    y: f32,
    layer: u32,
    ordinary: f32,
    layered: f32,
}

fn capture() -> Vec<Capture> {
    let path = std::path::PathBuf::from(
        std::env::var_os("MPR_TEXTURE_VIEWS").expect("texture-view hardware capture directory"),
    );
    let text = std::fs::read_to_string(path.join("sm75-values.tsv")).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("resource\tfilter\twrap\tx\ty\tlayer\tordinary\tlayered"));
    let rows = lines
        .map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            assert_eq!(fields.len(), 8);
            let linear = match fields[1] {
                "0" => false,
                "1" => true,
                _ => panic!("capture filter"),
            };
            Capture {
                layers: fields[0].parse().unwrap(),
                linear,
                wrap: fields[2].parse().unwrap(),
                x: fields[3].parse().unwrap(),
                y: fields[4].parse().unwrap(),
                layer: fields[5].parse::<i32>().unwrap() as u32,
                ordinary: fields[6].parse().unwrap(),
                layered: fields[7].parse().unwrap(),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1890);
    rows
}

#[test]
#[ignore = "requires MPR_TEXTURE_VIEWS hardware captures and a native GPU"]
fn mixed_2d_array_sampling_matches_sm75_capture() {
    let captures = capture();
    let gpu = Gpu::new();
    let program = mixed_program();
    let mut comparisons = 0;
    for layers in [0, 1, 3] {
        let image = image(layers);
        for linear in [false, true] {
            // SM75 TEX consumes an unsigned32 layer, TEXS unsigned16. At
            // 65536 those differ, so do not claim a direct comparison there.
            // This replay uses clamp-to-edge; Python also checks wrap/border.
            let rows = captures
                .iter()
                .filter(|row| {
                    row.layers == layers
                        && row.linear == linear
                        && row.wrap == 1
                        && row.layer != 65536
                })
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 90);
            let cases = rows
                .iter()
                .map(|row| input(&[(6, row.layer), (7, row.x.to_bits()), (10, row.y.to_bits())]))
                .collect::<Vec<_>>();
            let mut binding = binding(image.dimension, V::Float);
            let filter = if linear { TextureFilter::Linear } else { TextureFilter::Nearest };
            binding.view.sampling = Some(TextureSampling {
                mip_level_count: 1,
                min_filter: filter,
                mag_filter: filter,
                max_anisotropy: 1,
            });
            for profile in [Profile::Checked, Profile::Rendering] {
                let kernel = program
                    .emit_wgsl_with_textures(Options { profile, ..Default::default() }, &[binding])
                    .unwrap();
                validate(&kernel).unwrap();
                assert_eq!(kernel.textures.len(), 1);
                let results = gpu.run_with_resources(&kernel, &cases, |device, queue, pipeline| {
                    groups(
                        device,
                        queue,
                        &kernel,
                        |g| pipeline.get_bind_group_layout(g),
                        &image,
                        true,
                    )
                });
                for (row, actual) in rows.iter().zip(results) {
                    let expected = [
                        row.ordinary.to_bits(),
                        0,
                        0,
                        1.0f32.to_bits(),
                        row.layered.to_bits(),
                        0,
                        0,
                        1.0f32.to_bits(),
                        0,
                        0,
                        0,
                    ];
                    assert_eq!(actual, expected, "{profile:?}: {row:?}");
                    comparisons += 2;
                }
            }
        }
    }
    assert_eq!(comparisons, 2160);
    println!("{comparisons} mixed 2D/array Metal samples match retained SM75 values");
}

#[test]
#[ignore = "requires a native GPU"]
fn implicit_array_samples_use_the_view_layer_range() {
    let gpu = Gpu::new();
    let program = program(&[word(false, 7), EXIT], Stage::Fragment);
    let mut comparisons = 0;
    for layers in [0, 1, 3] {
        let image = image(layers);
        let binding = binding(image.dimension, V::Float);
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = program
                .emit_wgsl_with_textures(Options { profile, ..Default::default() }, &[binding])
                .unwrap();
            assert_eq!(kernel.fault_scope, FaultScope::Quad);
            // Every horizontal pair selects a different unsigned16 layer.
            // The final pair wraps 65536 to zero before view-range clamping.
            let words = kernel
                .inputs
                .iter()
                .map(|value| match value {
                    Input::Register(Register::Gpr(6)) => {
                        "array<u32, 4>(0u, 1u, 65535u, 65536u)[u32(position.x) / 2u]".into()
                    }
                    Input::Register(Register::Gpr(7 | 10)) => "bitcast<u32>(0.25)".into(),
                    _ => "0u".into(),
                })
                .collect::<Vec<_>>();
            let results = gpu.render_with_resources(
                &kernel,
                &words,
                &vec!["1u".into(); words.len()],
                [8, 2],
                |device, queue, pipeline| {
                    groups(
                        device,
                        queue,
                        &kernel,
                        |g| pipeline.get_bind_group_layout(g),
                        &image,
                        false,
                    )
                },
            );
            for (index, actual) in results.iter().enumerate() {
                let requested = [0, 1, 65535, 0][index % 8 / 2];
                let layer = requested.min(layers.max(1) - 1);
                let expected = (16 * layer + 1) as f32;
                assert_eq!(actual, &[expected.to_bits(), 0, 0, 1.0f32.to_bits(), 0, 0, 0]);
                comparisons += 1;
            }
        }
    }
    println!("{comparisons} implicit array samples match independent layer expectations");
}
