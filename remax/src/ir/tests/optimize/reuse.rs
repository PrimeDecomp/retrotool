use crate::{
    ir::{
        convergence::BranchKind, float::Operation as Float, memory::local, ssa::Builder, BlockId,
        Edge, Function, Operation, Terminator, Type, ValueId,
    },
    program::{Inputs, Program, Register, Value},
};

fn append(builder: &mut Builder, block: BlockId, operation: Operation) -> ValueId {
    let pc = 8 + block.0 * 0x100 + builder.function.blocks[block.0].instructions.len() * 8;
    builder.function.append(block, pc, Type::Bits32, operation)
}

fn finish(builder: Builder) -> Program {
    let function = builder.finish().0;
    function.verify().unwrap();
    Program { function }
}

fn diamond(dominating: bool) -> Program {
    use Register::{Gpr as R, Predicate as P};
    let mut builder = Builder::new(&[R(1), P(0)], &[R(0), R(2)]);
    let entry = builder.block(Some(0x80));
    // Deliberately store the join before its predecessors. Value numbering must
    // follow dominance, not block IDs or whichever arm happened to be visited last.
    let join = builder.block(Some(0x180));
    let left = builder.block(Some(0x280));
    let right = builder.block(Some(0x380));
    let value = builder.read(entry, R(1));
    let condition = builder.read(entry, P(0));
    if dominating {
        let one = builder.word(1);
        append(&mut builder, entry, Operation::Add(value, one));
    }
    builder.function.blocks[entry.0].terminator = Some(Terminator::Branch {
        condition,
        yes: Edge::new(left, vec![]),
        no: Edge::new(right, vec![]),
        kind: BranchKind::ControlFlow,
    });
    for block in [left, right, join] {
        // Equal literals have distinct initial IDs. Copies and transitive
        // expressions should still share the dominating calculation.
        let one = builder.word(1);
        let copy = append(&mut builder, block, Operation::Copy(value));
        let first = append(&mut builder, block, Operation::Add(copy, one));
        let second = append(&mut builder, block, Operation::Add(value, one));
        let sum = append(&mut builder, block, Operation::Add(first, second));
        if block == join {
            let incoming = builder.read(join, R(0));
            builder.function.blocks[block.0].terminator =
                Some(Terminator::Return(vec![incoming, sum]));
        } else {
            builder.write(block, R(0), sum);
            builder.function.blocks[block.0].terminator =
                Some(Terminator::Jump(Edge::new(join, vec![])));
        }
    }
    finish(builder)
}

fn changing_loop() -> Program {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1)], &[R(0)]);
    let entry = builder.block(Some(0x80));
    let body = builder.block(Some(0x180));
    let end = builder.block(Some(0x280));
    let initial = builder.read(entry, R(1));
    let zero = builder.word(0);
    let one = builder.word(1);
    let limit = builder.word(4);
    builder.write(entry, R(0), initial);
    builder.write(entry, R(2), zero);
    // Same expression as in the loop, but with the initial value. It must not
    // replace a calculation on the changing loop parameter.
    append(&mut builder, entry, Operation::Add(initial, one));
    builder.function.blocks[entry.0].terminator = Some(Terminator::Jump(Edge::new(body, vec![])));
    let value = builder.read(body, R(0));
    let counter = builder.read(body, R(2));
    let first = append(&mut builder, body, Operation::Add(value, one));
    let second = append(&mut builder, body, Operation::Add(value, one));
    let sum = append(&mut builder, body, Operation::Add(first, second));
    let next = append(&mut builder, body, Operation::Add(counter, one));
    builder.write(body, R(0), sum);
    builder.write(body, R(2), next);
    let condition = builder.function.append(body, 0x140, Type::Predicate, Operation::Compare {
        op: crate::ir::Relation::Less,
        a: next,
        b: limit,
        signed: false,
    });
    builder.function.blocks[body.0].terminator = Some(Terminator::Branch {
        condition,
        yes: Edge::new(body, vec![]),
        no: Edge::new(end, vec![]),
        kind: BranchKind::ControlFlow,
    });
    let result = builder.read(end, R(0));
    builder.function.blocks[end.0].terminator = Some(Terminator::Return(vec![result]));
    finish(builder)
}

fn modifiers() -> Program {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1)], &(0..8).map(R).collect::<Vec<_>>());
    let block = builder.block(Some(0x100));
    let value = builder.read(block, R(1));
    let mut results = Vec::new();
    for flags in 0..8 {
        let operation = Operation::Float(Float::Modify {
            value,
            absolute: flags & 1 != 0,
            negate: flags & 2 != 0,
            flush_to_zero: flags & 4 != 0,
        });
        append(&mut builder, block, operation.clone());
        results.push(append(&mut builder, block, operation));
    }
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(results));
    finish(builder)
}

fn memory() -> Program {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1)], &[R(0), R(2)]);
    builder.function.local_memory_bytes = 8;
    let block = builder.block(Some(0x100));
    let address = builder.read(block, R(1));
    let access = local::Access { address, bytes: 4 };
    let mut results = Vec::new();
    for bits in [7, 9] {
        let value = builder.word(bits);
        let pc = 8 + builder.function.blocks[block.0].instructions.len() * 8;
        builder.function.append_effect(
            block,
            pc,
            Operation::LocalStore(local::Store { access, values: vec![value] }),
        );
        results.push(append(
            &mut builder,
            block,
            Operation::LocalLoad(local::Load { access, part: 0, signed: false }),
        ));
    }
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(results));
    finish(builder)
}

fn operand_order() -> Program {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1), R(2)], &[R(0)]);
    let block = builder.block(Some(0x100));
    let a = builder.read(block, R(1));
    let b = builder.read(block, R(2));
    append(&mut builder, block, Operation::Add(a, b));
    let result = append(&mut builder, block, Operation::Add(b, a));
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(vec![result]));
    finish(builder)
}

fn inputs(word: Option<u32>, predicate: Option<bool>) -> Inputs {
    let registers = word
        .map(|bits| (Register::Gpr(1), Value::Word(bits)))
        .into_iter()
        .chain(predicate.map(|p| (Register::Predicate(0), Value::Predicate(p))))
        .collect();
    Inputs { registers, ..Default::default() }
}

pub(in crate::ir::tests) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    let words = [
        None,
        Some(0),
        Some(1),
        Some(4),
        Some(u32::MAX),
        Some(0x80000000),
        Some(0x80000001),
        Some(0x007fffff),
        Some(0x00800000),
        Some(0x7f800000),
        Some(0x7f812345),
        Some(0xffc12345),
    ];
    let cases = words
        .into_iter()
        .flat_map(|word| [None, Some(false), Some(true)].map(|p| inputs(word, p)))
        .collect::<Vec<_>>();
    vec![
        (diamond(false), cases.clone()),
        (diamond(true), cases.clone()),
        (changing_loop(), cases.clone()),
        (modifiers(), cases.clone()),
        (memory(), cases.clone()),
        (operand_order(), cases),
    ]
}

fn count(function: &Function, matches: impl Fn(&Operation) -> bool) -> usize {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches(&instruction.operation))
        .count()
}

#[test]
fn reuse_preserves_values_and_every_exhaustion_location() {
    for (original, cases) in comparison_cases() {
        let function = crate::ir::optimize::for_rendering(&original.function).unwrap();
        let optimized = Program { function };
        for input in &cases {
            for budget in 0..64 {
                assert_eq!(
                    optimized.interpret(input, budget),
                    original.interpret(input, budget),
                    "budget {budget}, inputs {input:?}"
                );
            }
        }
    }
}

#[test]
fn expressions_share_only_dominating_definitions() {
    for dominating in [false, true] {
        let original = diamond(dominating);
        let optimized = crate::ir::optimize::for_rendering(&original.function).unwrap();
        assert_eq!(
            count(&optimized, |op| matches!(op, Operation::Add(..))),
            if dominating { 4 } else { 6 }
        );
        assert_eq!(count(&optimized, |op| matches!(op, Operation::Copy(_))), 0);
        for predicate in [false, true] {
            let actual = Program { function: optimized.clone() }
                .interpret(&inputs(Some(7), Some(predicate)), 64)
                .unwrap()
                .into_outputs()
                .unwrap();
            assert_eq!(actual, [Value::Word(16), Value::Word(16)]);
        }
    }
    let original = changing_loop();
    let optimized = crate::ir::optimize::for_rendering(&original.function).unwrap();
    assert_eq!(count(&optimized, |op| matches!(op, Operation::Add(..))), 4);
    let result = Program { function: optimized }
        .interpret(&inputs(Some(1), None), 64)
        .unwrap()
        .into_outputs()
        .unwrap();
    assert_eq!(result, [Value::Word(46)]); // Four iterations of x = 2 * (x + 1).
}

#[test]
fn commutative_arithmetic_does_not_reorder_unavailable_origins() {
    let original = operand_order();
    let function = crate::ir::optimize::for_rendering(&original.function).unwrap();
    assert_eq!(count(&function, |op| matches!(op, Operation::Add(..))), 2);
    let optimized = Program { function };
    let input = Inputs::default();
    let expected = original.interpret(&input, 64).unwrap_err();
    assert!(expected.message.contains("Gpr(2)"), "{expected:?}");
    assert_eq!(optimized.interpret(&input, 64).unwrap_err(), expected);
}

#[test]
fn modifiers_and_mutable_reads_keep_distinct_semantics() {
    let original = modifiers();
    let optimized = crate::ir::optimize::for_rendering(&original.function).unwrap();
    assert_eq!(count(&optimized, |op| matches!(op, Operation::Float(_))), 8);
    for bits in [0, 1, 0x80000000, 0x80000001, 0x7f812345, 0xffc12345] {
        let expected = (0..8)
            .map(|flags| {
                let mut word = bits;
                if flags & 1 != 0 {
                    word &= 0x7fffffff;
                }
                if flags & 2 != 0 {
                    word ^= 0x80000000;
                }
                if flags & 4 != 0 && word & 0x7f800000 == 0 {
                    word &= 0x80000000;
                }
                Value::Word(word)
            })
            .collect::<Vec<_>>();
        let actual = Program { function: optimized.clone() }
            .interpret(&inputs(Some(bits), None), 64)
            .unwrap()
            .into_outputs()
            .unwrap();
        assert_eq!(actual, expected);
    }
    let original = memory();
    let optimized = crate::ir::optimize::for_rendering(&original.function).unwrap();
    assert_eq!(count(&optimized, |op| matches!(op, Operation::LocalLoad(_))), 2);
    assert_eq!(count(&optimized, |op| matches!(op, Operation::LocalStore(_))), 2);
    assert_eq!(
        Program { function: optimized }
            .interpret(&inputs(Some(0), None), 64)
            .unwrap()
            .into_outputs()
            .unwrap(),
        [Value::Word(7), Value::Word(9)]
    );
}
