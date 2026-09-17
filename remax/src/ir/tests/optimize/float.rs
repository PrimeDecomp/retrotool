use crate::{
    ir::{
        analysis::FloatFacts,
        convergence::BranchKind,
        float::{Binary, Operation as Float, ResultControl, Rounding},
        ssa::Builder,
        Definition, Edge, Operation, Terminator, Type,
    },
    program::{FloatModel, Inputs, InterpretOptions, Program, Register, Value},
};

fn modify(value: crate::ir::ValueId, flush_to_zero: bool) -> Operation {
    Operation::Float(Float::Modify { value, absolute: false, negate: false, flush_to_zero })
}

pub(in crate::ir::tests) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    use Register::{Gpr as R, Predicate as P};
    let mut builder = Builder::new(&[R(1), P(0)], &[R(0), R(2)]);
    let entry = builder.block(None);
    let raw = builder.read(entry, R(1));
    let condition = builder.read(entry, P(0));
    let flushed = builder.function.append(entry, 8, Type::Bits32, modify(raw, true));
    let repeated = builder.function.append(entry, 16, Type::Bits32, modify(flushed, true));
    let signed = builder.function.append(
        entry,
        24,
        Type::Bits32,
        Operation::Float(Float::Modify {
            value: repeated,
            absolute: true,
            negate: true,
            flush_to_zero: true,
        }),
    );
    let selected = builder.function.append(entry, 32, Type::Bits32, Operation::Select {
        condition,
        yes: signed,
        no: raw,
    });
    let result = builder.function.append(entry, 40, Type::Bits32, modify(selected, true));
    builder.function.blocks[entry.0].terminator = Some(Terminator::Return(vec![signed, result]));
    let function = builder.finish().0;
    function.verify().unwrap();
    let words = [
        0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x807fffff, 0x00800000, 0x80800000, 0x3f800000,
        0xbf800000, 0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000, 0x7fc12345, 0xffc12345,
        0x7f812345, 0xff812345,
    ];
    let mut cases = vec![Inputs::default()];
    for word in words {
        for predicate in [None, Some(false), Some(true)] {
            let mut registers = vec![(R(1), Value::Word(word))];
            registers.extend(predicate.map(|value| (P(0), Value::Predicate(value))));
            cases.push(Inputs { registers: registers.into_iter().collect(), ..Default::default() });
        }
    }
    vec![(Program { function }, cases)]
}

#[test]
fn redundant_flushing_preserves_bits_missing_inputs_and_exhaustion() {
    for (original, cases) in comparison_cases() {
        let optimized =
            Program { function: crate::ir::optimize::for_rendering(&original.function).unwrap() };
        let operations = &optimized.function.blocks[0].instructions;
        // Flush removal produces a copy; value reuse then keeps only its checkpoint.
        assert!(matches!(operations[1].operation, Operation::Nop));
        assert!(matches!(
            operations[2].operation,
            Operation::Float(Float::Modify {
                absolute: true,
                negate: true,
                flush_to_zero: false,
                ..
            })
        ));
        // A possible unflushed arm prevents eliminating the final flush.
        assert!(matches!(
            operations[4].operation,
            Operation::Float(Float::Modify { flush_to_zero: true, .. })
        ));
        for input in &cases {
            for budget in 0..8 {
                assert_eq!(original.interpret(input, budget), optimized.interpret(input, budget));
            }
        }
    }
}

#[test]
fn gradual_underflow_and_integer_bit_patterns_remain_distinct_from_float_results() {
    use Register::Gpr as R;
    let mut builder = Builder::new(&[R(1)], &[R(0), R(2), R(3), R(4), R(5)]);
    let block = builder.block(None);
    let input = builder.read(block, R(1));
    let half = builder.word(0.5f32.to_bits());
    let control =
        ResultControl { rounding: Rounding::NearestEven, flush_to_zero: false, saturate: true };
    let gradual = builder.function.append(
        block,
        8,
        Type::Bits32,
        Operation::Float(Float::Binary {
            op: Binary::Multiply { exponent: 0 },
            a: input,
            b: half,
            control,
        }),
    );
    let flushed = builder.function.append(block, 16, Type::Bits32, modify(gradual, true));
    let integer = builder.function.append(
        block,
        24,
        Type::Bits32,
        Operation::Float(Float::ToInteger {
            value: input,
            bits: 32,
            signed: false,
            rounding: Rounding::NearestEven,
        }),
    );
    let floating = builder.function.append(
        block,
        32,
        Type::Bits32,
        Operation::Float(Float::FromInteger {
            value: integer,
            bits: 32,
            signed: false,
            rounding: Rounding::NearestEven,
        }),
    );
    let rounded = builder.function.append(
        block,
        40,
        Type::Bits32,
        Operation::Float(Float::RoundIntegral { value: input, control }),
    );
    let results = vec![gradual, flushed, integer, floating, rounded];
    builder.function.blocks[block.0].terminator = Some(Terminator::Return(results.clone()));
    let function = builder.finish().0;
    function.verify().unwrap();
    let facts = FloatFacts::new(&function);
    assert_eq!(results.iter().map(|&v| facts.excludes_subnormal(v)).collect::<Vec<_>>(), [
        false, true, false, true, true
    ]);
    let program = Program { function };
    for word in [0, 1, 0x007fffff, 0x00800000, 0x00800001, 0x3f800000, 0x7f7fffff, 0x80800000] {
        let input = Inputs { registers: [(R(1), Value::Word(word))].into(), ..Default::default() };
        let output = program
            .interpret_with_options(&input, InterpretOptions {
                step_budget: 16,
                float_model: FloatModel::SoftwareIeee,
                ..Default::default()
            })
            .unwrap()
            .into_outputs()
            .unwrap();
        for (&value, output) in results.iter().zip(output) {
            if facts.excludes_subnormal(value) {
                let Value::Word(word) = output else { panic!("word result") };
                assert!(!f32::from_bits(word).is_subnormal());
            }
        }
    }
}

#[test]
fn loop_facts_require_both_entry_and_backedge_to_exclude_subnormals() {
    use Register::{Gpr as R, Predicate as P};
    for initial in [1, 0x80000000] {
        for (flush_backedge, carry_parameter) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut builder = Builder::new(&[R(1), P(0)], &[R(0)]);
            let entry = builder.block(None);
            let body = builder.block(Some(8));
            let end = builder.block(Some(32));
            let initial_value = builder.word(initial);
            let input = builder.read(entry, R(1));
            let condition = builder.read(entry, P(0));
            let parameter = builder
                .function
                .value(Type::Bits32, Definition::Parameter { block: body, index: 0 });
            builder.function.blocks[body.0].parameters.push(parameter);
            builder.function.blocks[entry.0].terminator =
                Some(Terminator::Jump(Edge::new(body, vec![initial_value])));
            let result = builder.function.append(body, 8, Type::Bits32, modify(parameter, true));
            let source = if carry_parameter { parameter } else { input };
            let backedge =
                builder.function.append(body, 16, Type::Bits32, modify(source, flush_backedge));
            builder.function.blocks[body.0].terminator = Some(Terminator::Branch {
                kind: BranchKind::ControlFlow,
                condition,
                yes: Edge::new(body, vec![backedge]),
                no: Edge::new(end, vec![]),
            });
            builder.function.blocks[end.0].terminator = Some(Terminator::Return(vec![result]));
            let function = builder.finish().0;
            function.verify().unwrap();
            assert_eq!(
                FloatFacts::new(&function).excludes_subnormal(parameter),
                initial != 1 && (flush_backedge || carry_parameter)
            );
            let optimized =
                Program { function: crate::ir::optimize::for_rendering(&function).unwrap() };
            let original = Program { function };
            for condition in [false, true] {
                let inputs = Inputs {
                    registers: [(R(1), Value::Word(1)), (P(0), Value::Predicate(condition))].into(),
                    ..Default::default()
                };
                for budget in 0..16 {
                    assert_eq!(
                        original.interpret(&inputs, budget),
                        optimized.interpret(&inputs, budget)
                    );
                }
            }
        }
    }
}
