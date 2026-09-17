use super::{bytes, lift, mov, offset, EXIT};
use crate::{
    ir::{Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Stage, Value},
    resources::{ConstantBank, ConstantMemory},
};

fn ldc(destination: u8, address: u8, displacement: i16, type_code: u8) -> u64 {
    0xef90003000070000
        | u64::from(destination)
        | (u64::from(address) << 8)
        | (u64::from(displacement as u16) << 20)
        | (u64::from(type_code) << 48)
}

fn memory(bytes: &[u8]) -> ConstantMemory { ConstantMemory::new(&[(3, bytes)]).unwrap() }

#[test]
fn subword_constant_loads_extend_the_requested_value() {
    for (ty, address, expected) in
        [(0, 1, 0x80), (1, 1, 0xffffff80), (2, 2, 0x80ff), (3, 2, 0xffff80ff), (4, 4, 0x11223344)]
    {
        let program = lift(&[ldc(0, 255, address, ty), EXIT], &[], &[Register::Gpr(0)]);
        let inputs = Inputs {
            constants: memory(&[0, 0x80, 0xff, 0x80, 0x44, 0x33, 0x22, 0x11]),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn wide_load_captures_its_aliased_address_before_both_writes() {
    use Register::Gpr as R;
    let program = lift(&[ldc(0, 0, -8, 5), EXIT], &[R(0)], &[R(0), R(1)]);
    let mut data = vec![0u8; 8];
    data.extend(0xdeadbeefu32.to_le_bytes());
    data.extend(0xfeedbeefu32.to_le_bytes());
    let inputs = Inputs {
        registers: [(R(0), Value::Word(16))].into(),
        constants: memory(&data),
        ..Default::default()
    };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(0xdeadbeef), Value::Word(0xfeedbeef)]
    );
    let program = lift(&[ldc(0, 0, 1, 0), EXIT], &[R(0)], &[R(0)]);
    let inputs = Inputs {
        registers: [(R(0), Value::Word(u32::MAX))].into(),
        constants: memory(&[19]),
        ..Default::default()
    };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(19)]
    );
}

#[test]
fn direct_and_indexed_reads_use_the_same_constant_memory() {
    use Register::Gpr as R;
    let direct = 0x4c98078c00070000; // MOV R0,c[3][0]
    let program = lift(&[direct, ldc(1, 255, 0, 4), EXIT], &[], &[R(0), R(1)]);
    let inputs = Inputs { constants: memory(&0x7fc12345u32.to_le_bytes()), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(0x7fc12345); 2]
    );
}

#[test]
fn a_discarded_load_still_requires_an_established_address_and_memory() {
    use Register::Gpr as R;
    let program = lift(&[ldc(255, 0, 0, 5), EXIT], &[], &[]);
    let error = program.interpret(&Inputs::default(), 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::UnavailableValue, Some(8)));
    let program = lift(&[ldc(255, 0, 0, 5), EXIT], &[R(0)], &[]);
    let mut inputs = Inputs { registers: [(R(0), Value::Word(0))].into(), ..Default::default() };
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
    inputs.constants = memory(&[0; 8]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        []
    );
}

#[test]
fn sparse_bytes_and_padding_do_not_satisfy_a_wider_read() {
    let data = [0, 0x81, 0, 0, 0xff];
    let known = [false, true, false, false, true];
    let inputs = Inputs {
        constants: ConstantMemory::from_banks(&[ConstantBank {
            bank: 3,
            bytes: &data,
            known: Some(&known),
        }])
        .unwrap(),
        ..Default::default()
    };
    let program = lift(&[ldc(0, 255, 1, 0), EXIT], &[], &[Register::Gpr(0)]);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(0x81)]
    );
    for (address, ty) in [(0, 4), (4, 4), (0, 5)] {
        let program = lift(&[ldc(0, 255, address, ty), EXIT], &[], &[Register::Gpr(0)]);
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));
    }
    let program = lift(&[ldc(0, 255, 0, 5), EXIT], &[], &[Register::Gpr(0)]);
    let inputs = Inputs { constants: memory(&[1, 2, 3, 4]), ..Default::default() };
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
}

#[test]
fn predication_guards_the_memory_access_and_alignment_is_not_silently_repaired() {
    use Register::{Gpr as R, Predicate as P};
    let program =
        lift(&[mov(0, 17, 7), ldc(0, 1, 0, 4) & !(7 << 16), EXIT], &[P(0), R(1)], &[R(0)]);
    let mut inputs =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(17)]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
    inputs.registers.insert(R(1), Value::Word(1));
    inputs.constants = memory(&[0; 8]);
    let error = program.interpret(&inputs, 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::InvalidInput, Some(offset(1))));
}

#[test]
fn sample_index_can_address_a_supplied_byte_table() {
    let words = bytes(&[0xefe8e0028007ff00, ldc(0, 0, 0, 0), EXIT]);
    let program = Program::lift(MaxwellCode::parse(&words).unwrap(), &Signature {
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        outputs: vec![Register::Gpr(0)],
        ..Default::default()
    })
    .unwrap();
    for (index, expected) in [0xcc, 0xc4, 0x4c, 0x44].into_iter().enumerate() {
        let inputs = Inputs {
            sample_index: Some(index as u32),
            constants: memory(&[0xcc, 0xc4, 0x4c, 0x44]),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn unsupported_memory_modes_fail_before_interpretation() {
    for word in [
        ldc(0, 255, 0, 6),
        ldc(0, 255, 0, 7),
        ldc(254, 255, 0, 5),
        ldc(0, 255, 0, 4) | (1 << 44),
        ldc(0, 255, 0, 4) | (2 << 44),
        ldc(0, 255, 0, 4) | (1 << 46),
    ] {
        let error = Program::lift(
            MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(),
            &Signature::default(),
        )
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

#[test]
fn verifier_rejects_invalid_memory_spans_and_address_types() {
    let function = lift(&[ldc(0, 255, 0, 4), EXIT], &[], &[Register::Gpr(0)]).function;
    for change in 0..5 {
        let mut bad = function.clone();
        let predicate = bad.value(Type::Predicate, Definition::Predicate(true));
        let operation = &mut bad.blocks[1].instructions[1].operation;
        let Operation::LoadConstantWord { bank, bytes, part, signed, address } = operation else {
            panic!()
        };
        match change {
            0 => *bank = 32,
            1 => *bytes = 3,
            2 => *part = 4,
            3 => *signed = true,
            _ => *address = predicate,
        }
        assert_eq!(bad.verify().unwrap_err().kind, DiagnosticKind::InvalidIr);
    }
}
