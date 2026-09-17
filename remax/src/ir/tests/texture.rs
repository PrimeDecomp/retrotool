use super::{add, branch, bytes, isetp, offset, EXIT};
use crate::{
    ir::{texture as semantic, BlockId, Definition, Operation, Terminator, Type},
    maxwell::{texture::TextureDimension, MaxwellCode},
    program::{
        texture::{self, Coordinates as C, FetchLevel as F, Operation as T, SampleLevel as L},
        DiagnosticKind, FloatModel, Inputs, InterpretOptions, Program, Register, Signature,
        SpecialFunctionModel, Stage, Value,
    },
};

const OPTIONS: InterpretOptions = InterpretOptions {
    step_budget: 1000,
    float_model: FloatModel::Reject,
    special_functions: SpecialFunctionModel::Reject,
};
const RGBA: [u32; 4] = [0x7fc12345, 0x80000000, 0xdeadbeef, 0xffffffff];

#[test]
fn contiguous_tex_packs_masks_and_captures_sources_before_writes() {
    // Observatory Surface: TEX.NODEP R2,R2,RZ,0xa,2D,RB. Test every mask,
    // including masks TEXS cannot encode, with destinations aliasing coordinates.
    for mask in 0..16u64 {
        let word = (0xc03a00a2aff70202 & !(15 << 31)) | (mask << 31);
        let program = lift(
            &[word, EXIT],
            Stage::Fragment,
            &[2, 3, 4, 5].map(Register::Gpr),
            &[2, 3, 4, 5].map(Register::Gpr),
        );
        let mut calls = 0;
        let result = program
            .interpret_with_textures(
                &inputs(&[(2, 0x3e800000), (3, 0x3f000000), (4, 17), (5, 29)]),
                OPTIONS,
                &mut |request: &texture::Request| {
                    assert_eq!(request.descriptor_word_offset, 10);
                    assert!(request.skip_helpers);
                    assert_eq!(request.operation, T::Sample {
                        coordinates: C::D2([0x3e800000, 0x3f000000]),
                        level: L::Implicit,
                        comparison: None,
                    });
                    calls += 1;
                    Ok(texture::Response::Texel(RGBA))
                },
            )
            .unwrap()
            .into_outputs()
            .unwrap();
        let mut expected = [0x3e800000, 0x3f000000, 17, 29];
        for (target, value) in expected.iter_mut().zip(
            RGBA.into_iter()
                .enumerate()
                .filter(|(channel, _)| mask & (1 << channel) != 0)
                .map(|(_, value)| value),
        ) {
            *target = value;
        }
        assert_eq!(result, expected.map(Value::Word));
        assert_eq!(calls, 1);
    }
}

#[test]
fn contiguous_tex_keeps_layer_lod_and_comparison_in_distinct_groups() {
    // ARRAY_2D, explicit LOD, depth comparison, RGBA. Coordinate group starts
    // at odd R1; metadata starts at odd R7. TEX has no split-pair restriction.
    let word = 0xc038000000070000u64
        | (3 << 55)
        | (1 << 50)
        | (0x123 << 36)
        | (15 << 31)
        | (3 << 28)
        | (7 << 20)
        | (1 << 8)
        | 1;
    let program = lift(
        &[word, EXIT],
        Stage::Vertex,
        &[1, 2, 3, 7, 8].map(Register::Gpr),
        &[1, 2, 3, 4].map(Register::Gpr),
    );
    let result = program
        .interpret_with_textures(
            &inputs(&[(1, 0xffffabcd), (2, 11), (3, 12), (7, 13), (8, 14)]),
            OPTIONS,
            &mut |request: &texture::Request| {
                assert_eq!(request.operation, T::Sample {
                    coordinates: C::D2Array { xy: [11, 12], layer: 0xabcd },
                    level: L::Explicit(13),
                    comparison: Some(14),
                });
                Ok(texture::Response::Comparison(0.5f32.to_bits()))
            },
        )
        .unwrap()
        .into_outputs()
        .unwrap();
    assert_eq!(result, [0.5, 0.5, 0.5, 1.0].map(|v: f32| Value::Word(v.to_bits())));
}

#[test]
fn contiguous_tex_unresolved_modes_and_register_wraps_are_rejected() {
    let base = 0xc03a00a2aff70202u64;
    for word in [
        base | (1 << 35),  // NDV needs separate derivative participation semantics.
        base | (1 << 54),  // Packed sample offsets.
        base | (1 << 58),  // LOD clamp.
        base | (2 << 55),  // Bias.
        base & !(7 << 51), // Residency predicate.
        (base & !(7 << 28)) | (1 << 28), // 1D array.
        (base & !0xff00) | (255 << 8), // Coordinate span wraps past RZ.
        (base & !0xff) | 255, // Two results wrap past RZ.
    ] {
        let result =
            Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
                local_memory_bytes: 0,
                indexed_attributes: vec![],
                stage: Stage::Fragment,
                inputs: vec![],
                outputs: vec![],
            });
        assert_eq!(result.unwrap_err().kind, DiagnosticKind::Unsupported, "{word:016x}");
    }
}

fn instruction(load: bool, mode: u8, mask: u8, dst: [u8; 2], src: [u8; 2]) -> u64 {
    (if load { 0xda00000000000000 } else { 0xd800000000000000 })
        | (u64::from(mode) << 53)
        | (u64::from(mask) << 50)
        | (0x123 << 36)
        | (u64::from(dst[1]) << 28)
        | (u64::from(src[1]) << 20)
        | (7 << 16)
        | (u64::from(src[0]) << 8)
        | u64::from(dst[0])
}

fn lift(words: &[u64], stage: Stage, inputs: &[Register], outputs: &[Register]) -> Program {
    Program::lift(MaxwellCode::parse(&bytes(words)).unwrap(), &Signature {
        local_memory_bytes: 0,
        indexed_attributes: vec![],
        stage,
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
    })
    .unwrap()
}

fn inputs(words: &[(u8, u32)]) -> Inputs {
    Inputs {
        registers: words.iter().map(|&(r, v)| (Register::Gpr(r), Value::Word(v))).collect(),
        ..Default::default()
    }
}

#[test]
fn swizzled_texture_modes_preserve_the_full_request() {
    // Independent mode layouts cross-checked against Switch recompiler sources.
    // Distinct raw words expose swapped coordinate, LOD and comparison operands.
    let [a, b, c, d] = [0x1234abcd, 0x3f400000, 0x4060009e, 0x3e8000d7];
    let sample = [
        (0, C::D1(a), L::Explicit(0), None),
        (1, C::D2([a, c]), L::Implicit, None),
        (2, C::D2([a, c]), L::Explicit(0), None),
        (3, C::D2([a, b]), L::Explicit(c), None),
        (4, C::D2([a, b]), L::Implicit, Some(c)),
        (5, C::D2([a, b]), L::Explicit(c), Some(d)),
        (6, C::D2([a, b]), L::Explicit(0), Some(c)),
        (7, C::D2Array { xy: [b, c], layer: 0xabcd }, L::Implicit, None),
        (8, C::D2Array { xy: [b, c], layer: 0xabcd }, L::Explicit(0), None),
        (9, C::D2Array { xy: [b, c], layer: 0xabcd }, L::Explicit(0), Some(d)),
        (10, C::D3([a, b, c]), L::Implicit, None),
        (11, C::D3([a, b, c]), L::Explicit(0), None),
        (12, C::Cube([a, b, c]), L::Implicit, None),
        (13, C::Cube([a, b, c]), L::Explicit(d), None),
    ];
    let load = [
        (0, C::D1(a), F::Mip(0), None),
        (1, C::D1(a), F::Mip(c), None),
        (2, C::D2([a, c]), F::Mip(0), None),
        (4, C::D2([a, b]), F::Mip(0), Some([-2, -7])),
        (5, C::D2([a, b]), F::Mip(c), None),
        (6, C::D2([a, b]), F::Sample(c), None),
        (7, C::D3([a, b, c]), F::Mip(0), None),
        (8, C::D2Array { xy: [c, d], layer: 0xabcd }, F::Mip(0), None),
        (12, C::D2([a, b]), F::Mip(c), Some([7, -3])),
    ];
    let cases = sample
        .into_iter()
        .map(|(m, coordinates, level, comparison)| {
            (false, m, T::Sample { coordinates, level, comparison })
        })
        .chain(load.into_iter().map(|(m, coordinates, level, offset)| {
            (true, m, T::Load { coordinates, level, offset })
        }));
    for (is_load, mode, expected) in cases {
        for skip_helpers in [false, true] {
            let word =
                instruction(is_load, mode, 4, [0, 4], [6, 10]) | (u64::from(skip_helpers) << 49);
            let program = lift(
                &[word, EXIT],
                Stage::Fragment,
                &[6, 7, 10, 11].map(Register::Gpr),
                &[0, 1, 4, 5].map(Register::Gpr),
            );
            let mut calls = 0;
            let comparison = matches!(expected, T::Sample { comparison: Some(_), .. });
            let result = program
                .interpret_with_textures(
                    &inputs(&[(6, a), (7, b), (10, c), (11, d)]),
                    OPTIONS,
                    &mut |request: &texture::Request| {
                        assert_eq!(*request, texture::Request {
                            descriptor_word_offset: 0x123,
                            stage: Stage::Fragment,
                            pc: 8,
                            occurrence: 0,
                            skip_helpers,
                            operation: expected,
                        });
                        calls += 1;
                        // A synthetic live-invocation trace; no filtering or helper simulation.
                        Ok(if comparison {
                            texture::Response::Comparison(RGBA[0])
                        } else {
                            texture::Response::Texel(RGBA)
                        })
                    },
                )
                .unwrap()
                .into_outputs()
                .expect("live invocation");
            let expected =
                if comparison { [RGBA[0], RGBA[0], RGBA[0], 1.0f32.to_bits()] } else { RGBA };
            assert_eq!(result, expected.map(Value::Word));
            assert_eq!(calls, 1);
        }
    }
}

#[test]
fn component_masks_pack_one_sample_across_both_register_groups() {
    use Register::Gpr as R;
    let masks: &[(u8, &[&[usize]])] = &[
        (255, &[&[0], &[1], &[2], &[3], &[0, 1], &[0, 3], &[1, 3], &[2, 3]]),
        (4, &[&[0, 1, 2], &[0, 1, 3], &[0, 2, 3], &[1, 2, 3], &[0, 1, 2, 3]]),
    ];
    for &(second, masks) in masks {
        for (mask, channels) in masks.iter().enumerate() {
            let program = lift(
                &[instruction(false, 0, mask as u8, [0, second], [6, 255]), EXIT],
                Stage::Vertex,
                &[R(0), R(1), R(4), R(5), R(6)],
                &[R(0), R(1), R(4), R(5)],
            );
            let mut calls = 0;
            let result = program
                .interpret_with_textures(
                    &inputs(&[(0, 99), (1, 99), (4, 99), (5, 99), (6, 0)]),
                    OPTIONS,
                    &mut |_: &texture::Request| {
                        calls += 1;
                        Ok(texture::Response::Texel(RGBA))
                    },
                )
                .unwrap()
                .into_outputs()
                .expect("live invocation");
            let mut expected = [99; 4];
            for (dst, &channel) in expected.iter_mut().zip(*channels) {
                *dst = RGBA[channel];
            }
            assert_eq!(result, expected.map(Value::Word), "second={second} mask={mask}");
            assert_eq!(calls, 1);
        }
    }
}

#[test]
fn sources_are_captured_before_any_aliased_destination_is_written() {
    let program = lift(
        &[instruction(false, 3, 4, [0, 4], [0, 4]), EXIT],
        Stage::Fragment,
        &[0, 1, 4].map(Register::Gpr),
        &[0, 1, 4, 5].map(Register::Gpr),
    );
    let result = program
        .interpret_with_textures(
            &inputs(&[(0, 11), (1, 22), (4, 33)]),
            OPTIONS,
            &mut |request: &texture::Request| {
                assert_eq!(request.operation, T::Sample {
                    coordinates: C::D2([11, 22]),
                    level: L::Explicit(33),
                    comparison: None
                });
                Ok(texture::Response::Texel(RGBA))
            },
        )
        .unwrap()
        .into_outputs()
        .expect("live invocation");
    assert_eq!(result, RGBA.map(Value::Word));
}

#[test]
fn predication_guards_inputs_and_provider_even_when_the_result_is_discarded() {
    use Register::{Gpr as R, Predicate as P};
    let word = instruction(false, 1, 0, [0, 255], [6, 10]) & !(7 << 16);
    let program = lift(&[word, EXIT], Stage::Fragment, &[R(0), R(6), R(10), P(0)], &[R(0)]);
    let mut data = inputs(&[(0, 123)]);
    data.registers.insert(P(0), Value::Predicate(false));
    assert_eq!(program.interpret(&data, 100).unwrap().into_outputs().expect("live invocation"), [
        Value::Word(123)
    ]);
    data.registers.insert(P(0), Value::Predicate(true));
    let error = program
        .interpret_with_textures(
            &data,
            OPTIONS,
            &mut |_: &texture::Request| -> Result<_, texture::Error> {
                panic!("missing coordinate")
            },
        )
        .unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::MissingInput, Some(8)));

    let discarded =
        lift(&[instruction(false, 0, 0, [255, 255], [255, 255]), EXIT], Stage::Vertex, &[], &[]);
    assert_eq!(
        discarded.interpret(&Inputs::default(), 100).unwrap_err().kind,
        DiagnosticKind::MissingInput
    );
    let mut calls = 0;
    assert_eq!(
        discarded
            .interpret_with_textures(
                &Inputs::default(),
                OPTIONS,
                &mut |request: &texture::Request| {
                    assert_eq!(request.operation, T::Sample {
                        coordinates: C::D1(0),
                        level: L::Explicit(0),
                        comparison: None
                    });
                    calls += 1;
                    Ok(texture::Response::Texel(RGBA))
                }
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation"),
        []
    );
    assert_eq!(calls, 1);
}

#[test]
fn provider_failures_and_response_mismatches_keep_the_machine_pc() {
    let program =
        lift(&[instruction(false, 0, 0, [255, 255], [255, 255]), EXIT], Stage::Vertex, &[], &[]);
    for (cause, kind) in [
        (texture::Error::Unavailable("quad".into()), DiagnosticKind::MissingInput),
        (texture::Error::InvalidResource("view".into()), DiagnosticKind::InvalidInput),
        (texture::Error::Unsupported("sampler".into()), DiagnosticKind::Unsupported),
    ] {
        let error = program
            .interpret_with_textures(&Inputs::default(), OPTIONS, &mut |_: &texture::Request| {
                Err(cause.clone())
            })
            .unwrap_err();
        assert_eq!((error.kind, error.pc), (kind, Some(8)));
        assert!(error.message.contains("0x123"));
    }
    for compare in [false, true] {
        let p = lift(
            &[instruction(false, if compare { 6 } else { 2 }, 0, [0, 255], [6, 10]), EXIT],
            Stage::Fragment,
            &[6, 7, 10].map(Register::Gpr),
            &[],
        );
        let error = p
            .interpret_with_textures(
                &inputs(&[(6, 0), (7, 0), (10, 0)]),
                OPTIONS,
                &mut |_: &texture::Request| {
                    Ok(if compare {
                        texture::Response::Texel(RGBA)
                    } else {
                        texture::Response::Comparison(0)
                    })
                },
            )
            .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::InvalidInput, Some(8)));
    }
}

#[test]
fn repeated_samples_are_not_coalesced_and_keep_implicit_lod() {
    use Register::Gpr as R;
    let words = [
        instruction(false, 1, 0, [0, 255], [6, 10]),
        add(20, 20, 1),
        isetp(0, 7, 20, 21, 1, 7, 7),
        branch(3, 0, 0),
        EXIT,
    ];
    let p = lift(&words, Stage::Fragment, &[R(6), R(10), R(20), R(21)], &[R(0)]);
    for _ in 0..2 {
        let mut calls = 0;
        let result = p
            .interpret_with_textures(
                &inputs(&[(6, 11), (10, 22), (20, 0), (21, 3)]),
                OPTIONS,
                &mut |r: &texture::Request| {
                    assert_eq!((r.pc, r.occurrence), (8, calls));
                    assert_eq!(r.operation, T::Sample {
                        coordinates: C::D2([11, 22]),
                        level: L::Implicit,
                        comparison: None
                    });
                    calls += 1;
                    Ok(texture::Response::Texel([calls as u32; 4]))
                },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        assert_eq!(calls, 3);
        assert_eq!(result, [Value::Word(3)]);
    }
}

#[test]
fn packed_texel_offsets_are_signed_nibbles() {
    let p = lift(
        &[instruction(true, 12, 0, [0, 255], [6, 10]), EXIT],
        Stage::Vertex,
        &[6, 7, 10, 11].map(Register::Gpr),
        &[],
    );
    for x in -8i32..8 {
        for y in -8i32..8 {
            let packed = 0xffffff00 | (x as u32 & 15) | ((y as u32 & 15) << 4);
            p.interpret_with_textures(
                &inputs(&[(6, 123), (7, 456), (10, 7), (11, packed)]),
                OPTIONS,
                &mut |r: &texture::Request| {
                    assert_eq!(r.operation, T::Load {
                        coordinates: C::D2([123, 456]),
                        level: F::Mip(7),
                        offset: Some([x, y])
                    });
                    Ok(texture::Response::Texel(RGBA))
                },
            )
            .unwrap()
            .into_outputs()
            .expect("live invocation");
        }
    }
}

#[test]
fn unused_source_groups_and_reserved_registers_do_not_require_inputs() {
    for (word, data, expected) in [
        (instruction(false, 0, 0, [0, 255], [6, 17]), inputs(&[(6, 77)]), C::D1(77)),
        (instruction(false, 3, 0, [0, 255], [254, 255]), inputs(&[(254, 77)]), C::D2([77, 0])),
    ] {
        let registers = data.registers.keys().copied().collect::<Vec<_>>();
        let p = lift(&[word, EXIT], Stage::Vertex, &registers, &[Register::Gpr(0)]);
        p.interpret_with_textures(&data, OPTIONS, &mut |r: &texture::Request| {
            assert_eq!(r.operation, T::Sample {
                coordinates: expected,
                level: L::Explicit(0),
                comparison: None
            });
            Ok(texture::Response::Texel(RGBA))
        })
        .unwrap()
        .into_outputs()
        .expect("live invocation");
    }
}

#[test]
fn unresolved_modes_and_invalid_register_groups_fail_at_lift() {
    let base = instruction(false, 3, 4, [0, 4], [6, 10]);
    for word in [
        instruction(false, 14, 0, [0, 255], [6, 10]),
        instruction(true, 3, 0, [0, 255], [6, 10]),
        instruction(false, 2, 5, [0, 4], [6, 10]),
        instruction(false, 3, 4, [0, 4], [7, 10]),
        instruction(false, 3, 4, [1, 4], [6, 10]),
        instruction(false, 3, 4, [0, 5], [6, 10]),
        instruction(false, 3, 4, [0, 0], [6, 10]),
        instruction(false, 3, 4, [0, 4], [255, 10]),
        base & !(1 << 59), // packed half-result family is not supported
    ] {
        let error = Program::lift(MaxwellCode::parse(&bytes(&[word, EXIT])).unwrap(), &Signature {
            indexed_attributes: vec![],
            stage: Stage::Fragment,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)), "{word:016x}");
    }
    let error = Program::lift(
        MaxwellCode::parse(&bytes(&[instruction(false, 1, 0, [0, 255], [6, 10]), EXIT])).unwrap(),
        &Signature { indexed_attributes: vec![], stage: Stage::Vertex, ..Default::default() },
    )
    .unwrap_err();
    assert_eq!((error.kind, error.pc), (DiagnosticKind::Unsupported, Some(8)));
}

#[test]
fn verifier_checks_texture_shape_operand_and_result_types() {
    let base = lift(
        &[instruction(false, 1, 0, [0, 255], [6, 10]), EXIT],
        Stage::Fragment,
        &[6, 10].map(Register::Gpr),
        &[],
    )
    .function;
    for mutation in 0..12 {
        let mut f = base.clone();
        let predicate = f.value(Type::Predicate, Definition::Predicate(true));
        let scalar = f.value(Type::Bits32, Definition::Word(0));
        let texture = f
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.operation, Operation::Texture(_)))
            .unwrap();
        let Operation::Texture(op) = &mut texture.operation else { unreachable!() };
        match mutation {
            0 => {
                op.coordinates.pop();
            }
            1 => op.descriptor = 0x2000,
            2 => op.layer = Some(scalar),
            3 => op.dimension = TextureDimension::D2Array,
            4 => f.stage = Stage::Vertex,
            5 => op.coordinates[0] = predicate,
            6 => {
                op.kind = semantic::Kind::Sample {
                    level: semantic::SampleLevel::Explicit(predicate),
                    comparison: None,
                }
            }
            7 => {
                op.kind = semantic::Kind::Sample {
                    level: semantic::SampleLevel::Implicit,
                    comparison: Some(predicate),
                }
            }
            8 => f.values[texture.result.unwrap().0].ty = Type::Bits32,
            9 => {
                op.dimension = TextureDimension::Cube;
                op.coordinates.push(scalar);
                op.kind =
                    semantic::Kind::Load { level: semantic::FetchLevel::Mip(scalar), offset: None };
            }
            10 => {
                op.kind = semantic::Kind::Load {
                    level: semantic::FetchLevel::Sample(scalar),
                    offset: Some([scalar; 2]),
                }
            }
            11 => {
                op.kind = semantic::Kind::Load {
                    level: semantic::FetchLevel::Mip(scalar),
                    offset: Some([scalar, predicate]),
                }
            }
            _ => unreachable!(),
        }
        let error = f.verify().unwrap_err();
        assert_eq!(
            (error.kind, error.pc),
            (DiagnosticKind::InvalidIr, Some(8)),
            "mutation={mutation}"
        );
    }
    for mutation in 0..2 {
        let mut f = base.clone();
        let scalar = f.value(Type::Bits32, Definition::Word(0));
        let component = f
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find(|i| matches!(i.operation, Operation::Component { .. }))
            .unwrap();
        if let Operation::Component { vector, index } = &mut component.operation {
            if mutation == 0 {
                *index = 4;
            } else {
                *vector = scalar;
            }
        }
        assert_eq!(f.verify().unwrap_err().kind, DiagnosticKind::InvalidIr);
    }
}

#[test]
fn vector_results_survive_copy_select_and_block_arguments() {
    // Exercise the IR's vector path independently from scalar register writes.
    let mut f =
        lift(&[instruction(false, 0, 0, [0, 255], [255, 255]), EXIT], Stage::Vertex, &[], &[
            Register::Gpr(0),
        ])
        .function;
    let vector = f
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find(|i| matches!(i.operation, Operation::Texture(_)))
        .unwrap()
        .result
        .unwrap();
    let block = BlockId(
        f.blocks.iter().position(|b| matches!(b.terminator, Some(Terminator::Return(_)))).unwrap(),
    );
    let param = f.value(Type::Words4, Definition::Parameter { block, index: 0 });
    f.blocks[block.0].parameters.push(param);
    for b in &mut f.blocks {
        for edge in b.terminator.as_mut().unwrap().edges_mut() {
            if edge.target == block {
                edge.arguments.push(vector);
            }
        }
    }
    let copy = f.append(block, offset(1), Type::Words4, Operation::Copy(param));
    let condition = f.value(Type::Predicate, Definition::Predicate(false));
    let selected = f.append(block, offset(1), Type::Words4, Operation::Select {
        condition,
        yes: vector,
        no: copy,
    });
    let component = f.append(block, offset(1), Type::Bits32, Operation::Component {
        vector: selected,
        index: 3,
    });
    f.blocks[block.0].terminator = Some(Terminator::Return(vec![component]));
    f.verify().unwrap();
    assert_eq!(
        f.interpret_with_options(&Inputs::default(), OPTIONS, crate::program::Providers {
            textures: Some(&mut |_: &texture::Request| Ok(texture::Response::Texel(RGBA))),
            memory: None
        })
        .unwrap()
        .into_outputs()
        .expect("live invocation"),
        [Value::Word(RGBA[3])]
    );
}
