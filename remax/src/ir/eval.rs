use std::collections::BTreeMap;

mod local;

use super::{attribute, BlockId, Definition, Function, Operation, Terminator, ValueId};
use crate::program::{
    memory, texture, Diagnostic, DiagnosticKind, Execution, FloatModel, Inputs, Interpolant,
    InterpolationLocation, InterpretOptions, Phase, Providers, Register, Value,
};

#[derive(Clone, Copy, Debug)]
enum Cell {
    Value(Value),
    Words4([u32; 4]),
    Reduced { kind: super::float::Reduction, argument: u32 },
    AttributeAddress(attribute::Address),
    Missing(Register),
    Unavailable(Register),
    LocalUnavailable(u32),
}

impl Cell {
    fn observable(self, pc: Option<usize>) -> Result<Value, Diagnostic> {
        let (kind, register) = match self {
            Self::LocalUnavailable(offset) => {
                return Err(Diagnostic::new(
                    Phase::Interpret,
                    pc,
                    DiagnosticKind::UnavailableValue,
                    format!("observable dependence on unavailable local byte {offset:#x}"),
                ));
            }
            Self::Value(value) => return Ok(value),
            Self::Words4(_) | Self::Reduced { .. } | Self::AttributeAddress(_) => {
                return Err(Diagnostic::new(
                    Phase::Interpret,
                    pc,
                    DiagnosticKind::InvalidIr,
                    "internal value used as an observable scalar",
                ));
            }
            Self::Missing(register) => (DiagnosticKind::MissingInput, register),
            Self::Unavailable(register) => (DiagnosticKind::UnavailableValue, register),
        };
        Err(Diagnostic::new(
            Phase::Interpret,
            pc,
            kind,
            format!("observable dependence on {register:?}"),
        ))
    }
}

impl Function {
    pub fn interpret_with_options(
        &self,
        inputs: &Inputs,
        options: InterpretOptions,
        mut providers: Providers<'_>,
    ) -> Result<Execution, Diagnostic> {
        let mut steps = options.step_budget;
        let error = |pc, kind, message| Diagnostic::new(Phase::Interpret, pc, kind, message);
        let mut values = vec![None; self.values.len()];
        let mut texture_occurrences = BTreeMap::<usize, u64>::new();
        let mut memory_occurrences = BTreeMap::<usize, u64>::new();
        let mut local_memory = local::Memory::new(self.local_memory_bytes);
        let mut discarded = false;
        for (index, value) in self.values.iter().enumerate() {
            values[index] = match value.definition {
                Definition::Word(bits) => Some(Cell::Value(Value::Word(bits))),
                Definition::Predicate(b) => Some(Cell::Value(Value::Predicate(b))),
                Definition::Unavailable(r) => Some(Cell::Unavailable(r)),
                Definition::UnavailableLocal(offset) => Some(Cell::LocalUnavailable(offset)),
                Definition::Input(r) => Some(match inputs.registers.get(&r).copied() {
                    Some(value) => {
                        if !matches!(
                            (r, value),
                            (Register::Gpr(_), Value::Word(_))
                                | (Register::OutputAttribute(_), Value::Word(_))
                                | (Register::Predicate(_), Value::Predicate(_))
                                | (Register::Carry, Value::Predicate(_))
                        ) {
                            return Err(error(
                                None,
                                DiagnosticKind::InvalidInput,
                                format!("input type does not match {r:?}"),
                            ));
                        }
                        Cell::Value(value)
                    }
                    None => Cell::Missing(r),
                }),
                _ => None,
            };
        }
        let mut consume = |pc| -> Result<(), Diagnostic> {
            steps = steps.checked_sub(1).ok_or_else(|| {
                error(pc, DiagnosticKind::StepLimit, "interpreter step budget exhausted".into())
            })?;
            Ok(())
        };
        let mut current = BlockId(0);
        loop {
            let block = &self.blocks[current.0];
            for instruction in &block.instructions {
                let pc = Some(instruction.pc);
                consume(pc)?;
                if matches!(instruction.operation, Operation::Nop) {
                    continue;
                }
                if matches!(instruction.operation, Operation::Demote) {
                    discarded = true;
                    continue;
                }
                let get = |id: ValueId| {
                    values[id.0].ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::InvalidIr,
                            format!("value {id:?} has not executed"),
                        )
                    })
                };
                if let Operation::LocalStore(ref store) = instruction.operation {
                    let sources = store
                        .values
                        .iter()
                        .map(|&value| get(value))
                        .collect::<Result<Vec<_>, _>>()?;
                    local_memory.store(
                        store,
                        get(store.access.address)?,
                        &sources,
                        instruction.pc,
                    )?;
                    continue;
                }
                let cell = if let Operation::LocalLoad(load) = instruction.operation {
                    local_memory.load(load, get(load.access.address)?, instruction.pc)?
                } else if let Operation::ReadConstant(address) = instruction.operation {
                    let word = inputs
                        .constants
                        .read_bytes::<4>(address.bank, u32::from(address.byte_offset))
                        .map_err(|cause| {
                            error(pc, DiagnosticKind::MissingInput, cause.to_string())
                        })?;
                    Cell::Value(Value::Word(u32::from_le_bytes(word)))
                } else if let Operation::LoadConstantWord { bank, address, bytes, part, signed } =
                    instruction.operation
                {
                    let Value::Word(address) = get(address)?.observable(pc)? else {
                        unreachable!("verified address type")
                    };
                    Cell::Value(Value::Word(load_constant(
                        inputs,
                        instruction.pc,
                        bank,
                        address,
                        bytes,
                        part,
                        signed,
                    )?))
                } else if let Operation::GlobalLoad(ref load) = instruction.operation {
                    let mut address = [0u32; 2];
                    for (out, value) in address.iter_mut().zip(load.address) {
                        let Value::Word(bits) = get(value)?.observable(pc)? else {
                            unreachable!("verified address type")
                        };
                        *out = bits;
                    }
                    let occurrence = memory_occurrences.entry(instruction.pc).or_default();
                    let request = memory::Read {
                        address: u64::from(address[0]) | (u64::from(address[1]) << 32),
                        bytes: load.bytes,
                        cache: load.cache,
                        native_u128: load.native_u128,
                        stage: self.stage,
                        pc: instruction.pc,
                        occurrence: *occurrence,
                    };
                    *occurrence = occurrence.checked_add(1).ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::StepLimit,
                            "memory execution count overflow".into(),
                        )
                    })?;
                    let provider = providers.memory.as_mut().ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::MissingInput,
                            "global memory provider is unavailable".into(),
                        )
                    })?;
                    let bytes = provider.read(&request).map_err(|e| {
                        error(
                            pc,
                            match &e {
                                memory::Error::Unavailable(_) => DiagnosticKind::MissingInput,
                                memory::Error::InvalidAccess(_) => DiagnosticKind::InvalidInput,
                                memory::Error::Unsupported(_) => DiagnosticKind::Unsupported,
                            },
                            e.to_string(),
                        )
                    })?;
                    let mut words = [0; 4];
                    for (i, &byte) in bytes[..load.bytes as usize].iter().enumerate() {
                        words[i / 4] |= u32::from(byte) << (i % 4 * 8);
                    }
                    if load.signed {
                        let shift = 32 - u32::from(load.bytes) * 8;
                        words[0] = ((words[0] << shift) as i32 >> shift) as u32;
                    }
                    Cell::Words4(words)
                } else if let Operation::Texture(ref op) = instruction.operation {
                    if discarded && op.skip_helpers {
                        return Err(error(
                            pc,
                            DiagnosticKind::Unsupported,
                            "NODEP texture results after demotion are not established".into(),
                        ));
                    }
                    let operation = op.request(|value| {
                        let Value::Word(bits) = get(value)?.observable(pc)? else {
                            unreachable!("verified texture operand type")
                        };
                        Ok(bits)
                    })?;
                    let occurrence = texture_occurrences.entry(instruction.pc).or_default();
                    let request = texture::Request {
                        descriptor_word_offset: op.descriptor,
                        stage: self.stage,
                        pc: instruction.pc,
                        occurrence: *occurrence,
                        skip_helpers: op.skip_helpers,
                        operation,
                    };
                    *occurrence = occurrence.checked_add(1).ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::StepLimit,
                            "texture execution count overflow".into(),
                        )
                    })?;
                    let provider = providers.textures.as_mut().ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::MissingInput,
                            "texture provider is unavailable".into(),
                        )
                    })?;
                    let response = provider.read(&request).map_err(|cause| {
                        let kind = match &cause {
                            texture::Error::Unavailable(_) => DiagnosticKind::MissingInput,
                            texture::Error::InvalidResource(_) => DiagnosticKind::InvalidInput,
                            texture::Error::Unsupported(_) => DiagnosticKind::Unsupported,
                        };
                        error(pc, kind, format!("texture descriptor {:#x}: {cause}", op.descriptor))
                    })?;
                    let comparison =
                        matches!(op.kind, super::texture::Kind::Sample { comparison: Some(_), .. });
                    Cell::Words4(match (comparison, response) {
                        (false, texture::Response::Texel(words)) => words,
                        (true, texture::Response::Comparison(bits)) => {
                            [bits, bits, bits, 1.0f32.to_bits()]
                        }
                        _ => {
                            return Err(error(
                                pc,
                                DiagnosticKind::InvalidInput,
                                "texture response kind does not match the instruction".into(),
                            ))
                        }
                    })
                } else if let Operation::Component { vector, index } = instruction.operation {
                    match get(vector)? {
                        Cell::Words4(words) => Cell::Value(Value::Word(words[usize::from(index)])),
                        missing @ (Cell::Missing(_)
                        | Cell::Unavailable(_)
                        | Cell::LocalUnavailable(_)) => missing,
                        _ => unreachable!("verified vector component type"),
                    }
                } else if let Operation::Copy(value) = instruction.operation {
                    get(value)?
                } else if let Operation::ReadSampleIndex = instruction.operation {
                    let index = inputs.sample_index.ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::MissingInput,
                            "fragment sample index is unavailable".into(),
                        )
                    })?;
                    Cell::Value(Value::Word(index))
                } else if let Operation::AttributeAddress { base, offset } = instruction.operation {
                    match get(offset)? {
                        Cell::Value(Value::Word(offset)) => {
                            Cell::AttributeAddress(attribute::Address { base, offset })
                        }
                        missing @ (Cell::Missing(_)
                        | Cell::Unavailable(_)
                        | Cell::LocalUnavailable(_)) => missing,
                        _ => unreachable!("verified logical attribute offset"),
                    }
                } else if let Operation::ReadAttribute(attribute) = instruction.operation {
                    let offset = self.resolve_attribute(attribute, &get, instruction.pc)?;
                    let word = inputs.attributes.get(&offset).copied().ok_or_else(|| {
                        error(
                            pc,
                            DiagnosticKind::MissingInput,
                            format!("vertex attribute {offset:#x} is unavailable"),
                        )
                    })?;
                    Cell::Value(Value::Word(word))
                } else if let Operation::Interpolate {
                    attribute,
                    mode,
                    location,
                    multiplier,
                    saturate,
                } = instruction.operation
                {
                    let byte_offset = self.resolve_attribute(attribute, &get, instruction.pc)?;
                    let location = match location {
                        super::InterpolationLocation::Default => InterpolationLocation::Default,
                        super::InterpolationLocation::Centroid => InterpolationLocation::Centroid,
                        super::InterpolationLocation::Offset(value) => {
                            let Value::Word(bits) = get(value)?.observable(pc)? else {
                                unreachable!("verified offset type")
                            };
                            InterpolationLocation::Offset(bits)
                        }
                    };
                    let input = Interpolant { byte_offset, mode, location };
                    let word = if let Some(&word) = inputs.interpolants.get(&input) {
                        word
                    } else if let Some(planes) = &inputs.offset_interpolation {
                        planes.evaluate(input, options.float_model, instruction.pc)?
                    } else {
                        return Err(error(
                            pc,
                            DiagnosticKind::MissingInput,
                            format!("interpolation input {input:?} is unavailable"),
                        ));
                    };
                    let arithmetic = multiplier.is_some() || saturate;
                    if arithmetic && options.float_model == FloatModel::Reject {
                        return Err(error(
                            pc,
                            DiagnosticKind::Unsupported,
                            "IPA arithmetic requires an explicit floating-point interpreter model"
                                .into(),
                        ));
                    }
                    let multiplier = multiplier.map(get).transpose()?;
                    match multiplier {
                        Some(Cell::Value(Value::Predicate(_))) => {
                            return Err(error(
                                pc,
                                DiagnosticKind::InvalidIr,
                                "non-word interpolation multiplier".into(),
                            ))
                        }
                        Some(
                            missing @ (Cell::Missing(_)
                            | Cell::Unavailable(_)
                            | Cell::LocalUnavailable(_)),
                        ) => missing,
                        _ if !arithmetic => Cell::Value(Value::Word(word)),
                        _ => {
                            let multiplier = multiplier.map(|v| match v {
                                Cell::Value(Value::Word(bits)) => bits,
                                _ => unreachable!(),
                            });
                            Cell::Value(Value::Word(super::float::interpolate(
                                word,
                                multiplier,
                                saturate,
                                options.float_model,
                            )))
                        }
                    }
                } else if let Operation::Reduce { kind, value } = instruction.operation {
                    match get(value)? {
                        Cell::Value(Value::Word(argument)) => Cell::Reduced { kind, argument },
                        missing @ (Cell::Missing(_)
                        | Cell::Unavailable(_)
                        | Cell::LocalUnavailable(_)) => missing,
                        _ => unreachable!("verified range-reduction input"),
                    }
                } else if let Operation::Float(super::float::Operation::Reduced {
                    function,
                    value,
                    saturate,
                }) = instruction.operation
                {
                    match get(value)? {
                        Cell::Reduced { kind, argument } => {
                            debug_assert_eq!(kind, function.reduction());
                            Cell::Value(Value::Word(super::float::reduced(
                                function,
                                argument,
                                saturate,
                                options,
                                instruction.pc,
                            )?))
                        }
                        missing @ (Cell::Missing(_)
                        | Cell::Unavailable(_)
                        | Cell::LocalUnavailable(_)) => missing,
                        _ => unreachable!("verified range-reduction provenance"),
                    }
                } else if let Operation::Select { condition, yes, no } = instruction.operation {
                    match get(condition)? {
                        Cell::Value(Value::Predicate(b)) => get(if b { yes } else { no })?,
                        missing @ (Cell::Missing(_)
                        | Cell::Unavailable(_)
                        | Cell::LocalUnavailable(_)) => missing,
                        _ => {
                            return Err(error(
                                pc,
                                DiagnosticKind::InvalidIr,
                                "non-predicate select".into(),
                            ))
                        }
                    }
                } else {
                    let args = instruction
                        .operation
                        .operands()
                        .into_iter()
                        .map(get)
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(&missing) = args.iter().find(|v| {
                        matches!(
                            v,
                            Cell::Missing(_) | Cell::Unavailable(_) | Cell::LocalUnavailable(_)
                        )
                    }) {
                        missing
                    } else {
                        let args = args
                            .into_iter()
                            .map(|v| match v {
                                Cell::Value(v) => v,
                                _ => unreachable!(),
                            })
                            .collect::<Vec<_>>();
                        Cell::Value(if let Operation::Float(op) = &instruction.operation {
                            op.evaluate(&args, options, inputs, instruction.pc)?
                        } else {
                            super::integer::evaluate(&instruction.operation, &args)
                                .expect("handled integer operation")
                        })
                    }
                };
                values[instruction.result.expect("verified value instruction").0] = Some(cell);
            }
            consume(block.pc)?;
            let get = |id: ValueId| {
                values[id.0].ok_or_else(|| {
                    error(
                        block.pc,
                        DiagnosticKind::InvalidIr,
                        format!("value {id:?} has not executed"),
                    )
                })
            };
            let edge = match block.terminator.as_ref().expect("verified terminator") {
                Terminator::Jump(edge) => edge,
                Terminator::Branch { condition, yes, no, .. } => {
                    match get(*condition)?.observable(block.pc)? {
                        Value::Predicate(true) => yes,
                        Value::Predicate(false) => no,
                        _ => {
                            return Err(error(
                                block.pc,
                                DiagnosticKind::InvalidIr,
                                "non-predicate branch".into(),
                            ))
                        }
                    }
                }
                Terminator::Return(ids) => {
                    return if discarded {
                        Ok(Execution::Discarded)
                    } else {
                        ids.iter()
                            .map(|&id| get(id)?.observable(block.pc))
                            .collect::<Result<Vec<_>, _>>()
                            .map(Execution::Outputs)
                    }
                }
            };
            // Read every source before overwriting any destination. This matters
            // on a backedge that exchanges two parameters of the same block.
            let arguments =
                edge.arguments.iter().map(|&id| get(id)).collect::<Result<Vec<_>, _>>()?;
            for (&parameter, argument) in
                self.blocks[edge.target.0].parameters.iter().zip(arguments)
            {
                values[parameter.0] = Some(argument);
            }
            current = edge.target;
        }
    }
}

impl Function {
    fn resolve_attribute(
        &self,
        input: attribute::Input,
        get: &impl Fn(ValueId) -> Result<Cell, Diagnostic>,
        pc: usize,
    ) -> Result<u16, Diagnostic> {
        let attribute::Input::Indexed(value) = input else {
            let attribute::Input::Direct(offset) = input else { unreachable!() };
            return Ok(offset);
        };
        let address = match get(value)? {
            Cell::AttributeAddress(address) => address,
            missing @ (Cell::Missing(_) | Cell::Unavailable(_) | Cell::LocalUnavailable(_)) => {
                missing.observable(Some(pc))?;
                unreachable!()
            }
            _ => unreachable!("verified attribute provenance"),
        };
        let offset = address.resolve().ok_or_else(|| {
            Diagnostic::new(
                Phase::Interpret,
                Some(pc),
                DiagnosticKind::InvalidInput,
                "indexed attribute address is outside the aligned generic domain",
            )
        })?;
        if !self.indexed_attributes.contains(&offset) {
            return Err(Diagnostic::new(
                Phase::Interpret,
                Some(pc),
                DiagnosticKind::MissingInput,
                format!("indexed attribute {offset:#x} is not declared"),
            ));
        }
        Ok(offset)
    }
}

fn load_constant(
    inputs: &Inputs,
    pc: usize,
    bank: u8,
    address: u32,
    bytes: u8,
    part: u8,
    signed: bool,
) -> Result<u32, Diagnostic> {
    let error = |kind, message| Diagnostic::new(Phase::Interpret, Some(pc), kind, message);
    if !address.is_multiple_of(u32::from(bytes)) {
        return Err(error(
            DiagnosticKind::InvalidInput,
            format!(
                "unaligned {bytes}-byte LDC at {address:#x}; invalid-load behavior is not emulated"
            ),
        ));
    }
    let read = match bytes {
        1 => inputs.constants.read_bytes::<1>(bank, address).map(|v| u32::from(v[0])),
        2 => inputs
            .constants
            .read_bytes::<2>(bank, address)
            .map(|v| u32::from(u16::from_le_bytes(v))),
        4 => inputs.constants.read_bytes::<4>(bank, address).map(u32::from_le_bytes),
        8 => inputs.constants.read_bytes::<8>(bank, address).map(|v| {
            u32::from_le_bytes(v[usize::from(part)..usize::from(part) + 4].try_into().unwrap())
        }),
        _ => unreachable!("verified LDC width"),
    }
    .map_err(|cause| error(DiagnosticKind::MissingInput, cause.to_string()))?;
    Ok(if signed {
        let shift = 32 - u32::from(bytes) * 8;
        ((read << shift) as i32 >> shift) as u32
    } else {
        read
    })
}
