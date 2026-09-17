use super::*;
use crate::program::{wgsl::Input, Inputs, Value};

mod array_views;

struct Image {
    dimension: D,
    size: [u32; 3],
    format: wgpu::TextureFormat,
    texel_bytes: u32,
    levels: Vec<Vec<u8>>,
    samples: u32,
}

impl Image {
    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
        let depth = self.format == wgpu::TextureFormat::Depth32Float;
        let clear = depth || self.samples > 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("remax test texture"),
            size: wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: self.size[2],
            },
            mip_level_count: self.levels.len().max(1) as u32,
            sample_count: self.samples,
            dimension: match self.dimension {
                D::D1 => wgpu::TextureDimension::D1,
                D::D3 => wgpu::TextureDimension::D3,
                _ => wgpu::TextureDimension::D2,
            },
            format: self.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | if clear {
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                } else {
                    wgpu::TextureUsages::COPY_DST
                },
            view_formats: &[],
        });
        if clear {
            let view = texture.create_view(&Default::default());
            let mut commands = device.create_command_encoder(&Default::default());
            if depth {
                let _pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.5),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
            } else {
                let _pass = commands.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 1.0,
                                g: 0.0,
                                b: 1.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
            }
            queue.submit(Some(commands.finish()));
        } else {
            for (level, bytes) in self.levels.iter().enumerate() {
                let width = (self.size[0] >> level).max(1);
                let height = (self.size[1] >> level).max(1);
                let layers = if self.dimension == D::D3 {
                    (self.size[2] >> level).max(1)
                } else {
                    self.size[2]
                };
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * self.texel_bytes),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d { width, height, depth_or_array_layers: layers },
                );
            }
        }
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(match self.dimension {
                D::D1 => wgpu::TextureViewDimension::D1,
                D::D2 => wgpu::TextureViewDimension::D2,
                D::D2Array => wgpu::TextureViewDimension::D2Array,
                D::D3 => wgpu::TextureViewDimension::D3,
                D::Cube => wgpu::TextureViewDimension::Cube,
            }),
            ..Default::default()
        })
    }
}

fn groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    k: &Kernel,
    layout: impl Fn(u32) -> wgpu::BindGroupLayout,
    image: &Image,
    compute: bool,
) -> Vec<(u32, wgpu::BindGroup)> {
    let view = image.upload(device, queue);
    let r = &k.textures[0];
    let filter = |f| match f {
        TextureFilter::Nearest => wgpu::FilterMode::Nearest,
        TextureFilter::Linear => wgpu::FilterMode::Linear,
    };
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        compare: r.comparison_sampler.then_some(wgpu::CompareFunction::LessEqual),
        min_filter: r
            .binding
            .view
            .sampling
            .map_or(wgpu::FilterMode::Nearest, |s| filter(s.min_filter)),
        mag_filter: r
            .binding
            .view
            .sampling
            .map_or(wgpu::FilterMode::Nearest, |s| filter(s.mag_filter)),
        ..Default::default()
    });
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: wgpu::BindingResource::TextureView(&view),
    }];
    if r.uses_sampler {
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout(2),
        entries: &entries,
    });
    let mut groups = vec![];
    for i in if compute { 1..2 } else { 0..2 } {
        groups.push((
            i,
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout(i),
                entries: &[],
            }),
        ));
    }
    groups.push((2, group));
    groups
}

fn input(registers: &[(u8, u32)]) -> Inputs {
    let mut inputs = Inputs {
        registers: [0, 1, 4, 5, 6, 7, 10, 11]
            .into_iter()
            .map(|r| (Register::Gpr(r), Value::Word(0)))
            .chain([(Register::Predicate(0), Value::Predicate(true))])
            .collect(),
        ..Default::default()
    };
    for &(r, v) in registers {
        inputs.registers.insert(Register::Gpr(r), Value::Word(v));
    }
    inputs
}
fn float_bits(color: [u16; 4]) -> [u32; 4] {
    color.map(|h| match h {
        0 => 0,
        0x3400 => 0.25f32.to_bits(),
        0x3800 => 0.5f32.to_bits(),
        0x3a00 => 0.75f32.to_bits(),
        0x3c00 => 1.0f32.to_bits(),
        _ => panic!("fixture half"),
    })
}
fn floats(size: [u32; 3], dimension: D, levels: usize) -> Image {
    let colors = [
        [0x3c00u16, 0, 0, 0x3c00],
        [0, 0x3c00, 0, 0x3c00],
        [0, 0, 0x3c00, 0x3c00],
        [0x3c00, 0x3c00, 0, 0x3c00],
        [0x3c00, 0, 0x3c00, 0x3c00],
    ];
    Image {
        dimension,
        size,
        format: wgpu::TextureFormat::Rgba16Float,
        texel_bytes: 8,
        samples: 1,
        levels: (0..levels)
            .map(|level| {
                let texels = (size[0] >> level).max(1)
                    * (size[1] >> level).max(1)
                    * if dimension == D::D3 { (size[2] >> level).max(1) } else { size[2] };
                (0..texels)
                    .flat_map(|_| colors[level].into_iter().flat_map(u16::to_le_bytes))
                    .collect()
            })
            .collect(),
    }
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn texture_kernels_sample_and_load_real_resources() {
    let gpu = super::super::gpu::Gpu::new();
    let mut checked = contiguous_samples(&gpu);
    // Integer loads exercise raw channel bits, 1D/2D/array/volume layouts and
    // packed offsets independently of texture filtering or sRGB conversion.
    for (mode, dim, size, case, expected) in [
        (0, D::D1, [4, 1, 1], vec![(6, 2)], 2u32),
        (2, D::D2, [4, 4, 1], vec![(6, 2), (10, 1)], 6),
        (12, D::D2, [4, 4, 1], vec![(6, 2), (7, 1), (10, 0), (11, 0xffffff0f)], 5),
        (8, D::D2Array, [4, 4, 2], vec![(6, 0xffff0001), (10, 2), (11, 1)], 22),
        (7, D::D3, [4, 4, 2], vec![(6, 2), (7, 1), (10, 1)], 22),
    ] {
        for signed in [false, true] {
            let p = program(&[word(true, mode), EXIT], Stage::Vertex);
            let mut b = binding(dim, if signed { V::Sint } else { V::Uint });
            b.sampler_binding = None;
            let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
            let image = Image {
                dimension: dim,
                size,
                format: if signed {
                    wgpu::TextureFormat::Rgba32Sint
                } else {
                    wgpu::TextureFormat::Rgba32Uint
                },
                texel_bytes: 16,
                samples: 1,
                levels: vec![(0..size.iter().product::<u32>())
                    .flat_map(|i| {
                        [i, 0x80000000 | i, 0xffff0000 | i, u32::MAX - i]
                            .into_iter()
                            .flat_map(u32::to_le_bytes)
                    })
                    .collect()],
            };
            let mut bad = case.clone();
            bad.push((6, u32::MAX));
            if dim == D::D2Array {
                bad.push((6, 7));
            }
            let cases = [input(&case), input(&bad)];
            let results = gpu.run_with_resources(&k, &cases, |device, queue, pipeline| {
                groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &image, true)
            });
            assert_eq!(
                results[0],
                [
                    expected,
                    0x80000000 | expected,
                    0xffff0000 | expected,
                    u32::MAX - expected,
                    0,
                    0,
                    0
                ],
                "mode={mode} signed={signed}"
            );
            assert_eq!(&results[1][4..], [128, 8, 0]);
            checked += 2;
        }
    }
    // Explicit LOD must select the authored mip rather than the base level.
    let p = program(&[word(false, 3), EXIT], Stage::Vertex);
    let k = p.emit_wgsl_with_textures(Options::default(), &[binding(D::D2, V::Float)]).unwrap();
    let image = floats([16, 16, 1], D::D2, 5);
    let cases = (0..3)
        .map(|m| input(&[(6, 0.5f32.to_bits()), (7, 0.5f32.to_bits()), (10, (m as f32).to_bits())]))
        .collect::<Vec<_>>();
    let results = gpu.run_with_resources(&k, &cases, |device, queue, pipeline| {
        groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &image, true)
    });
    for (i, r) in results.iter().enumerate() {
        let mut expected = [0, 0, 0, 1.0f32.to_bits(), 0, 0, 0];
        expected[i] = 1.0f32.to_bits();
        assert_eq!(r, &expected);
    }
    checked += 3;

    // A one-mip view with identical min/mag filtering can execute under a
    // predicate without derivatives. Preserve actual bilinear filtering and
    // check the declared mip count before relying on that specialization.
    let p = program(&[word(false, 1) & !(7 << 16), EXIT], Stage::Fragment);
    let mut b = binding(D::D2, V::Float);
    b.view.sampling = Some(TextureSampling {
        mip_level_count: 1,
        min_filter: TextureFilter::Linear,
        mag_filter: TextureFilter::Linear,
        max_anisotropy: 1,
    });
    let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
    let mut filtered = floats([2, 2, 1], D::D2, 1);
    filtered.levels[0] =
        [[0x3c00u16, 0, 0, 0x3c00], [0, 0x3c00, 0, 0x3c00], [0, 0, 0x3c00, 0x3c00], [
            0x3c00, 0x3c00, 0x3c00, 0x3c00,
        ]]
        .into_iter()
        .flatten()
        .flat_map(u16::to_le_bytes)
        .collect();
    let yes = input(&[(6, 0.5f32.to_bits()), (10, 0.5f32.to_bits())]);
    let mut no = yes.clone();
    no.registers.insert(Register::Predicate(0), Value::Predicate(false));
    no.registers.remove(&Register::Gpr(6));
    let cases = [yes, no];
    for mismatch in [false, true] {
        if mismatch {
            filtered.levels.push(vec![0; 8]);
        }
        let results = gpu.run_with_resources(&k, &cases, |device, queue, pipeline| {
            groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &filtered, true)
        });
        if mismatch {
            assert_eq!(&results[0][4..], [128, 8, 0]);
        } else {
            assert_eq!(&results[0][..4], float_bits([0x3800, 0x3800, 0x3800, 0x3c00]));
            assert_eq!(&results[0][4..], [0, 0, 0]);
        }
        assert_eq!(results[1], [0; 7]);
        checked += 2;
    }

    // Scalar depth reads use the declared mapping; comparison results expand
    // to (d,d,d,1) before the machine register mask.
    let depth = Image {
        dimension: D::D2,
        size: [4, 4, 1],
        format: wgpu::TextureFormat::Depth32Float,
        texel_bytes: 4,
        levels: vec![],
        samples: 1,
    };
    for mode in [2, 3, 6] {
        use crate::ir::tests::{add, mov};
        let words = if mode == 3 {
            vec![mov(10, u32::MAX, 7), add(10, 10, 1), word(false, mode), EXIT]
        } else {
            vec![word(mode == 2, mode), EXIT]
        };
        let p = program(&words, Stage::Fragment);
        let mut b = binding(D::D2, V::Depth);
        if mode != 6 {
            b.depth_components = Some([
                DepthComponent::One,
                DepthComponent::Depth,
                DepthComponent::Zero,
                DepthComponent::Depth,
            ]);
        }
        let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
        let cases = if mode == 2 {
            [input(&[(6, 1), (10, 1)]), input(&[(6, 2), (10, 2)])]
        } else {
            [
                input(&[(6, 0.25f32.to_bits()), (7, 0.25f32.to_bits()), (10, 0.25f32.to_bits())]),
                input(&[(6, 0.75f32.to_bits()), (7, 0.75f32.to_bits()), (10, 0.75f32.to_bits())]),
            ]
        };
        let results = gpu.run_with_resources(&k, &cases, |device, queue, pipeline| {
            groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &depth, true)
        });
        for (i, r) in results.iter().enumerate() {
            let expected = if mode != 6 {
                float_bits([0x3c00, 0x3800, 0, 0x3800])
            } else {
                let d = if i == 0 { 1.0f32.to_bits() } else { 0 };
                [d, d, d, 1.0f32.to_bits()]
            };
            assert_eq!(&r[..4], expected);
            assert_eq!(r[4], 0);
        }
        checked += 2;
    }
    let p = program(&[word(true, 6), EXIT], Stage::Vertex);
    let mut b = binding(D::D2, V::Float);
    b.view.multisample = true;
    b.sampler_binding = None;
    let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
    let ms = Image {
        dimension: D::D2,
        size: [4, 4, 1],
        format: wgpu::TextureFormat::Rgba8Unorm,
        texel_bytes: 4,
        levels: vec![],
        samples: 4,
    };
    let cases = (0..6).map(|s| input(&[(6, 1), (7, 1), (10, s)])).collect::<Vec<_>>();
    let results = gpu.run_with_resources(&k, &cases, |device, queue, pipeline| {
        groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &ms, true)
    });
    for (i, r) in results.iter().enumerate() {
        if i < 4 {
            assert_eq!(&r[..4], float_bits([0x3c00, 0, 0x3c00, 0x3c00]));
            assert_eq!(r[4], 0);
        } else {
            assert_eq!(r[4], 128);
        }
    }
    checked += 6;

    // Full-screen rasterization supplies real quad derivatives. A 16x16 view
    // over 8x8 pixels selects mip 1; adding bias 1 selects mip 2.
    for bias in [0.0, 1.0] {
        let p = program(&[word(false, 1), EXIT], Stage::Fragment);
        let mut b = binding(D::D2, V::Float);
        b.view.lod_bias = bias;
        let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
        assert_eq!(k.fault_scope, FaultScope::Quad);
        let words = k
            .inputs
            .iter()
            .map(|i| match i {
                Input::Register(Register::Gpr(6)) => "bitcast<u32>(position.x / 8.0)".into(),
                Input::Register(Register::Gpr(10)) => "bitcast<u32>(position.y / 8.0)".into(),
                _ => "0u".into(),
            })
            .collect::<Vec<_>>();
        let valid = vec!["1u".to_string(); words.len()];
        let results =
            gpu.render_with_resources(&k, &words, &valid, [8, 8], |device, queue, pipeline| {
                groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &image, false)
            });
        let mut expected = [0, 0, 0, 1.0f32.to_bits(), 0, 0, 0];
        expected[if bias == 0.0 { 1 } else { 2 }] = 1.0f32.to_bits();
        for r in results {
            assert_eq!(r, expected);
        }
        checked += 64;
        for fault in if bias == 0.0 { 0..3 } else { 0..0 } {
            let mut bad = valid.clone();
            let mut bad_words = words.clone();
            let slot =
                k.inputs.iter().position(|i| *i == Input::Register(Register::Gpr(6))).unwrap();
            if fault == 0 {
                bad[slot] = "select(1u,0u,position.x < 1.0 && position.y < 1.0)".into();
            } else {
                bad_words[slot] = format!(
                    "select({},{}u,position.x < 1.0 && position.y < 1.0)",
                    words[slot],
                    if fault == 1 { 0x7fc00001u32 } else { 0x7f800000 }
                );
            }
            let results = gpu.render_with_resources(
                &k,
                &bad_words,
                &bad,
                [8, 8],
                |device, queue, pipeline| {
                    groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &image, false)
                },
            );
            assert_eq!(&results[0][4..], [if fault == 0 { 32 } else { 256 }, 8, 0]);
            // The caller must reject this whole fully covered quad, including
            // its locally successful lanes. No valid-color claim is made there.
            for (i, r) in results.iter().enumerate() {
                if ![0, 1, 8, 9].contains(&i) {
                    assert_eq!(r, &expected);
                }
            }
            checked += 64;
        }
    }
    // Original source66 TEXS uses a 2D-array layer and aliases its source and
    // result register groups. The host fixture has two deliberately distinct layers.
    let p = Program::lift(
        MaxwellCode::parse(&bytes(&[0xd8f000a06117060a, EXIT])).unwrap(),
        &Signature {
            local_memory_bytes: 0,
            indexed_attributes: vec![],
            stage: Stage::Fragment,
            inputs: [6, 7, 17].map(Register::Gpr).to_vec(),
            outputs: [10, 11, 6, 7].map(Register::Gpr).to_vec(),
        },
    )
    .unwrap();
    let mut b = binding(D::D2Array, V::Float);
    b.view.texture_index = 10;
    let k = p.emit_wgsl_with_textures(Options::default(), &[b]).unwrap();
    let mut array = floats([4, 4, 2], D::D2Array, 1);
    array.levels[0][4 * 4 * 8..].copy_from_slice(
        &(0..16)
            .flat_map(|_| [0u16, 0x3c00, 0, 0x3c00].into_iter().flat_map(u16::to_le_bytes))
            .collect::<Vec<_>>(),
    );
    let words = k
        .inputs
        .iter()
        .map(|i| match i {
            Input::Register(Register::Gpr(6)) => "4294901761u".into(),
            Input::Register(Register::Gpr(7)) => "bitcast<u32>(position.x/8.0)".into(),
            Input::Register(Register::Gpr(17)) => "bitcast<u32>(position.y/8.0)".into(),
            _ => panic!(),
        })
        .collect::<Vec<_>>();
    let results = gpu.render_with_resources(
        &k,
        &words,
        &vec!["1u".into(); words.len()],
        [8, 8],
        |device, queue, pipeline| {
            groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), &array, false)
        },
    );
    for r in results {
        assert_eq!(r, [0, 1.0f32.to_bits(), 0, 1.0f32.to_bits(), 0, 0, 0]);
    }
    checked += 64;
    checked += global_loads_before_implicit_sampling(&gpu, &image);
    println!("Checked {checked} native texture results, including implicit-LOD rasterization and fault handling");
}

fn contiguous_samples(gpu: &super::super::gpu::Gpu) -> usize {
    use crate::program::wgsl::Profile;

    let image = floats([16, 16, 1], D::D2, 5);
    let mut checked = 0;
    for mask in 1..16u64 {
        // TEX.LL, R0 result group, R6/R7 coordinates and R10 LOD. Each mip
        // has one distinct RGB channel and alpha one; expected values come
        // from that image construction, independently of the interpreter.
        let word = 0xc038000000070000
            | (3 << 55)
            | (0x123 << 36)
            | (mask << 31)
            | (2 << 28)
            | (10 << 20)
            | (6 << 8);
        let program =
            Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
                local_memory_bytes: 0,
                indexed_attributes: vec![],
                stage: Stage::Vertex,
                inputs: [0, 1, 2, 3, 6, 7, 10].map(Register::Gpr).to_vec(),
                outputs: [0, 1, 2, 3].map(Register::Gpr).to_vec(),
            })
            .unwrap();
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = program
                .emit_wgsl_with_textures(Options { profile, ..Default::default() }, &[binding(
                    D::D2,
                    V::Float,
                )])
                .unwrap();
            validate(&kernel).unwrap();
            let cases = (0..3)
                .map(|mip| {
                    let mut case = input(&[
                        (6, 0.5f32.to_bits()),
                        (7, 0.5f32.to_bits()),
                        (10, (mip as f32).to_bits()),
                    ]);
                    for register in 0..4 {
                        case.registers.insert(Register::Gpr(register), Value::Word(23));
                    }
                    case
                })
                .collect::<Vec<_>>();
            let results = gpu.run_with_resources(&kernel, &cases, |device, queue, pipeline| {
                groups(device, queue, &kernel, |g| pipeline.get_bind_group_layout(g), &image, true)
            });
            for (mip, result) in results.iter().enumerate() {
                let mut expected = [23, 23, 23, 23, 0, 0, 0];
                let channels = (0..4).filter(|channel| mask & (1 << channel) != 0);
                for (target, channel) in channels.enumerate() {
                    expected[target] =
                        if channel == mip || channel == 3 { 1f32.to_bits() } else { 0 };
                }
                assert_eq!(result, &expected, "TEX {profile:?} mask {mask:x}, mip {mip}");
            }
            checked += results.len();
        }
    }
    checked
}

fn global_loads_before_implicit_sampling(gpu: &super::super::gpu::Gpu, image: &Image) -> usize {
    use wgpu::util::DeviceExt;

    use crate::{
        ir::tests::global_memory::ldg,
        program::wgsl::memory,
        resources::memory::{Region, Snapshot},
    };

    // Each fragment loads its U coordinate through a 64-bit global address;
    // V comes from the raster position. The load result feeds real derivatives.
    let p = program(&[ldg(6, 4, 0, true, 4), word(false, 1), EXIT], Stage::Fragment);
    let k = p
        .emit_wgsl_with_textures(
            Options {
                global_memory: Some(memory::Binding { group: 3, binding: 0 }),
                ..Default::default()
            },
            &[binding(D::D2, V::Float)],
        )
        .unwrap();
    validate(&k).unwrap();
    assert_eq!(k.fault_scope, FaultScope::Quad);
    let data =
        (0..64).flat_map(|i| ((i % 8) as f32 / 8.0 + 0.0625).to_le_bytes()).collect::<Vec<_>>();
    for fault in 0..4 {
        let mut known = vec![true; data.len()];
        if fault == 2 {
            known[..4].fill(false);
        }
        let snapshot =
            Snapshot::new(&[Region { address: 0x1000, bytes: &data, known: Some(&known) }])
                .unwrap();
        let words = k
            .inputs
            .iter()
            .map(|i| match i {
                Input::Register(Register::Gpr(4)) => {
                    let address = "4096u + 4u*(u32(position.x)+8u*u32(position.y))";
                    if fault == 1 {
                        format!("select({address},4092u,position.x<1.0 && position.y<1.0)")
                    } else {
                        address.into()
                    }
                }
                Input::Register(Register::Gpr(10)) => "bitcast<u32>(position.y/8.0)".into(),
                _ => "0u".into(),
            })
            .collect::<Vec<_>>();
        let mut valid = vec!["1u".to_string(); words.len()];
        if fault == 3 {
            let slot =
                k.inputs.iter().position(|i| *i == Input::Register(Register::Gpr(4))).unwrap();
            valid[slot] = "select(1u,0u,position.x<1.0 && position.y<1.0)".into();
        }
        let results =
            gpu.render_with_resources(&k, &words, &valid, [8, 8], |device, queue, pipeline| {
                let mut result =
                    groups(device, queue, &k, |g| pipeline.get_bind_group_layout(g), image, false);
                let bytes =
                    snapshot.words().iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });
                result.push((
                    3,
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(3),
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    }),
                ));
                result
            });
        if fault != 0 {
            assert_eq!(&results[0][4..], [[512, 8, 0], [1024, 8, 0], [32, 8, 0]][fault - 1]);
        }
        for (i, result) in results.iter().enumerate() {
            // A local success does not establish validity of the faulted quad.
            // Only unaffected quads may claim a correct sampled color.
            if fault == 0 || ![0, 1, 8, 9].contains(&i) {
                assert_eq!(result, &[0, 1.0f32.to_bits(), 0, 1.0f32.to_bits(), 0, 0, 0]);
            }
        }
    }
    256
}
