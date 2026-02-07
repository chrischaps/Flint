//! ElevenLabs audio generation provider
//!
//! Generates sound effects via the ElevenLabs sound generation API.
//! Audio generation is fast (~3-10s), so `generate()` blocks synchronously.

use crate::config::FlintConfig;
use crate::job::GenerationJob;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const DEFAULT_ELEVENLABS_URL: &str = "https://api.elevenlabs.io/v1/sound-generation";
const REQUEST_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: usize = 3;
const RETRY_BASE_DELAY_MS: u64 = 500;

/// ElevenLabs provider for AI audio/sound effect generation
pub struct ElevenLabsProvider {
    api_key: String,
    api_url: String,
}

impl ElevenLabsProvider {
    /// Create a new ElevenLabsProvider from config
    pub fn from_config(config: &FlintConfig) -> Result<Self> {
        let api_key = config
            .api_key("elevenlabs")
            .ok_or_else(|| {
                FlintError::GenerationError(
                    "ElevenLabs API key not configured. Set FLINT_ELEVENLABS_API_KEY or add to .flint/config.toml".to_string(),
                )
            })?
            .to_string();

        let api_url = config
            .api_url("elevenlabs")
            .unwrap_or(DEFAULT_ELEVENLABS_URL)
            .to_string();

        Ok(Self { api_key, api_url })
    }

    /// Generate audio and return raw bytes
    fn generate_audio(&self, prompt: &str, duration: f64) -> Result<Vec<u8>> {
        let payload = serde_json::json!({
            "text": prompt,
            "duration_seconds": duration
        });

        self.post_audio_with_retry(&payload)
    }

    fn post_audio_with_retry(&self, payload: &serde_json::Value) -> Result<Vec<u8>> {
        for attempt in 0..MAX_RETRIES {
            let agent = build_agent();
            let response = agent
                .post(&self.api_url)
                .header("xi-api-key", &self.api_key)
                .header("Content-Type", "application/json")
                .send_json(payload);

            match response {
                Ok(ok) => {
                    let mut reader = ok.into_body().into_reader();
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut reader, &mut bytes).map_err(|e| {
                        FlintError::GenerationError(format!("Failed to read audio data: {}", e))
                    })?;
                    return Ok(bytes);
                }
                Err(e) => {
                    if attempt + 1 < MAX_RETRIES && is_retryable_error(&e) {
                        sleep_backoff(attempt);
                        continue;
                    }
                    return Err(FlintError::GenerationError(format!(
                        "ElevenLabs API request failed: {}",
                        e
                    )));
                }
            }
        }

        Err(FlintError::GenerationError(
            "ElevenLabs API request failed after retries".to_string(),
        ))
    }
}

fn build_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .build();
    config.into()
}

fn is_retryable_error(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::Timeout(_)
        | ureq::Error::Io(_)
        | ureq::Error::ConnectionFailed
        | ureq::Error::HostNotFound => true,
        ureq::Error::StatusCode(code) => matches!(code, 429 | 500 | 502 | 503 | 504),
        _ => false,
    }
}

fn sleep_backoff(attempt: usize) {
    let delay_ms = RETRY_BASE_DELAY_MS.saturating_mul(1u64 << attempt);
    std::thread::sleep(Duration::from_millis(delay_ms));
}

impl GenerationProvider for ElevenLabsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn supported_kinds(&self) -> Vec<AssetKind> {
        vec![AssetKind::Audio]
    }

    fn health_check(&self) -> Result<ProviderStatus> {
        if self.api_key.is_empty() {
            return Ok(ProviderStatus::NoApiKey);
        }
        Ok(ProviderStatus::Available)
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let start = std::time::Instant::now();
        let prompt = self.build_prompt(request, style);
        let duration = request
            .audio_params
            .as_ref()
            .map(|p| p.duration)
            .unwrap_or(3.0);

        std::fs::create_dir_all(output_dir)?;

        // Generate audio
        let audio_bytes = self.generate_audio(&prompt, duration)?;

        // ElevenLabs returns MP3 by default
        let output_path = output_dir.join(format!("{}.mp3", request.name));
        std::fs::write(&output_path, &audio_bytes)?;

        let elapsed = start.elapsed().as_secs_f64();

        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();

        Ok(GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: prompt,
            provider: "elevenlabs".to_string(),
            duration_secs: elapsed,
            content_hash: hash,
            metadata: HashMap::new(),
        })
    }

    fn submit_job(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
    ) -> Result<GenerationJob> {
        let mut job = GenerationJob::new("elevenlabs", &request.name);
        job.prompt = Some(self.build_prompt(request, style));
        Ok(job)
    }

    fn poll_job(&self, _job: &GenerationJob) -> Result<JobPollResult> {
        // ElevenLabs is synchronous
        Ok(JobPollResult::Complete)
    }

    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        let request = GenerateRequest {
            name: job.asset_name.clone(),
            description: job.prompt.clone().unwrap_or_default(),
            kind: AssetKind::Audio,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };
        self.generate(&request, None, output_dir)
    }

    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String {
        match style {
            Some(s) => s.enrich_prompt(&request.description),
            None => request.description.clone(),
        }
    }
}

/// Parse an ElevenLabs error response for testing
pub fn parse_elevenlabs_error(json: &str) -> Result<String> {
    let response: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FlintError::GenerationError(format!("Invalid JSON: {}", e)))?;

    let message = response
        .get("detail")
        .and_then(|d| d.get("message"))
        .and_then(|m| m.as_str())
        .or_else(|| response.get("detail").and_then(|d| d.as_str()))
        .unwrap_or("Unknown error")
        .to_string();

    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_elevenlabs_error_detail() {
        let json = r#"{"detail":{"status":"error","message":"Invalid API key"}}"#;
        let msg = parse_elevenlabs_error(json).unwrap();
        assert_eq!(msg, "Invalid API key");
    }

    #[test]
    fn test_parse_elevenlabs_error_string() {
        let json = r#"{"detail":"Unauthorized"}"#;
        let msg = parse_elevenlabs_error(json).unwrap();
        assert_eq!(msg, "Unauthorized");
    }
}
