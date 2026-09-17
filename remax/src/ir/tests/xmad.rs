use super::{bytes, lift, EXIT};
use crate::{
    ir::{Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Value},
    resources::ConstantMemory,
};

fn register(destination: u8, a: u8, b: u8, c: u8) -> u64 {
    0x5b00000000070000
        | u64::from(destination)
        | (u64::from(a) << 8)
        | (u64::from(b) << 20)
        | (u64::from(c) << 39)
}

#[test]
fn original_xmad_packing_preserves_the_aliased_accumulator() {
    use Register::Gpr as R;
    // XMAD.PSL.CLO R0,R1,1,R0. Every possible driver sample-table byte.
    let program = lift(&[0x3604001000170100, EXIT], &[R(0), R(1)], &[R(0)]);
    for byte in 0..=255 {
        let inputs = Inputs {
            registers: [(R(0), Value::Word(byte << 8)), (R(1), Value::Word(byte << 4))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word((byte << 20) | (byte << 8))]
        );
    }
}

#[test]
fn three_partial_products_reconstruct_wrapping_word_multiplication() {
    use Register::Gpr as R;
    // Low*low + (high*low + low*high)<<16. The accumulator aliases the
    // destination in the last two instructions. High*high cannot affect low32.
    let program = lift(
        &[
            register(0, 1, 2, 255),
            register(0, 1, 2, 0) | (1 << 53) | (1 << 36),
            register(0, 1, 2, 0) | (1 << 35) | (1 << 36),
            EXIT,
        ],
        &[R(1), R(2)],
        &[R(0)],
    );
    let mut words = vec![0, 1, 0x7fff, 0x8000, 0xffff, 0x10000, 0x80000000, u32::MAX];
    let mut seed = 0xc01dfaceu32;
    for _ in 0..24 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        words.push(seed);
    }
    for &a in &words {
        for &b in &words {
            let inputs = Inputs {
                registers: [(R(1), Value::Word(a)), (R(2), Value::Word(b))].into(),
                ..Default::default()
            };
            assert_eq!(
                program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
                [Value::Word(a.wrapping_mul(b))],
                "{a:08x} * {b:08x}"
            );
        }
    }
}

#[test]
fn signed_halfwords_do_not_sign_extend_the_accumulator_or_merge_source() {
    use Register::Gpr as R;
    // A.hi=2, B.hi=-3/65533, B.lo=5, C=0xfffffff0.
    // CBCC and MRG consume B.lo even when multiplication selects B.hi.
    for (mode, signed_b, merge, expected) in [
        (0, false, false, 0x0001ffea),
        (1, false, false, 0x0002ffea),
        (2, false, false, 0x0002fff9),
        (4, false, false, 0x0006ffea),
        (4, true, false, 0x0004ffea),
        (4, true, true, 0x0005ffea),
    ] {
        let word = register(4, 5, 6, 7)
            | (1 << 53)
            | (1 << 35)
            | (mode << 50)
            | (u64::from(signed_b) << 49)
            | (u64::from(merge) << 37);
        let program = lift(&[word, EXIT], &[R(5), R(6), R(7)], &[R(4)]);
        let inputs = Inputs {
            registers: [
                (R(5), Value::Word(0x00020003)),
                (R(6), Value::Word(0xfffd0005)),
                (R(7), Value::Word(0xfffffff0)),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)],
            "{word:016x}"
        );
    }
    // Both negative low halves: (-32768) * (-1), zero accumulator.
    let program = lift(&[register(0, 1, 2, 255) | (3 << 48), EXIT], &[R(1), R(2)], &[R(0)]);
    let inputs = Inputs {
        registers: [(R(1), Value::Word(0x12348000)), (R(2), Value::Word(0x5678ffff))].into(),
        ..Default::default()
    };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(32768)]
    );
}

#[test]
fn immediate_and_constant_forms_read_their_distinct_operand_positions() {
    use Register::Gpr as R;
    let mut bank = [0u8; 0x28];
    bank[0x24..].copy_from_slice(&11u32.to_le_bytes());
    let inputs = Inputs {
        registers: [(R(5), Value::Word(3)), (R(7), Value::Word(42))].into(),
        constants: ConstantMemory::new(&[(3, &bank)]).unwrap(),
        ..Default::default()
    };
    for (word, expected) in [
        (0x3602038ffff70504, 39),  // 3 * signed immediate -1 + 42
        (0x5100038c00970504, 137), // 3 * 42 + c3[0x24]
        (0x4e00038c00970504, 75),  // 3 * c3[0x24] + 42
    ] {
        let program = lift(&[word, EXIT], &[R(5), R(7)], &[R(4)]);
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn predication_guards_xmad_constant_reads_and_keeps_incoming_destination() {
    use Register::{Gpr as R, Predicate as P};
    let word = 0x5100038c00970504 & !(7 << 16);
    for destination in [4, 255] {
        let program = lift(&[(word & !255) | destination, EXIT], &[R(4), P(0)], &[R(4)]);
        let mut inputs = Inputs {
            registers: [(R(4), Value::Word(17)), (P(0), Value::Predicate(false))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(17)]
        );
        inputs.registers.insert(P(0), Value::Predicate(true));
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));
    }
}

#[test]
fn unresolved_xmad_modes_and_side_effects_are_rejected() {
    for word in [
        0x360403cffff70504,               // .X
        0x360c038ffff70504,               // .CSFU
        register(0, 1, 2, 3) | (1 << 47), // .CC
        register(0, 1, 2, 3) | (5 << 50), // unknown mode
        register(0, 1, 2, 3) | (1 << 28), // unrecognized bit
        0x5100038c00970504 | (1 << 54),   // constant-C .X
        0x4e00038c00970504 | (1 << 54),   // constant-B .X
    ] {
        let error = Program::lift(
            MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
            &Signature::default(),
        )
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)), "{word:016x}");
    }
}

#[test]
fn verifier_checks_product_operand_types_independently() {
    let mut function = lift(&[register(0, 1, 2, 255), EXIT], &[], &[]).function;
    let predicate = function.value(Type::Predicate, Definition::Predicate(false));
    let product = function
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::Multiply(..)))
        .unwrap();
    let Operation::Multiply(a, _) = &mut product.operation else { unreachable!() };
    *a = predicate;
    assert!(function.verify().unwrap_err().message.contains("Bits32"));
}
