use Register::{Gpr as R, Predicate as P};

use super::{gpu::Gpu, validate};
use crate::{
    ir::tests::{
        bytes,
        global_memory::{inputs, interpret, ldg},
        lift, EXIT,
    },
    maxwell::MaxwellCode,
    program::{
        wgsl::{memory::Binding, Options},
        DiagnosticKind, Inputs, Program, Register, Signature, Value,
    },
    resources::{
        memory::{Region, Snapshot},
        ConstantMemory,
    },
};

fn options() -> Options {
    Options { global_memory: Some(Binding { group: 2, binding: 0 }), ..Default::default() }
}

#[test]
fn global_reads_require_a_distinct_snapshot_binding_and_retain_u128_uncertainty() {
    let p = lift(&[ldg(0, 4, 0, true, 6), EXIT], &[R(4), R(5)], &[R(0), R(1), R(2), R(3)]);
    assert_eq!(p.emit_wgsl(Options::default()).unwrap_err().kind, DiagnosticKind::MissingInput);
    let k = p.emit_wgsl(options()).unwrap();
    assert_eq!(k.global_memory, options().global_memory);
    validate(&k);
    let p = lift(&[ldg(0, 4, 0, true, 7), EXIT], &[R(4), R(5)], &[R(0)]);
    assert_eq!(p.emit_wgsl(options()).unwrap_err().kind, DiagnosticKind::Unsupported);
    let p = lift(&[0x4c98078000070000, ldg(0, 4, 0, true, 4), EXIT], &[R(4), R(5)], &[R(0)]);
    assert_eq!(
        p.emit_wgsl(Options {
            global_memory: Some(Binding { group: 1, binding: 0 }),
            ..Default::default()
        })
        .unwrap_err()
        .kind,
        DiagnosticKind::InvalidInput
    );
}

#[test]
fn predicated_and_all_scalar_load_forms_validate() {
    for ty in 0..7 {
        for wide in [false, true] {
            let p = lift(&[ldg(0, 4, -16, wide, ty), EXIT], &[R(4), R(5)], &[R(0)]);
            validate(&p.emit_wgsl(options()).unwrap());
        }
    }
    let p = lift(&[ldg(0, 4, 0, true, 4) & !(7 << 16), EXIT], &[R(0), R(4), R(5), P(0)], &[R(0)]);
    validate(&p.emit_wgsl(options()).unwrap());
}

#[test]
fn memory_faults_do_not_remove_lanes_before_implicit_sampling() {
    use crate::{
        maxwell::texture::TextureDimension,
        program::{
            wgsl::{texture, FaultScope},
            Stage,
        },
    };
    // A direct global read precedes a decoded TEXS instruction. Snapshot faults
    // must be recorded without returning from the fragment kernel early.
    let words = bytes(&[ldg(20, 20, 0, true, 4), 0xd830012ff0770606, EXIT]);
    let p = Program::lift(MaxwellCode::parse(&words).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        inputs: [6, 7, 20, 21].map(R).to_vec(),
        outputs: vec![R(6)],
    })
    .unwrap();
    let mut binding = texture::Binding {
        view: texture::TextureBinding {
            texture_index: 0x12,
            dimension: TextureDimension::D2,
            value_type: texture::TextureValueType::Float,
            multisample: false,
            lod_bias: 0.0,
            sampling: None,
        },
        group: 3,
        texture_binding: 0,
        sampler_binding: Some(1),
        depth_components: None,
    };
    let k = p.emit_wgsl_with_textures(options(), &[binding]).unwrap();
    super::texture::validate(&k).unwrap();
    assert_eq!(k.fault_scope, FaultScope::Quad);
    let main = k.source.split("fn remax_main").nth(1).unwrap();
    let sample = main.rfind("textureSampleBias(").unwrap();
    assert!(!main[..sample].contains("return remax_fault"));
    binding.group = 2; // Collision with the global-memory snapshot.
    assert_eq!(
        p.emit_wgsl_with_textures(options(), &[binding]).unwrap_err().kind,
        DiagnosticKind::InvalidInput
    );
}

pub(super) fn compare_snapshot_loads(gpu: &Gpu) -> usize {
    let data = (0..96).map(|i| (i * 37 + 129) as u8).collect::<Vec<_>>();
    let known = (0..96).map(|i| i % 11 != 0).collect::<Vec<_>>();
    let snapshot = Snapshot::new(&[
        Region { address: 0xfffffff7, bytes: &data[..14], known: None },
        Region { address: 0x100000005, bytes: &data[14..], known: None },
        Region { address: 0x111100002000, bytes: &data, known: Some(&known) },
        Region { address: u64::MAX - 31, bytes: &data[..32], known: None },
        Region { address: 0, bytes: &data[..16], known: None },
    ])
    .unwrap();
    let addresses = (0..32)
        .chain(0xffffffe0..0x100000068)
        .chain(0x111100001ff0..0x111100002070)
        .chain(u64::MAX - 31..=u64::MAX)
        .collect::<Vec<_>>();
    let mut cases = addresses.into_iter().map(inputs).collect::<Vec<_>>();
    cases.push(Inputs::default());
    let mut total = 0;
    for ty in 0..7 {
        for displacement in [0, -16, 32] {
            let count = if ty <= 4 {
                1
            } else if ty == 5 {
                2
            } else {
                4
            };
            let outputs = (0..count).map(|i| R(4 + i)).collect::<Vec<_>>();
            let p = lift(&[ldg(4, 4, displacement, true, ty), EXIT], &[R(4), R(5)], &outputs);
            check(gpu, &p, &cases, &snapshot);
            total += cases.len();
        }
    }
    // An original ordinary .128 load from startup source 328. Address source
    // and result group alias; the actual virtual address is caller supplied.
    let p = lift(&[0xeed6a00000070808, 0xf0f0000034170000, EXIT], &[R(8), R(9)], &[
        R(8),
        R(9),
        R(10),
        R(11),
    ]);
    let args = Inputs {
        registers: [(R(8), Value::Word(0)), (R(9), Value::Word(1))].into(),
        ..Default::default()
    };
    check(gpu, &p, &[args], &snapshot);
    total += 1;
    // Bind global addresses through the same c[0] descriptor used by native
    // storage shaders, checking the join between constant and global providers.
    let p =
        lift(&[0x4c18820014470606, 0x4c1008001457ff07, ldg(0, 6, 0, true, 4), EXIT], &[R(6)], &[
            R(0),
        ]);
    let mut constants = vec![0; 0x518];
    constants[0x510..].copy_from_slice(&0xfffffff0u64.to_le_bytes());
    let memory = ConstantMemory::new(&[(0, &constants)]).unwrap();
    let cases = (0..16)
        .map(|i| Inputs {
            registers: [(R(6), Value::Word(i))].into(),
            constants: memory.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    check(gpu, &p, &cases, &snapshot);
    total += cases.len();
    let p = lift(&[ldg(0, 4, 0, true, 4) & !(7 << 16), EXIT], &[R(0), R(4), R(5), P(0)], &[R(0)]);
    let cases = [false, true]
        .into_iter()
        .map(|enabled| Inputs {
            registers: [(R(0), Value::Word(0xa5a5)), (P(0), Value::Predicate(enabled))].into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    check(gpu, &p, &cases, &snapshot);
    total += cases.len();
    let (p, mut snapshot, cases) = crate::ir::tests::global_memory::probe_lighting();
    let k = p.emit_wgsl(Options { approximate_arithmetic: true, ..options() }).unwrap();
    validate(&k);
    for (input, result) in cases.iter().zip(gpu.run_with_snapshot(&k, &cases, &snapshot)) {
        let expected = p
            .interpret_with_resources(
                input,
                crate::program::InterpretOptions {
                    step_budget: 10000,
                    float_model: crate::program::FloatModel::SoftwareIeee,
                    ..Default::default()
                },
                crate::program::Providers { memory: Some(&mut snapshot), ..Default::default() },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        assert_eq!(&result[4..], [0, 0, 0]);
        for (value, actual) in expected.iter().zip(&result) {
            assert_eq!(value, &Value::Word(*actual));
        }
    }
    total += cases.len();
    let (p, mut snapshot, cases) = crate::ir::tests::probe::nonlinear();
    let k = p
        .emit_wgsl(Options {
            approximate_arithmetic: true,
            approximate_special_functions: true,
            offset_interpolation: Some(
                crate::program::interpolation::OffsetModel::SignedSixteenths,
            ),
            ..options()
        })
        .unwrap();
    validate(&k);
    // Each 16-sample table is a separate draw's constant state, shared by its
    // 64 invocations. Do not vary uniform bytes inside one GPU dispatch.
    let results = cases
        .chunks(64)
        .flat_map(|batch| gpu.run_with_snapshot(&k, batch, &snapshot))
        .collect::<Vec<_>>();
    for (input, result) in cases.iter().zip(results) {
        let expected = p
            .interpret_with_resources(
                input,
                crate::program::InterpretOptions {
                    step_budget: 10000,
                    float_model: crate::program::FloatModel::SoftwareIeee,
                    special_functions: crate::program::SpecialFunctionModel::HostF32,
                },
                crate::program::Providers { memory: Some(&mut snapshot), ..Default::default() },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        assert_eq!(&result[4..], [0, 0, 0]);
        for (value, actual) in expected.iter().zip(&result) {
            let Value::Word(word) = value else { panic!() };
            let (expected, actual) = (f32::from_bits(*word), f32::from_bits(*actual));
            assert!(
                (actual - expected).abs() <= 2e-5 * expected.abs().max(1.),
                "nonlinear probe: {actual} != {expected}"
            );
        }
        assert_eq!(result[3], 1f32.to_bits());
    }
    total += cases.len();
    total
}

fn check(gpu: &Gpu, p: &Program, cases: &[Inputs], snapshot: &Snapshot) {
    let k = p.emit_wgsl(options()).unwrap();
    validate(&k);
    let results = gpu.run_with_snapshot(&k, cases, snapshot);
    for (input, result) in cases.iter().zip(results) {
        match interpret(p, input, snapshot) {
            Ok(words) => {
                assert_eq!(result[k.outputs.len()], 0, "{input:?}: {result:?}");
                let words = words
                    .into_iter()
                    .map(|v| match v {
                        Value::Word(w) => w,
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(&result[..words.len()], words, "{input:?}");
            }
            Err(e) => {
                assert_ne!(result[k.outputs.len()], 0, "{input:?}: expected {e:?}");
                assert_eq!(result[k.outputs.len() + 1] as usize, e.pc.unwrap_or(0));
                let status = if e.kind == DiagnosticKind::InvalidInput {
                    4
                } else if e.message.contains("unknown memory") {
                    1024
                } else if e.message.contains("unmapped memory") || e.message.contains("wraps") {
                    512
                } else {
                    32
                };
                assert_eq!(result[k.outputs.len()], status, "{e:?}");
            }
        }
    }
}
