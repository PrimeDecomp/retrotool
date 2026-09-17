use super::{bytes, mov, offset, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Stage, Value},
};

const PIXLD: u64 = 0xefe8e0028007ff00;

fn lift(words: &[u64], inputs: Vec<Register>) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        inputs,
        outputs: vec![Register::Gpr(0)],
    })
    .unwrap()
}

#[test]
fn sample_index_is_an_explicit_invocation_input() {
    let program = lift(&[PIXLD, EXIT], vec![]);
    let error = program.interpret(&Inputs::default(), 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));
    for sample in [0, 1, 3, 7, 15] {
        let inputs = Inputs { sample_index: Some(sample), ..Default::default() };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(sample)]
        );
    }
}

#[test]
fn a_false_guard_preserves_the_destination_without_reading_sample_state() {
    use Register::Predicate as P;
    let program = lift(&[mov(0, 91, 7), PIXLD & !(7 << 16), EXIT], vec![P(0)]);
    let mut inputs =
        Inputs { registers: [(P(0), Value::Predicate(false))].into(), ..Default::default() };
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(91)]
    );
    inputs.registers.insert(P(0), Value::Predicate(true));
    let error = program.interpret(&inputs, 100).unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(offset(1))));
    inputs.sample_index = Some(3);
    assert_eq!(
        program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
        [Value::Word(3)]
    );
}

#[test]
fn unsupported_raster_interfaces_and_unresolved_result_predicates_are_rejected() {
    for (stage, word) in [
        (Stage::Scalar, PIXLD),
        (Stage::Vertex, PIXLD),
        (Stage::Fragment, PIXLD & !(255 << 8)),
        (Stage::Fragment, PIXLD & !(7 << 45)),
        (Stage::Fragment, PIXLD | (1 << 20)),
        (Stage::Fragment, PIXLD & !(7 << 31)), // COUNT
        (Stage::Fragment, PIXLD | (2 << 31)),  // reserved mode 7
    ] {
        let error = Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
            indexed_attributes: vec![],
            stage,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
    }
}

#[test]
fn verifier_checks_sample_reads_against_the_stage() {
    let mut program = lift(&[PIXLD, EXIT], vec![]);
    program.function.stage = Stage::Vertex;
    assert!(program.function.verify().unwrap_err().message.contains("sample index"));
}
