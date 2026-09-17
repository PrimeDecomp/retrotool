use super::{
    emit::{name, type_name, Emitter},
    Input,
};
use crate::{
    ir::{Instruction, Logic, Operation, Relation, Type, ValueId},
    program::Diagnostic,
};

impl Emitter<'_> {
    pub(super) fn instruction(&mut self, instruction: &Instruction) -> Result<(), Diagnostic> {
        let pc = instruction.pc;
        if matches!(instruction.operation, Operation::Nop) {
            return Ok(());
        }
        if matches!(instruction.operation, Operation::Demote) {
            self.line("fragment_discarded = true;");
            return Ok(());
        }
        if let Operation::LocalStore(ref store) = instruction.operation {
            self.local_store(store, pc);
            return Ok(());
        }
        let result = instruction.result.expect("verified value instruction");
        self.line("{");
        let mut valid = self.all_known(&instruction.operation.operands());
        let expression = match &instruction.operation {
            Operation::Nop => unreachable!("handled checkpoint"),
            Operation::Demote => unreachable!("handled demotion"),
            Operation::LocalStore(_) => unreachable!("handled store effect"),
            Operation::LocalLoad(load) => {
                let (expression, available) = self.local_load(*load, pc);
                valid = available;
                expression
            }
            Operation::Copy(value) => name(*value),
            Operation::AttributeAddress { base, offset } => {
                format!("RemaxAttributeAddress({base}u, {})", name(*offset))
            }
            Operation::Reduce { kind, value } => {
                format!("{}({})", type_name(Type::Reduced(*kind)), name(*value))
            }
            Operation::Add(a, b) => format!("{} + {}", name(*a), name(*b)),
            Operation::Multiply(a, b) => format!("{} * {}", name(*a), name(*b)),
            Operation::Negate(value) => format!("0u - {}", name(*value)),
            Operation::NotBits(value) => format!("~{}", name(*value)),
            Operation::ReverseBits(value) => format!("reverseBits({})", name(*value)),
            // Keep the established bounded helper for Naga versions predating
            // countLeadingZeros and their incorrect Metal firstLeadingBit path.
            // Removal: docs/rendering-compatibility.md#leading-bit-builtins.
            Operation::CountLeadingZeros(value) => {
                format!("remax_flo({}, false, true)", name(*value))
            }
            Operation::ExtractBits { value, offset, count, signed } => format!(
                "remax_extract_bits({}, {}, {}, {signed})",
                name(*value),
                name(*offset),
                name(*count),
            ),
            Operation::PredicateNot(value) => format!("!{}", name(*value)),
            Operation::Logic { op, a, b } => {
                format!("{} {} {}", name(*a), logic(*op), name(*b))
            }
            Operation::PredicateLogic { op, a, b } => {
                format!("{} {} {}", name(*a), predicate_logic(*op), name(*b))
            }
            Operation::Shift { a, b, right, signed, wrap } => {
                shift(&name(*a), &name(*b), *right, *signed, *wrap)
            }
            Operation::Compare { op, a, b, signed } => {
                let operand = |value| {
                    if *signed {
                        format!("bitcast<i32>({})", name(value))
                    } else {
                        name(value)
                    }
                };
                format!("{} {} {}", operand(*a), relation(*op), operand(*b))
            }
            Operation::Select { condition, yes, no } => {
                valid = format!(
                    "{} && select({}, {}, {})",
                    self.known(*condition),
                    self.known(*no),
                    self.known(*yes),
                    name(*condition)
                );
                let result_type = self.function.values[result.0].ty;
                select(result_type, *condition, *yes, *no)
            }
            Operation::Component { vector, index } => format!("{}[{index}]", name(*vector)),
            Operation::ReadConstant(address) => self.load_constant(
                address.bank,
                format!("{}u", address.byte_offset),
                4,
                0,
                false,
                instruction,
            ),
            Operation::LoadConstantWord { bank, address, bytes, part, signed } => {
                self.observe(&self.known(*address), pc);
                self.load_constant(*bank, name(*address), *bytes, *part, *signed, instruction)
            }
            Operation::ReadAttribute(attribute) => {
                self.attribute(*attribute, pc, |emitter, offset| {
                    Ok(emitter.read_input(Input::VertexAttribute(offset), pc))
                })?
            }
            Operation::ReadSampleIndex => self.read_input(Input::SampleIndex, pc),
            Operation::Float(op) => self.float(op, pc, &valid)?,
            Operation::Interpolate { attribute, mode, location, multiplier, saturate } => self
                .attribute(*attribute, pc, |emitter, offset| {
                    emitter.interpolate(offset, *mode, *location, *multiplier, *saturate, pc)
                })?,
            Operation::Texture(op) => {
                self.observe(&valid, pc);
                self.texture(op, pc)
            }
            Operation::GlobalLoad(load) => {
                self.observe(&valid, pc);
                self.global_load(load, pc)
            }
        };
        self.line(&format!("{} = {expression};", name(result)));
        if !self.analysis.availability.is_available(result) {
            self.line(&format!("{} = {valid};", self.known(result)));
        }
        self.line("}");
        Ok(())
    }
}

/// WGSL's select accepts scalar/vector values, so opaque payloads select their
/// fields together under one condition. No native representation is exposed.
fn select(ty: Type, condition: ValueId, yes: ValueId, no: ValueId) -> String {
    let (condition, yes, no) = (name(condition), name(yes), name(no));
    let field = |field| format!("select({no}.{field}, {yes}.{field}, {condition})");
    match ty {
        Type::Reduced(_) => format!("{}({})", type_name(ty), field("argument")),
        Type::AttributeAddress => {
            format!("RemaxAttributeAddress({}, {})", field("base"), field("offset"))
        }
        _ => format!("select({no}, {yes}, {condition})"),
    }
}

pub(super) fn relation(op: Relation) -> &'static str {
    match op {
        Relation::Less => "<",
        Relation::Equal => "==",
        Relation::LessEqual => "<=",
        Relation::Greater => ">",
        Relation::NotEqual => "!=",
        Relation::GreaterEqual => ">=",
    }
}

fn logic(op: Logic) -> &'static str {
    match op {
        Logic::And => "&",
        Logic::Or => "|",
        Logic::Xor => "^",
    }
}

fn predicate_logic(operation: Logic) -> &'static str {
    match operation {
        Logic::And => "&&",
        Logic::Or => "||",
        Logic::Xor => "!=",
    }
}

fn shift(value: &str, amount: &str, right: bool, signed: bool, wrap: bool) -> String {
    let shifted = if right && signed {
        format!("bitcast<u32>(bitcast<i32>({value}) >> ({amount} & 31u))")
    } else {
        let operator = if right { ">>" } else { "<<" };
        format!("({value} {operator} ({amount} & 31u))")
    };
    if wrap {
        return shifted;
    }

    // WGSL masks shift counts. Native non-wrapping shifts instead produce
    // zero or the sign extension when the count reaches the word width.
    let overflow = if right && signed {
        format!("bitcast<u32>(bitcast<i32>({value}) >> 31u)")
    } else {
        "0u".into()
    };
    format!("select({shifted}, {overflow}, {amount} >= 32u)")
}
