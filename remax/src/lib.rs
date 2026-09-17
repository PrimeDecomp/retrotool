//! Maxwell shader decoding and translation, independent of game containers and
//! graphics APIs. Callers provide bounded code, hardware headers and established
//! resource properties; translation reports its required inputs and assumptions.
//!
//! The [`program`] module provides verified SSA, a scalar interpreter and callable
//! WGSL kernels. [`maxwell::graphics`] links supported SPHv3 graphics pairs;
//! the viewer uses this path experimentally. [`wgsl`] retains the temporary
//! legacy translator during migration. General GPU execution analysis remains
//! incomplete. The decoded [`maxwell::Operation`] is a machine instruction,
//! not semantic IR.

pub mod maxwell;
pub mod program;
pub mod resources;
pub mod wgsl;

mod ir;
mod structured;
