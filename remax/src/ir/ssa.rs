//! Deferred sealing in the style of Braun et al.'s SSA construction: reads in
//! an unfinished block create parameters, then sealing supplies predecessor
//! definitions and eliminates trivial parameters. There is no implicit zero for
//! a missing definition. The worklist avoids recursion through long CFGs.
//!
//! Algorithm reference: Cranelift frontend/ssa.rs at c07af319ce8538898cfa5fa72573a1c0c8197cd0.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) use super::rewrite::ValueRemap;
use super::{rewrite::Aliases, Block, BlockId, Definition, Function, Type, ValueId};
use crate::program::Register;

pub(crate) struct Builder {
    pub function: Function,
    inputs: BTreeSet<Register>,
    definitions: Vec<BTreeMap<Variable, ValueId>>,
    pending: Vec<(BlockId, Variable, usize)>,
}

/// Construction names are separate from the public machine-register interface.
/// Local words exist only while promoting private storage into ordinary SSA.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Variable {
    Register(Register),
    LocalWord(u32),
}

impl Variable {
    fn ty(self) -> Type {
        match self {
            Self::Register(register) => register.ty(),
            Self::LocalWord(_) => Type::Bits32,
        }
    }
}

impl Builder {
    pub fn new(inputs: &[Register], outputs: &[Register]) -> Self {
        Self {
            function: Function { outputs: outputs.to_vec(), ..Default::default() },
            inputs: inputs.iter().copied().collect(),
            definitions: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Add SSA state to an already verified function. Existing values, parameters
    /// and edge arguments are retained; only newly read variables are sealed.
    pub fn extend(function: Function) -> Self {
        Self {
            definitions: vec![BTreeMap::new(); function.blocks.len()],
            function,
            inputs: BTreeSet::new(),
            pending: Vec::new(),
        }
    }

    pub fn block(&mut self, pc: Option<usize>) -> BlockId {
        let id = BlockId(self.function.blocks.len());
        self.function.blocks.push(Block {
            pc,
            convergence: vec![],
            parameters: vec![],
            instructions: vec![],
            terminator: None,
        });
        self.definitions.push(BTreeMap::new());
        id
    }

    pub fn read(&mut self, block: BlockId, register: Register) -> ValueId {
        match register {
            Register::Gpr(255) => return self.word(0),
            Register::Predicate(7) => return self.predicate(true),
            _ => {}
        }
        self.read_variable(block, Variable::Register(register))
    }

    pub fn read_local(&mut self, block: BlockId, byte_offset: u32) -> ValueId {
        self.read_variable(block, Variable::LocalWord(byte_offset))
    }

    pub fn write_local(&mut self, block: BlockId, byte_offset: u32, value: ValueId) {
        self.definitions[block.0].insert(Variable::LocalWord(byte_offset), value);
    }

    fn read_variable(&mut self, block: BlockId, variable: Variable) -> ValueId {
        if let Some(&value) = self.definitions[block.0].get(&variable) {
            return value;
        }
        let definition = if block.0 == 0 {
            match variable {
                Variable::LocalWord(offset) => Definition::UnavailableLocal(offset),
                Variable::Register(register) if self.inputs.contains(&register) => {
                    Definition::Input(register)
                }
                Variable::Register(register) => Definition::Unavailable(register),
            }
        } else {
            let index = self.function.blocks[block.0].parameters.len();
            self.pending.push((block, variable, index));
            Definition::Parameter { block, index }
        };
        let value = self.function.value(variable.ty(), definition);
        if block.0 != 0 {
            self.function.blocks[block.0].parameters.push(value);
        }
        self.definitions[block.0].insert(variable, value);
        value
    }

    pub fn write(&mut self, block: BlockId, register: Register, value: ValueId) {
        if !matches!(register, Register::Gpr(255) | Register::Predicate(7)) {
            self.definitions[block.0].insert(Variable::Register(register), value);
        }
    }

    pub fn word(&mut self, bits: u32) -> ValueId {
        self.function.value(Type::Bits32, Definition::Word(bits))
    }

    pub fn predicate(&mut self, value: bool) -> ValueId {
        self.function.value(Type::Predicate, Definition::Predicate(value))
    }

    pub fn finish(mut self) -> (Function, ValueRemap) {
        let mut predecessors = vec![Vec::new(); self.function.blocks.len()];
        for (block, data) in self.function.blocks.iter().enumerate() {
            for (edge, target) in data
                .terminator
                .as_ref()
                .expect("unterminated builder block")
                .edges()
                .iter()
                .enumerate()
            {
                predecessors[target.target.0].push((BlockId(block), edge));
            }
        }
        let mut next = 0;
        while let Some(&(block, variable, parameter)) = self.pending.get(next) {
            next += 1;
            for &(source, edge) in &predecessors[block.0] {
                let value = self.read_variable(source, variable);
                let mut edges =
                    self.function.blocks[source.0].terminator.as_mut().unwrap().edges_mut();
                assert_eq!(edges[edge].arguments.len(), parameter);
                edges[edge].arguments.push(value);
            }
        }
        let remap = self.function.remove_trivial_parameters(&predecessors);
        (self.function, remap)
    }
}

impl Function {
    fn remove_trivial_parameters(&mut self, predecessors: &[Vec<(BlockId, usize)>]) -> ValueRemap {
        let mut aliases = Aliases::new(self);
        loop {
            let mut changed = false;
            for (block, incoming) in predecessors.iter().enumerate() {
                for index in (0..self.blocks[block].parameters.len()).rev() {
                    let parameter = self.blocks[block].parameters[index];
                    let mut other = None;
                    let mut trivial = true;
                    for &(source, edge) in incoming {
                        let edges = self.blocks[source.0].terminator.as_ref().unwrap().edges();
                        let value = aliases.resolve(edges[edge].arguments[index]);
                        if value == parameter {
                            continue;
                        }
                        if other.is_some_and(|prior| prior != value) {
                            trivial = false;
                            break;
                        }
                        other = Some(value);
                    }
                    let Some(other) = other.filter(|_| trivial) else {
                        continue;
                    };
                    aliases.replace(parameter, other);
                    self.blocks[block].parameters.remove(index);
                    for &(source, edge) in incoming {
                        self.blocks[source.0].terminator.as_mut().unwrap().edges_mut()[edge]
                            .arguments
                            .remove(index);
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        aliases.apply(self)
    }
}
