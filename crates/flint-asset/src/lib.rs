//! Flint Asset - Content-addressed asset pipeline
//!
//! This crate provides asset management with content-addressed storage,
//! metadata catalogs, and reference resolution.

mod catalog;
mod resolver;
mod store;
mod types;

pub use catalog::AssetCatalog;
pub use resolver::{AssetResolver, ResolutionStrategy, ResolveResult};
pub use store::ContentStore;
pub use types::{AssetFile, AssetMeta, AssetRef, AssetType};
