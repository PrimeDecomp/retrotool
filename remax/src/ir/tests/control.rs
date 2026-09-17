use super::{add, branch, bytes, isetp, lift, mov, offset, EXIT};
use crate::{
    ir::{convergence::Transfer, Terminator},
    maxwell::{MaxwellCode, ShaderProgramHeader},
    program::{DiagnosticKind, Inputs, Program, Register, Signature, Stage, Value},
};

pub(super) fn setup(from: usize, to: usize) -> u64 {
    let displacement = offset(to) as i64 - offset(from) as i64 - 8;
    0xe290000000000000 | ((displacement as u64 & 0xffffff) << 20)
}

pub(super) fn sync(predicate: u8) -> u64 { 0xf0f800000000000f | (u64::from(predicate) << 16) }

#[test]
fn nested_sync_transfers_preserve_outer_context_and_values() {
    let program = lift(
        &[
            mov(0, 10, 7),
            setup(1, 8),
            setup(2, 6),
            sync(0),
            add(0, 0, 1),
            sync(7),
            add(0, 0, 2),
            sync(7),
            add(0, 0, 4),
            EXIT,
        ],
        &[Register::Predicate(0)],
        &[Register::Gpr(0)],
    );
    for (predicate, expected) in [(true, 16), (false, 17)] {
        let inputs = Inputs {
            registers: [(Register::Predicate(0), Value::Predicate(predicate))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 100).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
    assert_eq!(program.function.blocks.iter().map(|b| b.convergence.len()).max(), Some(2));
}

#[test]
fn a_sync_loop_keeps_ssa_transfers_simultaneous() {
    let program = sync_loop();
    // No incoming predicate: SSY's zero bits must not read P0.
    for limit in [0, 1, 2, 7, 31] {
        let inputs = Inputs {
            registers: [(Register::Gpr(2), Value::Word(limit))].into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 1_000).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(limit)]
        );
    }
}

pub(super) fn sync_loop() -> Program {
    lift(
        &[
            mov(0, 0, 7),
            setup(1, 7),
            isetp(0, 7, 0, 2, 6, 7, 7), // unsigned R0 >= R2
            sync(0),
            add(0, 0, 1),
            branch(5, 2, 7),
            0, // unreachable padding is not part of the graph
            EXIT,
        ],
        &[Register::Gpr(2)],
        &[Register::Gpr(0)],
    )
}

#[test]
fn shared_pcs_with_distinct_pending_joins_are_distinct_execution_points() {
    let program = lift(
        &[
            mov(0, 0, 7),
            setup(1, 12),
            branch(2, 5, 0),
            setup(3, 10),
            branch(4, 6, 7),
            mov(0, 4, 7),
            add(0, 0, 1),
            sync(1),
            add(0, 0, 2),
            sync(7),
            add(0, 0, 8),
            sync(7),
            EXIT,
        ],
        &[Register::Predicate(0), Register::Predicate(1)],
        &[Register::Gpr(0)],
    );
    for (outer_path, skip, expected) in
        [(true, true, 5), (true, false, 7), (false, true, 9), (false, false, 11)]
    {
        let inputs = Inputs {
            registers: [
                (Register::Predicate(0), Value::Predicate(outer_path)),
                (Register::Predicate(1), Value::Predicate(skip)),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            program.interpret(&inputs, 200).unwrap().into_outputs().expect("live invocation"),
            [Value::Word(expected)]
        );
    }
    let contexts = program
        .function
        .blocks
        .iter()
        .filter(|block| block.pc == Some(offset(6)))
        .map(|block| block.convergence.len())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(contexts, [1, 2].into());
}

#[test]
fn synchronization_rejects_unestablished_control_modes_and_escaping_paths() {
    for (words, pc) in [
        (vec![setup(0, 3), branch(1, 3, 7), sync(7), EXIT], offset(3)),
        (vec![setup(0, 3), EXIT, sync(7), EXIT], offset(1)),
        (vec![setup(0, 3) | 15, mov(0, 1, 7), sync(7), EXIT], offset(0)),
        (vec![setup(0, 3), branch(1, 2, 7) | (1 << 6), sync(7), EXIT], offset(1)),
        (vec![setup(0, 3), branch(1, 2, 7) | (1 << 7), sync(7), EXIT], offset(1)),
        (vec![setup(0, 3), sync(7) & !15, sync(7), EXIT], offset(1)),
    ] {
        let code = bytes(&words);
        let diagnostic =
            Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).unwrap_err();
        assert_eq!((diagnostic.kind, diagnostic.pc), (DiagnosticKind::Unsupported, Some(pc)));
    }
    let code = bytes(&[sync(7), EXIT]);
    let diagnostic =
        Program::lift(MaxwellCode::parse(&code).unwrap(), &Signature::default()).unwrap_err();
    assert_eq!((diagnostic.kind, diagnostic.pc), (DiagnosticKind::InvalidInput, Some(offset(0))));
    let program = lift(&[sync(15), EXIT], &[], &[]); // never executes
    assert_eq!(
        program
            .interpret(&Inputs::default(), 100)
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        []
    );
}

#[test]
fn verifier_checks_context_changes_independently_of_discovery() {
    let program = lift(&[setup(0, 3), mov(0, 1, 7), sync(7), EXIT], &[], &[Register::Gpr(0)]);
    let setup_block = program.function.blocks.iter().position(|b| b.pc == Some(offset(0))).unwrap();
    let sync_block = program.function.blocks.iter().position(|b| b.pc == Some(offset(2))).unwrap();
    let join_block = program.function.blocks.iter().position(|b| b.pc == Some(offset(3))).unwrap();

    let mut missing_setup = program.function.clone();
    missing_setup.blocks[setup_block].terminator.as_mut().unwrap().edges_mut()[0].convergence =
        Transfer::Preserve;
    assert!(missing_setup.verify().unwrap_err().message.contains("context"));

    let mut missing_pop = program.function.clone();
    missing_pop.blocks[sync_block].terminator.as_mut().unwrap().edges_mut()[0].convergence =
        Transfer::Preserve;
    assert!(missing_pop.verify().unwrap_err().message.contains("context"));

    let mut wrong_join = program.function.clone();
    wrong_join.blocks[join_block].pc = Some(offset(3) + 8);
    assert!(wrong_join.verify().unwrap_err().message.contains("join"));

    let mut escaping_return = program.function.clone();
    escaping_return.blocks[sync_block].terminator = Some(Terminator::Return(vec![]));
    assert!(escaping_return.verify().unwrap_err().message.contains("pending convergence"));
}

#[test]
#[ignore = "requires MPR_MATERIAL_FIXTURE pointing to the captured vine draw"]
fn original_vine_fragment_preserves_its_sync_region() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::path::PathBuf::from(
        std::env::var_os("MPR_MATERIAL_FIXTURE")
            .ok_or("set MPR_MATERIAL_FIXTURE to research/material-integration/vine")?,
    );
    let vertex = std::fs::read(directory.join("vertex.maxwell"))?;
    let vertex_header =
        ShaderProgramHeader::parse(&std::fs::read(directory.join("vertex.header"))?)?;
    Program::lift(MaxwellCode::parse(&vertex)?, &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage: Stage::Vertex,
        inputs: vec![],
        outputs: (0..0x400)
            .step_by(4)
            .filter(|&offset| vertex_header.vtg_attribute_enabled(offset, true) == Some(true))
            .map(Register::OutputAttribute)
            .collect(),
    })?;
    let code = std::fs::read(directory.join("fragment.maxwell"))?;
    let header = ShaderProgramHeader::parse(&std::fs::read(directory.join("fragment.header"))?)?;
    let outputs = header.pixel_io().ok_or("expected fragment header")?.output_registers();
    let program = Program::lift(MaxwellCode::parse(&code)?, &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage: Stage::Fragment,
        inputs: vec![],
        outputs: outputs.registers().map(Register::Gpr).collect(),
    })?;
    let transfers = program
        .function
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .terminator
                .as_ref()
                .unwrap()
                .edges()
                .into_iter()
                .filter(|edge| edge.convergence != Transfer::Preserve)
                .map(move |edge| (block.pc.unwrap(), edge.convergence))
        })
        .collect::<Vec<_>>();
    assert_eq!(transfers.len(), 3);
    assert!(transfers.contains(&(
        0x608,
        Transfer::Enter(crate::ir::convergence::Region { setup_pc: 0x608, join_pc: 0xbd8 })
    )));
    assert!(transfers.contains(&(0x748, Transfer::Leave)));
    assert!(transfers.contains(&(0xbd0, Transfer::Leave)));
    let analysis = crate::ir::analysis::Analysis::new(&program.function);
    let mut aligned_loads = std::collections::BTreeSet::new();
    for instruction in program.function.blocks.iter().flat_map(|block| &block.instructions) {
        if let crate::ir::Operation::LoadConstantWord { bank: 6, address, bytes, .. } =
            instruction.operation
        {
            assert!(
                analysis.bits.get(address).is_aligned(bytes),
                "light-record load at {:#x} lacks a {bytes}-byte alignment proof",
                instruction.pc
            );
            aligned_loads.insert(instruction.pc);
        }
    }
    assert_eq!(aligned_loads.len(), 9);
    let available = (0..program.function.values.len())
        .filter(|&index| analysis.availability.is_available(crate::ir::ValueId(index)))
        .count();
    assert!(analysis.steps.maximum().is_none());
    // The real shader samples before entering divergent light accumulation.
    // Follow edges rather than sorting PCs: this establishes an execution prefix,
    // not merely the location of instructions in the binary.
    let mut block_id = crate::ir::BlockId(0);
    let mut prefix_textures = Vec::new();
    let mut visited = vec![false; program.function.blocks.len()];
    let first_branch = loop {
        assert!(!visited[block_id.0], "cycle in the expected sampling prefix");
        visited[block_id.0] = true;
        let block = &program.function.blocks[block_id.0];
        for instruction in &block.instructions {
            if let crate::ir::Operation::Texture(operation) = &instruction.operation {
                prefix_textures.push((instruction.pc, operation.descriptor));
            }
        }
        match block.terminator.as_ref().unwrap() {
            Terminator::Jump(edge) => block_id = edge.target,
            Terminator::Branch { .. } => break block.pc.unwrap(),
            Terminator::Return(_) => panic!("expected the clustered-light branch"),
        }
    };
    assert_eq!(prefix_textures, [
        (0xf0, 12),
        (0xf8, 10),
        (0x590, 8),
        (0x5b0, 16),
        (0x5b8, 20),
        (0x5c8, 14),
        (0x5d0, 18),
    ]);
    assert_eq!(first_branch, 0x748);
    assert_eq!(
        program
            .function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction.operation, crate::ir::Operation::Texture(_)))
            .count(),
        prefix_textures.len()
    );
    eprintln!(
        "vine fragment: {} SSA blocks, {} values ({available} need no validity flag), preserved SSY/SYNC region",
        program.function.blocks.len(),
        program.function.values.len()
    );
    Ok(())
}
