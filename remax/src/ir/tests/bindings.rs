use super::{add, lift, mov, offset, EXIT};
use crate::{
    program::{bindings::ConstantAccess, DiagnosticKind, Inputs, Register, Value},
    resources::{ConstantBank, ConstantMemory},
};

#[test]
fn exact_integer_addresses_preserve_wrapping_and_full_load_spans() {
    use Register::Gpr as R;
    // MOV 0xfffffffc; IADD 4; LDC.64 R0,c0[R1+8]. The two results each
    // validate eight bytes starting at offset8, and produce one manifest entry.
    let program = lift(&[mov(1, u32::MAX - 3, 7), add(1, 1, 4), 0xef95000000870100, EXIT], &[], &[
        R(0),
        R(1),
    ]);
    let requirements = program.constant_requirements();
    assert_eq!(requirements.accesses(), &[ConstantAccess {
        pc: offset(2),
        bank: 0,
        byte_offset: Some(8),
        byte_count: 8
    }]);
    let bytes = [1; 16];
    let mut known = [true; 16];
    known[15] = false;
    let partial =
        ConstantMemory::from_banks(&[ConstantBank { bank: 0, bytes: &bytes, known: Some(&known) }])
            .unwrap();
    let report = requirements.check(&partial);
    assert!(!report.is_complete());
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].pc, Some(offset(2)));
    assert!(requirements.check(&ConstantMemory::new(&[(0, &bytes)]).unwrap()).is_complete());
    assert!(!requirements.check(&ConstantMemory::new(&[(0, &bytes[..12])]).unwrap()).is_complete());
}

#[test]
fn known_uniform_contents_are_runtime_addresses_not_specializations() {
    use Register::Gpr as R;
    let program = lift(&[0x4c98078c00070001, 0xef94000000470100, EXIT], &[], &[R(0)]);
    let requirements = program.constant_requirements();
    for address in [0u32, 16, 1000] {
        let memory = ConstantMemory::new(&[(3, &address.to_le_bytes()), (0, &[0; 32])]).unwrap();
        let report = requirements.check(&memory);
        assert!(!report.is_complete());
        assert!(report.failures.is_empty());
        assert_eq!(report.validated_accesses, 1);
        assert_eq!(report.dynamic_accesses, [ConstantAccess {
            pc: offset(1),
            bank: 0,
            byte_offset: None,
            byte_count: 4
        }]);
    }
}

#[test]
fn preflight_reports_guarded_failures_without_claiming_they_execute() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(&[mov(0, 7, 7), 0x4c98078c00000000, EXIT], &[P(0)], &[R(0)]);
    let report = program.constant_requirements().check(&ConstantMemory::default());
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].pc, Some(offset(1)));
    let inputs =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(7)]
    );
}

#[test]
fn subword_loads_validate_exact_bytes_and_instruction_alignment() {
    use Register::Gpr as R;
    for (load, address, bytes, aligned) in [
        (0xef90000000070100, 5, 1, true),
        (0xef92000000070100, 4, 2, true),
        (0xef92000000070100, 5, 2, false),
    ] {
        let program = lift(&[mov(1, address, 7), load, EXIT], &[], &[R(0)]);
        let requirements = program.constant_requirements();
        assert_eq!(requirements.accesses()[0].byte_count, bytes);
        let memory = ConstantMemory::new(&[(0, &[0; 7])]).unwrap();
        let report = requirements.check(&memory);
        assert_eq!(report.is_complete(), aligned);
        if !aligned {
            assert_eq!(report.failures[0].kind, DiagnosticKind::InvalidInput);
        }
    }
}
