//! Flint Asset - Content-addressed asset pipeline
//!
//! This crate provides asset management with content-addressed storage,
//! metadata catalogs, and reference resolution.

mod catalog;
// Tooling only (it reads the real disk); the player never asks. #179
#[cfg(not(target_arch = "wasm32"))]
pub mod project;
mod resolver;
mod store;
mod types;

pub use catalog::AssetCatalog;
#[cfg(not(target_arch = "wasm32"))]
pub use project::Project;
pub use resolver::{AssetResolver, ResolutionStrategy, ResolveResult};
pub use store::ContentStore;
pub use types::{AssetFile, AssetMeta, AssetRef, AssetType};
