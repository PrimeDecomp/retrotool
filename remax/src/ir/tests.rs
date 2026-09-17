use super::{
    ssa::Builder, BlockId, Definition, Edge, Function, Operation, Relation, Terminator, Type,
};
use crate::{
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Value},
    resources::ConstantMemory,
};

mod analysis;
mod attributes;
mod bindings;
mod bits;
mod control;
mod conversion;
mod demote;
mod float;
mod global_memory;
mod indexed_attributes;
mod integer;
mod interpolation;
mod local_memory;
mod memory;
mod minmax;
mod multiply;
mod optimize;
mod probe;
mod raster;
mod reduction;
mod rounding;
mod special;
mod texture;
mod transport;
mod trigonometry;
mod wgsl;
mod xmad;

const EXIT: u64 = 0xe30000000007000f;

fn offset(index: usize) -> usize { index / 3 * 32 + (index % 3 + 1) * 8 }

fn bytes(words: &[u64]) -> Vec<u8> {
    words
        .chunks(3)
        .flat_map(|g| {
            [
                0x001f8000fc0007e0,
                g[0],
                *g.get(1).unwrap_or(&0x50b0000000070f00),
                *g.get(2).unwrap_or(&0x50b0000000070f00),
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes)
        })
        .collect()
}

fn mov(destination: u8, value: u32, predicate: u8) -> u64 {
    0x010000000000f000
        | (u64::from(value) << 20)
        | (u64::from(predicate) << 16)
        | u64::from(destination)
}

fn add(destination: u8, source: u8, immediate: u32) -> u64 {
    0x1c00000000070000
        | (u64::from(immediate) << 20)
        | (u64::from(source) << 8)
        | u64::from(destination)
}

fn isetp(
    destination: u8,
    secondary: u8,
    a: u8,
    b: u8,
    relation: u8,
    accumulator: u8,
    execution: u8,
) -> u64 {
    0x5b60000000000000
        | (u64::from(relation) << 49)
        | (u64::from(accumulator) << 39)
        | (u64::from(b) << 20)
        | (u64::from(execution) << 16)
        | (u64::from(a) << 8)
        | (u64::from(destination) << 3)
        | u64::from(secondary)
}

fn branch(from: usize, to: usize, predicate: u8) -> u64 {
    let delta = offset(to) as i64 - offset(from) as i64 - 8;
    0xe24000000000000f | ((delta as u64 & 0xffffff) << 20) | (u64::from(predicate) << 16)
}

fn lift(words: &[u64], inputs: &[Register], outputs: &[Register]) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        indexed_attributes: vec![],
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn predicated_write_merges_with_the_incoming_bits() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(&[mov(0, 0x7fc12345, 0), EXIT], &[R(0), P(0)], &[R(0)]);
    assert!(program.function.blocks.iter().any(|b| !b.parameters.is_empty()));
    for enabled in [false, true] {
        let inputs = Inputs {
            registers: [(R(0), Value::Word(0x80000000)), (P(0), Value::Predicate(enabled))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(if enabled { 0x7fc12345 } else { 0x80000000 })]
        );
    }
}

#[test]
fn unavailable_join_input_is_not_zero_or_an_implicit_argument() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(&[mov(0, 1, 0), EXIT], &[P(0)], &[R(0)]);
    let mut inputs = Inputs::default();
    inputs.registers.insert(P(0), Value::Predicate(true));
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(1)]
    );
    inputs.registers.insert(P(0), Value::Predicate(false));
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
    inputs.registers.insert(R(0), Value::Word(99));
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
    inputs.registers.remove(&P(0));
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
}

#[test]
fn unobserved_scratch_does_not_create_a_color_output_requirement() {
    use Register::Gpr as R;
    let words = [0x5c98078000170000, EXIT]; // MOV R0,R1
    let program = lift(&words, &[], &[]);
    assert_eq!(
        program
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        []
    );
    let program = lift(&words, &[], &[R(0)]);
    assert_eq!(
        program.interpret(&Inputs::default(), 100).unwrap_err().kind,
        DiagnosticKind::UnavailableValue
    );
}

#[test]
fn predication_guards_constant_reads_as_well_as_register_writes() {
    use Register::{Gpr as R, Predicate as P};
    let load = 0x4c98078c00000000; // @P0 MOV R0,c[3][0]
    let program = lift(&[mov(0, 7, 7), load, EXIT], &[P(0)], &[R(0)]);
    let mut inputs = Inputs::default();
    inputs.registers.insert(P(0), Value::Predicate(false));
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(7)]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    let error = program.interpret(&inputs, 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(1))));
    inputs.constants = ConstantMemory::new(&[(3, &91u32.to_le_bytes())]).unwrap();
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(91)]
    );
}

#[test]
fn machine_loop_updates_ssa_parameters_and_preserves_unsigned_comparison() {
    use Register::Gpr as R;
    let program = lift(
        &[mov(0, 0, 7), add(0, 0, 1), isetp(0, 7, 0, 2, 1, 7, 7), branch(3, 1, 0), EXIT],
        &[R(2)],
        &[R(0)],
    );
    for limit in [0, 1, 2, 7, 127] {
        let inputs =
            Inputs { registers: [(R(2), Value::Word(limit))].into(), ..Default::default() };
        assert_eq!(
            program.interpret(&inputs, 10_000).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(limit.max(1))]
        );
    }
    let inputs = Inputs { registers: [(R(2), Value::Word(u32::MAX))].into(), ..Default::default() };
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::StepLimit);
}

#[test]
fn comparison_results_capture_the_old_accumulator_and_execution_guard() {
    use Register::{Gpr as R, Predicate as P};
    // @P0 ISETP.LT P0,P1,R0,R1,P0: the second result uses !relation & old P0.
    let program =
        lift(&[isetp(0, 1, 0, 1, 1, 0, 0), EXIT], &[R(0), R(1), P(0), P(1)], &[P(0), P(1)]);
    for (a, b, enabled, expected) in
        [(1, 2, true, [true, false]), (2, 1, true, [false, true]), (1, 2, false, [false, true])]
    {
        let inputs = Inputs {
            registers: [
                (R(0), Value::Word(a)),
                (R(1), Value::Word(b)),
                (P(0), Value::Predicate(enabled)),
                (P(1), Value::Predicate(true)),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            expected.map(Value::Predicate)
        );
    }
}

#[test]
fn integer_bits_wrap_and_reserved_registers_remain_constants() {
    use Register::{Gpr as R, Predicate as P};
    let program = lift(&[add(0, 0, 1), mov(255, 7, 7), EXIT], &[R(0)], &[R(0), R(255), P(7)]);
    let inputs = Inputs { registers: [(R(0), Value::Word(u32::MAX))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(0), Value::Word(0), Value::Predicate(true)]
    );
}

#[test]
fn machine_shift_modes_have_defined_large_shift_behavior() {
    use Register::Gpr as R;
    for (right, signed, wrap, input, amount, expected) in [
        (false, false, false, 1, 31, 0x80000000),
        (false, false, false, 1, 32, 0),
        (false, false, true, 1, 33, 2),
        (true, false, false, 0x80000000, 31, 1),
        (true, false, false, 0x80000000, 32, 0),
        (true, true, false, 0x80000000, 32, u32::MAX),
        (true, true, false, 0x7fffffff, 32, 0),
        (true, true, true, 0x80000000, 33, 0xc0000000),
    ] {
        let instruction = if right { 0x5c28000000070000 } else { 0x5c48000000070000 }
            | (u64::from(signed) << 48)
            | (u64::from(wrap) << 39)
            | (2 << 20)
            | (1 << 8);
        let program = lift(&[instruction, EXIT], &[R(1), R(2)], &[R(0)]);
        let inputs = Inputs {
            registers: [(R(1), Value::Word(input)), (R(2), Value::Word(amount))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn sealing_a_long_definition_chain_is_iterative() {
    use Register::Gpr as R;
    let mut words = vec![add(0, 0, 1); 2048];
    words.push(EXIT);
    let program = lift(&words, &[R(0)], &[R(0)]);
    let inputs = Inputs { registers: [(R(0), Value::Word(11))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 10_000).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(2059)]
    );
    assert!(program.function.blocks.iter().all(|b| b.parameters.is_empty()));
}

#[test]
fn conditional_exit_and_scheduling_word_branch_targets() {
    use Register::{Gpr as R, Predicate as P};
    let conditional_exit = EXIT & !(7 << 16);
    let program = lift(&[mov(0, 1, 7), conditional_exit, mov(0, 2, 7), EXIT], &[P(0)], &[R(0)]);
    for (p, expected) in [(false, 2), (true, 1)] {
        let inputs =
            Inputs { registers: [(P(0), Value::Predicate(p))].into(), ..Default::default() };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
    let jump_to_schedule = 0xe24000000007000f | ((32 - 8 - 8) << 20);
    let program = lift(&[jump_to_schedule, 0, 0, mov(0, 3, 7), EXIT], &[], &[R(0)]);
    assert_eq!(
        program
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(3)]
    );
}

#[test]
fn unreachable_padding_is_ignored_but_reachable_unsupported_work_reports_its_pc() {
    let code = bytes(&[EXIT, 0, 0]);
    assert!(Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).is_ok());
    for word in [0, 0xe2a0000000000000, 0xe33000000007000f] {
        let code = bytes(&[word, EXIT]);
        let error =
            Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

fn swapping_loop() -> Function {
    let mut b = Builder::new(&[], &[Register::Gpr(0), Register::Gpr(1)]);
    let entry = b.block(None);
    let body = b.block(Some(8));
    let end = b.block(Some(16));
    let a0 = b.word(23);
    let b0 = b.word(41);
    let n0 = b.word(3);
    b.function.blocks[entry.0].terminator =
        Some(Terminator::Jump(Edge::new(body, vec![a0, b0, n0])));
    let mut parameters = Vec::new();
    for index in 0..3 {
        parameters
            .push(b.function.value(Type::Bits32, Definition::Parameter { block: body, index }));
    }
    b.function.blocks[body.0].parameters = parameters.clone();
    let zero = b.word(0);
    let minus_one = b.word(u32::MAX);
    let condition = b.function.append(body, 8, Type::Predicate, Operation::Compare {
        op: Relation::Greater,
        a: parameters[2],
        b: zero,
        signed: false,
    });
    let count = b.function.append(body, 8, Type::Bits32, Operation::Add(parameters[2], minus_one));
    b.function.blocks[body.0].terminator = Some(Terminator::Branch {
        kind: crate::ir::convergence::BranchKind::ControlFlow,
        condition,
        yes: Edge::new(body, vec![parameters[1], parameters[0], count]),
        no: Edge::new(end, vec![]),
    });
    b.function.blocks[end.0].terminator = Some(Terminator::Return(parameters[..2].to_vec()));
    b.function
}

#[test]
fn backedge_arguments_transfer_simultaneously() {
    let function = swapping_loop();
    function.verify().unwrap();
    assert_eq!(
        Program { function }
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        [Value::Word(41), Value::Word(23)]
    );
}

#[test]
fn verifier_rejects_invalid_edges_types_and_use_before_definition() {
    let function = swapping_loop();
    let mut bad = function.clone();
    bad.blocks[0].terminator.as_mut().unwrap().edges_mut()[0].arguments.pop();
    assert!(bad.verify().unwrap_err().message.contains("argument count"));
    let mut bad = function.clone();
    bad.blocks[1].instructions[0].operation = Operation::Copy(bad.blocks[1].parameters[0]);
    assert!(bad.verify().unwrap_err().message.contains("result type"));
    let mut bad = function.clone();
    bad.blocks[1].instructions[1].operation =
        Operation::Copy(bad.blocks[1].instructions[1].result.unwrap());
    assert!(bad.verify().unwrap_err().message.contains("before its definition"));
    let mut bad = function.clone();
    bad.blocks[1].terminator.as_mut().unwrap().edges_mut()[1].target = BlockId(9999);
    assert!(bad.verify().is_err());
    let mut bad = function;
    bad.blocks[2].terminator = None;
    assert!(bad.verify().unwrap_err().message.contains("no terminator"));
}

#[test]
fn verifier_rejects_a_definition_that_does_not_dominate_the_use() {
    use Register::{Gpr as R, Predicate as P};
    let mut p = lift(&[mov(0, 5, 0), EXIT], &[R(0), P(0)], &[R(0)]);
    let value = p
        .function
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find(|i| matches!(i.operation, Operation::Copy(_)))
        .unwrap()
        .result
        .unwrap();
    for b in &mut p.function.blocks {
        if let Some(Terminator::Return(values)) = &mut b.terminator {
            values[0] = value;
        }
    }
    assert!(p.function.verify().unwrap_err().message.contains("does not dominate"));
}
