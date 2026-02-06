//! Flint Asset Gen - AI-powered asset generation pipeline
//!
//! Provides a pluggable provider framework for generating textures, 3D models,
//! and audio via AI services (Flux, Meshy, ElevenLabs) with style guide support,
//! job tracking, and content-addressed catalog integration.

pub mod batch;
pub mod config;
pub mod human_task;
pub mod job;
pub mod manifest;
pub mod provider;
pub mod providers;
pub mod semantic;
pub mod style;
pub mod validate;

pub use config::FlintConfig;
pub use job::{GenerationJob, JobStatus, JobStore};
pub use manifest::BuildManifest;
pub use provider::{
    AssetKind, AudioParams, GenerateRequest, GenerateResult, GenerationProvider, ModelParams,
    ProviderStatus, TextureParams,
};
pub use semantic::SemanticAssetDef;
pub use style::StyleGuide;
