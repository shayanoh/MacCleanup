//! What can be cleaned: project build artifacts found in the scan tree and
//! the known caches of developer tools.

pub mod artifacts;

pub use artifacts::{ArtifactKind, Candidate, detect_artifacts};
