// Cargo has no "supported OS" field, so stop other systems here with a clear
// message. To support another OS later, narrow or remove this check (a patch
// release is enough: nothing on crates.io has to change).
#[cfg(not(target_os = "macos"))]
compile_error!("maccleanup only supports macOS for now");

pub mod accounting;
pub mod anim;
pub mod app;
pub mod bg;
pub mod catalog;
pub mod cli;
pub mod docker;
pub mod elevate;
pub mod filter;
pub mod fmt;
pub mod glyphs;
pub mod input;
pub mod jobs;
pub mod perms;
pub mod safety;
pub mod scan;
pub mod snapshots;
pub mod softdelete;
pub mod targets;
#[cfg(test)]
pub(crate) mod testutil;
pub mod treemap;
pub mod ui;
pub mod usage;
pub mod volume;
