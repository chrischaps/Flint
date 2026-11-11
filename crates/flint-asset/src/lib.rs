//! Flint Asset - Content-addressed asset pipeline
//!
//! This crate provides asset management with content-addressed storage,
//! metadata catalogs, and reference resolution.

mod catalog;
// The store paths are tooling (the real disk); the player never asks. #179
// The project itself is flint_core::project, which the runtime can use.
#[cfg(not(target_arch = "wasm32"))]
pub mod project;
mod resolver;
mod store;
mod types;

pub use catalog::AssetCatalog;
#[cfg(not(target_arch = "wasm32"))]
pub use project::{Project, ProjectError, ProjectStore};
pub use resolver::{AssetResolver, ResolutionStrategy, ResolveResult};
pub use store::ContentStore;
pub use types::{AssetFile, AssetMeta, AssetRef, AssetType};
