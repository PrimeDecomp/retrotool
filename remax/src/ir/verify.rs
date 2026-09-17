use petgraph::{
    algo::dominators::simple_fast,
    graph::{DiGraph, NodeIndex},
};

use super::{attribute, Definition, Function, Operation, Terminator, Type, ValueId};
use crate::program::{Diagnostic, DiagnosticKind, InterpolationMode, Phase, Register, Stage};

impl Function {
    pub fn verify(&self) -> Result<(), Diagnostic> {
        let error = |pc, message: String| {
            Diagnostic::new(Phase::Verify, pc, DiagnosticKind::InvalidIr, message)
        };
        if self.blocks.is_empty() {
            return Err(error(None, "missing entry block".into()));
        }
        if self.indexed_attributes.iter().any(|&offset| {
            self.stage == Stage::Scalar
                || !(0x80..0x280).contains(&offset)
                || !offset.is_multiple_of(4)
        }) || self.indexed_attributes.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(error(None, "invalid indexed attribute domain".into()));
        }
        let valid_register = |r: Register, constant: bool| match r {
            Register::Gpr(r) => r != 255 || constant,
            Register::Predicate(r) => r < if constant { 8 } else { 7 },
            Register::Carry => true,
            Register::OutputAttribute(offset) => {
                self.stage == Stage::Vertex && offset < 0x400 && offset.is_multiple_of(4)
            }
        };
        if self.outputs.iter().any(|&r| !valid_register(r, true)) {
            return Err(error(None, "invalid output interface".into()));
        }
        let mut graph = DiGraph::<(), ()>::new();
        for _ in &self.blocks {
            graph.add_node(());
        }
        for (index, block) in self.blocks.iter().enumerate() {
            let term = block
                .terminator
                .as_ref()
                .ok_or_else(|| error(block.pc, format!("block {index} has no terminator")))?;
            for edge in term.edges() {
                let target = self
                    .blocks
                    .get(edge.target.0)
                    .ok_or_else(|| error(block.pc, "edge target is outside the function".into()))?;
                if edge.target.0 == 0 {
                    return Err(error(block.pc, "entry block has an incoming edge".into()));
                }
                if edge.arguments.len() != target.parameters.len() {
                    return Err(error(block.pc, "edge argument count mismatch".into()));
                }
                graph.add_edge(NodeIndex::new(index), NodeIndex::new(edge.target.0), ());
            }
        }
        if !self.blocks[0].parameters.is_empty() {
            return Err(error(None, "entry block has parameters".into()));
        }
        self.verify_convergence()?;
        let dominators = simple_fast(&graph, NodeIndex::new(0));
        let value = |id: ValueId, pc| {
            self.values
                .get(id.0)
                .ok_or_else(|| error(pc, format!("value {id:?} is outside the function")))
        };
        // Definition locations are checked independently from the builder.
        for (id, v) in self.values.iter().enumerate() {
            let expected = match v.definition {
                Definition::Word(_) => Type::Bits32,
                Definition::Predicate(_) => Type::Predicate,
                Definition::UnavailableLocal(offset) => {
                    if !offset.is_multiple_of(4)
                        || offset.checked_add(4).is_none_or(|end| end > self.local_memory_bytes)
                    {
                        return Err(error(None, "invalid uninitialized local word".into()));
                    }
                    Type::Bits32
                }
                Definition::Input(r) | Definition::Unavailable(r) => {
                    if !valid_register(r, false) {
                        return Err(error(None, "invalid incoming state location".into()));
                    }
                    r.ty()
                }
                Definition::Parameter { block, index } => {
                    if self.blocks.get(block.0).and_then(|b| b.parameters.get(index))
                        != Some(&ValueId(id))
                    {
                        return Err(error(None, "parameter definition location mismatch".into()));
                    }
                    v.ty
                }
                Definition::Instruction { block, index } => {
                    if self
                        .blocks
                        .get(block.0)
                        .and_then(|b| b.instructions.get(index))
                        .and_then(|i| i.result)
                        != Some(ValueId(id))
                    {
                        return Err(error(None, "instruction definition location mismatch".into()));
                    }
                    v.ty
                }
            };
            if expected != v.ty {
                return Err(error(None, "literal/input type mismatch".into()));
            }
        }
        for (block_id, block) in self.blocks.iter().enumerate() {
            let node = NodeIndex::new(block_id);
            let dom = dominators
                .dominators(node)
                .ok_or_else(|| error(block.pc, "unreachable block".into()))?
                .collect::<Vec<_>>();
            let check_use = |id: ValueId, position: usize, pc| -> Result<(), Diagnostic> {
                let v = value(id, pc)?;
                let (definition_block, definition_position) = match v.definition {
                    Definition::Parameter { block, .. } => (block, None),
                    Definition::Instruction { block, index } => (block, Some(index)),
                    _ => return Ok(()),
                };
                if !dom.contains(&NodeIndex::new(definition_block.0)) {
                    return Err(error(
                        pc,
                        format!("definition of {id:?} does not dominate its use"),
                    ));
                }
                if definition_block.0 == block_id
                    && definition_position.is_some_and(|index| index >= position)
                {
                    return Err(error(pc, format!("{id:?} is used before its definition")));
                }
                Ok(())
            };
            for (index, &id) in block.parameters.iter().enumerate() {
                if !matches!(value(id, block.pc)?.definition, Definition::Parameter { block, index: i } if block.0 == block_id && i == index)
                {
                    return Err(error(block.pc, "parameter identity mismatch".into()));
                }
            }
            for (index, instruction) in block.instructions.iter().enumerate() {
                let pc = Some(instruction.pc);
                if matches!(instruction.operation, Operation::Nop) {
                    if instruction.result.is_some() {
                        return Err(error(pc, "execution checkpoint has a value result".into()));
                    }
                    continue;
                }
                if matches!(instruction.operation, Operation::Demote) {
                    if instruction.result.is_some() || self.stage != Stage::Fragment {
                        return Err(error(
                            pc,
                            "demotion requires a resultless fragment effect".into(),
                        ));
                    }
                    continue;
                }
                if let Operation::LocalStore(ref store) = instruction.operation {
                    if instruction.result.is_some() || !store.valid() {
                        return Err(error(pc, "invalid local-store effect".into()));
                    }
                    for operand in instruction.operation.operands() {
                        check_use(operand, index, pc)?;
                        if value(operand, pc)?.ty != Type::Bits32 {
                            return Err(error(pc, "local store requires word operands".into()));
                        }
                    }
                    continue;
                }
                let result = value(
                    instruction.result.ok_or_else(|| {
                        error(pc, "value-producing operation has no result".into())
                    })?,
                    pc,
                )?;
                if !matches!(result.definition, Definition::Instruction { block, index: i } if block.0 == block_id && i == index)
                {
                    return Err(error(pc, "instruction result identity mismatch".into()));
                }
                for operand in instruction.operation.operands() {
                    check_use(operand, index, pc)?;
                }
                let require = |id, ty| -> Result<(), Diagnostic> {
                    if value(id, pc)?.ty != ty {
                        return Err(error(pc, format!("{id:?} must have type {ty:?}")));
                    }
                    Ok(())
                };
                let result_type = match instruction.operation {
                    Operation::Nop => unreachable!("verified checkpoint above"),
                    Operation::Demote => unreachable!("verified demotion above"),
                    Operation::LocalStore(_) => unreachable!("verified effect above"),
                    Operation::LocalLoad(load) => {
                        require(load.access.address, Type::Bits32)?;
                        if !load.valid() {
                            return Err(error(pc, "invalid local load".into()));
                        }
                        Type::Bits32
                    }
                    Operation::Copy(v) => value(v, pc)?.ty,
                    Operation::AttributeAddress { base, offset } => {
                        require(offset, Type::Bits32)?;
                        if self.stage == Stage::Scalar
                            || !(0x80..0x280).contains(&base)
                            || !base.is_multiple_of(4)
                        {
                            return Err(error(pc, "invalid logical attribute address".into()));
                        }
                        Type::AttributeAddress
                    }
                    Operation::Reduce { kind, value: v } => {
                        require(v, Type::Bits32)?;
                        Type::Reduced(kind)
                    }
                    Operation::Add(a, b)
                    | Operation::Multiply(a, b)
                    | Operation::Logic { a, b, .. }
                    | Operation::Shift { a, b, .. } => {
                        require(a, Type::Bits32)?;
                        require(b, Type::Bits32)?;
                        Type::Bits32
                    }
                    Operation::Negate(v)
                    | Operation::NotBits(v)
                    | Operation::ReverseBits(v)
                    | Operation::CountLeadingZeros(v) => {
                        require(v, Type::Bits32)?;
                        Type::Bits32
                    }
                    Operation::ExtractBits { value, offset, count, .. } => {
                        require(value, Type::Bits32)?;
                        require(offset, Type::Bits32)?;
                        require(count, Type::Bits32)?;
                        Type::Bits32
                    }
                    Operation::Compare { a, b, .. } => {
                        require(a, Type::Bits32)?;
                        require(b, Type::Bits32)?;
                        Type::Predicate
                    }
                    Operation::PredicateNot(v) => {
                        require(v, Type::Predicate)?;
                        Type::Predicate
                    }
                    Operation::PredicateLogic { a, b, .. } => {
                        require(a, Type::Predicate)?;
                        require(b, Type::Predicate)?;
                        Type::Predicate
                    }
                    Operation::Select { condition, yes, no } => {
                        require(condition, Type::Predicate)?;
                        require(no, value(yes, pc)?.ty)?;
                        value(yes, pc)?.ty
                    }
                    Operation::ReadConstant(address) => {
                        if address.bank >= 32 || !address.byte_offset.is_multiple_of(4) {
                            return Err(error(pc, "invalid constant word address".into()));
                        }
                        Type::Bits32
                    }
                    Operation::LoadConstantWord { bank, address, bytes, part, signed } => {
                        require(address, Type::Bits32)?;
                        if bank >= 32
                            || !matches!((bytes, part), (1 | 2 | 4, 0) | (8, 0 | 4))
                            || (signed && bytes > 2)
                        {
                            return Err(error(
                                pc,
                                "invalid constant load width/result part".into(),
                            ));
                        }
                        Type::Bits32
                    }
                    Operation::ReadSampleIndex => {
                        if self.stage != Stage::Fragment {
                            return Err(error(
                                pc,
                                "sample index read outside a fragment interface".into(),
                            ));
                        }
                        Type::Bits32
                    }
                    Operation::ReadAttribute(input) => {
                        if self.stage != Stage::Vertex {
                            return Err(error(pc, "invalid vertex attribute read".into()));
                        }
                        match input {
                            attribute::Input::Direct(offset)
                                if offset < 0x400 && offset.is_multiple_of(4) => {}
                            attribute::Input::Indexed(address) => {
                                require(address, Type::AttributeAddress)?
                            }
                            _ => return Err(error(pc, "invalid vertex attribute address".into())),
                        }
                        Type::Bits32
                    }
                    Operation::Float(ref op) => {
                        for operand in op.operands() {
                            require(operand, op.operand_type())?;
                        }
                        if matches!(op, super::float::Operation::FromInteger { bits, .. } if !matches!(bits, 8 | 16 | 32))
                        {
                            return Err(error(pc, "invalid integer-to-float source width".into()));
                        }
                        if matches!(op, super::float::Operation::ToInteger { bits, .. } if !matches!(bits, 16 | 32))
                        {
                            return Err(error(
                                pc,
                                "invalid float-to-integer destination width".into(),
                            ));
                        }
                        op.result_type()
                    }
                    Operation::Texture(ref op) => {
                        if !op.valid(self.stage) {
                            return Err(error(pc, "invalid texture operation/interface".into()));
                        }
                        for operand in op.operands() {
                            require(operand, Type::Bits32)?;
                        }
                        Type::Words4
                    }
                    Operation::GlobalLoad(ref load) => {
                        if !load.valid() {
                            return Err(error(pc, "invalid global load width/modifier".into()));
                        }
                        for address in load.address {
                            require(address, Type::Bits32)?;
                        }
                        Type::Words4
                    }
                    Operation::Component { vector, index } => {
                        require(vector, Type::Words4)?;
                        if index >= 4 {
                            return Err(error(pc, "vector component is out of range".into()));
                        }
                        Type::Bits32
                    }
                    Operation::Interpolate { attribute, mode, location, multiplier, .. } => {
                        if self.stage != Stage::Fragment
                            || mode == InterpolationMode::State
                            || (mode == InterpolationMode::Constant
                                && matches!(location, super::InterpolationLocation::Offset(_)))
                            || (mode == InterpolationMode::Perspective) != multiplier.is_some()
                        {
                            return Err(error(
                                pc,
                                "invalid interpolation operation/interface".into(),
                            ));
                        }
                        match attribute {
                            attribute::Input::Direct(offset)
                                if offset < 0x400 && offset.is_multiple_of(4) => {}
                            attribute::Input::Indexed(address) => {
                                require(address, Type::AttributeAddress)?
                            }
                            _ => {
                                return Err(error(
                                    pc,
                                    "invalid interpolation attribute address".into(),
                                ))
                            }
                        }
                        if let Some(value) = multiplier {
                            require(value, Type::Bits32)?;
                        }
                        if let super::InterpolationLocation::Offset(value) = location {
                            require(value, Type::Bits32)?;
                        }
                        Type::Bits32
                    }
                };
                if result_type != result.ty {
                    return Err(error(pc, "operation result type mismatch".into()));
                }
            }
            let term = block.terminator.as_ref().unwrap();
            for operand in term.operands() {
                check_use(operand, block.instructions.len(), block.pc)?;
            }
            if let Terminator::Branch { condition, .. } = term {
                if value(*condition, block.pc)?.ty != Type::Predicate {
                    return Err(error(block.pc, "branch condition is not a predicate".into()));
                }
            }
            if let Terminator::Return(values) = term {
                if values.len() != self.outputs.len() {
                    return Err(error(block.pc, "return arity mismatch".into()));
                }
                for (&id, &output) in values.iter().zip(&self.outputs) {
                    if value(id, block.pc)?.ty != output.ty() {
                        return Err(error(block.pc, "return type mismatch".into()));
                    }
                }
            }
            for edge in term.edges() {
                for (&argument, &parameter) in
                    edge.arguments.iter().zip(&self.blocks[edge.target.0].parameters)
                {
                    if value(argument, block.pc)?.ty != value(parameter, block.pc)?.ty {
                        return Err(error(block.pc, "edge argument type mismatch".into()));
                    }
                }
            }
        }
        Ok(())
    }
}
