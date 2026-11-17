//! Flint Asset Gen - AI-powered asset generation pipeline
//!
//! Provides a pluggable provider framework for generating textures, 3D models,
//! and audio via AI services (Flux, Meshy, ElevenLabs) with style guide support,
//! job tracking, and content-addressed catalog integration.
//!
//! Every paid call goes through a [`Calls`]: one transport and retry policy,
//! and a [`flint_ai_core::CostMeter`] that refuses a call before it could pass
//! `--max-cost` and records each one in the project's cost ledger (#721).

pub mod batch;
pub mod calls;
pub mod config;
pub mod human_task;
pub mod job;
pub mod manifest;
pub mod provider;
pub mod providers;
pub mod registration;
pub mod semantic;
pub mod style;
pub mod time;
pub mod validate;

pub use calls::Calls;
pub use config::FlintConfig;
pub use job::{GenerationJob, JobStatus, JobStore};
pub use manifest::BuildManifest;
pub use provider::{
    AssetKind, AudioParams, GenerateRequest, GenerateResult, GenerationProvider, ModelParams,
    ProviderStatus, TextureParams,
};
pub use registration::{
    register_generated_asset, register_generated_asset_in, register_generated_asset_with_roots,
    write_asset_sidecar, write_asset_sidecar_with_root, RegisteredAsset,
};
pub use semantic::SemanticAssetDef;
pub use style::StyleGuide;
