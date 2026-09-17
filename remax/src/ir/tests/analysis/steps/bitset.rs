use crate::{
    ir::{
        analysis::Analysis, convergence::BranchKind, ssa::Builder, BlockId, Definition, Edge,
        Function, Logic, Operation, Relation, Terminator, Type, ValueId,
    },
    program::{DiagnosticKind, Inputs, Program, Register, Value},
};

struct Fixture {
    program: Program,
    body: BlockId,
    mask: ValueId,
    field: Option<ValueId>,
    shift: Option<ValueId>,
    comparison: ValueId,
}

/// None uses mask & (mask - 1). Some uses a wrapped/unwrapped shift by
/// CLZ(reverse(mask)), with the copy/extraction/predicate spelling found in MPR.
fn fixture(shift: Option<bool>, backedge_on_true: bool) -> Fixture {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1)], &[R(0), R(2)]);
    let entry = builder.block(None);
    let body = builder.block(Some(0x100));
    let end = builder.block(Some(0x200));
    let input = builder.read(entry, R(1));
    let zero = builder.word(0);
    let one = builder.word(1);
    let width = builder.word(32);
    let minus_one = builder.word(u32::MAX);
    let always = builder.predicate(true);
    let mask =
        builder.function.value(Type::Bits32, Definition::Parameter { block: body, index: 0 });
    let counter =
        builder.function.value(Type::Bits32, Definition::Parameter { block: body, index: 1 });
    builder.function.blocks[body.0].parameters = vec![mask, counter];
    builder.function.blocks[entry.0].terminator =
        Some(Terminator::Jump(Edge::new(body, vec![input, zero])));
    let mut append = |ty, operation| {
        let pc = 0x108 + builder.function.blocks[body.0].instructions.len() * 8;
        builder.function.append(body, pc, ty, operation)
    };
    let (cleared, field, shift) = if let Some(wrap) = shift {
        let reverse = append(Type::Bits32, Operation::ReverseBits(mask));
        let field = append(Type::Bits32, Operation::ExtractBits {
            value: reverse,
            offset: zero,
            count: width,
            signed: false,
        });
        let index = append(Type::Bits32, Operation::CountLeadingZeros(field));
        let index = append(Type::Bits32, Operation::Copy(index));
        let shifted = append(Type::Bits32, Operation::Shift {
            a: one,
            b: index,
            right: false,
            signed: false,
            wrap,
        });
        let cleared = append(Type::Bits32, Operation::NotBits(shifted));
        (cleared, Some(field), Some(shifted))
    } else {
        (append(Type::Bits32, Operation::Add(mask, minus_one)), None, None)
    };
    let next = append(Type::Bits32, Operation::Logic { op: Logic::And, a: mask, b: cleared });
    let count = append(Type::Bits32, Operation::Add(counter, one));
    let comparison = append(Type::Predicate, Operation::Compare {
        op: Relation::Equal,
        a: next,
        b: zero,
        signed: false,
    });
    let condition = append(Type::Predicate, Operation::PredicateLogic {
        op: Logic::And,
        a: comparison,
        b: always,
    });
    let condition = if backedge_on_true {
        append(Type::Predicate, Operation::PredicateNot(condition))
    } else {
        condition
    };
    let carried = append(Type::Bits32, Operation::Copy(next));
    let backedge = Edge::new(body, vec![carried, count]);
    let exit = Edge::new(end, vec![]);
    let (yes, no) = if backedge_on_true { (backedge, exit) } else { (exit, backedge) };
    builder.function.blocks[body.0].terminator =
        Some(Terminator::Branch { kind: BranchKind::ControlFlow, condition, yes, no });
    builder.function.blocks[end.0].terminator = Some(Terminator::Return(vec![count, next]));
    let function = builder.finish().0;
    function.verify().unwrap();
    Fixture { program: Program { function }, body, mask, field, shift, comparison }
}

fn inputs(mask: u32) -> Inputs {
    Inputs { registers: [(Register::Gpr(1), Value::Word(mask))].into(), ..Default::default() }
}

fn masks() -> Vec<u32> {
    (0..256)
        .chain((0..32).flat_map(|bit| [1 << bit, !(1 << bit)]))
        .chain([u32::MAX, 0x55555555, 0xaaaaaaaa])
        .collect()
}

#[test]
fn bitset_loop_bounds_cover_every_visit_and_the_final_transfer() {
    for shift in [None, Some(false), Some(true)] {
        for backedge_on_true in [false, true] {
            let fixture = fixture(shift, backedge_on_true);
            let program = &fixture.program;
            let cost = program.function.blocks[fixture.body.0].instructions.len() as u64 + 1;
            let bounds = Analysis::new(&program.function).steps;
            assert_eq!(bounds.before(fixture.body), Some(1 + 31 * cost));
            assert_eq!(bounds.before(BlockId(2)), Some(1 + 32 * cost));
            assert_eq!(bounds.maximum(), Some(2 + 32 * cost));
            assert!(!bounds.is_acyclic());
            for mask in masks() {
                // Independent count_ones expectation, including the do-while's
                // single visit for an empty mask. One fewer step must fail at
                // the final return, not silently return a partial traversal.
                let visits = mask.count_ones().max(1);
                let budget = 2 + u64::from(visits) * cost;
                let input = inputs(mask);
                assert_eq!(program.interpret(&input, budget).unwrap().into_outputs().unwrap(), [
                    Value::Word(visits),
                    Value::Word(0)
                ]);
                let error = program.interpret(&input, budget - 1).unwrap_err();
                assert_eq!((error.kind, error.pc), (DiagnosticKind::StepLimit, Some(0x200)));
            }
        }
    }
}

fn operation(function: &mut Function, value: ValueId) -> &mut Operation {
    let Definition::Instruction { block, index } = function.values[value.0].definition else {
        panic!("instruction")
    };
    &mut function.blocks[block.0].instructions[index].operation
}

#[test]
fn near_matches_do_not_claim_progress_or_hide_a_larger_enclosing_cycle() {
    for change in 0..5 {
        let mut fixture = fixture(Some(false), true);
        let previous_bound = Analysis::new(&fixture.program.function).steps.maximum().unwrap();
        let function = &mut fixture.program.function;
        let two = function.value(Type::Bits32, Definition::Word(2));
        let narrow = function.value(Type::Bits32, Definition::Word(31));
        match change {
            0 => {
                function.blocks[fixture.body.0].terminator.as_mut().unwrap().edges_mut()[0]
                    .arguments[0] = fixture.mask
            }
            1 => {
                let Operation::Shift { a, .. } = operation(function, fixture.shift.unwrap()) else {
                    unreachable!()
                };
                *a = two;
            }
            2 => {
                let Operation::ExtractBits { count, .. } =
                    operation(function, fixture.field.unwrap())
                else {
                    unreachable!()
                };
                *count = narrow;
            }
            3 => {
                let Operation::Compare { a, .. } = operation(function, fixture.comparison) else {
                    unreachable!()
                };
                *a = fixture.mask;
            }
            4 => {
                let arguments =
                    function.blocks[0].terminator.as_ref().unwrap().edges()[0].arguments.clone();
                function.blocks[2].terminator =
                    Some(Terminator::Jump(Edge::new(fixture.body, arguments)));
            }
            _ => unreachable!(),
        }
        function.verify().unwrap();
        let bounds = Analysis::new(function).steps;
        assert!(bounds.maximum().is_none(), "change {change}");
        assert!(bounds.before(fixture.body).is_none());
        let mask = match change {
            0 => 3,        // The first clear leaves a bit, but the backedge restores it.
            3 => u32::MAX, // Testing the old mask allows a 33rd visit.
            _ => 1,
        };
        let error = fixture.program.interpret(&inputs(mask), previous_bound).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::StepLimit, "change {change}");
    }
}

pub(in crate::ir::tests) fn comparison_cases() -> Vec<(Program, Vec<Inputs>, u32)> {
    [None, Some(false), Some(true)]
        .into_iter()
        .map(|shift| {
            let fixture = fixture(shift, true);
            let bound = Analysis::new(&fixture.program.function).steps.maximum().unwrap() as u32;
            let cases = masks().into_iter().map(inputs).chain([Inputs::default()]).collect();
            (fixture.program, cases, bound)
        })
        .collect()
}
