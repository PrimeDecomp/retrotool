use super::{options, Gpu, Profile, P, R};
use crate::{
    ir::{ssa::Builder, Operation, Terminator, Type},
    program::{Inputs, Program, Value},
    resources::{ConstantBank, ConstantMemory},
};

const LOAD_PC: usize = 32;

/// Two joined field addresses in a record array. A zero stride also exercises
/// the distinction between knowing an address's bits and having its inputs.
fn record_load(stride: u32, alternative: u32) -> Program {
    let mut builder = Builder::new(&[R(1), P(0)], &[R(0), R(1)]);
    let block = builder.block(Some(8));
    let index = builder.read(block, R(1));
    let condition = builder.read(block, P(0));
    let stride = builder.word(stride);
    let zero = builder.word(0);
    let alternative = builder.word(alternative);
    let base = builder.function.append(block, 8, Type::Bits32, Operation::Multiply(index, stride));
    let field = builder.function.append(block, 16, Type::Bits32, Operation::Select {
        condition,
        yes: zero,
        no: alternative,
    });
    let address = builder.function.append(block, 24, Type::Bits32, Operation::Add(base, field));
    let outputs = [0, 4]
        .map(|part| {
            builder.function.append(block, LOAD_PC, Type::Bits32, Operation::LoadConstantWord {
                bank: 0,
                address,
                bytes: 8,
                part,
                signed: false,
            })
        })
        .to_vec();
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(outputs));
    builder.function.verify().unwrap();
    Program { function: builder.function }
}

#[test]
fn aligned_loads_omit_only_the_proven_check() {
    for (stride, alternative, aligned) in [(80, 8, true), (80, 4, false), (0, 0, true)] {
        for profile in [Profile::Checked, Profile::Rendering] {
            let kernel = record_load(stride, alternative).emit_wgsl(options(profile)).unwrap();
            super::validate(&kernel);
            assert!(kernel.constant_preconditions.accesses().is_empty());
            let body = kernel.source.split("fn remax_main(").nth(1).unwrap();
            let layout = profile == Profile::Checked;
            assert!(body.contains(&format!("8u, {layout}, {}, &memory_status)", !aligned)));
        }
    }
}

pub(super) fn compare_profiles(gpu: &Gpu) -> usize {
    let data = (0..192u8).map(|byte| byte.wrapping_mul(37).wrapping_add(129)).collect::<Vec<_>>();
    let known = (0..data.len()).map(|byte| ![7, 80, 87].contains(&byte)).collect::<Vec<_>>();
    let snapshots = [
        ConstantMemory::default(),
        ConstantMemory::new(&[(0, &data)]).unwrap(),
        ConstantMemory::from_banks(&[ConstantBank { bank: 0, bytes: &data, known: Some(&known) }])
            .unwrap(),
    ];
    let indices = [0, 1, 2, 3, 0x10000000, 0x33333334, u32::MAX];
    let mut comparisons = 0;
    for (stride, alternative) in [(80u32, 8), (80, 4), (0, 0)] {
        let program = record_load(stride, alternative);
        let kernels = [Profile::Checked, Profile::Rendering]
            .map(|profile| program.emit_wgsl(options(profile)).unwrap());
        for (snapshot, constants) in snapshots.iter().enumerate() {
            let mut cases = Vec::new();
            let mut expectations = Vec::new();
            for index in indices.into_iter().map(Some).chain([None]) {
                for condition in [Some(true), Some(false), None] {
                    let registers = index
                        .map(|word| (R(1), Value::Word(word)))
                        .into_iter()
                        .chain(condition.map(|value| (P(0), Value::Predicate(value))))
                        .collect();
                    cases.push(Inputs {
                        registers,
                        constants: constants.clone(),
                        ..Default::default()
                    });
                    let address = index.zip(condition).map(|(index, condition)| {
                        index.wrapping_mul(stride).wrapping_add(if condition {
                            0
                        } else {
                            alternative
                        }) as usize
                    });
                    let status = match address {
                        None => 32,
                        Some(_) if snapshot == 0 => 1,
                        Some(start) if start.saturating_add(8) > data.len() => 2,
                        Some(start) if start % 8 != 0 => 4,
                        Some(start)
                            if snapshot == 2 && known[start..start + 8].contains(&false) =>
                        {
                            8
                        }
                        Some(_) => 0,
                    };
                    expectations.push((address, status));
                }
            }
            for kernel in &kernels {
                let results = gpu.run(kernel, &cases);
                for (result, (address, status)) in results.iter().zip(&expectations) {
                    assert_eq!(
                        result[2], *status,
                        "record ({stride}, {alternative}), address {address:?}, snapshot {snapshot}"
                    );
                    assert_eq!(result[3], if *status == 0 { 0 } else { LOAD_PC as u32 });
                    if *status == 0 {
                        let start = address.unwrap();
                        let words = data[start..start + 8]
                            .chunks_exact(4)
                            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                            .collect::<Vec<_>>();
                        assert_eq!(result[..2], words);
                    }
                    comparisons += 1;
                }
            }
        }
    }
    comparisons
}
