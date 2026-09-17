use crate::{
    ir::{
        analysis::{Constants, StepBounds},
        convergence::BranchKind,
        ssa::Builder,
        BlockId, Edge, Function, Operation, Terminator, Type,
    },
    program::{Inputs, Program, Register, Value},
    resources::{ConstantBank, ConstantMemory},
};

pub(in crate::ir::tests) mod bitset;

/// Unequal branch lengths join before an optional cycle. The exit can be
/// reached either directly or through the cycle; only its acyclic form is bounded.
fn function(cyclic: bool) -> Function {
    use Register::{Gpr as R, Predicate as P};
    let mut builder = Builder::new(&[R(1), P(0), P(1), P(2)], &[R(0)]);
    let blocks = (0..6)
        .map(|index| builder.block((index != 0).then_some(index * 0x100)))
        .collect::<Vec<_>>();
    let conditions = [P(0), P(1), P(2)].map(|register| builder.read(blocks[0], register));
    let address = builder.read(blocks[0], R(1));
    let output = builder.word(17);
    for (index, count) in [2, 1, 4, 3, 2, 0].into_iter().enumerate() {
        for instruction in 0..count {
            let pc = index * 0x100 + (instruction + 1) * 8;
            if index == 0 && instruction == 1 {
                // Retain a resource effect even though the loaded result is dead.
                builder.function.append(
                    blocks[index],
                    pc,
                    Type::Bits32,
                    Operation::LoadConstantWord {
                        bank: 3,
                        address,
                        bytes: 4,
                        part: 0,
                        signed: false,
                    },
                );
            } else {
                builder.function.append_effect(blocks[index], pc, Operation::Nop);
            }
        }
    }
    let edge = |index| Edge::new(blocks[index], vec![]);
    let branch = |condition, yes, no| Terminator::Branch {
        kind: BranchKind::ControlFlow,
        condition,
        yes: edge(yes),
        no: edge(no),
    };
    let terms = [
        branch(conditions[0], 1, 2),
        Terminator::Jump(edge(3)),
        Terminator::Jump(edge(3)),
        branch(conditions[1], 4, 5),
        if cyclic { branch(conditions[2], 4, 5) } else { Terminator::Jump(edge(5)) },
        Terminator::Return(vec![output]),
    ];
    for (block, term) in builder.function.blocks.iter_mut().zip(terms) {
        block.terminator = Some(term);
    }
    let function = builder.finish().0;
    function.verify().unwrap();
    function
}

#[test]
fn a_cycle_does_not_erase_bounds_before_it_or_bound_paths_after_it() {
    for cyclic in [false, true] {
        let function = function(cyclic);
        let bounds = StepBounds::new(&function, &Constants::new(&function));
        assert_eq!(
            (0..6).map(|index| bounds.before(BlockId(index))).collect::<Vec<_>>(),
            if cyclic {
                vec![Some(0), Some(3), Some(3), Some(8), None, None]
            } else {
                vec![Some(0), Some(3), Some(3), Some(8), Some(12), Some(15)]
            }
        );
        assert_eq!(bounds.maximum(), if cyclic { None } else { Some(16) });

        // Independently enumerate CFG walks, including repeated loop entries.
        // Every published bound must hold for every visit, not just the first.
        let mut walks = vec![(BlockId(0), 0u64)];
        while let Some((id, before)) = walks.pop() {
            if let Some(bound) = bounds.before(id) {
                assert!(before <= bound, "block {id:?} consumed {before}, bound {bound}");
            }
            let block = &function.blocks[id.0];
            let after = before + block.instructions.len() as u64 + 1;
            if after < 64 {
                walks.extend(
                    block
                        .terminator
                        .as_ref()
                        .unwrap()
                        .edges()
                        .iter()
                        .map(|edge| (edge.target, after)),
                );
            }
        }
    }
}

pub(in crate::ir::tests) fn comparison_cases() -> Vec<(Program, Vec<Inputs>)> {
    use Register::{Gpr as R, Predicate as P};
    let memory = ConstantMemory::from_banks(&[ConstantBank {
        bank: 3,
        bytes: &[0; 8],
        known: Some(&[true, true, true, true, false, false, false, false]),
    }])
    .unwrap();
    let mut cases = Vec::new();
    for predicates in 0..8 {
        for address in [None, Some(0), Some(1), Some(4), Some(8)] {
            let mut input = Inputs { constants: memory.clone(), ..Default::default() };
            for index in 0..3 {
                input.registers.insert(P(index), Value::Predicate(predicates & (1 << index) != 0));
            }
            input.registers.extend(address.map(|word| (R(1), Value::Word(word))));
            cases.push(input);
        }
    }
    for missing in 0..3 {
        let mut input = cases[1].clone();
        input.registers.remove(&P(missing));
        cases.push(input);
    }
    [false, true].map(|cyclic| (Program { function: function(cyclic) }, cases.clone())).into()
}
