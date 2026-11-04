//! Flint ProcGen — Procedural generation subsystem for the Flint engine.
//!
//! This crate provides the foundational types for procedural content
//! generation: deterministic seeding, mesh and image output formats,
//! material descriptions, and error handling. Higher-level constructs
//! (generator traits, registry, specs) build on these primitives.

// The tree generator's parameter schema is one large `json!` literal.
#![recursion_limit = "256"]

pub mod algorithms;
mod cache;
mod error;
pub mod export_glb;
mod generator;
pub mod generators;
pub mod mock;
pub mod model;
mod output;
pub mod param_ui;
mod queue;
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
pub use generators::creature::CreatureGenerator;
pub use generators::register_built_in_generators;
pub use generators::texture::TextureGenerator;
pub use generators::tree::TreeGenerator;
pub use output::{GeneratorOutput, OutputKind};
pub use queue::{
    AssetKind, CompletedAsset, FailedAsset, GenerationRequest, ProcGenQueue, QueueStats,
};
pub use registry::GeneratorRegistry;
pub use resolver::ProcGenResolver;
pub use rng::SeededRng;
pub use seed::Seed;
pub use skinning::{BoneDef, SkeletonDef, SkinnedMeshData, SkinnedVertex};
pub use spec::{discover_specs, LodLevel, ProcGenSpec, SeedConfig, SeedMode, SpecMeta};
pub use types::{
    BoundingBox, BranchSegment, ChannelSemantics, ImageData, MaterialData, MeshData, SubMesh,
    Vertex,
};

pub use model::{read_model, read_png, Model, ModelLayout, ModelMesh};
pub use param_ui::{
    lint_params, param_reference_markdown, parse_param_schema, ParamCondition, ParamFieldSpec,
    ParamFieldType, ParamIssue,
};
pub use validate::{
    validate_model, validate_output, CheckStatus, ValidationCheck, ValidationConstraints,
    ValidationReport,
};

// Texture pattern types
pub use generators::texture::pipeline::{
    op_default_params, op_param_schema, op_schema, run_pipeline_with_snapshots, ALL_OP_TYPES,
};
pub use generators::texture::{
    derive_maps, OpPortInfo, Pattern, PatternCell, PatternField, PerlinOrganicParams,
    PerlinOrganicPattern, TextureField, TextureMapParams, TextureOp, TilingGridParams,
    TilingGridPattern, VoronoiBrickParams, VoronoiBrickPattern,
};

// The README's and CONTRIBUTING guide's Rust examples run as doctests, so
// the docs can't drift from the API they describe.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(doctest)]
#[doc = include_str!("../CONTRIBUTING.md")]
pub struct ContributingDoctests;
