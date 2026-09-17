use super::{bytes, mov, offset, Operation, EXIT};
use crate::{
    maxwell::MaxwellCode,
    program::{
        texture, DiagnosticKind, Execution, Inputs, InterpretOptions, Program, Register, Signature,
        Stage, Value,
    },
};

const KILL: u64 = 0xe33000000007000f;

fn program(words: &[u64], inputs: Vec<Register>, outputs: Vec<Register>) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        stage: Stage::Fragment,
        inputs,
        outputs,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn demotion_is_predicated_and_does_not_observe_exports() {
    let p =
        program(&[KILL & !(7 << 16), EXIT], vec![Register::Predicate(0)], vec![Register::Gpr(0)]);
    let mut inputs = Inputs::default();
    assert_eq!(p.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::MissingInput);
    inputs.registers.insert(Register::Predicate(0), Value::Predicate(false));
    assert_eq!(p.interpret(&inputs, 100).unwrap_err().kind, DiagnosticKind::UnavailableValue);
    inputs.registers.insert(Register::Predicate(0), Value::Predicate(true));
    assert_eq!(p.interpret(&inputs, 100).unwrap(), Execution::Discarded);

    let p = program(&[KILL, KILL, mov(0, 17, 7), EXIT], vec![], vec![Register::Gpr(0)]);
    assert_eq!(p.interpret(&Inputs::default(), 100).unwrap(), Execution::Discarded);
    assert_eq!(p.interpret(&Inputs::default(), 1).unwrap_err().kind, DiagnosticKind::StepLimit);
}

#[test]
fn demotion_preserves_resource_execution_and_faults() {
    let p = program(
        &[
            KILL,
            mov(6, 0.25f32.to_bits(), 7),
            mov(10, 0.75f32.to_bits(), 7),
            0xd830123040a70600, // implicit TEXS after demotion
            EXIT,
        ],
        vec![],
        vec![],
    );
    let mut calls = Vec::new();
    let result = p
        .interpret_with_textures(
            &Inputs::default(),
            InterpretOptions { step_budget: 100, ..Default::default() },
            &mut |request: &texture::Request| {
                calls.push(request.pc);
                Ok(texture::Response::Texel([0; 4]))
            },
        )
        .unwrap();
    assert_eq!(result, Execution::Discarded);
    assert_eq!(calls, [offset(3)]);

    let p = program(&[KILL, 0x4c98078000070000, EXIT], vec![], vec![]);
    let error = p.interpret(&Inputs::default(), 100).unwrap_err();
    assert_eq!((error.pc, error.kind), (Some(offset(1)), DiagnosticKind::MissingInput));
}

#[test]
fn kill_rejects_other_stages_conditions_and_modifier_bits() {
    for stage in [Stage::Scalar, Stage::Vertex] {
        assert!(Program::lift(MaxwellCode::parse(&bytes(&[KILL, EXIT])).unwrap(), &Signature {
            stage,
            ..Default::default()
        })
        .is_err());
    }
    for word in [KILL ^ 1, KILL | (1 << 32)] {
        assert!(Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
            stage: Stage::Fragment,
            ..Default::default()
        })
        .is_err());
    }
}

#[test]
fn scalar_interpreter_does_not_invent_nodep_results_for_demoted_helpers() {
    let p = program(&[KILL, mov(6, 0, 7), mov(10, 0, 7), 0xd832123040a70600, EXIT], vec![], vec![]);
    let error = p
        .interpret_with_textures(
            &Inputs::default(),
            InterpretOptions { step_budget: 100, ..Default::default() },
            &mut |_: &texture::Request| panic!("suppressed helper result is not established"),
        )
        .unwrap_err();
    assert_eq!((error.pc, error.kind), (Some(offset(3)), DiagnosticKind::Unsupported));
}

#[test]
fn verifier_requires_fragment_stage_and_no_result_for_demotion() {
    let mut p = program(&[KILL, mov(0, 1, 7), EXIT], vec![], vec![Register::Gpr(0)]);
    p.function.stage = Stage::Vertex;
    assert!(p.function.verify().is_err());
    p.function.stage = Stage::Fragment;
    let value =
        p.function.blocks.iter().flat_map(|b| &b.instructions).find_map(|i| i.result).unwrap();
    let effect = p
        .function
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.instructions)
        .find(|i| matches!(i.operation, Operation::Demote))
        .unwrap();
    effect.result = Some(value);
    assert!(p.function.verify().is_err());
}
