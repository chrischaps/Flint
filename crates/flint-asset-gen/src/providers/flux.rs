//! Flux texture generation provider (fal.ai)
//!
//! Generates textures via the Flux image generation API.
//! Textures are fast (~10s) so `generate()` blocks synchronously.

use crate::config::FlintConfig;
use crate::job::GenerationJob;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const DEFAULT_FLUX_URL: &str = "https://queue.fal.run/fal-ai/flux/dev";
const REQUEST_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: usize = 3;
const RETRY_BASE_DELAY_MS: u64 = 500;

/// Flux provider for AI texture generation via fal.ai
pub struct FluxProvider {
    api_key: String,
    api_url: String,
}

impl FluxProvider {
    /// Create a new FluxProvider from config
    pub fn from_config(config: &FlintConfig) -> Result<Self> {
        let api_key = config
            .api_key("flux")
            .ok_or_else(|| {
                FlintError::GenerationError(
                    "Flux API key not configured. Set FLINT_FLUX_API_KEY or add to .flint/config.toml".to_string(),
                )
            })?
            .to_string();

        let api_url = config
            .api_url("flux")
            .unwrap_or(DEFAULT_FLUX_URL)
            .to_string();

        Ok(Self { api_key, api_url })
    }

    /// Submit a request to fal.ai and poll for completion
    fn submit_and_wait(
        &self,
        prompt: &str,
        width: u32,
        height: u32,
        seed: Option<u64>,
    ) -> Result<serde_json::Value> {
        let mut payload = serde_json::json!({
            "prompt": prompt,
            "image_size": {
                "width": width,
                "height": height
            },
            "num_images": 1,
            "enable_safety_checker": false
        });

        if let Some(s) = seed {
            payload["seed"] = serde_json::json!(s);
        }

        self.post_json_with_retry(&payload)
    }

    /// Download an image from a URL to a local file
    fn download_image(&self, url: &str, output_path: &Path) -> Result<()> {
        let bytes = self.download_bytes_with_retry(url)?;
        std::fs::write(output_path, &bytes)?;
        Ok(())
    }

    fn post_json_with_retry(&self, payload: &serde_json::Value) -> Result<serde_json::Value> {
        for attempt in 0..MAX_RETRIES {
            let agent = build_agent();
            let response = agent
                .post(&self.api_url)
                .header("Authorization", &format!("Key {}", self.api_key))
                .header("Content-Type", "application/json")
                .send_json(payload);

            match response {
                Ok(mut ok) => {
                    return ok.body_mut().read_json().map_err(|e| {
                        FlintError::GenerationError(format!(
                            "Failed to parse Flux response: {}",
                            e
                        ))
                    });
                }
                Err(e) => {
                    if attempt + 1 < MAX_RETRIES && is_retryable_error(&e) {
                        sleep_backoff(attempt);
                        continue;
                    }
                    return Err(FlintError::GenerationError(format!(
                        "Flux API request failed: {}",
                        e
                    )));
                }
            }
        }

        Err(FlintError::GenerationError(
            "Flux API request failed after retries".to_string(),
        ))
    }

    fn download_bytes_with_retry(&self, url: &str) -> Result<Vec<u8>> {
        for attempt in 0..MAX_RETRIES {
            let agent = build_agent();
            let response = agent.get(url).call();

            match response {
                Ok(ok) => {
                    let mut reader = ok.into_body().into_reader();
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut reader, &mut bytes).map_err(|e| {
                        FlintError::GenerationError(format!("Failed to read image data: {}", e))
                    })?;
                    return Ok(bytes);
                }
                Err(e) => {
                    if attempt + 1 < MAX_RETRIES && is_retryable_error(&e) {
                        sleep_backoff(attempt);
                        continue;
                    }
                    return Err(FlintError::GenerationError(format!(
                        "Failed to download image: {}",
                        e
                    )));
                }
            }
        }

        Err(FlintError::GenerationError(
            "Image download failed after retries".to_string(),
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

impl GenerationProvider for FluxProvider {
    fn name(&self) -> &str {
        "flux"
    }

    fn supported_kinds(&self) -> Vec<AssetKind> {
        vec![AssetKind::Texture]
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

        let params = request
            .texture_params
            .as_ref()
            .cloned()
            .unwrap_or_default();

        std::fs::create_dir_all(output_dir)?;

        // Submit and wait for result
        let response = self.submit_and_wait(&prompt, params.width, params.height, params.seed)?;

        // Extract image URL from response
        let image_url = response
            .get("images")
            .and_then(|imgs| imgs.as_array())
            .and_then(|arr| arr.first())
            .and_then(|img| img.get("url"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| {
                FlintError::GenerationError(format!(
                    "Unexpected Flux response format: {}",
                    serde_json::to_string_pretty(&response).unwrap_or_default()
                ))
            })?;

        // Download the image
        let output_path = output_dir.join(format!("{}.png", request.name));
        self.download_image(image_url, &output_path)?;

        let duration = start.elapsed().as_secs_f64();

        // Compute content hash
        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();

        let mut metadata = HashMap::new();
        if let Some(seed) = response.get("seed").and_then(|s| s.as_u64()) {
            metadata.insert("seed".to_string(), seed.to_string());
        }

        Ok(GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: prompt,
            provider: "flux".to_string(),
            duration_secs: duration,
            content_hash: hash,
            metadata,
        })
    }

    fn submit_job(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
    ) -> Result<GenerationJob> {
        // Flux is fast enough to be synchronous, but we support async for consistency
        let mut job = GenerationJob::new("flux", &request.name);
        job.prompt = Some(self.build_prompt(request, style));
        Ok(job)
    }

    fn poll_job(&self, _job: &GenerationJob) -> Result<JobPollResult> {
        // Flux completes synchronously
        Ok(JobPollResult::Complete)
    }

    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        // For Flux, we generate synchronously via generate()
        let request = GenerateRequest {
            name: job.asset_name.clone(),
            description: job.prompt.clone().unwrap_or_default(),
            kind: AssetKind::Texture,
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

/// Parse a Flux API response for testing
pub fn parse_flux_response(json: &str) -> Result<String> {
    let response: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FlintError::GenerationError(format!("Invalid JSON: {}", e)))?;

    response
        .get("images")
        .and_then(|imgs| imgs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| FlintError::GenerationError("No image URL in response".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flux_response() {
        let json = r#"{
            "images": [
                {
                    "url": "https://example.com/generated.png",
                    "width": 1024,
                    "height": 1024,
                    "content_type": "image/png"
                }
            ],
            "seed": 42,
            "has_nsfw_concepts": [false],
            "prompt": "a brick wall"
        }"#;

        let url = parse_flux_response(json).unwrap();
        assert_eq!(url, "https://example.com/generated.png");
    }

    #[test]
    fn test_parse_flux_response_invalid() {
        let json = r#"{"error": "something went wrong"}"#;
        assert!(parse_flux_response(json).is_err());
    }

    #[test]
    fn test_flux_build_prompt_no_style() {
        // We can't create a FluxProvider without an API key, but we can test the prompt building
        // through a mock scenario
        let request = GenerateRequest {
            name: "test".to_string(),
            description: "red brick wall".to_string(),
            kind: AssetKind::Texture,
            texture_params: None,
            model_params: None,
            audio_params: None,
            tags: vec![],
        };

        // Without a provider, test prompt building logic directly
        let style = StyleGuide {
            name: "test".to_string(),
            description: None,
            prompt_prefix: Some("Fantasy".to_string()),
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec![],
            materials: crate::style::MaterialConstraints::default(),
            geometry: crate::style::GeometryConstraints::default(),
        };

        let enriched = style.enrich_prompt(&request.description);
        assert!(enriched.contains("Fantasy"));
        assert!(enriched.contains("red brick wall"));
    }
}
