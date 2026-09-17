use Register::{Gpr as R, Predicate as P};

use super::{add, branch, bytes, isetp, lift, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        memory, DiagnosticKind, Inputs, InterpretOptions, Program, Providers, Register, Signature,
        Value,
    },
    resources::memory::{Region, Snapshot},
};

pub(super) fn ldg(dst: u8, address: u8, displacement: i32, wide: bool, ty: u8) -> u64 {
    0xeed0000000070000
        | u64::from(dst)
        | (u64::from(address) << 8)
        | ((displacement as u64 & 0xffffff) << 20)
        | (u64::from(wide) << 45)
        | (u64::from(ty) << 48)
}

pub(super) fn inputs(address: u64) -> Inputs {
    Inputs {
        registers: [
            (R(4), Value::Word(address as u32)),
            (R(5), Value::Word((address >> 32) as u32)),
        ]
        .into(),
        ..Default::default()
    }
}

pub(super) fn interpret(
    p: &Program,
    inputs: &Inputs,
    mut snapshot: &Snapshot,
) -> Result<Vec<Value>, crate::program::Diagnostic> {
    p.interpret_with_resources(
        inputs,
        InterpretOptions { step_budget: 10000, ..Default::default() },
        Providers { memory: Some(&mut snapshot), ..Default::default() },
    )
    .map(|execution| execution.into_outputs().expect("live invocation"))
}

#[test]
fn subword_extension_and_aliased_wide_result_read_one_access() {
    let data = [0x81, 0x92, 0xa3, 0xb4, 0xc5, 0xd6, 0xe7, 0xf8, 9, 10, 11, 12, 13, 14, 15, 16];
    let snapshot =
        Snapshot::new(&[Region { address: 0x1234567800, bytes: &data, known: None }]).unwrap();
    for (ty, expected) in [
        (0, vec![0x81]),
        (1, vec![0xffffff81]),
        (2, vec![0x9281]),
        (3, vec![0xffff9281]),
        (4, vec![0xb4a39281]),
        (5, vec![0xb4a39281, 0xf8e7d6c5]),
        (6, vec![0xb4a39281, 0xf8e7d6c5, 0x0c0b0a09, 0x100f0e0d]),
    ] {
        let outputs = (0..expected.len()).map(|i| R(4 + i as u8)).collect::<Vec<_>>();
        let p = lift(&[ldg(4, 4, 0, true, ty), EXIT], &[R(4), R(5)], &outputs);
        assert_eq!(
            interpret(&p, &inputs(0x1234567800), &snapshot).unwrap(),
            expected.into_iter().map(Value::Word).collect::<Vec<_>>()
        );
    }
}

#[test]
fn global_address_offsets_preserve_high_words_and_do_not_write_machine_carry() {
    for (wide, base, offset, expected) in [
        (true, 0x1200000000, -16, 0x11fffffff0),
        (true, 0x12fffffff0, 32, 0x1300000010),
        (false, 0xfffffff0, 32, 0x100000010),
        (false, 0, -16, 0xfffffffffffffff0),
    ] {
        let p = lift(&[ldg(0, 4, offset, wide, 4), EXIT], &[R(4), R(5), Register::Carry], &[
            R(0),
            Register::Carry,
        ]);
        let mut args = inputs(base);
        args.registers.insert(Register::Carry, Value::Predicate(true));
        let mut calls = 0;
        let result = p
            .interpret_with_resources(
                &args,
                InterpretOptions { step_budget: 1000, ..Default::default() },
                Providers {
                    memory: Some(&mut |r: &memory::Read| {
                        calls += 1;
                        assert_eq!(r.address, expected);
                        assert_eq!(r.bytes, 4);
                        Ok([0xff; 16])
                    }),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        assert_eq!(calls, 1);
        assert_eq!(result, [Value::Word(u32::MAX), Value::Predicate(true)]);
    }
    for wide in [false, true] {
        let p = lift(&[ldg(0, 255, 0xffffff, wide, 0), EXIT], &[], &[R(0)]);
        let snapshot =
            Snapshot::new(&[Region { address: 0xffffff, bytes: &[0x5a], known: None }]).unwrap();
        assert_eq!(interpret(&p, &Inputs::default(), &snapshot).unwrap(), [Value::Word(0x5a)]);
    }
}

#[test]
fn predicate_guards_missing_memory_but_discarding_a_load_does_not() {
    let p = lift(&[ldg(255, 4, 0, true, 4) & !(7 << 16), EXIT], &[P(0), R(4), R(5)], &[]);
    let mut args =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert!(p.interpret(&args, 100).is_ok());
    args.registers.insert(P(0), Value::Predicate(true));
    assert_eq!(p.interpret(&args, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
    args.registers.extend(inputs(0x1000).registers);
    let e = p.interpret(&args, 100).unwrap_err();
    assert_eq!(e.pc, Some(8));
    assert!(e.message.contains("provider"));
    assert!(interpret(&p, &args, &Snapshot::default()).is_err());
}

#[test]
fn repeated_reads_keep_occurrences_cache_and_original_u128_modifier_for_replay() {
    let word = 0xeed7a00000070604; // Original LDG.E.CI.U.128 R4,[R6].
    let p = lift(&[word, word, EXIT], &[R(6), R(7)], &[R(4), R(5), R(6), R(7)]);
    let args = Inputs {
        registers: [(R(6), Value::Word(0x1000)), (R(7), Value::Word(17))].into(),
        ..Default::default()
    };
    let mut seen = vec![];
    let result = p
        .interpret_with_resources(
            &args,
            InterpretOptions { step_budget: 1000, ..Default::default() },
            Providers {
                memory: Some(&mut |r: &memory::Read| {
                    seen.push(*r);
                    assert!(r.native_u128);
                    assert_eq!(r.cache, memory::Cache::Ci);
                    Ok([0x55; 16])
                }),
                ..Default::default()
            },
        )
        .unwrap()
        .into_outputs()
        .expect("live invocation");
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].address, 0x1100001000);
    assert_eq!(seen[1].address, 0x5555555555555555); // First read overwrote R6/R7.
    assert_eq!(result, [Value::Word(0x55555555); 4]);
    let snapshot = Snapshot::default();
    assert_eq!(interpret(&p, &args, &snapshot).unwrap_err().kind, DiagnosticKind::Unsupported);

    let p = lift(
        &[ldg(0, 4, 0, true, 4), add(1, 1, 1), isetp(0, 7, 1, 2, 1, 7, 7), branch(3, 0, 0), EXIT],
        &[R(1), R(2), R(4), R(5)],
        &[R(0)],
    );
    let mut args = inputs(0);
    args.registers.extend([(R(1), Value::Word(0)), (R(2), Value::Word(3))]);
    let mut occurrences = vec![];
    p.interpret_with_resources(
        &args,
        InterpretOptions { step_budget: 1000, ..Default::default() },
        Providers {
            memory: Some(&mut |r: &memory::Read| {
                occurrences.push(r.occurrence);
                Ok([0; 16])
            }),
            ..Default::default()
        },
    )
    .unwrap()
    .into_outputs()
    .expect("live invocation");
    assert_eq!(occurrences, [0, 1, 2]);
}

#[test]
fn invalid_register_spans_and_ir_widths_are_rejected() {
    for word in [
        ldg(0, 3, 0, true, 4),
        ldg(0, 254, 0, true, 4),
        ldg(254, 4, 0, true, 5),
        ldg(1, 4, 0, true, 6),
        ldg(0, 4, 0, true, 4) | (1 << 44),
    ] {
        let b = bytes(&[word, EXIT]);
        assert!(Program::lift(MaxwellCode::parse(&b).unwrap(), &Signature::default()).is_err());
    }
    let p = lift(&[ldg(0, 4, 0, true, 4), EXIT], &[R(4), R(5)], &[R(0)]);
    for (width, signed, u128) in [(3, false, false), (4, true, false), (4, false, true)] {
        let mut f = p.function.clone();
        let op = f
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find_map(|i| {
                if let crate::ir::Operation::GlobalLoad(l) = &mut i.operation {
                    Some(l)
                } else {
                    None
                }
            })
            .unwrap();
        op.bytes = width;
        op.signed = signed;
        op.native_u128 = u128;
        assert_eq!(f.verify().unwrap_err().kind, DiagnosticKind::InvalidIr);
    }
}

#[test]
fn dependency_waits_keep_synchronous_accesses_and_do_not_authorize_other_barriers() {
    let p = lift(
        &[
            ldg(0, 4, 0, true, 4),
            0xf0f0000034170000,
            ldg(1, 4, 0, true, 4),
            0xf0f0000000070021,
            EXIT,
        ],
        &[R(4), R(5)],
        &[R(0), R(1)],
    );
    let mut calls = 0u8;
    let result = p
        .interpret_with_resources(
            &inputs(0x1000),
            InterpretOptions { step_budget: 1000, ..Default::default() },
            Providers {
                memory: Some(&mut |request: &memory::Read| {
                    assert_eq!(request.address, 0x1000);
                    calls += 1;
                    Ok([calls; 16])
                }),
                ..Default::default()
            },
        )
        .unwrap()
        .into_outputs()
        .expect("live invocation");
    assert_eq!(calls, 2);
    assert_eq!(result, [Value::Word(0x01010101), Value::Word(0x02020202)]);
    let p = lift(&[0xf0f0000034100000, EXIT], &[R(0), P(0)], &[R(0)]);
    for enabled in [false, true] {
        assert_eq!(
            p.interpret(
                &Inputs {
                    registers: [(P(0), Value::Predicate(enabled)), (R(0), Value::Word(7))].into(),
                    ..Default::default()
                },
                1000
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
            [Value::Word(7)]
        );
    }
    for word in
        [0xf0f0000038170000, 0xf0f0000034170000 | (1 << 43), 0xef98000000070000, 0xf0a8000000070000]
    {
        assert_eq!(
            Program::lift(
                MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
                &Signature::default()
            )
            .unwrap_err()
            .kind,
            DiagnosticKind::Unsupported
        );
    }
}

// Complete reachable PS from startup source329, ProbeLighting/x/permutation1.
// Flat attribute0x80 indexes 80-byte records; RGB at the beginning of the record
// is multiplied by two uniform RGB factors and 1/pi, with alpha forced to one.
// The trailing self-branch and padding after EXIT are not part of execution.
pub(super) fn probe_lighting() -> (Program, Snapshot, Vec<Inputs>) {
    use crate::program::{Interpolant, InterpolationLocation, InterpolationMode, Stage};
    let words = bytes(&[
        0x4c98078c00470006,
        0xe083ff880ff7ff00,
        0x36007f8005070001,
        0x3620009005070000,
        0x4c10800014470004,
        0x4c1008001457ff05,
        0xeed6a00000070400,
        0x4c98078c00570007,
        0x4c98078c00670008,
        0x4c68100c00070606,
        0x4c68100c00170707,
        0x4c68100c00270808,
        0x5c68100000670000,
        0x5c68100000770101,
        0x5c68100000870202,
        0x0103f8000007f003,
        0x1e23ea2f98370000,
        0x1e23ea2f98370101,
        0x1e23ea2f98370202,
        EXIT,
    ]);
    let p = Program::lift(MaxwellCode::parse(&words).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        outputs: [0, 1, 2, 3].map(R).to_vec(),
        ..Default::default()
    })
    .unwrap();
    let base = 0xfffffff0u64;
    let indices = [0u32, 1, 2, 0x10001];
    let data = indices
        .iter()
        .enumerate()
        .map(|(i, _)| {
            [0.25 * (i + 1) as f32, 0.5 * (i + 1) as f32, (i + 1) as f32, 99.0]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let snapshot = Snapshot::new(
        &indices
            .iter()
            .zip(&data)
            .map(|(&index, data)| Region {
                address: base + u64::from(index) * 80,
                bytes: data,
                known: None,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut driver = vec![0; 0x518];
    driver[0x510..].copy_from_slice(&base.to_le_bytes());
    let uniforms = [2.0f32, 4.0, 8.0, 0.0, 0.5, 0.25, 0.125]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let constants = crate::resources::ConstantMemory::new(&[(0, &driver), (3, &uniforms)]).unwrap();
    let cases = indices
        .into_iter()
        .map(|index| Inputs {
            constants: constants.clone(),
            interpolants: [(
                Interpolant {
                    byte_offset: 0x80,
                    mode: InterpolationMode::Constant,
                    location: InterpolationLocation::Default,
                },
                index,
            )]
            .into(),
            ..Default::default()
        })
        .collect();
    (p, snapshot, cases)
}

#[test]
fn original_probe_lighting_reads_records_and_scales_rgb() {
    let (p, mut snapshot, cases) = probe_lighting();
    for (i, input) in cases.iter().enumerate() {
        let actual = p
            .interpret_with_resources(
                input,
                InterpretOptions {
                    step_budget: 10000,
                    float_model: crate::program::FloatModel::SoftwareIeee,
                    ..Default::default()
                },
                Providers { memory: Some(&mut snapshot), ..Default::default() },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        let inv_pi = f32::from_bits(0x3ea2f983);
        let expected = [
            0.25 * (i + 1) as f32 * inv_pi,
            0.5 * (i + 1) as f32 * inv_pi,
            (i + 1) as f32 * inv_pi,
            1.0,
        ];
        assert_eq!(actual, expected.map(|v| Value::Word(v.to_bits())));
    }
}
