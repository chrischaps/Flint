//! Meshy 3D model generation provider
//!
//! Generates 3D models via the Meshy v2 text-to-3d API. A model takes
//! minutes, so the task is billed when Meshy accepts it and then polled.
//! With a job store in its `Calls`, the task is a [`GenerationJob`] from the
//! moment it is accepted, updated as it is polled: a run that stops (or
//! times out) leaves a job `flint asset job resume <id>` finishes without
//! paying again (#721).

use crate::calls::{cost_error, endpoint_label, Calls};
use crate::config::FlintConfig;
use crate::job::{GenerationJob, JobStatus};
use crate::provider::*;
use crate::style::StyleGuide;
use flint_ai_core::HttpRequest;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_MESHY_URL: &str = "https://api.meshy.ai/openapi/v2/text-to-3d";

/// Meshy provider for AI 3D model generation
pub struct MeshyProvider {
    api_key: String,
    api_url: String,
    calls: Calls,
}

impl MeshyProvider {
    /// Create a new MeshyProvider from config, its calls through `calls`
    pub fn from_config(config: &FlintConfig, calls: Calls) -> Result<Self> {
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

        Ok(Self {
            api_key,
            api_url,
            calls,
        })
    }

    fn auth(&self, req: HttpRequest) -> HttpRequest {
        req.with_header("Authorization", format!("Bearer {}", self.api_key))
    }

    /// Submit a text-to-3d task (the paid call) and return the task ID
    fn submit_task(&self, prompt: &str, negative: Option<&str>) -> Result<String> {
        let mut payload = serde_json::json!({
            "mode": "refine",
            "prompt": prompt,
            "should_remesh": true
        });
        if let Some(neg) = negative {
            payload["negative_prompt"] = serde_json::json!(neg);
        }
        let req = self.auth(HttpRequest::post_json(&self.api_url, &payload));
        let response = self.calls.request_json(&req, "Meshy API request")?;
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
        let response = self
            .calls
            .request_json(&self.auth(HttpRequest::get(url)), "Meshy poll")?;
        Ok(task_status(&response))
    }

    fn save(&self, job: &GenerationJob) {
        if let Some(store) = &self.calls.jobs {
            if let Err(e) = store.save(job) {
                tracing::warn!("could not record Meshy job {}: {}", job.id, e);
            }
        }
    }

    /// Poll `job`'s task until it is done, then download the GLB, keeping
    /// the job record current at every step.
    fn finish(
        &self,
        job: &mut GenerationJob,
        output_dir: &Path,
        start: std::time::Instant,
    ) -> Result<GenerateResult> {
        let task_id = job
            .remote_id
            .clone()
            .ok_or_else(|| FlintError::GenerationError("Job has no remote ID".to_string()))?;
        let hint = if self.calls.jobs.is_some() {
            format!(
                "; it is paid for: `flint asset job resume {}` finishes it",
                job.id
            )
        } else {
            String::new()
        };
        let resume_hint = || hint.clone();
        for _ in 0..self.calls.max_polls {
            self.calls.sleep(self.calls.poll_interval);
            let status = match self.poll_task(&task_id) {
                Ok(s) => s,
                Err(e) => {
                    let hint = resume_hint();
                    job.error = Some(e.to_string());
                    self.save(job);
                    return Err(FlintError::GenerationError(format!("{e}{hint}")));
                }
            };
            match status {
                MeshyTaskStatus::Processing(progress) => {
                    tracing::info!("Processing... {}%", progress);
                    job.status = JobStatus::Processing;
                    job.progress = progress;
                    self.save(job);
                }
                MeshyTaskStatus::Complete { model_url } => {
                    let url = model_url.ok_or_else(|| {
                        FlintError::GenerationError("No GLB URL in completion response".to_string())
                    })?;
                    std::fs::create_dir_all(output_dir)?;
                    let output_path = output_dir.join(format!("{}.glb", job.asset_name));
                    let glb = self
                        .calls
                        .request(&HttpRequest::get(url), "Meshy model download")?;
                    std::fs::write(&output_path, &glb.body)?;

                    let output = output_path.to_string_lossy().to_string();
                    job.status = JobStatus::Complete;
                    job.progress = 100;
                    job.error = None;
                    job.output_path = Some(output.clone());
                    self.save(job);

                    let hash = flint_core::ContentHash::from_file(&output_path)
                        .map(|h| h.to_prefixed_hex())
                        .ok();
                    let mut metadata = HashMap::new();
                    metadata.insert("task_id".to_string(), task_id);
                    if self.calls.jobs.is_some() {
                        metadata.insert("job_id".to_string(), job.id.clone());
                    }
                    return Ok(GenerateResult {
                        output_path: output,
                        prompt_used: job.prompt.clone().unwrap_or_default(),
                        provider: "meshy".to_string(),
                        duration_secs: start.elapsed().as_secs_f64(),
                        content_hash: hash,
                        metadata,
                    });
                }
                MeshyTaskStatus::Failed(msg) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(msg.clone());
                    self.save(job);
                    return Err(FlintError::GenerationError(format!(
                        "Meshy generation failed: {}",
                        msg
                    )));
                }
            }
        }
        let hint = resume_hint();
        let msg = format!(
            "Meshy generation timed out after {} poll attempts",
            self.calls.max_polls
        );
        job.error = Some(msg.clone());
        self.save(job);
        Err(FlintError::GenerationError(format!("{msg}{hint}")))
    }
}

#[derive(Debug, PartialEq)]
enum MeshyTaskStatus {
    Processing(u8),
    Complete { model_url: Option<String> },
    Failed(String),
}

fn task_status(response: &serde_json::Value) -> MeshyTaskStatus {
    let status = response
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("UNKNOWN");
    let progress = response
        .get("progress")
        .and_then(|p| p.as_u64())
        .unwrap_or(0)
        .min(100) as u8;
    match status {
        "SUCCEEDED" => MeshyTaskStatus::Complete {
            model_url: response
                .get("model_urls")
                .and_then(|u| u.get("glb"))
                .and_then(|u| u.as_str())
                .map(|s| s.to_string()),
        },
        "FAILED" | "EXPIRED" => MeshyTaskStatus::Failed(
            response
                .get("task_error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string(),
        ),
        _ => MeshyTaskStatus::Processing(progress),
    }
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

    /// One task per model.
    fn billed_units(&self, _request: &GenerateRequest) -> f64 {
        1.0
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

        let units = self.billed_units(request);
        self.calls
            .meter
            .admit_call("meshy", units)
            .map_err(cost_error)?;
        let task_id = self.submit_task(&prompt, negative.as_deref())?;
        // Meshy accepted the task: it's billed, and from here a job.
        let cost =
            self.calls
                .meter
                .record_call("meshy", &endpoint_label(&self.api_url, "meshy"), units);
        let mut job = GenerationJob::new("meshy", &request.name);
        job.remote_id = Some(task_id.clone());
        job.prompt = Some(prompt);
        job.estimated_usd = Some(cost);
        self.save(&job);
        tracing::info!("Submitted Meshy task {} (job {})", task_id, job.id);

        self.finish(&mut job, output_dir, start)
    }

    fn resume(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        if job.provider != "meshy" {
            return Err(FlintError::GenerationError(format!(
                "job {} is {}'s, not Meshy's",
                job.id, job.provider
            )));
        }
        let mut job = job.clone();
        self.finish(&mut job, output_dir, std::time::Instant::now())
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
    use crate::job::JobStore;
    use crate::providers::test_support::{calls_over, request, temp_dir, test_config};
    use flint_ai_core::{HttpResponse, Method, ScriptedTransport};

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
        assert_eq!(
            task_status(&serde_json::from_str(json).unwrap()),
            MeshyTaskStatus::Failed("Generation failed due to content policy".into())
        );
    }

    fn json(v: serde_json::Value) -> std::result::Result<HttpResponse, String> {
        ScriptedTransport::json(v)
    }

    #[test]
    fn a_generate_is_a_job_from_submit_to_download() {
        let script = ScriptedTransport::new(vec![
            json(serde_json::json!({"result": "task-1"})),
            json(serde_json::json!({"status": "IN_PROGRESS", "progress": 40})),
            json(serde_json::json!({"status": "SUCCEEDED", "progress": 100,
                "model_urls": {"glb": "http://meshy.test/m.glb"}})),
            Ok(HttpResponse::bytes(b"glTF fake".to_vec())),
        ]);
        let dir = temp_dir("meshy");
        let store = JobStore::new(dir.join("jobs"));
        let (calls, meter) = calls_over(&script, None);
        let meshy = MeshyProvider::from_config(
            &test_config("meshy", "http://meshy.test/openapi/v2/text-to-3d"),
            calls.with_jobs(store.clone()),
        )
        .unwrap();
        let result = meshy
            .generate(&request("chair", AssetKind::Model), None, &dir.join("out"))
            .unwrap();
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"glTF fake");

        let sent = script.requests();
        assert_eq!(sent[0].header("authorization"), Some("Bearer test-key"));
        assert_eq!(sent[1].method, Method::Get);
        assert_eq!(
            sent[1].url,
            "http://meshy.test/openapi/v2/text-to-3d/task-1"
        );

        let jobs = store.list().unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.remote_id.as_deref(), Some("task-1"));
        assert_eq!(job.status, JobStatus::Complete);
        assert_eq!(job.progress, 100);
        assert_eq!(
            job.output_path.as_deref(),
            Some(result.output_path.as_str())
        );
        assert_eq!(result.metadata["job_id"], job.id);
        // One task at the table's $0.60, the same in the job and the ledger.
        assert_eq!(meter.calls(), 1);
        assert!((job.estimated_usd.unwrap() - 0.60).abs() < 1e-12);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_timed_out_job_is_kept_and_resumed_without_paying_again() {
        let script = ScriptedTransport::new(vec![
            json(serde_json::json!({"result": "task-2"})),
            json(serde_json::json!({"status": "IN_PROGRESS", "progress": 10})),
            json(serde_json::json!({"status": "IN_PROGRESS", "progress": 60})),
        ]);
        let dir = temp_dir("meshy-resume");
        let store = JobStore::new(dir.join("jobs"));
        let (calls, meter) = calls_over(&script, None);
        let calls = calls
            .with_jobs(store.clone())
            .with_polling(std::time::Duration::ZERO, 2);
        let meshy = MeshyProvider::from_config(&test_config("meshy", "http://meshy.test/t"), calls)
            .unwrap();
        let e = meshy
            .generate(&request("table", AssetKind::Model), None, &dir.join("out"))
            .unwrap_err();
        let job = store.list().unwrap().remove(0);
        assert!(e.to_string().contains("timed out"), "{e}");
        assert!(
            e.to_string()
                .contains(&format!("flint asset job resume {}", job.id)),
            "{e}"
        );
        assert_eq!(job.status, JobStatus::Processing);
        assert_eq!(job.progress, 60);

        // A later run picks the task up where it was.
        let script = ScriptedTransport::new(vec![
            json(serde_json::json!({"status": "SUCCEEDED",
                "model_urls": {"glb": "http://meshy.test/t.glb"}})),
            Ok(HttpResponse::bytes(b"glTF".to_vec())),
        ]);
        let (calls, meter2) = calls_over(&script, None);
        let meshy = MeshyProvider::from_config(
            &test_config("meshy", "http://meshy.test/t"),
            calls.with_jobs(store.clone()),
        )
        .unwrap();
        let result = meshy.resume(&job, &dir.join("out")).unwrap();
        assert!(result.output_path.ends_with("table.glb"));
        assert_eq!(store.load(&job.id).unwrap().status, JobStatus::Complete);
        assert_eq!(meter.calls(), 1);
        assert_eq!(meter2.calls(), 0, "a resume pays for nothing");
        assert!(script.requests().iter().all(|r| r.method == Method::Get));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_task_marks_its_job_failed() {
        let script = ScriptedTransport::new(vec![
            json(serde_json::json!({"result": "task-3"})),
            json(serde_json::json!({"status": "FAILED", "task_error": {"message": "policy"}})),
        ]);
        let dir = temp_dir("meshy-fail");
        let store = JobStore::new(dir.join("jobs"));
        let (calls, _) = calls_over(&script, None);
        let meshy = MeshyProvider::from_config(
            &test_config("meshy", "http://meshy.test/t"),
            calls.with_jobs(store.clone()),
        )
        .unwrap();
        let e = meshy
            .generate(&request("x", AssetKind::Model), None, &dir)
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "Generation error: Meshy generation failed: policy"
        );
        let job = store.list().unwrap().remove(0);
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("policy"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
