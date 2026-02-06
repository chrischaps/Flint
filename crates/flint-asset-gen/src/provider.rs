//! Generation provider trait and request/result types

use flint_core::Result;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

use crate::job::GenerationJob;
use crate::style::StyleGuide;

/// The kind of asset to generate
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Texture,
    Model,
    Audio,
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetKind::Texture => write!(f, "texture"),
            AssetKind::Model => write!(f, "model"),
            AssetKind::Audio => write!(f, "audio"),
        }
    }
}

/// Parameters specific to texture generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureParams {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub seamless: bool,
}

impl Default for TextureParams {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            seed: None,
            seamless: false,
        }
    }
}

/// Parameters specific to 3D model generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelParams {
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default = "default_model_format")]
    pub format: String,
}

fn default_model_format() -> String {
    "glb".to_string()
}

impl Default for ModelParams {
    fn default() -> Self {
        Self {
            seed: None,
            format: default_model_format(),
        }
    }
}

/// Parameters specific to audio generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioParams {
    /// Duration in seconds
    #[serde(default = "default_audio_duration")]
    pub duration: f64,
    #[serde(default)]
    pub seed: Option<u64>,
}

fn default_audio_duration() -> f64 {
    3.0
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            duration: default_audio_duration(),
            seed: None,
        }
    }
}

/// A request to generate an asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Human-readable name for the asset
    pub name: String,
    /// Description / prompt for generation
    pub description: String,
    /// Kind of asset to generate
    pub kind: AssetKind,
    /// Texture-specific parameters
    #[serde(default)]
    pub texture_params: Option<TextureParams>,
    /// Model-specific parameters
    #[serde(default)]
    pub model_params: Option<ModelParams>,
    /// Audio-specific parameters
    #[serde(default)]
    pub audio_params: Option<AudioParams>,
    /// Optional tags to attach to the generated asset
    #[serde(default)]
    pub tags: Vec<String>,
}

/// The result of a successful generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResult {
    /// Path to the generated file on disk
    pub output_path: String,
    /// The final prompt that was sent to the provider
    pub prompt_used: String,
    /// Provider name
    pub provider: String,
    /// Generation time in seconds
    pub duration_secs: f64,
    /// Content hash (sha256:...)
    #[serde(default)]
    pub content_hash: Option<String>,
    /// Any provider-specific metadata
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

/// Status returned by a provider health check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStatus {
    Available,
    Unavailable(String),
    NoApiKey,
}

/// Result of polling an async job
#[derive(Debug, Clone)]
pub enum JobPollResult {
    /// Still processing, with progress percentage (0-100)
    Processing(u8),
    /// Completed successfully
    Complete,
    /// Failed with error message
    Failed(String),
}

/// Trait implemented by each generation provider (Flux, Meshy, ElevenLabs, Mock)
pub trait GenerationProvider: Send {
    /// Provider name (e.g. "flux", "meshy", "elevenlabs", "mock")
    fn name(&self) -> &str;

    /// Asset types this provider can generate
    fn supported_kinds(&self) -> Vec<AssetKind>;

    /// Check if the provider is available (API key set, service reachable)
    fn health_check(&self) -> Result<ProviderStatus>;

    /// Generate an asset synchronously (blocks until complete)
    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult>;

    /// Submit an async generation job (for long-running operations like 3D models)
    fn submit_job(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
    ) -> Result<GenerationJob>;

    /// Poll the status of an async job
    fn poll_job(&self, job: &GenerationJob) -> Result<JobPollResult>;

    /// Download the result of a completed async job
    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult>;

    /// Build the enriched prompt from request + style guide (for inspection)
    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String;
}
