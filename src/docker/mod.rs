//! The Docker page: what Docker Desktop holds (images, containers, volumes,
//! build cache), read as exact numbers from the Docker Engine API and
//! analysed here, never trusting Docker's own shared/unique figures.

pub mod analysis;
pub mod api;
pub mod cli;
pub mod model;
pub mod state;
#[cfg(test)]
pub(crate) mod testdata;
pub mod view;
pub mod vmtools;

pub use model::*;
