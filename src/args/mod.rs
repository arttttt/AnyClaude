//! Argument handling pipeline for AnyClaude.
//!
//! This module provides a structured, declarative approach to argument processing:
//!
//! ```text
//! User Input → Parse → Classify → Transform → Assemble → SpawnParams
//! ```
//!
//! Each stage is a pure function that can be unit-tested independently.

mod assembler;
mod classifier;
mod env_builder;
mod pipeline;
mod registry;
mod session;

pub use assembler::ArgAssembler;
pub use classifier::{classify, ClassifiedArg, ClassifyResult};
pub use env_builder::EnvSet;
pub use pipeline::{build_restart_params, build_spawn_params, SpawnParams};
pub use registry::{flag_registry, FlagArity, FlagBehavior, FlagDef};
pub use session::{encode_project_path, resolve_session, SessionResolution, SessionSource};

/// How to handle session continuation when building spawn parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Initial spawn — use base args + `--session-id <id>`.
    Initial,
    /// Restart — resume our session via `--resume <id>`.
    Resume,
}
