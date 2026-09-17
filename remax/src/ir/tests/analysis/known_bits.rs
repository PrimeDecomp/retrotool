use crate::{
    ir::{
        analysis::{Analysis, Bits},
        ssa::Builder,
        Definition, Edge, Logic, Operation, Terminator, Type,
    },
    program::Register,
};

/// All ternary bit patterns over four selected positions, with other bits
/// fixed. Every represented concrete value is enumerated independently.
fn patterns(positions: [u32; 4], fixed: u32) -> Vec<(Bits, Vec<u32>)> {
    let varying = positions.iter().fold(0, |mask, &bit| mask | (1 << bit));
    (0..81)
        .map(|mut code| {
            let mut zero = !fixed & !varying;
            let mut one = fixed & !varying;
            for bit in positions {
                match code % 3 {
                    0 => zero |= 1 << bit,
                    1 => one |= 1 << bit,
                    _ => {}
                }
                code /= 3;
            }
            let bits = Bits::from_masks(zero, one);
            let words = (0..16)
                .map(|choice| {
                    positions.iter().enumerate().fold(fixed & !varying, |word, (index, &bit)| {
                        word | (((choice >> index) & 1) << bit)
                    })
                })
                .filter(|&word| bits.contains(word))
                .collect();
            (bits, words)
        })
        .collect()
}

#[test]
fn bit_transfers_contain_every_concrete_arithmetic_result() {
    for (positions, fixed) in
        [([0, 1, 2, 3], 0), ([0, 1, 2, 3], u32::MAX), ([0, 8, 17, 31], 0xa5a55a5a)]
    {
        let patterns = patterns(positions, fixed);
        for (a, av) in &patterns {
            for (b, bv) in &patterns {
                let results = [a.add(*b), a.multiply(*b), a.and(*b), a.or(*b), a.xor(*b)];
                for &a in av {
                    for &b in bv {
                        let concrete = [a.wrapping_add(b), a.wrapping_mul(b), a & b, a | b, a ^ b];
                        for (fact, word) in results.iter().zip(concrete) {
                            assert!(
                                fact.contains(word),
                                "{fact:?} excludes {word:#x} from {a:#x}, {b:#x}"
                            );
                        }
                    }
                }
                for &word in av.iter().chain(bv) {
                    assert!(a.join(*b).contains(word));
                }
            }
        }
    }
}

#[test]
fn shifts_and_fields_preserve_wrapping_counts_and_sign_padding() {
    let amounts = [0, 1, 7, 16, 31, 32, 33, 63, u32::MAX];
    for (bits, words) in patterns([0, 8, 17, 31], 0x7fff00ff) {
        for word in words {
            assert!(bits.not().contains(!word));
            assert!(bits.reverse().contains(word.reverse_bits()));
            assert!(bits.leading_zeros().contains(word.leading_zeros()));
            for amount in amounts {
                for right in [false, true] {
                    for signed in [false, true] {
                        for wrap in [false, true] {
                            let count = if wrap { amount & 31 } else { amount };
                            let expected = match (right, signed, count) {
                                (true, true, _) => ((word as i32) >> count.min(31)) as u32,
                                (_, _, 32..) => 0,
                                (true, false, _) => word >> count,
                                (false, _, _) => word << count,
                            };
                            assert!(bits.shift(amount, right, signed, wrap).contains(expected));
                        }
                    }
                }
                for count in amounts {
                    for signed in [false, true] {
                        let sign = if count == 0 {
                            0
                        } else {
                            (word >> amount.saturating_add(count - 1).min(31)) & 1
                        };
                        let expected = (0..32).fold(0, |result, bit| {
                            let source = u64::from(amount) + bit;
                            let value = if bit < u64::from(count) && source < 32 {
                                (word >> source) & 1
                            } else if signed && count != 0 {
                                sign
                            } else {
                                0
                            };
                            result | (value << bit)
                        });
                        assert!(
                            bits.extract(amount, count, signed).contains(expected),
                            "field offset {amount}, count {count}, signed {signed}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn alignment_facts_survive_modular_record_addressing() {
    let records = Bits::UNKNOWN.multiply(Bits::constant(80));
    assert!(records.is_aligned(8));
    assert!(!records.add(Bits::constant(4)).is_aligned(8));
    assert!(records.add(Bits::constant(4)).is_aligned(4));
    let wrapped = Bits::constant(0xfffffff0).add(Bits::unsigned_range(16, 23));
    for word in 0..8 {
        assert!(wrapped.contains(word));
    }
    assert!(!wrapped.contains(8));
    assert_eq!(Bits::UNKNOWN.and(Bits::constant(0)).exact(), Some(0));
    assert_eq!(Bits::UNKNOWN.or(Bits::constant(u32::MAX)).exact(), Some(u32::MAX));
}

#[test]
fn loop_joins_revoke_initial_facts_but_preserve_inductive_alignment() {
    use Register::{Gpr as R, Predicate as P};
    for stride in [4, 8] {
        let mut builder = Builder::new(&[R(1), P(0)], &[R(0)]);
        let entry = builder.block(None);
        let body = builder.block(Some(8));
        let end = builder.block(Some(16));
        let initial = builder.word(0);
        let increment = builder.word(stride);
        let condition = builder.read(entry, P(0));
        let unknown = builder.read(entry, R(1));
        let parameter =
            builder.function.value(Type::Bits32, Definition::Parameter { block: body, index: 0 });
        builder.function.blocks[body.0].parameters.push(parameter);
        builder.function.blocks[entry.0].terminator =
            Some(Terminator::Jump(Edge::new(body, vec![initial])));
        let next =
            builder.function.append(body, 8, Type::Bits32, Operation::Add(parameter, increment));
        builder.function.blocks[body.0].terminator = Some(Terminator::Branch {
            kind: crate::ir::convergence::BranchKind::ControlFlow,
            condition,
            yes: Edge::new(body, vec![next]),
            no: Edge::new(end, vec![]),
        });
        builder.function.blocks[end.0].terminator = Some(Terminator::Return(vec![parameter]));
        let mut function = builder.function;
        function.verify().unwrap();
        let bits = Analysis::new(&function).bits.get(parameter);
        assert!(bits.exact().is_none());
        assert!(bits.is_aligned(stride as u8));
        assert!(!bits.is_aligned(stride as u8 * 2));
        for value in [0u32, 1, 2, 127, 0x1fffffff, u32::MAX] {
            assert!(bits.contains(value.wrapping_mul(stride)));
        }
        function.blocks[body.0].terminator.as_mut().unwrap().edges_mut()[0].arguments[0] = unknown;
        function.verify().unwrap();
        assert_eq!(Analysis::new(&function).bits.get(parameter), Bits::UNKNOWN);
    }
}

#[test]
fn numeric_facts_do_not_establish_incoming_availability() {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[], &[R(0)]);
    let block = builder.block(None);
    let missing = builder.read(block, R(1));
    let zero = builder.word(0);
    let value = builder.function.append(block, 8, Type::Bits32, Operation::Logic {
        op: Logic::And,
        a: missing,
        b: zero,
    });
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(vec![value]));
    let function = builder.finish().0;
    function.verify().unwrap();
    let analysis = Analysis::new(&function);
    assert_eq!(analysis.bits.get(value).exact(), Some(0));
    assert!(!analysis.availability.is_available(value));
}
