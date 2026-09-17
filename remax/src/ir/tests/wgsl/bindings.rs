use Register::{Gpr as R, Predicate as P};

use super::{gpu::Gpu, validate};
use crate::{
    ir::tests::{lift, mov, offset, EXIT},
    program::{
        wgsl::{Options, Profile},
        DiagnosticKind, Inputs, Register, Value,
    },
    resources::{ConstantBank, ConstantMemory},
};

mod alignment;

fn options(profile: Profile) -> Options { Options { profile, ..Default::default() } }

#[test]
fn rendering_preconditions_cover_guarded_reads_and_full_multiword_spans() {
    let guarded = lift(&[mov(0, 7, 7), 0x4c98078000000000, EXIT], &[P(0)], &[R(0)]);
    let checked = guarded.emit_wgsl(options(Profile::Checked)).unwrap();
    let rendering = guarded.emit_wgsl(options(Profile::Rendering)).unwrap();
    validate(&checked);
    validate(&rendering);
    assert!(checked.constant_preconditions.bind(&ConstantMemory::default()).is_ok());
    let error = rendering.constant_preconditions.bind(&ConstantMemory::default()).unwrap_err();
    assert_eq!(error.pc, Some(offset(1)));
    assert_eq!(error.kind, DiagnosticKind::MissingInput);
    // A conservative binding contract can reject an untaken read. The checked
    // profile remains available for diagnosing only the executed path.
    let input =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(guarded.interpret(&input, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(7)
    ]);

    let wide = lift(&[mov(1, 8, 7), 0xef95000000070100, EXIT], &[], &[R(0), R(1)]);
    let kernel = wide.emit_wgsl(options(Profile::Rendering)).unwrap();
    validate(&kernel);
    let preconditions = &kernel.constant_preconditions;
    assert_eq!(preconditions.accesses().len(), 1);
    assert_eq!(preconditions.accesses()[0].byte_count, 8);
    let data = [0; 16];
    let mut known = [true; 16];
    known[15] = false;
    let sparse =
        ConstantMemory::from_banks(&[ConstantBank { bank: 0, bytes: &data, known: Some(&known) }])
            .unwrap();
    assert!(preconditions.bind(&sparse).is_err());
    assert!(preconditions.bind(&ConstantMemory::new(&[(0, &data[..12])]).unwrap()).is_err());
    assert!(preconditions.bind(&ConstantMemory::new(&[(0, &data)]).unwrap()).is_ok());

    let unaligned = lift(&[mov(1, 5, 7), 0xef92000000070100, EXIT], &[], &[R(0)]);
    let kernel = unaligned.emit_wgsl(options(Profile::Rendering)).unwrap();
    assert_eq!(
        kernel
            .constant_preconditions
            .bind(&ConstantMemory::new(&[(0, &data)]).unwrap())
            .unwrap_err()
            .kind,
        DiagnosticKind::InvalidInput
    );
}

#[test]
fn uniform_addresses_remain_dynamic_in_both_profiles() {
    let program = lift(&[0x4c98078c00070001, 0xef94000000470100, EXIT], &[], &[R(0)]);
    let kernel = program.emit_wgsl(options(Profile::Rendering)).unwrap();
    validate(&kernel);
    assert_eq!(kernel.constant_preconditions.accesses().len(), 1);
    assert_eq!(kernel.constant_requirements.accesses().len(), 2);
    for address in [0u32, 16, 1000] {
        let constants = ConstantMemory::new(&[(3, &address.to_le_bytes()), (0, &[0; 32])]).unwrap();
        // Only c3[0] is promised to the shader. Its value is free to select a
        // dynamic c0 address, including an address that must fault on execution.
        assert!(kernel.constant_preconditions.bind(&constants).is_ok());
        assert_eq!(
            kernel.constant_requirements.bind(&constants).unwrap_err().kind,
            DiagnosticKind::Unsupported
        );
    }
}

pub(super) fn compare_profiles(gpu: &Gpu) -> usize {
    let mut comparisons = 0;
    for (kind, address, width, signed) in [
        (0, 3, 1, false),
        (1, 1, 1, true),
        (2, 6, 2, false),
        (3, 2, 2, true),
        (4, 12, 4, false),
        (5, 8, 8, false),
    ] {
        let load = 0xef90000000070100 | (kind << 48);
        let outputs = if width == 8 { vec![R(0), R(1)] } else { vec![R(0)] };
        let program = lift(&[mov(1, address, 7), load, EXIT], &[], &outputs);
        let kernels = [Profile::Checked, Profile::Rendering]
            .map(|profile| program.emit_wgsl(options(profile)).unwrap());
        for (seed, reorder) in [(129u8, false), (217, true)] {
            let data = (0..32u8)
                .map(|index| seed.wrapping_add(index.wrapping_mul(37)))
                .collect::<Vec<_>>();
            let mut known = [false; 32];
            known[address as usize..address as usize + width].fill(true);
            let bank = ConstantBank { bank: 0, bytes: &data, known: Some(&known) };
            let extra = ConstantBank { bank: 7, bytes: &[0xaa; 17], known: None };
            let banks = if reorder { [extra, bank] } else { [bank, extra] };
            let constants = ConstantMemory::from_banks(&banks).unwrap();
            let bytes = &data[address as usize..address as usize + width];
            let expected = match (width, signed) {
                (1, false) => vec![u32::from(bytes[0])],
                (1, true) => vec![bytes[0] as i8 as i32 as u32],
                (2, false) => vec![u32::from(u16::from_le_bytes(bytes.try_into().unwrap()))],
                (2, true) => vec![i16::from_le_bytes(bytes.try_into().unwrap()) as i32 as u32],
                _ => bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect(),
            };
            let input = Inputs { constants, ..Default::default() };
            for kernel in &kernels {
                let result = gpu.run(kernel, std::slice::from_ref(&input));
                assert_eq!(&result[0][..outputs.len()], expected);
                assert_eq!(&result[0][outputs.len()..], [0, 0, 0]);
                comparisons += 1;
            }
        }
    }

    // Changing a uniform-held address must retain range/alignment/validity
    // faults at the original PC, after the fixed c3 read moves to the CPU.
    let program = lift(&[0x4c98078c00070001, 0xef94000000470100, EXIT], &[], &[R(0)]);
    let kernels = [Profile::Checked, Profile::Rendering]
        .map(|profile| program.emit_wgsl(options(profile)).unwrap());
    for (address, known, status) in [(0u32, true, 0), (31, true, 2), (1, true, 4), (0, false, 8)] {
        let address_bytes = address.to_le_bytes();
        let validity = [known; 32];
        let constants = ConstantMemory::from_banks(&[
            ConstantBank { bank: 3, bytes: &address_bytes, known: None },
            ConstantBank { bank: 0, bytes: &[0x5a; 32], known: Some(&validity) },
        ])
        .unwrap();
        let input = Inputs { constants, ..Default::default() };
        for kernel in &kernels {
            let result = gpu.run(kernel, std::slice::from_ref(&input));
            assert_eq!(result[0][1], status);
            assert_eq!(result[0][2], if status == 0 { 0 } else { offset(1) as u32 });
            if status == 0 {
                assert_eq!(result[0][0], 0x5a5a5a5a);
            }
            comparisons += 1;
        }
    }
    comparisons
        + compare_dynamic_profiles(gpu)
        + compare_malformed_packed_buffers(gpu)
        + alignment::compare_profiles(gpu)
}

fn compare_dynamic_profiles(gpu: &Gpu) -> usize {
    // A dynamic-only program has no fixed-span preconditions. Rendering may
    // trust the snapshot layout, but must still diagnose every unavailable
    // address and byte, including padding at a partial final word.
    let data = (0..37u8).map(|byte| byte.wrapping_mul(37).wrapping_add(129)).collect::<Vec<_>>();
    let known = (0..data.len()).map(|byte| ![3, 31, 36].contains(&byte)).collect::<Vec<_>>();
    let snapshots = [
        ConstantMemory::default(),
        ConstantMemory::new(&[(0, &data)]).unwrap(),
        ConstantMemory::from_banks(&[
            ConstantBank { bank: 7, bytes: &[0xaa; 17], known: None },
            ConstantBank { bank: 0, bytes: &data, known: Some(&known) },
        ])
        .unwrap(),
    ];
    let mut comparisons = 0;
    for (kind, width, signed) in
        [(0, 1, false), (1, 1, true), (2, 2, false), (3, 2, true), (4, 4, false), (5, 8, false)]
    {
        let outputs = if width == 8 { vec![R(0), R(1)] } else { vec![R(0)] };
        let program = lift(&[0xef90000000070100 | (kind << 48), EXIT], &[R(1)], &outputs);
        let kernels = [Profile::Checked, Profile::Rendering]
            .map(|profile| program.emit_wgsl(options(profile)).unwrap());
        for kernel in &kernels {
            validate(kernel);
            assert!(kernel.constant_preconditions.accesses().is_empty());
        }
        for (snapshot, constants) in snapshots.iter().enumerate() {
            let addresses = (0..46).chain([u32::MAX - 1, u32::MAX]).map(Some).chain([None]);
            let cases = addresses
                .map(|address| Inputs {
                    registers: address.into_iter().map(|v| (R(1), Value::Word(v))).collect(),
                    constants: constants.clone(),
                    ..Default::default()
                })
                .collect::<Vec<_>>();
            for kernel in &kernels {
                let results = gpu.run(kernel, &cases);
                for (input, result) in cases.iter().zip(&results) {
                    let address = input.registers.get(&R(1)).map(|value| match value {
                        Value::Word(word) => *word as usize,
                        _ => unreachable!(),
                    });
                    // Fault precedence is part of the diagnostic contract:
                    // incoming availability, bank presence, bounds, alignment,
                    // then byte validity. A full LDC.64 spans both results.
                    let status = match address {
                        None => 32,
                        Some(_) if snapshot == 0 => 1,
                        Some(start) if start.saturating_add(width) > data.len() => 2,
                        Some(start) if start % width != 0 => 4,
                        Some(start)
                            if snapshot == 2 && known[start..start + width].contains(&false) =>
                        {
                            8
                        }
                        Some(_) => 0,
                    };
                    assert_eq!(
                        result[outputs.len()],
                        status,
                        "address {address:?}, width {width}, snapshot {snapshot}"
                    );
                    assert_eq!(
                        result[outputs.len() + 1],
                        if status == 0 { 0 } else { offset(0) as u32 }
                    );
                    if status == 0 {
                        let start = address.unwrap();
                        let bytes = &data[start..start + width];
                        let expected = match (width, signed) {
                            (1, false) => vec![u32::from(bytes[0])],
                            (1, true) => vec![bytes[0] as i8 as i32 as u32],
                            (2, false) => {
                                vec![u32::from(u16::from_le_bytes(bytes.try_into().unwrap()))]
                            }
                            (2, true) => {
                                vec![i16::from_le_bytes(bytes.try_into().unwrap()) as i32 as u32]
                            }
                            _ => bytes
                                .chunks_exact(4)
                                .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
                                .collect(),
                        };
                        assert_eq!(&result[..outputs.len()], expected);
                    }
                    comparisons += 1;
                }
            }
        }
    }
    comparisons
}

fn compare_malformed_packed_buffers(gpu: &Gpu) -> usize {
    use wgpu::util::DeviceExt;

    let program = lift(&[0xef94000000070100, EXIT], &[R(1)], &[R(0)]);
    let mut kernel = program.emit_wgsl(options(Profile::Checked)).unwrap();
    // Deliberately bypass the harness's typed upload for the CHECKED kernel.
    // Rendering's snapshot loader must never receive these invalid buffers.
    let binding = kernel.constant_memory.take().unwrap();
    let input = Inputs { registers: [(R(1), Value::Word(0))].into(), ..Default::default() };
    let malformed = [
        (63, 0, 0),           // truncated header
        (64, 63, 4),          // payload inside header
        (64, 65, 4),          // payload beyond allocation
        (65, u32::MAX, 4),    // wrapping payload arithmetic
        (65, 64, 65537),      // oversized native bank
        (65, 64, 8),          // truncated payload
        (65, 64, 0x80000004), // missing validity bitmap
        (75, 64, 0x80000025), // truncated two-word validity bitmap
    ];
    for (count, start, length) in malformed {
        let mut words = vec![0u32; count];
        words[0] = start;
        words[1] = length;
        let result =
            gpu.run_with_resources(&kernel, std::slice::from_ref(&input), |device, _, pipeline| {
                let bytes = words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("malformed checked-mode snapshot"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(binding.group),
                    entries: &[wgpu::BindGroupEntry {
                        binding: binding.binding,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                vec![(binding.group, group)]
            });
        assert_eq!(
            result[0],
            [0, 1, offset(0) as u32, 0],
            "malformed header {count}/{start}/{length}"
        );
    }
    malformed.len()
}
