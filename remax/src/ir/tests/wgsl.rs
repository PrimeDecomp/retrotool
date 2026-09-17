use super::{add, branch, bytes, isetp, lift, mov, swapping_loop, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        wgsl::{ControlFlow, Input, Kernel, Options},
        DiagnosticKind, Execution, F2iNanBehavior, FloatModel, Inputs, InterpretOptions, Phase,
        Program, Register, Signature, SpecialFunctionModel, Stage, Value,
    },
    structured::{Node, Shape, Strategy},
};

mod analysis;
mod bindings;
pub(super) mod gpu;
mod graphics;
mod indexed_attributes;
mod interpolation;
mod local;
mod loops;
mod memory;
mod multiply;
mod reduction;
mod texture;
mod trigonometry;

fn validate(kernel: &Kernel) {
    let input_count = kernel.inputs.len().max(1);
    let source = format!(
        r#"{}
@compute @workgroup_size(1)
fn main() {{
    var words: array<u32, {input_count}>;
    var valid: array<u32, {input_count}>;
    let result = remax_main(words, valid);
}}
"#,
        kernel.source,
    );
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}\n{source}"));
    let module = naga_next::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    naga_next::valid::Validator::new(
        naga_next::valid::ValidationFlags::all(),
        naga_next::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{e:?}\n{source}"));
}

fn options() -> Options {
    Options {
        approximate_arithmetic: true,
        approximate_special_functions: true,
        ..Default::default()
    }
}

#[test]
fn reduced_functions_validate_opaque_joins_and_loop_transfers() {
    for (p, _) in super::reduction::comparison_cases()
        .into_iter()
        .chain(super::trigonometry::comparison_cases())
    {
        assert_eq!(p.emit_wgsl(Options::default()).unwrap_err().kind, DiagnosticKind::Unsupported);
        let kernel = p.emit_wgsl(options()).unwrap();
        assert!(kernel.approximate_special_functions);
        assert!(!kernel.approximate_arithmetic);
        validate(&kernel);
    }
    validate(&super::trigonometry::original::program().emit_wgsl(options()).unwrap());
}

#[test]
fn offset_interpolation_emits_explicit_plane_inputs_and_records_its_model() {
    use crate::program::interpolation::OffsetModel;
    for (p, _) in super::interpolation::comparison_cases() {
        assert_eq!(p.emit_wgsl(options()).unwrap_err().kind, DiagnosticKind::Unsupported);
        assert_eq!(
            p.emit_wgsl(Options {
                offset_interpolation: Some(OffsetModel::SignedSixteenths),
                ..Default::default()
            })
            .unwrap_err()
            .kind,
            DiagnosticKind::Unsupported
        );
        let kernel = p
            .emit_wgsl(Options {
                offset_interpolation: Some(OffsetModel::SignedSixteenths),
                ..options()
            })
            .unwrap();
        assert_eq!(kernel.offset_interpolation, Some(OffsetModel::SignedSixteenths));
        assert_eq!(
            kernel.inputs.iter().filter(|i| matches!(i, Input::InterpolationPlane { .. })).count(),
            3
        );
        assert!(!kernel.requires_uniform_call); // Planes are supplied before kernel execution.
        assert!(!kernel.source.contains("dpdx("));
        validate(&kernel);
    }
}

#[test]
fn carry_programs_emit_valid_structured_kernels() {
    for (p, _) in
        super::integer::comparison_cases().into_iter().chain(super::bits::comparison_cases())
    {
        validate(&p.emit_wgsl(Options::default()).unwrap());
    }
    validate(&super::control::sync_loop().emit_wgsl(Options::default()).unwrap());
}

#[test]
fn minmax_validates_with_an_explicit_numeric_model() {
    for (program, _) in super::minmax::comparison_cases() {
        assert_eq!(
            program.emit_wgsl(Options::default()).unwrap_err().kind,
            DiagnosticKind::Unsupported
        );
        let kernel = program.emit_wgsl(options()).unwrap();
        assert!(kernel.approximate_arithmetic);
        validate(&kernel);
    }
}

#[test]
fn integral_rounding_uses_an_explicit_model_and_validates_all_controls() {
    for (p, _) in super::rounding::comparison_cases() {
        assert_eq!(p.emit_wgsl(Options::default()).unwrap_err().kind, DiagnosticKind::Unsupported);
        validate(&p.emit_wgsl(options()).unwrap());
    }
}

// Complete startup SNVN source 567 vertex program, without scheduling/padding.
// Converts attribute 0x2fc from signed integer to float and writes x and 2*x
// to two three-component outputs. The full original instruction sequence is
// short enough to retain; no extracted resource or generated shader is needed.
const ORIGINAL_VERTEX: [u64; 10] = [
    0xefd87f802fc7ff00,
    0x5cb8000000072a00,
    0x3868104000070001,
    0xeff07f800807ff00,
    0xeff07f800847ff00,
    0xeff07f800887ff00,
    0xeff07f800907ff01,
    0xeff07f800947ff01,
    0xeff07f800987ff01,
    EXIT,
];

fn original_vertex() -> Program {
    Program::lift(MaxwellCode::parse(&bytes(&ORIGINAL_VERTEX)).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Vertex,
        outputs: [0x80, 0x84, 0x88, 0x90, 0x94, 0x98].map(Register::OutputAttribute).to_vec(),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn original_vertex_program_keeps_its_input_and_output_contract() {
    let p = original_vertex();
    let kernel = p.emit_wgsl(options()).unwrap();
    assert_eq!(kernel.inputs, [Input::VertexAttribute(0x2fc)]);
    validate(&kernel);
    for input in [0u32, 1, u32::MAX, 0x80000000, 0x7fffffff, 0x12345678] {
        let values = p
            .interpret_with_options(
                &Inputs { attributes: [(0x2fc, input)].into(), ..Default::default() },
                InterpretOptions {
                    step_budget: 100,
                    float_model: FloatModel::SoftwareIeee,
                    ..Default::default()
                },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        let x = input as i32 as f32;
        assert_eq!(values, [x, x, x, 2.0 * x, 2.0 * x, 2.0 * x].map(|f| Value::Word(f.to_bits())));
    }
}

#[test]
fn regions_keep_common_joins_and_conditional_returns() {
    use Register::{Gpr as R, Predicate as P};
    for words in [
        vec![mov(0, 1, 7), mov(0, 2, 0), add(0, 0, 3), EXIT],
        vec![mov(0, 1, 7), EXIT & !(7 << 16), mov(0, 2, 7), EXIT],
        vec![branch(0, 3, 0), mov(0, 1, 7), branch(2, 4, 7), mov(0, 2, 7), EXIT],
    ] {
        let p = lift(&words, &[P(0)], &[R(0)]);
        let kernel = p.emit_wgsl(Options::default()).unwrap();
        assert_eq!(kernel.control_flow, ControlFlow::Regions);
        assert!(!kernel.source.contains("switch block_id"));
        validate(&kernel);
    }
}

#[test]
fn structuring_verifier_rejects_changed_control_paths() {
    use Register::{Gpr as R, Predicate as P};
    let p = lift(&[mov(0, 1, 7), mov(0, 2, 0), EXIT], &[P(0)], &[R(0)]);
    let mut shape = crate::structured::build(
        &p.function,
        &crate::ir::analysis::EntryPrefix::new(&p.function),
        Strategy::Acyclic,
    )
    .unwrap();
    if let Shape::Regions(r) = &mut shape {
        r.0.swap(0, 1);
    } else {
        panic!()
    }
    assert_eq!(shape.verify(&p.function).unwrap_err().phase, Phase::Structure);
    let mut shape = crate::structured::build(
        &p.function,
        &crate::ir::analysis::EntryPrefix::new(&p.function),
        Strategy::Acyclic,
    )
    .unwrap();
    if let Shape::Regions(r) = &mut shape {
        r.0.iter_mut()
            .find_map(|node| match node {
                Node::Block { arms, .. } => arms.as_mut(),
                _ => None,
            })
            .unwrap()
            .swap(0, 1);
    } else {
        panic!()
    }
    assert!(shape.verify(&p.function).is_err());
}

#[test]
fn scalar_loops_use_bounded_dispatch_and_simultaneous_transfers() {
    let p = Program { function: swapping_loop() };
    let kernel = p.emit_wgsl(Options { step_budget: 128, ..Default::default() }).unwrap();
    assert_eq!(kernel.control_flow, ControlFlow::ScalarDispatcher);
    assert_eq!(kernel.step_budget, 128);
    validate(&kernel);
}

#[test]
fn forward_graphs_with_overlapping_branches_remain_valid() {
    use Register::{Gpr as R, Predicate as P};
    let mut state = 0x73129bcdu32;
    for _ in 0..64 {
        let mut words = vec![mov(0, 19, 7)];
        for from in 1..24 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            words.push(if state & 1 == 0 {
                let to = from + 1 + (state as usize % (24 - from));
                branch(from, to, ((state >> 1) & 1) as u8)
            } else {
                add(0, 0, state & 0x7ffff)
            });
        }
        words.push(EXIT);
        let p = lift(&words, &[P(0), P(1)], &[R(0)]);
        validate(&p.emit_wgsl(Options::default()).unwrap());
    }
}

#[test]
fn numeric_policy_distinguishes_bit_operations_arithmetic_and_special_functions() {
    use Register::Gpr as R;
    for (word, arithmetic, special) in [
        (0x5c58000000270100, true, false),
        (0x5980018000270100, true, false),
        (0x5080000000470100, false, true),
    ] {
        let p = lift(&[word, EXIT], &[R(1), R(2), R(3)], &[R(0)]);
        assert_eq!(p.emit_wgsl(Options::default()).unwrap_err().kind, DiagnosticKind::Unsupported);
        let kernel = p.emit_wgsl(options()).unwrap();
        assert_eq!(
            (kernel.approximate_arithmetic, kernel.approximate_special_functions),
            (arithmetic, special)
        );
        validate(&kernel);
    }
    let directed = lift(&[0x5c58000000270100 | (1 << 39), EXIT], &[R(1), R(2)], &[R(0)]);
    assert_eq!(directed.emit_wgsl(options()).unwrap_err().pc, Some(8));
    for rounding in 0..4 {
        for word in [0x5cb0000000171a00 | (rounding << 39), 0x5cb8000000172a00 | (rounding << 39)] {
            let kernel =
                lift(&[word, EXIT], &[R(1)], &[R(0)]).emit_wgsl(Options::default()).unwrap();
            assert!(!kernel.approximate_arithmetic);
            validate(&kernel);
        }
    }
}

#[test]
fn memory_inputs_share_the_existing_sparse_bank_contract() {
    let word = 0xef90000000470100; // LDC.U8 R0,c0[R1+4]
    let p = lift(&[word, EXIT], &[Register::Gpr(1)], &[Register::Gpr(0)]);
    let kernel = p.emit_wgsl(Options::default()).unwrap();
    assert_eq!(kernel.inputs, [Input::Register(Register::Gpr(1))]);
    assert!(kernel.constant_memory.is_some());
    validate(&kernel);
}

#[test]
fn scalar_structuring_does_not_authorize_derivative_control_flow() {
    let words = [0xd820008ff0370808 & !(7 << 16), EXIT];
    let p = Program::lift(MaxwellCode::parse(&bytes(&words)).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        ..Default::default()
    })
    .unwrap();
    let mut binding = texture::binding(
        crate::maxwell::texture::TextureDimension::D2,
        crate::resources::texture::TextureValueType::Float,
    );
    binding.view.texture_index = 8;
    let error = p.emit_wgsl_with_textures(options(), &[binding]).unwrap_err();
    assert_eq!(
        (error.phase, error.kind, error.pc),
        (Phase::Emit, DiagnosticKind::Unsupported, Some(8))
    );
    assert!(error.message.contains("GPU execution analysis"));
}

#[test]
#[ignore = "requires a native GPU; run with --ignored --nocapture"]
fn emitted_kernels_match_the_interpreter_on_a_native_gpu() {
    use Register::{Gpr as R, Predicate as P};
    let gpu = gpu::Gpu::new();
    let mut programs = vec![
        (Program { function: swapping_loop() }, Options::default()),
        (
            lift(&[mov(0, 7, 7), mov(0, 11, 0), add(0, 0, 3), EXIT], &[P(0)], &[R(0)]),
            Options::default(),
        ),
        (lift(&[mov(0, 7, 0), EXIT], &[P(0)], &[R(0)]), Options::default()),
        (
            lift(
                &[mov(0, 0, 7), add(0, 0, 1), isetp(0, 7, 0, 1, 1, 7, 7), branch(3, 1, 0), EXIT],
                &[R(1)],
                &[R(0)],
            ),
            Options { step_budget: 100, ..Default::default() },
        ),
    ];
    for profile in
        [crate::program::wgsl::Profile::Checked, crate::program::wgsl::Profile::Rendering]
    {
        let program = Program::lift(
            MaxwellCode::parse(&bytes(&[0xe33000000000000f, mov(0, 17, 7), EXIT])).unwrap(),
            &Signature {
                stage: Stage::Fragment,
                inputs: vec![P(0)],
                outputs: vec![R(0)],
                ..Default::default()
            },
        )
        .unwrap();
        programs.push((program, Options { profile, ..Default::default() }));
    }
    for round in 0..4 {
        for signed in [false, true] {
            for word in [
                0x5cb0000000170a00 | (u64::from(signed) << 12) | (round << 39),
                0x5cb8000000170a00 | (u64::from(signed) << 13) | (round << 39),
            ] {
                programs.push((lift(&[word, EXIT], &[R(1)], &[R(0)]), Options {
                    f2i_nan_behavior: Some(F2iNanBehavior::Indefinite),
                    ..Default::default()
                }));
            }
        }
    }
    let mut comparisons = 0;
    for (p, opts) in programs {
        let kernel = p.emit_wgsl(opts).unwrap();
        let mut cases = vec![];
        let mut state = 0x37a491c5u32;
        for i in 0..256 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let word = match i {
                0 => 0,
                1 => 1,
                2 => 0x80000000,
                3 => 0xffffffff,
                4 => 0x7f800000,
                5 => 0xff800000,
                6 => 0x7fc12345,
                7 => 0x7f812345,
                _ => state,
            };
            let input = Inputs {
                registers: [(R(1), Value::Word(word)), (P(0), Value::Predicate(i % 2 == 0))].into(),
                f2i_nan_behavior: opts.f2i_nan_behavior,
                ..Default::default()
            };
            cases.push(input);
        }
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    // Finite exactly representable arithmetic isolates emission from backend
    // approximation errors. Neither these inputs nor this host establish SM53.
    for word in [0x5c58000000270100, 0x5c68000000270100, 0x5980018000270100, 0x5080000000470100] {
        let p = lift(&[word, EXIT], &[R(1), R(2), R(3)], &[R(0)]);
        let kernel = p.emit_wgsl(options()).unwrap();
        let cases = (1..128)
            .map(|i| Inputs {
                registers: [
                    (R(1), Value::Word(4.0f32.to_bits())),
                    (R(2), Value::Word((i as f32 * 0.25).to_bits())),
                    (R(3), Value::Word((-2.0f32).to_bits())),
                ]
                .into(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    // Sparse known bytes, absent banks, misalignment, and missing addresses.
    for word in [0xef90000000470100, 0xef94000000470100] {
        let p = lift(&[word, EXIT], &[R(1)], &[R(0)]);
        let kernel = p.emit_wgsl(Options::default()).unwrap();
        let data = (0..64).collect::<Vec<u8>>();
        let known = (0..64).map(|i| i % 3 != 0).collect::<Vec<_>>();
        let memory =
            crate::resources::ConstantMemory::from_banks(&[crate::resources::ConstantBank {
                bank: 0,
                bytes: &data,
                known: Some(&known),
            }])
            .unwrap();
        let cases = (0..80)
            .map(|i| Inputs {
                registers: if i == 79 {
                    Default::default()
                } else {
                    [(R(1), Value::Word(i))].into()
                },
                constants: memory.clone(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    // Two entries into one cycle: no single loop header dominates the body.
    let p = lift(
        &[
            mov(0, 0, 7),
            branch(1, 6, 0),
            add(0, 0, 1),
            add(2, 2, 1),
            isetp(1, 7, 2, 3, 1, 7, 7),
            (EXIT & !(7 << 16)) | (1 << 16) | (1 << 19),
            add(0, 0, 2),
            add(2, 2, 1),
            isetp(1, 7, 2, 3, 1, 7, 7),
            branch(9, 2, 1),
            EXIT,
        ],
        &[P(0), R(2), R(3)],
        &[R(0)],
    );
    let k = p.emit_wgsl(Options::default()).unwrap();
    assert_eq!(k.control_flow, ControlFlow::ScalarDispatcher);
    let cases = (0..128)
        .map(|i| Inputs {
            registers: [
                (P(0), Value::Predicate(i % 2 != 0)),
                (R(2), Value::Word(0)),
                (R(3), Value::Word(i / 2)),
            ]
            .into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    compare(&gpu, &p, &k, &cases);
    comparisons += cases.len();
    // Execute original vertex logic through a compute entry point, comparing
    // both emitters with the interpreter. Rasterization is outside this test.
    let original = original_vertex();
    let kernel = original.emit_wgsl(options()).unwrap();
    let mut state = 0x719ac45fu32;
    let cases = (0..1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Inputs { attributes: [(0x2fc, state)].into(), ..Default::default() }
        })
        .collect::<Vec<_>>();
    compare(&gpu, &original, &kernel, &cases);
    let legacy =
        crate::wgsl::lower_vertex(MaxwellCode::parse(&bytes(&ORIGINAL_VERTEX)).unwrap()).unwrap();
    assert!(legacy.constant_reads.is_empty() && legacy.constant_memory_binding.is_none());
    assert!(legacy.input_registers.is_empty() && legacy.input_predicates.is_empty());
    let mut counterpart = kernel.clone();
    counterpart.source = format!(
        r#"{}
struct RemaxResult {{
    words: array<u32, 6>,
    status: u32,
    pc: u32,
    discarded: u32,
}}
fn remax_main(words: array<u32, 1>, valid: array<u32, 1>) -> RemaxResult {{
    var initial: MprVertexState;
    var attributes: array<u32, 256>;
    attributes[191] = words[0];
    var constants: array<u32, 1>;
    let state = mpr_vertex(initial, attributes, constants);
    var result: RemaxResult;
    result.words = array<u32, 6>(
        state.attributes[32], state.attributes[33], state.attributes[34],
        state.attributes[36], state.attributes[37], state.attributes[38],
    );
    return result;
}}
"#,
        legacy.wgsl,
    );
    compare(&gpu, &original, &counterpart, &cases);
    comparisons += cases.len() * 2;
    let sync_cases = (0..32)
        .map(|limit| Inputs {
            registers: [(R(2), Value::Word(limit))].into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let integer_cases = super::integer::comparison_cases()
        .into_iter()
        .chain(super::bits::comparison_cases())
        .chain([(super::control::sync_loop(), sync_cases)]);
    for (p, cases) in integer_cases {
        let kernel = p.emit_wgsl(Options::default()).unwrap();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    comparisons += analysis::compare_analysis_results(&gpu);
    comparisons += loops::compare_loops(&gpu);
    comparisons += memory::compare_snapshot_loads(&gpu);
    comparisons += local::compare_local_memory(&gpu);
    comparisons += bindings::compare_profiles(&gpu);
    for case in super::conversion::cases() {
        for profile in
            [crate::program::wgsl::Profile::Checked, crate::program::wgsl::Profile::Rendering]
        {
            let kernel = case
                .program
                .emit_wgsl(Options {
                    profile,
                    f2i_nan_behavior: Some(case.nan_behavior),
                    ..options()
                })
                .unwrap();
            validate(&kernel);
            for (result, expected) in gpu.run(&kernel, &case.inputs).iter().zip(&case.expected) {
                assert_eq!(result[0], *expected);
                assert_eq!(result[1], 0, "unexpected conversion fault");
            }
            comparisons += case.inputs.len();
        }
    }
    comparisons += interpolation::raster_planes(&gpu);
    comparisons += indexed_attributes::compare_indexed_attributes(&gpu);
    for (p, cases) in super::interpolation::comparison_cases() {
        let kernel = p
            .emit_wgsl(Options {
                offset_interpolation: Some(
                    crate::program::interpolation::OffsetModel::SignedSixteenths,
                ),
                ..options()
            })
            .unwrap();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    comparisons += reduction::compare_exponentiation(&gpu);
    comparisons += trigonometry::compare_trigonometry(&gpu);
    for (p, cases) in super::reduction::comparison_cases() {
        let kernel = p.emit_wgsl(options()).unwrap();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    for (p, cases) in super::rounding::comparison_cases() {
        let kernel = p.emit_wgsl(options()).unwrap();
        compare(&gpu, &p, &kernel, &cases);
        comparisons += cases.len();
    }
    for (program, cases) in super::minmax::comparison_cases() {
        let kernel = program.emit_wgsl(options()).unwrap();
        compare(&gpu, &program, &kernel, &cases);
        comparisons += cases.len();
    }
    println!("Checked {comparisons} GPU invocations against interpreter and numeric references");
}

fn compare(gpu: &gpu::Gpu, p: &Program, k: &Kernel, cases: &[Inputs]) {
    let results = gpu.run(k, cases);
    for (input, result) in cases.iter().zip(results) {
        let expected = p.interpret_with_options(input, InterpretOptions {
            step_budget: u64::from(k.step_budget),
            float_model: FloatModel::SoftwareIeee,
            special_functions: SpecialFunctionModel::HostF32,
        });
        match expected {
            Ok(Execution::Outputs(values)) => {
                assert_eq!(result[k.outputs.len().max(1)], 0, "GPU fault: {result:?}");
                assert_eq!(result[k.outputs.len().max(1) + 2], 0, "unexpected demotion");
                let words = values
                    .into_iter()
                    .map(|v| match v {
                        Value::Word(v) => v,
                        Value::Predicate(v) => u32::from(v),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(&result[..words.len()], words, "input={input:?}");
            }
            Ok(Execution::Discarded) => {
                assert_eq!(result[k.outputs.len().max(1)], 0, "GPU fault: {result:?}");
                assert_eq!(result[k.outputs.len().max(1) + 2], 1, "missing demotion");
            }
            Err(e) => {
                assert_ne!(result[k.outputs.len().max(1)], 0, "missing GPU fault for {e:?}");
                assert_eq!(
                    result[k.outputs.len().max(1) + 1] as usize,
                    e.pc.unwrap_or(0),
                    "fault PC: {e:?}"
                );
                assert_eq!(
                    result[k.outputs.len().max(1)] == 16,
                    e.kind == DiagnosticKind::StepLimit,
                    "budget fault precedence: {e:?}, GPU {result:?}"
                );
            }
        }
    }
}
