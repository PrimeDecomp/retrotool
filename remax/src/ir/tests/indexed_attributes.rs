//! AL2P provenance is separate from numeric register contents. These tests
//! exercise logical selection, availability and transport; native address bits
//! and the hardware predicate encoding are intentionally not exposed.

use Register::{Gpr as R, Predicate as P};

use super::{add, branch, bytes, isetp, mov, offset, EXIT};
use crate::{
    ir::{analysis::Analysis, attribute::Input, transport::Transport, Definition, Operation, Type},
    maxwell::MaxwellCode,
    program::{
        Diagnostic, DiagnosticKind, FloatModel, Inputs, Interpolant, InterpolationLocation,
        InterpolationMode, InterpretOptions, Program, Register, Signature, Stage, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::SoftwareIeee,
    special_functions: crate::program::SpecialFunctionModel::Reject,
};

fn address(destination: u8, predicate: u8, source: u8, base: u16) -> u64 {
    0xefa0000000070000
        | u64::from(destination)
        | (u64::from(source) << 8)
        | (u64::from(base) << 20)
        | (u64::from(predicate) << 44)
}

fn load(destination: u8, address: u8) -> u64 {
    0xefd87f8000070000 | u64::from(destination) | (u64::from(address) << 8)
}

fn copy(destination: u8, source: u8) -> u64 {
    0x5c98078000070000 | u64::from(destination) | (u64::from(source) << 20)
}

fn interpolate(destination: u8, address: u8, predicate: u8, base: u16) -> u64 {
    // IPA.PERSPECTIVE Rdst,a[Raddress],R5,Ppredicate. Default location.
    0xe040004000070000
        | u64::from(destination)
        | (u64::from(address) << 8)
        | (5 << 20)
        | (u64::from(base) << 28)
        | (u64::from(predicate) << 47)
}

fn lift(words: &[u64], stage: Stage, inputs: &[Register]) -> Result<Program, Diagnostic> {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        stage,
        indexed_attributes: vec![0x80, 0x84, 0x90, 0x94],
        inputs: inputs.to_vec(),
        outputs: vec![R(0)],
    })
}

fn input(relative: u32) -> Inputs {
    let interpolant = |byte_offset| Interpolant {
        byte_offset,
        mode: InterpolationMode::Perspective,
        location: InterpolationLocation::Default,
    };
    Inputs {
        registers: [(R(1), Value::Word(relative)), (R(5), Value::Word(2f32.to_bits()))].into(),
        attributes: [(0x80, 0x7fc12345), (0x90, 0x80000000)].into(),
        interpolants: [(interpolant(0x80), 0.25f32.to_bits()), (interpolant(0x90), 8f32.to_bits())]
            .into(),
        ..Default::default()
    }
}

fn complementary_loads() -> [u64; 6] {
    [
        address(2, 7, 1, 0x80),
        address(3, 7, 1, 0x90),
        load(2, 2) & !(7 << 16) | (8 << 16), // @!P0 ALD R2,a[R2]
        load(2, 3) & !(7 << 16),             // @P0 ALD R2,a[R3]
        copy(0, 2),
        EXIT,
    ]
}

#[test]
fn complementary_loads_replace_an_address_with_data_without_exposing_native_bits() {
    let words = complementary_loads();
    let program = lift(&words, Stage::Vertex, &[R(1), P(0)]).unwrap();
    for enabled in [false, true] {
        let mut inputs = input(0);
        inputs.registers.insert(P(0), Value::Predicate(enabled));
        let selected = if enabled { 0x90 } else { 0x80 };
        let expected = inputs.attributes[&selected];
        inputs.attributes.retain(|&offset, _| offset == selected);
        assert_eq!(program.interpret(&inputs, 100).unwrap().into_outputs().unwrap(), [
            Value::Word(expected)
        ]);
        inputs.attributes.clear();
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::MissingInput);
        assert_eq!(error.pc, Some(offset(if enabled { 3 } else { 2 })));
    }
    // A different predicate leaves a real path from the address to the output.
    let mut different = words;
    different[3] |= 1 << 16;
    assert!(lift(&different, Stage::Vertex, &[R(1), P(0), P(1)]).is_err());
    // Reusing the predicate register does not preserve the old SSA predicate.
    let mut rewritten = words.to_vec();
    rewritten.insert(3, isetp(0, 7, 4, 255, 2, 7, 7));
    assert!(lift(&rewritten, Stage::Vertex, &[R(1), R(4), P(0)]).is_err());
    // An earlier observation of the intermediate register must remain invalid.
    let mut observed = words.to_vec();
    observed.insert(3, add(4, 2, 1));
    assert!(lift(&observed, Stage::Vertex, &[R(1), P(0)]).is_err());
}

#[test]
fn a_guarded_read_recovers_the_address_after_a_complementary_data_write() {
    let mut words = complementary_loads();
    words.swap(2, 3);
    let program = lift(&words, Stage::Vertex, &[R(1), P(0)]).unwrap();
    for enabled in [false, true] {
        let selected = if enabled { 0x90 } else { 0x80 };
        let mut inputs = input(0);
        inputs.registers.insert(P(0), Value::Predicate(enabled));
        let expected = inputs.attributes[&selected];
        inputs.attributes.retain(|&offset, _| offset == selected);
        assert_eq!(program.interpret(&inputs, 100).unwrap().into_outputs().unwrap(), [
            Value::Word(expected)
        ]);
        inputs.attributes.clear();
        let error = program.interpret(&inputs, 100).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::MissingInput);
        assert_eq!(error.pc, Some(offset(if enabled { 2 } else { 3 })));
    }
    let mut different = words;
    different[3] |= 1 << 16;
    assert!(lift(&different, Stage::Vertex, &[R(1), P(0), P(1)]).is_err());
    let mut rewritten = words.to_vec();
    rewritten.insert(3, isetp(0, 7, 4, 255, 2, 7, 7));
    assert!(lift(&rewritten, Stage::Vertex, &[R(1), R(4), P(0)]).is_err());
    let mut observed = words.to_vec();
    observed.insert(3, add(4, 2, 1));
    assert!(lift(&observed, Stage::Vertex, &[R(1), P(0)]).is_err());
}

#[test]
fn only_the_selected_attribute_is_required_and_address_capture_survives_overwrites() {
    let program = lift(
        &[
            address(2, 0, 1, 0x80),
            copy(3, 2),
            mov(1, 0x1234, 7),
            mov(2, 0x5678, 7),
            load(0, 3),
            EXIT,
        ],
        Stage::Vertex,
        &[R(1)],
    )
    .unwrap();
    for (relative, expected) in [(0, 0x7fc12345), (16, 0x80000000)] {
        assert_eq!(
            program
                .interpret(&input(relative), 100)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected)]
        );
    }
    // 0x84 is in the interface, but absent in this invocation. Declaring it
    // cannot force an error until that particular component is selected.
    let error = program.interpret(&input(4), 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(4))));
}

#[test]
fn invalid_offsets_cannot_wrap_or_alias_an_undeclared_component() {
    let program =
        lift(&[address(2, 0, 1, 0x80), load(0, 2), EXIT], Stage::Vertex, &[R(1)]).unwrap();
    for relative in [1, 0x200, 0xffffff80, u32::MAX] {
        let error = program.interpret(&input(relative), 100).unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::InvalidInput, Some(offset(1))));
    }
    let mut inputs = input(8);
    inputs.attributes.insert(0x88, 99); // A value outside the declared domain is not an input.
    assert_eq!(program.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
}

#[test]
fn missing_selector_is_observed_at_the_read_not_at_address_construction() {
    let program = lift(
        &[address(2, 0, 1, 0x80), mov(0, 19, 7), load(0, 2) & !(7 << 16) | (1 << 16), EXIT],
        Stage::Vertex,
        &[R(1), P(1)],
    )
    .unwrap();
    let mut inputs =
        Inputs { registers: [(P(1), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(19)]
    );
    inputs.registers.insert(P(1), Value::Predicate(true));
    let error = program.interpret(&inputs, 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(2))));
}

#[test]
fn indexed_interpolation_uses_the_selected_numerator_and_matching_provenance() {
    let program = lift(
        &[address(2, 0, 1, 0x80), copy(3, 2), interpolate(0, 3, 0, 0x80), EXIT],
        Stage::Fragment,
        &[R(1), R(5)],
    )
    .unwrap();
    for (relative, expected) in [(0, 0.5f32), (16, 16.)] {
        assert_eq!(
            program
                .interpret_with_options(&input(relative), OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected.to_bits())]
        );
    }
    let error = program.interpret_with_options(&input(4), OPTIONS).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(2))));
}

#[test]
fn indexed_interpolation_rejects_mismatched_or_reinterpreted_tokens() {
    for words in [
        // Numerically identical offsets do not prove native predicate identity.
        vec![address(2, 0, 1, 0x80), address(3, 1, 1, 0x80), interpolate(0, 2, 1, 0x80), EXIT],
        vec![address(2, 0, 1, 0x80), interpolate(0, 2, 0, 0x90), EXIT],
        vec![address(2, 0, 1, 0x80), interpolate(0, 2, 0, 0x80) | (1 << 50), EXIT],
        // The multiplier must be ordinary bits, even when it aliases the address.
        vec![address(5, 0, 1, 0x80), interpolate(0, 5, 0, 0x80), EXIT],
        vec![address(2, 0, 1, 0x80), add(0, 2, 0), EXIT],
        vec![address(0, 0, 1, 0x80), EXIT],
        vec![address(2, 0, 1, 0x80), mov(0, 19, 0), EXIT],
    ] {
        let error = lift(&words, Stage::Fragment, &[R(1), R(5)]).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Unsupported, "{words:x?}: {error}");
    }
}

fn joined_program() -> Program {
    lift(
        &[
            address(2, 0, 1, 0x80),
            address(2, 0, 4, 0x80) & !(7 << 16) | (1 << 16),
            interpolate(0, 2, 0, 0x80),
            EXIT,
        ],
        Stage::Fragment,
        &[R(1), R(4), R(5), P(1)],
    )
    .unwrap()
}

#[test]
fn corresponding_join_edges_preserve_pairing_and_crossed_edges_do_not() {
    let mut program = joined_program();
    for enabled in [false, true] {
        let mut inputs = input(0);
        inputs.registers.extend([(R(4), Value::Word(16)), (P(1), Value::Predicate(enabled))]);
        let expected = if enabled { 16f32 } else { 0.5 };
        assert_eq!(
            program
                .interpret_with_options(&inputs, OPTIONS)
                .unwrap()
                .into_outputs()
                .expect("live invocation"),
            [Value::Word(expected.to_bits())]
        );
    }
    let block = program
        .function
        .blocks
        .iter()
        .position(|block| {
            block
                .parameters
                .iter()
                .filter(|p| program.function.values[p.0].ty == Type::AttributeAddress)
                .count()
                == 2
        })
        .expect("distinct address/predicate join parameters");
    let parameters = &program.function.blocks[block].parameters;
    let slots = parameters
        .iter()
        .enumerate()
        .filter(|(_, p)| program.function.values[p.0].ty == Type::AttributeAddress)
        .map(|(slot, _)| slot)
        .collect::<Vec<_>>();
    let (a, b) = (parameters[slots[0]], parameters[slots[1]]);
    assert!(Transport::new(&program.function).equivalent(&program.function, a, b));
    let mut incoming = Vec::new();
    for predecessor in &program.function.blocks {
        for edge in predecessor.terminator.as_ref().unwrap().edges() {
            if edge.target.0 == block {
                incoming.push(edge.arguments[slots[1]]);
            }
        }
    }
    assert_eq!(incoming.len(), 2);
    incoming.reverse();
    for predecessor in &mut program.function.blocks {
        for edge in predecessor.terminator.as_mut().unwrap().edges_mut() {
            if edge.target.0 == block {
                edge.arguments[slots[1]] = incoming.remove(0);
            }
        }
    }
    let transport = Transport::new(&program.function);
    assert_eq!(transport.origins(a), transport.origins(b));
    assert!(!transport.equivalent(&program.function, a, b));
}

#[test]
fn independent_verifier_rejects_raw_addresses_and_invalid_input_domains() {
    let program =
        lift(&[address(2, 0, 1, 0x80), load(0, 2), EXIT], Stage::Vertex, &[R(1)]).unwrap();
    let mut wrong_type = program.function.clone();
    for block in &mut wrong_type.blocks {
        for instruction in &block.instructions {
            if let Operation::ReadAttribute(Input::Indexed(address)) = instruction.operation {
                wrong_type.values[address.0].ty = Type::Bits32;
            }
        }
    }
    assert!(wrong_type.verify().is_err());
    for domain in [vec![0x7c], vec![0x81], vec![0x280], vec![0x80, 0x80], vec![0x90, 0x80]] {
        let mut invalid = program.function.clone();
        invalid.indexed_attributes = domain;
        assert!(invalid.verify().is_err());
    }
}

pub(super) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let mut programs = Vec::new();
    for stage in [Stage::Vertex, Stage::Fragment] {
        let read = if stage == Stage::Vertex { load(0, 2) } else { interpolate(0, 2, 0, 0x80) };
        let program = lift(&[address(2, 0, 1, 0x80), read, EXIT], stage, &[R(1), R(5)]).unwrap();
        programs.push((program, [0, 4, 8, 16, 20, 1, 0x200, 0xffffff80].map(input).to_vec()));
    }
    let cases = [false, true].map(|enabled| {
        let mut inputs = input(0);
        inputs.registers.extend([(R(4), Value::Word(16)), (P(1), Value::Predicate(enabled))]);
        inputs
    });
    programs.push((joined_program(), cases.to_vec()));
    programs.push(select_cases());
    let mut cases = Vec::new();
    for enabled in [None, Some(false), Some(true)] {
        for available in [None, Some(0x80), Some(0x90)] {
            let mut inputs = input(0);
            if let Some(enabled) = enabled {
                inputs.registers.insert(P(0), Value::Predicate(enabled));
            }
            inputs.attributes.retain(|&offset, _| Some(offset) == available);
            cases.push(inputs);
        }
    }
    for reverse in [false, true] {
        let mut words = complementary_loads();
        if reverse {
            words.swap(2, 3);
        }
        programs.push((lift(&words, Stage::Vertex, &[R(1), P(0)]).unwrap(), cases.clone()));
    }
    // Loop-carried addresses are captured values, not additions repeated at use.
    let program = lift(
        &[
            address(2, 0, 1, 0x80),
            address(3, 1, 4, 0x80),
            copy(6, 2),
            copy(2, 3),
            copy(3, 6),
            add(7, 7, 1),
            isetp(2, 7, 7, 8, 1, 7, 7),
            branch(7, 2, 2),
            load(0, 2),
            EXIT,
        ],
        Stage::Vertex,
        &[R(1), R(4), R(7), R(8)],
    )
    .unwrap();
    let cases = (1..=4)
        .map(|count| {
            let mut inputs = input(0);
            inputs.registers.extend([
                (R(4), Value::Word(16)),
                (R(7), Value::Word(0)),
                (R(8), Value::Word(count)),
            ]);
            inputs
        })
        .collect();
    programs.push((program, cases));
    programs
}

fn select_cases() -> (Program, Vec<Inputs>) {
    // A future if-conversion may select opaque values. Use the IR operation
    // directly: Maxwell SEL decoding is a separate frontend concern.
    let mut program = lift(
        &[address(2, 0, 1, 0x80), address(3, 1, 4, 0x90), copy(6, 2), load(0, 6), EXIT],
        Stage::Vertex,
        &[R(1), R(4)],
    )
    .unwrap();
    let condition = program.function.value(Type::Predicate, Definition::Input(P(2)));
    let addresses = program
        .function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::AttributeAddress { .. }))
        .map(|instruction| instruction.result.unwrap())
        .collect::<Vec<_>>();
    let copy = program
        .function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
        .find(|instruction| matches!(instruction.operation, Operation::Copy(_)))
        .unwrap();
    copy.operation = Operation::Select { condition, yes: addresses[1], no: addresses[0] };
    program.function.verify().unwrap();
    let cases = [false, true].map(|enabled| {
        let mut inputs = input(0);
        inputs.registers.insert(P(2), Value::Predicate(enabled));
        if enabled {
            inputs.registers.remove(&R(1));
            inputs.registers.insert(R(4), Value::Word(0));
        }
        inputs
    });
    (program, cases.to_vec())
}

#[test]
fn selected_address_preserves_base_and_ignores_the_unchosen_missing_offset() {
    let (program, cases) = select_cases();
    for (inputs, expected) in cases.iter().zip([0x7fc12345, 0x80000000]) {
        assert_eq!(
            program.interpret(inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn loop_carried_addresses_preserve_simultaneous_transfers() {
    let (program, cases) = comparison_cases().pop().unwrap();
    for (index, inputs) in cases.iter().enumerate() {
        let expected = if index % 2 == 0 { 0x80000000 } else { 0x7fc12345 };
        assert_eq!(
            program.interpret(inputs, 1000).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
}

#[test]
fn known_selector_alignment_limits_candidates_without_specializing_its_value() {
    // SHL R1,R1,4; AL2P R2,P0,R1+0x80; ALD R0,a[R2].
    let program =
        lift(&[0x3848000000470101, address(2, 0, 1, 0x80), load(0, 2), EXIT], Stage::Vertex, &[R(
            1,
        )])
        .unwrap();
    let analysis = Analysis::new(&program.function);
    let address = program
        .function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .find(|i| matches!(i.operation, Operation::AttributeAddress { .. }))
        .unwrap()
        .result
        .unwrap();
    for (slot, expected) in [true, false, true, false].into_iter().enumerate() {
        assert_eq!(analysis.attributes.permits(address, slot), expected);
    }
}
