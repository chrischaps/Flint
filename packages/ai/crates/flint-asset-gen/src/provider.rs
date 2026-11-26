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

/// Trait implemented by each generation provider (Flux, Meshy, ElevenLabs, Mock).
///
/// A paid provider takes a [`crate::Calls`] when it is made
/// ([`crate::providers::create_provider`]) and sends every request through
/// it: the meter is asked before the call that costs money and records it
/// once the provider has accepted it (#721).
pub trait GenerationProvider: Send {
    /// Provider name (e.g. "flux", "meshy", "elevenlabs", "mock")
    fn name(&self) -> &str;

    /// Asset types this provider can generate
    fn supported_kinds(&self) -> Vec<AssetKind>;

    /// Check if the provider is available (API key set, service reachable)
    fn health_check(&self) -> Result<ProviderStatus>;

    /// Generate an asset (blocks until complete). A provider whose work
    /// takes minutes keeps a [`GenerationJob`] record while it waits, when
    /// its `Calls` has a job store.
    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult>;

    /// How many of the provider's price units `request` is billed (see
    /// `prices.toml`: megapixels for Flux, seconds for ElevenLabs, tasks for
    /// Meshy). A free provider counts 0.
    fn billed_units(&self, request: &GenerateRequest) -> f64 {
        let _ = request;
        0.0
    }

    /// Wait for a job a generate submitted and didn't see finish (the
    /// process stopped, or the wait timed out), then download it. Nothing
    /// new is paid for. Only a provider with remote jobs has any.
    fn resume(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        let _ = output_dir;
        Err(flint_core::FlintError::GenerationError(format!(
            "job {} is {}'s, which has no remote jobs to resume",
            job.id,
            self.name()
        )))
    }

    /// Build the enriched prompt from request + style guide (for inspection)
    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String;
}
