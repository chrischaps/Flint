//! Flint ProcGen — Procedural generation subsystem for the Flint engine.
//!
//! This crate provides the foundational types for procedural content
//! generation: deterministic seeding, mesh and image output formats,
//! material descriptions, and error handling. Higher-level constructs
//! (generator traits, registry, specs) build on these primitives.

mod error;
mod output;
mod seed;
mod types;

pub use error::{ProcGenError, Result};
pub use output::{GeneratorOutput, OutputKind};
pub use seed::Seed;
pub use types::{BoundingBox, ChannelSemantics, ImageData, MaterialData, MeshData, Vertex};
