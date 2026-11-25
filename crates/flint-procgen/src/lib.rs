//! Flint ProcGen — Procedural generation subsystem for the Flint engine.
//!
//! This crate provides the foundational types for procedural content
//! generation: deterministic seeding, mesh and image output formats,
//! material descriptions, and error handling. Higher-level constructs
//! (generator traits, registry, specs) build on these primitives.
// Unit tests read their files off the disk and assert on TOML as written;
// the lib build still holds the rules (#761).
#![cfg_attr(test, allow(clippy::disallowed_methods))]
// The tree generator's parameter schema is one large `json!` literal.
#![recursion_limit = "256"]

pub mod algorithms;
mod cache;
pub mod disk_cache;
mod error;
pub mod export_glb;
mod generator;
pub mod generators;
pub mod living;
pub mod living_knobs;
// How far one picture is from another: the refine loops' ruler, and
// `flint render --reference`'s, which a build without the ai package keeps
// (#1333). flint-procgen-ai re-exports it as `flint_procgen_ai::measure`.
pub mod measure;
pub mod mock;
pub mod model;
mod output;
pub mod param_ui;
pub mod params;
mod queue;
mod registry;
pub mod resolver;
mod rng;
pub mod scene_tables;
mod seed;
pub mod skinning;
mod spec;
mod types;
pub mod validate;

pub use cache::{CacheStats, ProcGenCache, ProcGenCacheConfig};
pub use disk_cache::{DiskCache, DiskCacheConfig, DiskStats};
pub use error::{ProcGenError, Result};
pub use export_glb::{mesh_lods_to_glb, mesh_to_glb, skinned_mesh_to_glb};
pub use generator::{GenerationCost, Generator, LivingGenerator};
pub use generators::creature::CreatureGenerator;
pub use generators::register_built_in_generators;
pub use generators::sweep::SweepGenerator;
pub use generators::terrain::TerrainGenerator;
pub use generators::texture::TextureGenerator;
pub use generators::tree::TreeGenerator;
// The living props' runtime (their rings, clocks, knobs and blending) is
// flint-living's (#1024); this crate keeps what a generator makes for them.
pub use living::{
    boil_bones, CardBoil, LeafBinding, LivingClock, LivingJitter, LivingPose, LivingRig,
    LivingSkeleton, PoseStream, RowDrift, RowLine, RowWord, SegmentPose, WarpedTime, BOIL_BONES,
    JITTER_CHANNELS, JITTER_RATE_MAX, ROW_RATE_MAX, STAGGER_PER_LEVEL,
};
pub use living_knobs::{
    species_def_of, species_keys, wooze_knobs_of, KnobType, KnobValue, LivingKnob,
};
pub use output::{GeneratorOutput, OutputKind};
pub use queue::{
    AssetKind, CompletedAsset, FailedAsset, GenerationRequest, ProcGenQueue, QueueStats,
};
pub use registry::GeneratorRegistry;
pub use resolver::ProcGenResolver;
pub use rng::SeededRng;
pub use scene_tables::{
    apply_preset_file, commit_look, edit_species, list_presets, list_presets_in, look_edits,
    preset_save_dir, read_preset, read_preset_in, settle_scene, write_preset, write_preset_in,
    BoilDef, CommitReport, LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingTables,
    LivingVarianceDef, WoozeDef, WoozeIssue, WoozeKnobs, WoozePreset, PRESET_SUFFIX,
};
pub use seed::Seed;
pub use skinning::{BoneDef, SkeletonDef, SkinnedMeshData, SkinnedVertex, MAX_SKINNED_BONES};
pub use spec::{
    discover_specs, DriftConfig, EchoBlend, EchoConfig, EchoCopy, EchoPlace, EchoPlaces,
    JitterConfig, LivingConfig, LodLevel, NoiseConfig, PoseSource, ProcGenSpec, SeedConfig,
    SeedMode, SpecMeta, Stagger, VarianceConfig,
};
// What flint-living's echoes read of a spec's (#1024)
pub use spec::{slide_shown, MAX_ECHOES, MAX_ECHO_SPREAD};
pub use types::{
    BoundingBox, BranchSegment, ChannelSemantics, ImageData, MaterialData, MeshData, PixelFormat,
    SubMesh, Vertex,
};

pub use model::{read_model, read_png, Model, ModelLayout, ModelMesh};
pub use param_ui::{
    param_reference_markdown, param_reference_markdown_in, param_reference_text,
    parse_param_schema, ParamCondition, ParamFieldSpec, ParamFieldType,
};
pub use params::{
    parse_path, prop_type, schema_at, string_enum, validate_params, NumBounds, ParamIssue, PathStep,
};
pub use validate::{
    validate_model, validate_output, CheckStatus, ValidationCheck, ValidationConstraints,
    ValidationReport,
};

// Texture pattern types
pub use generators::texture::pipeline::{
    op_default_params, op_descriptor, op_param_schema, op_ports, op_schema,
    run_pipeline_with_snapshots, ALL_OP_TYPES, OPS,
};
pub use generators::texture::{
    derive_maps, op_reference_markdown, OpDescriptor, OpParams, OpPortInfo, ParamDesc, ParamKind,
    Pattern, PatternCell, PatternField, PerlinOrganicParams, PerlinOrganicPattern, TextureField,
    TextureMapParams, TextureOp, TilingGridParams, TilingGridPattern, VoronoiBrickParams,
    VoronoiBrickPattern,
};

// The README's and CONTRIBUTING guide's Rust examples run as doctests, so
// the docs can't drift from the API they describe.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(doctest)]
#[doc = include_str!("../CONTRIBUTING.md")]
pub struct ContributingDoctests;
