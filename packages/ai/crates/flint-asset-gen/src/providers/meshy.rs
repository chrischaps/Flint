//! Meshy 3D model generation provider
//!
//! Generates 3D models via the Meshy v2 text-to-3d API. A textured model is
//! two tasks, each billed when Meshy accepts it (#1187): a **preview** (the
//! prompt to a mesh) and, once that has succeeded, a **refine** of it
//! (`preview_task_id`: the mesh textured). Each takes minutes and is polled.
//! With a job store in its `Calls`, the model is a [`GenerationJob`] from the
//! moment the preview is accepted, updated as each task is polled and
//! naming both tasks: a run that stops (or times out) at either stage
//! leaves a job `flint asset job resume <id>` finishes without paying for
//! what was already paid for (#721).

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

/// What a preview costs on Meshy's default model (meshy-7.1), in credits.
const PREVIEW_CREDITS: f64 = 20.0;
/// What a refine costs at the default 2K textures, in credits.
const REFINE_CREDITS: f64 = 10.0;

/// The two tasks of a textured model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Preview,
    Refine,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Preview => "preview",
            Stage::Refine => "refine",
        }
    }

    fn credits(self) -> f64 {
        match self {
            Stage::Preview => PREVIEW_CREDITS,
            Stage::Refine => REFINE_CREDITS,
        }
    }

    /// The job's progress for this task's: the preview is the first half,
    /// the refine the second.
    fn progress(self, task: u8) -> u8 {
        let half = task.min(100) / 2;
        match self {
            Stage::Preview => half,
            Stage::Refine => 50 + half,
        }
    }
}

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

    /// What `credits` count in the unit Meshy's price names: credits when
    /// the table prices a credit (the built-in table's), else a share of one
    /// textured model, so a project's `unit = "task"` price (#721's advice)
    /// still charges its price for the two tasks together.
    fn units(&self, credits: f64) -> f64 {
        match self.calls.meter.prices().call_price("meshy") {
            Some(p) if p.unit != "credit" => credits / (PREVIEW_CREDITS + REFINE_CREDITS),
            _ => credits,
        }
    }

    /// The ledger's `model` for a stage's task: the endpoint and the stage.
    fn ledger_model(&self, stage: Stage) -> String {
        format!(
            "{} {}",
            endpoint_label(&self.api_url, "meshy"),
            stage.name()
        )
    }

    /// POST a task (a paid call) and return its ID
    fn post_task(&self, payload: &serde_json::Value, stage: Stage) -> Result<String> {
        let req = self.auth(HttpRequest::post_json(&self.api_url, payload));
        let what = format!("Meshy {} request", stage.name());
        let response = self.calls.request_json(&req, &what)?;
        response
            .get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                FlintError::GenerationError(format!(
                    "Unexpected Meshy {} response: {}",
                    stage.name(),
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

    /// What an error at `stage` adds when the job is kept: what's paid for
    /// and the command that goes on from there. Before the refine is
    /// submitted only the preview is paid for, which [`Calls::resume_hint`]
    /// can't say.
    fn resume_hint(&self, job: &GenerationJob, stage: Stage) -> String {
        if self.calls.jobs.is_none() {
            return String::new();
        }
        match (stage, job.refine_id.is_some()) {
            (Stage::Refine, false) => format!(
                "; the preview is paid for: `flint asset job resume {}` refines it without paying for it again",
                job.id
            ),
            _ => self.calls.resume_hint(job),
        }
    }

    /// Keep `e` in the job and return it with the resume hint.
    fn fail(&self, job: &mut GenerationJob, stage: Stage, e: FlintError) -> FlintError {
        let hint = self.resume_hint(job, stage);
        // A generation error's own words, so the prefix isn't said twice.
        let msg = match e {
            FlintError::GenerationError(m) => m,
            other => other.to_string(),
        };
        job.error = Some(msg.clone());
        self.calls.save_job(job);
        FlintError::GenerationError(format!("{msg}{hint}"))
    }

    /// Submit the refine of `job`'s succeeded preview: a paid call, asked of
    /// the meter first and recorded the moment Meshy accepts it.
    fn submit_refine(&self, job: &mut GenerationJob, preview_id: &str) -> Result<()> {
        let units = self.units(Stage::Refine.credits());
        if let Err(e) = self.calls.meter.admit_call("meshy", units) {
            return Err(self.fail(job, Stage::Refine, cost_error(e)));
        }
        let payload = serde_json::json!({
            "mode": "refine",
            "preview_task_id": preview_id,
            "enable_pbr": true
        });
        let refine_id = match self.post_task(&payload, Stage::Refine) {
            Ok(id) => id,
            Err(e) => return Err(self.fail(job, Stage::Refine, e)),
        };
        let cost = self
            .calls
            .meter
            .record_call("meshy", &self.ledger_model(Stage::Refine), units);
        job.refine_id = Some(refine_id.clone());
        job.estimated_usd = Some(job.estimated_usd.unwrap_or(0.0) + cost);
        job.error = None;
        self.calls.save_job(job);
        tracing::info!(
            "Submitted Meshy refine {} of preview {} (job {})",
            refine_id,
            preview_id,
            job.id
        );
        Ok(())
    }

    /// Poll `task_id`, `job`'s task at `stage`, until it succeeds, keeping
    /// the job record current at every step. Returns its GLB's URL.
    fn wait(&self, job: &mut GenerationJob, task_id: &str, stage: Stage) -> Result<Option<String>> {
        for _ in 0..self.calls.max_polls {
            self.calls.sleep(self.calls.poll_interval);
            let status = match self.poll_task(task_id) {
                Ok(s) => s,
                Err(e) => return Err(self.fail(job, stage, e)),
            };
            match status {
                MeshyTaskStatus::Processing(progress) => {
                    tracing::info!("Meshy {}: {}%", stage.name(), progress);
                    job.status = JobStatus::Processing;
                    job.progress = stage.progress(progress);
                    self.calls.save_job(job);
                }
                MeshyTaskStatus::Complete { model_url } => {
                    job.status = JobStatus::Processing;
                    job.progress = stage.progress(100);
                    self.calls.save_job(job);
                    return Ok(model_url);
                }
                MeshyTaskStatus::Failed(msg) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(msg.clone());
                    self.calls.save_job(job);
                    return Err(FlintError::GenerationError(format!(
                        "Meshy {} failed: {}",
                        stage.name(),
                        msg
                    )));
                }
            }
        }
        let msg = format!(
            "Meshy {} timed out after {} poll attempts",
            stage.name(),
            self.calls.max_polls
        );
        Err(self.fail(job, stage, FlintError::GenerationError(msg)))
    }

    /// Take `job` from wherever it is to a downloaded GLB: wait for its
    /// preview and submit the refine unless that is done, then wait for the
    /// refine and download what it made.
    fn finish(
        &self,
        job: &mut GenerationJob,
        output_dir: &Path,
        start: std::time::Instant,
    ) -> Result<GenerateResult> {
        let preview_id = job
            .remote_id
            .clone()
            .ok_or_else(|| FlintError::GenerationError("Job has no remote ID".to_string()))?;
        if job.refine_id.is_none() {
            self.wait(job, &preview_id, Stage::Preview)?;
            self.submit_refine(job, &preview_id)?;
        }
        let refine_id = job
            .refine_id
            .clone()
            .expect("the refine is submitted by now");
        let url = self.wait(job, &refine_id, Stage::Refine)?.ok_or_else(|| {
            FlintError::GenerationError("No GLB URL in completion response".to_string())
        })?;
        std::fs::create_dir_all(output_dir)?;
        let output_path = output_dir.join(format!("{}.glb", job.asset_name));
        let glb = match self
            .calls
            .request(&HttpRequest::get(url), "Meshy model download")
        {
            Ok(glb) => glb,
            Err(e) => return Err(self.fail(job, Stage::Refine, e)),
        };
        std::fs::write(&output_path, &glb.body)?;

        let output = output_path.to_string_lossy().to_string();
        job.status = JobStatus::Complete;
        job.progress = 100;
        job.error = None;
        job.output_path = Some(output.clone());
        self.calls.save_job(job);

        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), refine_id);
        metadata.insert("preview_task_id".to_string(), preview_id);
        if self.calls.jobs.is_some() {
            metadata.insert("job_id".to_string(), job.id.clone());
        }
        Ok(GenerateResult {
            output_path: output,
            prompt_used: job.prompt.clone().unwrap_or_default(),
            provider: "meshy".to_string(),
            duration_secs: start.elapsed().as_secs_f64(),
            content_hash: hash,
            metadata,
        })
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
        "FAILED" | "CANCELED" | "EXPIRED" => MeshyTaskStatus::Failed(
            response
                .get("task_error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or(if status == "CANCELED" {
                    "the task was canceled"
                } else {
                    "Unknown error"
                })
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

    /// A textured model: its preview and its refine.
    fn billed_units(&self, _request: &GenerateRequest) -> f64 {
        self.units(PREVIEW_CREDITS + REFINE_CREDITS)
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let start = std::time::Instant::now();
        let prompt = self.build_prompt(request, style);
        if style.and_then(|s| s.negative()).is_some() {
            // The v2 API documents no negative prompt (#1187).
            tracing::info!("Meshy takes no negative prompt; the style guide's is not sent");
        }
        std::fs::create_dir_all(output_dir)?;

        // The whole model is asked for up front: a preview the budget
        // couldn't refine would be paid for and of no use.
        self.calls
            .meter
            .admit_call("meshy", self.billed_units(request))
            .map_err(cost_error)?;
        let payload = serde_json::json!({
            "mode": "preview",
            "prompt": prompt,
            "should_remesh": true
        });
        let preview_id = self.post_task(&payload, Stage::Preview)?;
        // Meshy accepted the preview: it's billed, and from here a job.
        let cost = self.calls.meter.record_call(
            "meshy",
            &self.ledger_model(Stage::Preview),
            self.units(Stage::Preview.credits()),
        );
        let mut job = GenerationJob::new("meshy", &request.name);
        job.remote_id = Some(preview_id.clone());
        job.prompt = Some(prompt);
        job.estimated_usd = Some(cost);
        self.calls.save_job(&job);
        tracing::info!("Submitted Meshy preview {} (job {})", preview_id, job.id);

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
#[path = "meshy_tests.rs"]
mod tests;
