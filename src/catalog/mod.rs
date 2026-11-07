//! What can be cleaned: project build artifacts found in the scan tree and
//! the known caches of developer tools.

pub mod artifacts;
pub mod command;
pub mod tools;

pub use artifacts::{ArtifactKind, Candidate, detect_artifacts};
pub use command::{
    CommandResult, CommandRunner, CommandSpec, RunControl, SystemRunner, is_lock_wait,
};
pub mod resolve;
pub mod runtimes;
pub use tools::{Mode, ToolEntry, discover_tools};
pub mod appfiles;
pub mod apppolicy;
pub mod apps;
pub mod leftovers;
pub mod split;
