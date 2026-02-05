//! Flint Core - Foundational types for the Flint engine
//!
//! This crate provides the core types that all other Flint crates depend on:
//! - `EntityId` - Stable entity identifiers
//! - `ContentHash` - SHA-256 based content hashing
//! - `Transform`, `Vec3` - Spatial types
//! - Error types and Result alias

mod error;
mod hash;
mod id;
mod types;

pub use error::{FlintError, Result};
pub use hash::ContentHash;
pub use id::EntityId;
pub use types::{Color, Transform, Vec3};
