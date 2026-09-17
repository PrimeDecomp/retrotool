//! Semantic SSA. Numeric values carry bits; operations provide interpretation.
//! Block arguments are simultaneous transfers. Interpolation and texture reads stay at their
//! original guarded execution point. Explicit convergence contexts preserve
//! synchronization structure; they do not establish host lane participation.
//! Local loads and resultless stores retain execution order. SSA represents
//! their operands/results, not memory state; optimization must respect effects.

use crate::program::{ConstantAddress, InterpolationMode, Register, Stage};

pub(crate) mod analysis;
pub(crate) mod attribute;
pub(crate) mod convergence;
mod eval;
pub(crate) mod float;
mod integer;
pub(crate) mod memory;
pub(crate) mod optimize;
mod rewrite;
pub(crate) mod ssa;
pub(crate) mod texture;
pub(crate) mod transport;
mod verify;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Type {
    Bits32,
    Predicate,
    Words4,
    /// Symbolic RRO result. Only transport and a matching unmodified MUFU
    /// consumer may observe it; distinct reduction domains cannot be mixed.
    Reduced(float::Reduction),
    /// Logical attribute provenance shared by AL2P's address and predicate
    /// outputs. Neither output's native bits have been recovered.
    AttributeAddress,
}

impl Type {
    pub fn is_opaque(self) -> bool { matches!(self, Self::Reduced(_) | Self::AttributeAddress) }
}

impl Register {
    pub(crate) fn ty(self) -> Type {
        match self {
            Self::Gpr(_) | Self::OutputAttribute(_) => Type::Bits32,
            Self::Predicate(_) | Self::Carry => Type::Predicate,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Definition {
    Word(u32),
    Predicate(bool),
    Input(Register),
    Unavailable(Register),
    /// Initial contents of a promoted local word, before any executed store.
    UnavailableLocal(u32),
    Parameter {
        block: BlockId,
        index: usize,
    },
    Instruction {
        block: BlockId,
        index: usize,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Value {
    pub ty: Type,
    pub definition: Definition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Logic {
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum InterpolationLocation {
    Default,
    Centroid,
    Offset(ValueId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Relation {
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Operation {
    /// An erased operation's execution checkpoint. It retains source PC and
    /// budget accounting but has no data result or memory effect.
    Nop,
    /// Permanently suppress this fragment's outputs while retaining execution
    /// as a helper. This effect is ordered; it is not a branch or return.
    Demote,
    Copy(ValueId),
    AttributeAddress {
        base: u16,
        offset: ValueId,
    },
    /// Capture an RRO argument without inventing the intermediate encoding.
    Reduce {
        kind: float::Reduction,
        value: ValueId,
    },
    Add(ValueId, ValueId),
    /// Low 32 bits of the product, independent of signed interpretation.
    Multiply(ValueId, ValueId),
    Negate(ValueId),
    NotBits(ValueId),
    ReverseBits(ValueId),
    /// Returns 32 for zero.
    CountLeadingZeros(ValueId),
    /// Extract a field with zero or sign padding beyond the source word.
    /// A zero count returns zero; offset/count do not wrap at 32 bits.
    ExtractBits {
        value: ValueId,
        offset: ValueId,
        count: ValueId,
        signed: bool,
    },
    Logic {
        op: Logic,
        a: ValueId,
        b: ValueId,
    },
    Shift {
        a: ValueId,
        b: ValueId,
        right: bool,
        signed: bool,
        wrap: bool,
    },
    Compare {
        op: Relation,
        a: ValueId,
        b: ValueId,
        signed: bool,
    },
    PredicateNot(ValueId),
    PredicateLogic {
        op: Logic,
        a: ValueId,
        b: ValueId,
    },
    Select {
        condition: ValueId,
        yes: ValueId,
        no: ValueId,
    },
    ReadConstant(ConstantAddress),
    /// One result word from an aligned constant-buffer access. Both halves of
    /// an eight-byte load validate the full span; the address is captured before
    /// any destination write. Constant inputs are immutable during execution.
    LoadConstantWord {
        bank: u8,
        address: ValueId,
        bytes: u8,
        part: u8,
        signed: bool,
    },
    ReadAttribute(attribute::Input),
    ReadSampleIndex,
    Float(float::Operation),
    Texture(texture::Operation),
    GlobalLoad(memory::Load),
    LocalLoad(memory::local::Load),
    LocalStore(memory::local::Store),
    Component {
        vector: ValueId,
        index: u8,
    },
    /// An interpolation intrinsic, kept at its original guarded execution point.
    Interpolate {
        attribute: attribute::Input,
        mode: InterpolationMode,
        location: InterpolationLocation,
        multiplier: Option<ValueId>,
        saturate: bool,
    },
}

impl Operation {
    pub fn operands(&self) -> Vec<ValueId> {
        match *self {
            Self::Copy(a)
            | Self::AttributeAddress { offset: a, .. }
            | Self::Reduce { value: a, .. }
            | Self::Negate(a)
            | Self::NotBits(a)
            | Self::ReverseBits(a)
            | Self::CountLeadingZeros(a)
            | Self::PredicateNot(a) => vec![a],
            Self::Add(a, b)
            | Self::Multiply(a, b)
            | Self::Logic { a, b, .. }
            | Self::Shift { a, b, .. }
            | Self::Compare { a, b, .. }
            | Self::PredicateLogic { a, b, .. } => vec![a, b],
            Self::Select { condition, yes, no } => vec![condition, yes, no],
            Self::ExtractBits { value, offset, count, .. } => vec![value, offset, count],
            Self::Nop | Self::Demote | Self::ReadConstant(_) | Self::ReadSampleIndex => vec![],
            Self::ReadAttribute(input) => input.operand().into_iter().collect(),
            Self::Float(ref op) => op.operands(),
            Self::Texture(ref op) => op.operands(),
            Self::GlobalLoad(ref load) => load.address.to_vec(),
            Self::LocalLoad(load) => vec![load.access.address],
            Self::LocalStore(ref store) => {
                std::iter::once(store.access.address).chain(store.values.iter().copied()).collect()
            }
            Self::Component { vector, .. } => vec![vector],
            Self::Interpolate { attribute, location, multiplier, .. } => {
                let mut values =
                    attribute.operand().into_iter().chain(multiplier).collect::<Vec<_>>();
                if let InterpolationLocation::Offset(value) = location {
                    values.push(value);
                }
                values
            }
            Self::LoadConstantWord { address, .. } => vec![address],
        }
    }

    pub fn map_operands(&mut self, mut f: impl FnMut(ValueId) -> ValueId) {
        match self {
            Self::Copy(a)
            | Self::AttributeAddress { offset: a, .. }
            | Self::Reduce { value: a, .. }
            | Self::Negate(a)
            | Self::NotBits(a)
            | Self::ReverseBits(a)
            | Self::CountLeadingZeros(a)
            | Self::PredicateNot(a) => *a = f(*a),
            Self::Add(a, b)
            | Self::Multiply(a, b)
            | Self::Logic { a, b, .. }
            | Self::Shift { a, b, .. }
            | Self::Compare { a, b, .. }
            | Self::PredicateLogic { a, b, .. } => {
                *a = f(*a);
                *b = f(*b);
            }
            Self::Select { condition, yes, no } => {
                *condition = f(*condition);
                *yes = f(*yes);
                *no = f(*no);
            }
            Self::ExtractBits { value, offset, count, .. } => {
                *value = f(*value);
                *offset = f(*offset);
                *count = f(*count);
            }
            Self::Nop | Self::Demote | Self::ReadConstant(_) | Self::ReadSampleIndex => {}
            Self::ReadAttribute(input) => input.map_operand(f),
            Self::Float(op) => op.map_operands(f),
            Self::Texture(op) => op.map_operands(f),
            Self::GlobalLoad(load) => load.address.iter_mut().for_each(|v| *v = f(*v)),
            Self::LocalLoad(load) => load.access.address = f(load.access.address),
            Self::LocalStore(store) => {
                store.access.address = f(store.access.address);
                store.values.iter_mut().for_each(|v| *v = f(*v));
            }
            Self::Component { vector, .. } => *vector = f(*vector),
            Self::Interpolate { attribute, location, multiplier, .. } => {
                attribute.map_operand(&mut f);
                *multiplier = multiplier.map(&mut f);
                if let InterpolationLocation::Offset(value) = location {
                    *value = f(*value);
                }
            }
            Self::LoadConstantWord { address, .. } => {
                *address = f(*address);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Instruction {
    /// Effect-only instructions have no SSA result. The verifier checks this
    /// against the operation rather than introducing a dummy value or unit type.
    pub result: Option<ValueId>,
    pub operation: Operation,
    pub pc: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Edge {
    pub target: BlockId,
    pub arguments: Vec<ValueId>,
    pub convergence: convergence::Transfer,
}

impl Edge {
    pub fn new(target: BlockId, arguments: Vec<ValueId>) -> Self {
        Self { target, arguments, convergence: convergence::Transfer::Preserve }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Terminator {
    Jump(Edge),
    Branch { condition: ValueId, yes: Edge, no: Edge, kind: convergence::BranchKind },
    Return(Vec<ValueId>),
}

impl Terminator {
    pub fn edges(&self) -> Vec<&Edge> {
        match self {
            Self::Jump(e) => vec![e],
            Self::Branch { yes, no, .. } => vec![yes, no],
            Self::Return(_) => vec![],
        }
    }

    pub fn edges_mut(&mut self) -> Vec<&mut Edge> {
        match self {
            Self::Jump(e) => vec![e],
            Self::Branch { yes, no, .. } => vec![yes, no],
            Self::Return(_) => vec![],
        }
    }

    pub fn operands(&self) -> Vec<ValueId> {
        let mut values = match self {
            Self::Branch { condition, .. } => vec![*condition],
            Self::Return(values) => values.clone(),
            Self::Jump(_) => vec![],
        };
        for edge in self.edges() {
            values.extend(&edge.arguments);
        }
        values
    }

    pub fn map_operands(&mut self, mut f: impl FnMut(ValueId) -> ValueId) {
        match self {
            Self::Branch { condition, .. } => *condition = f(*condition),
            Self::Return(values) => values.iter_mut().for_each(|v| *v = f(*v)),
            Self::Jump(_) => {}
        }
        for edge in self.edges_mut() {
            edge.arguments.iter_mut().for_each(|v| *v = f(*v));
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub pc: Option<usize>,
    pub convergence: Vec<convergence::Region>,
    pub parameters: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Option<Terminator>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Function {
    pub stage: Stage,
    pub local_memory_bytes: u32,
    pub indexed_attributes: Vec<u16>,
    pub blocks: Vec<Block>,
    pub values: Vec<Value>,
    pub outputs: Vec<Register>,
}

impl Function {
    pub fn value(&mut self, ty: Type, definition: Definition) -> ValueId {
        let id = ValueId(self.values.len());
        self.values.push(Value { ty, definition });
        id
    }

    pub fn append(&mut self, block: BlockId, pc: usize, ty: Type, operation: Operation) -> ValueId {
        let index = self.blocks[block.0].instructions.len();
        let result = self.value(ty, Definition::Instruction { block, index });
        self.blocks[block.0].instructions.push(Instruction { result: Some(result), operation, pc });
        result
    }

    pub fn append_effect(&mut self, block: BlockId, pc: usize, operation: Operation) {
        self.blocks[block.0].instructions.push(Instruction { result: None, operation, pc });
    }
}
