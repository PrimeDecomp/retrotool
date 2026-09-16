//! Maxwell shader decoding and translation, independent of game containers and
//! graphics APIs. Callers provide bounded code, hardware headers and established
//! resource properties; translation reports its required inputs and assumptions.
//!
//! The current WGSL backend is the existing register-based translator. Semantic
//! SSA and structured shader IR are the next implementation step; the decoded
//! [`maxwell::Operation`] is a machine instruction, not semantic IR.

pub mod maxwell;
pub mod resources;
pub mod wgsl;
