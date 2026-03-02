//! Flint ProcGen — Procedural generation subsystem for the Flint engine.
//!
//! This crate provides the foundational types for procedural content
//! generation: deterministic seeding, mesh and image output formats,
//! material descriptions, and error handling. Higher-level constructs
//! (generator traits, registry, specs) build on these primitives.

pub mod algorithms;
mod error;
mod generator;
pub mod generators;
pub mod mock;
mod output;
mod registry;
mod rng;
mod seed;
mod spec;
mod types;

pub use error::{ProcGenError, Result};
pub use generator::{GenerationCost, Generator};
pub use generators::register_built_in_generators;
pub use generators::texture::TextureGenerator;
pub use generators::tree::TreeGenerator;
pub use output::{GeneratorOutput, OutputKind};
pub use registry::GeneratorRegistry;
pub use rng::SeededRng;
pub use seed::Seed;
pub use spec::{discover_specs, LodLevel, ProcGenSpec, SeedConfig, SeedMode, SpecMeta};
pub use types::{
    BoundingBox, BranchSegment, ChannelSemantics, ImageData, MaterialData, MeshData, Vertex,
};

// Texture pattern types
pub use generators::texture::{
    derive_maps, Pattern, PatternCell, PatternField, PerlinOrganicParams, PerlinOrganicPattern,
    TextureMapParams, TilingGridParams, TilingGridPattern, VoronoiBrickParams,
    VoronoiBrickPattern,
};
