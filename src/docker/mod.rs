//! The Docker page: what Docker Desktop holds (images, containers, volumes,
//! build cache), read as exact numbers from the Docker Engine API and
//! analysed here, never trusting Docker's own shared/unique figures.

pub mod api;
pub mod model;

pub use model::*;
