//! Meshy 3D model generation provider
//!
//! Generates 3D models via the Meshy v2 text-to-3d API.
//! Model generation is long-running (~2-5 min), so `generate()` polls
//! with progress output, and `submit_job()` is available for async use.

use crate::config::FlintConfig;
use crate::job::GenerationJob;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_MESHY_URL: &str = "https://api.meshy.ai/openapi/v2/text-to-3d";
const POLL_INTERVAL_SECS: u64 = 10;

/// Meshy provider for AI 3D model generation
pub struct MeshyProvider {
    api_key: String,
    api_url: String,
}

impl MeshyProvider {
    /// Create a new MeshyProvider from config
    pub fn from_config(config: &FlintConfig) -> Result<Self> {
        let api_key = config
            .api_key("meshy")
            .ok_or_else(|| {
                FlintError::GenerationError(
                    "Meshy API key not configured. Set FLINT_MESHY_API_KEY or add to .flint/config.toml".to_string(),
                )
            })?
            .to_string();

        let api_url = config
            .api_url("meshy")
            .unwrap_or(DEFAULT_MESHY_URL)
            .to_string();

        Ok(Self { api_key, api_url })
    }

    /// Submit a text-to-3d task and return the task ID
    fn submit_task(&self, prompt: &str, negative: Option<&str>) -> Result<String> {
        let mut payload = serde_json::json!({
            "mode": "refine",
            "prompt": prompt,
            "should_remesh": true
        });

        if let Some(neg) = negative {
            payload["negative_prompt"] = serde_json::json!(neg);
        }

        let response: serde_json::Value = ureq::post(&self.api_url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send_json(&payload)
            .map_err(|e| FlintError::GenerationError(format!("Meshy API request failed: {}", e)))?
            .body_mut()
            .read_json()
            .map_err(|e| FlintError::GenerationError(format!("Failed to parse Meshy response: {}", e)))?;

        response
            .get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                FlintError::GenerationError(format!(
                    "Unexpected Meshy submit response: {}",
                    serde_json::to_string_pretty(&response).unwrap_or_default()
                ))
            })
    }

    /// Poll task status
    fn poll_task(&self, task_id: &str) -> Result<MeshyTaskStatus> {
        let url = format!("{}/{}", self.api_url, task_id);

        let response: serde_json::Value = ureq::get(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .call()
            .map_err(|e| FlintError::GenerationError(format!("Meshy poll failed: {}", e)))?
            .body_mut()
            .read_json()
            .map_err(|e| FlintError::GenerationError(format!("Failed to parse poll response: {}", e)))?;

        let status = response
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("UNKNOWN");

        let progress = response
            .get("progress")
            .and_then(|p| p.as_u64())
            .unwrap_or(0) as u8;

        match status {
            "SUCCEEDED" => {
                let model_url = response
                    .get("model_urls")
                    .and_then(|u| u.get("glb"))
                    .and_then(|u| u.as_str())
                    .map(|s| s.to_string());

                Ok(MeshyTaskStatus::Complete { model_url })
            }
            "FAILED" | "EXPIRED" => {
                let msg = response
                    .get("task_error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                Ok(MeshyTaskStatus::Failed(msg))
            }
            _ => Ok(MeshyTaskStatus::Processing(progress)),
        }
    }

    /// Download a GLB file from URL
    fn download_glb(&self, url: &str, output_path: &Path) -> Result<()> {
        let mut reader = ureq::get(url)
            .call()
            .map_err(|e| FlintError::GenerationError(format!("Failed to download model: {}", e)))?
            .into_body()
            .into_reader();

        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut bytes)
            .map_err(|e| FlintError::GenerationError(format!("Failed to read model data: {}", e)))?;

        std::fs::write(output_path, &bytes)?;
        Ok(())
    }
}

enum MeshyTaskStatus {
    Processing(u8),
    Complete { model_url: Option<String> },
    Failed(String),
}

impl GenerationProvider for MeshyProvider {
    fn name(&self) -> &str {
        "meshy"
    }

    fn supported_kinds(&self) -> Vec<AssetKind> {
        vec![AssetKind::Model]
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
        let negative = style.and_then(|s| s.negative()).map(|s| s.to_string());

        std::fs::create_dir_all(output_dir)?;

        // Submit task
        let task_id = self.submit_task(&prompt, negative.as_deref())?;
        eprintln!("  Submitted job: {}", task_id);

        // Poll until complete
        loop {
            std::thread::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS));

            match self.poll_task(&task_id)? {
                MeshyTaskStatus::Processing(progress) => {
                    eprintln!("  Processing... {}%", progress);
                }
                MeshyTaskStatus::Complete { model_url } => {
                    let url = model_url.ok_or_else(|| {
                        FlintError::GenerationError("No GLB URL in completion response".to_string())
                    })?;

                    let output_path = output_dir.join(format!("{}.glb", request.name));
                    self.download_glb(&url, &output_path)?;

                    let duration = start.elapsed().as_secs_f64();
                    let hash = flint_core::ContentHash::from_file(&output_path)
                        .map(|h| h.to_prefixed_hex())
                        .ok();

                    let mut metadata = HashMap::new();
                    metadata.insert("task_id".to_string(), task_id);

                    return Ok(GenerateResult {
                        output_path: output_path.to_string_lossy().to_string(),
                        prompt_used: prompt,
                        provider: "meshy".to_string(),
                        duration_secs: duration,
                        content_hash: hash,
                        metadata,
                    });
                }
                MeshyTaskStatus::Failed(msg) => {
                    return Err(FlintError::GenerationError(format!(
                        "Meshy generation failed: {}",
                        msg
                    )));
                }
            }
        }
    }

    fn submit_job(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
    ) -> Result<GenerationJob> {
        let prompt = self.build_prompt(request, style);
        let negative = style.and_then(|s| s.negative()).map(|s| s.to_string());
        let task_id = self.submit_task(&prompt, negative.as_deref())?;

        let mut job = GenerationJob::new("meshy", &request.name);
        job.remote_id = Some(task_id);
        job.prompt = Some(prompt);
        Ok(job)
    }

    fn poll_job(&self, job: &GenerationJob) -> Result<JobPollResult> {
        let remote_id = job.remote_id.as_deref().ok_or_else(|| {
            FlintError::GenerationError("Job has no remote ID".to_string())
        })?;

        match self.poll_task(remote_id)? {
            MeshyTaskStatus::Processing(p) => Ok(JobPollResult::Processing(p)),
            MeshyTaskStatus::Complete { .. } => Ok(JobPollResult::Complete),
            MeshyTaskStatus::Failed(msg) => Ok(JobPollResult::Failed(msg)),
        }
    }

    fn download_result(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        let remote_id = job.remote_id.as_deref().ok_or_else(|| {
            FlintError::GenerationError("Job has no remote ID".to_string())
        })?;

        // Poll one more time to get the model URL
        match self.poll_task(remote_id)? {
            MeshyTaskStatus::Complete { model_url } => {
                let url = model_url.ok_or_else(|| {
                    FlintError::GenerationError("No GLB URL in completion response".to_string())
                })?;

                std::fs::create_dir_all(output_dir)?;
                let output_path = output_dir.join(format!("{}.glb", job.asset_name));
                self.download_glb(&url, &output_path)?;

                let hash = flint_core::ContentHash::from_file(&output_path)
                    .map(|h| h.to_prefixed_hex())
                    .ok();

                Ok(GenerateResult {
                    output_path: output_path.to_string_lossy().to_string(),
                    prompt_used: job.prompt.clone().unwrap_or_default(),
                    provider: "meshy".to_string(),
                    duration_secs: 0.0,
                    content_hash: hash,
                    metadata: HashMap::new(),
                })
            }
            MeshyTaskStatus::Processing(_) => Err(FlintError::GenerationError(
                "Job not yet complete".to_string(),
            )),
            MeshyTaskStatus::Failed(msg) => Err(FlintError::GenerationError(format!(
                "Job failed: {}",
                msg
            ))),
        }
    }

    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String {
        match style {
            Some(s) => s.enrich_prompt(&request.description),
            None => request.description.clone(),
        }
    }
}

/// Parse a Meshy submit response for testing
pub fn parse_meshy_submit(json: &str) -> Result<String> {
    let response: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FlintError::GenerationError(format!("Invalid JSON: {}", e)))?;

    response
        .get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| FlintError::GenerationError("No task ID in response".to_string()))
}

/// Parse a Meshy poll response for testing
pub fn parse_meshy_poll(json: &str) -> Result<(String, u8, Option<String>)> {
    let response: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FlintError::GenerationError(format!("Invalid JSON: {}", e)))?;

    let status = response
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();

    let progress = response
        .get("progress")
        .and_then(|p| p.as_u64())
        .unwrap_or(0) as u8;

    let model_url = response
        .get("model_urls")
        .and_then(|u| u.get("glb"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Ok((status, progress, model_url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_meshy_submit_response() {
        let json = r#"{"result":"018d2158-xxxx-yyyy-zzzz-aabbccddee"}"#;
        let task_id = parse_meshy_submit(json).unwrap();
        assert_eq!(task_id, "018d2158-xxxx-yyyy-zzzz-aabbccddee");
    }

    #[test]
    fn test_parse_meshy_poll_pending() {
        let json = r#"{"status":"PENDING","progress":25}"#;
        let (status, progress, url) = parse_meshy_poll(json).unwrap();
        assert_eq!(status, "PENDING");
        assert_eq!(progress, 25);
        assert!(url.is_none());
    }

    #[test]
    fn test_parse_meshy_poll_complete() {
        let json = r#"{
            "status": "SUCCEEDED",
            "progress": 100,
            "model_urls": {
                "glb": "https://example.com/model.glb",
                "fbx": "https://example.com/model.fbx"
            }
        }"#;
        let (status, progress, url) = parse_meshy_poll(json).unwrap();
        assert_eq!(status, "SUCCEEDED");
        assert_eq!(progress, 100);
        assert_eq!(url.unwrap(), "https://example.com/model.glb");
    }

    #[test]
    fn test_parse_meshy_poll_failed() {
        let json = r#"{
            "status": "FAILED",
            "progress": 50,
            "task_error": {"message": "Generation failed due to content policy"}
        }"#;
        let (status, progress, _) = parse_meshy_poll(json).unwrap();
        assert_eq!(status, "FAILED");
        assert_eq!(progress, 50);
    }
}
