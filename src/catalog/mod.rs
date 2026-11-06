//! What can be cleaned: project build artifacts found in the scan tree and
//! the known caches of developer tools.

pub mod artifacts;
pub mod command;
pub mod tools;

pub use artifacts::{ArtifactKind, Candidate, detect_artifacts};
pub use command::{CommandResult, CommandRunner, CommandSpec, SystemRunner};
pub mod resolve;
pub use tools::{Mode, ToolEntry, discover_tools};
pub mod split;
