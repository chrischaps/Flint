//! Flint ProcGen — Procedural generation subsystem for the Flint engine.
//!
//! This crate provides the foundational types for procedural content
//! generation: deterministic seeding, mesh and image output formats,
//! material descriptions, and error handling. Higher-level constructs
//! (generator traits, registry, specs) build on these primitives.

pub mod algorithms;
mod cache;
mod error;
pub mod export_glb;
mod generator;
pub mod generators;
pub mod mock;
mod output;
pub mod param_ui;
mod registry;
pub mod resolver;
mod rng;
mod seed;
pub mod skinning;
mod spec;
mod types;
pub mod validate;

pub use cache::{CacheStats, ProcGenCache, ProcGenCacheConfig};
pub use error::{ProcGenError, Result};
pub use export_glb::{mesh_lods_to_glb, mesh_to_glb, skinned_mesh_to_glb};
pub use generator::{GenerationCost, Generator};
pub use generators::register_built_in_generators;
pub use generators::creature::CreatureGenerator;
pub use generators::texture::TextureGenerator;
pub use generators::tree::TreeGenerator;
pub use output::{GeneratorOutput, OutputKind};
pub use registry::GeneratorRegistry;
pub use resolver::ProcGenResolver;
pub use rng::SeededRng;
pub use seed::Seed;
pub use spec::{discover_specs, LodLevel, ProcGenSpec, SeedConfig, SeedMode, SpecMeta};
pub use skinning::{BoneDef, SkeletonDef, SkinnedMeshData, SkinnedVertex};
pub use types::{
    BoundingBox, BranchSegment, ChannelSemantics, ImageData, MaterialData, MeshData, SubMesh,
    Vertex,
};

pub use param_ui::{parse_param_schema, ParamFieldSpec, ParamFieldType};
pub use validate::{CheckStatus, ValidationCheck, ValidationConstraints, ValidationReport, validate_output};

// Texture pattern types
pub use generators::texture::{
    derive_maps, Pattern, PatternCell, PatternField, PerlinOrganicParams, PerlinOrganicPattern,
    TextureMapParams, TilingGridParams, TilingGridPattern, VoronoiBrickParams,
    VoronoiBrickPattern,
};
