//! Flux texture generation provider (fal.ai)
//!
//! Generates textures through fal's queue (#1186). A POST to
//! `queue.fal.run/fal-ai/flux/dev` answers at once with a receipt
//! (`request_id`, `status_url`, `response_url`), not an image: the request
//! is polled at `status_url` until `COMPLETED`, and the model's output (the
//! `images`) is a GET of `response_url`. A synchronous endpoint
//! (`fal.run/...`, set as the provider's `api_url`) answers with the images
//! straight away, and that is read too.
//!
//! fal bills per megapixel, each image rounded up ([`megapixels`]); the call
//! is checked against the budget before it is sent and recorded once fal
//! takes it. With a job store in its `Calls`, a queued request is a
//! [`GenerationJob`] from then on, so an image fal made after the run
//! stopped is still fetched by `flint asset job resume <id>`, paying for
//! nothing new.

use crate::calls::{cost_error, endpoint_label, Calls};
use crate::config::FlintConfig;
use crate::job::{GenerationJob, JobStatus};
use crate::provider::*;
use crate::style::StyleGuide;
use flint_ai_core::http::shorten;
use flint_ai_core::HttpRequest;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

const DEFAULT_FLUX_URL: &str = "https://queue.fal.run/fal-ai/flux/dev";

/// Seconds between a queued image's status checks, at most: an image takes
/// seconds, not Meshy's minutes.
const FLUX_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Flux provider for AI texture generation via fal.ai
pub struct FluxProvider {
    api_key: String,
    api_url: String,
    calls: Calls,
}

/// Megapixels fal bills a `width` x `height` image: the pixels over a
/// million, rounded up, at least one.
pub fn megapixels(width: u32, height: u32) -> f64 {
    (f64::from(width) * f64::from(height) / 1e6).ceil().max(1.0)
}

/// What fal answered a submit with.
#[derive(Debug, PartialEq)]
enum Answer {
    /// The model's output: a synchronous endpoint's answer.
    Output(serde_json::Value),
    /// The queue's receipt.
    Queued(Receipt),
}

/// Where a queued request is followed.
#[derive(Debug, PartialEq)]
struct Receipt {
    request_id: String,
    status_url: String,
    response_url: String,
}

/// Where a queued request stands, by its status.
#[derive(Debug, PartialEq)]
enum QueueStatus {
    /// `IN_QUEUE`, with the requests ahead of it if fal said.
    Queued(Option<u64>),
    /// `IN_PROGRESS`.
    Running,
    /// `COMPLETED` without an error: the output is at `response_url`.
    Done,
    /// `COMPLETED` with an error (fal marks a failed request complete).
    Failed(String),
}

fn answer(response: serde_json::Value) -> Option<Answer> {
    if image_url(&response).is_some() {
        return Some(Answer::Output(response));
    }
    let field = |k: &str| response.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let request_id = field("request_id")?;
    let response_url = field("response_url")?;
    let status_url = field("status_url").unwrap_or_else(|| status_url_of(&response_url));
    Some(Answer::Queued(Receipt {
        request_id,
        status_url,
        response_url,
    }))
}

/// A request's status URL from its response URL, as fal makes them
/// (`.../requests/{id}` and `.../requests/{id}/status`).
fn status_url_of(response_url: &str) -> String {
    format!("{}/status", response_url.trim_end_matches('/'))
}

fn queue_status(response: &serde_json::Value) -> QueueStatus {
    match response.get("status").and_then(|s| s.as_str()) {
        Some("COMPLETED") => match response.get("error").filter(|e| !e.is_null()) {
            None => QueueStatus::Done,
            Some(e) => {
                let msg = e
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string());
                QueueStatus::Failed(match response.get("error_type").and_then(|t| t.as_str()) {
                    Some(t) => format!("{msg} ({t})"),
                    None => msg,
                })
            }
        },
        Some("IN_PROGRESS") => QueueStatus::Running,
        // IN_QUEUE, or anything fal adds later: keep waiting.
        _ => QueueStatus::Queued(response.get("queue_position").and_then(|p| p.as_u64())),
    }
}

impl FluxProvider {
    /// Create a new FluxProvider from config, its calls through `calls`
    pub fn from_config(config: &FlintConfig, calls: Calls) -> Result<Self> {
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

        Ok(Self {
            api_key,
            api_url,
            calls,
        })
    }

    fn auth(&self, req: HttpRequest) -> HttpRequest {
        req.with_header("Authorization", format!("Key {}", self.api_key))
    }

    /// GET `url` with the key, as JSON.
    fn get_json(&self, url: &str, what: &str) -> Result<serde_json::Value> {
        let response = self
            .calls
            .request_json(&self.auth(HttpRequest::get(url)), what)?;
        tracing::debug!("{what}: {response}");
        Ok(response)
    }

    /// The paid request: one image of `width` x `height`.
    fn submit(
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
        let req = self.auth(HttpRequest::post_json(&self.api_url, &payload));
        let response = self.calls.request_json(&req, "Flux API request")?;
        tracing::debug!("Flux API request: {response}");
        Ok(response)
    }

    /// Poll `job`'s queued request until fal has made the image, then fetch
    /// the output and the image, keeping the job record current. The first
    /// check is at once: a warm fal answers in a few seconds.
    fn finish(
        &self,
        job: &mut GenerationJob,
        status_url: Option<String>,
        output_dir: &Path,
        start: Instant,
    ) -> Result<GenerateResult> {
        let response_url = job.remote_url.clone().ok_or_else(|| {
            FlintError::GenerationError(format!("Flux job {} has no response URL", job.id))
        })?;
        let status_url = status_url.unwrap_or_else(|| status_url_of(&response_url));
        let hint = self.calls.resume_hint(job);
        let fail = |job: &mut GenerationJob, msg: String| {
            job.error = Some(msg.clone());
            self.calls.save_job(job);
            FlintError::GenerationError(format!("{msg}{hint}"))
        };
        let interval = FLUX_POLL_INTERVAL.min(self.calls.poll_interval);
        for n in 0..self.calls.max_polls {
            if n > 0 {
                self.calls.sleep(interval);
            }
            let status = match self.get_json(&status_url, "Flux status check") {
                Ok(s) => queue_status(&s),
                Err(e) => return Err(fail(job, e.to_string())),
            };
            match status {
                QueueStatus::Queued(ahead) => {
                    if let Some(ahead) = ahead {
                        tracing::info!("In fal's queue, {ahead} ahead");
                    }
                }
                QueueStatus::Running => {
                    if job.status != JobStatus::Processing {
                        job.status = JobStatus::Processing;
                        self.calls.save_job(job);
                    }
                }
                QueueStatus::Failed(msg) => {
                    // Nothing to resume: fal made no image.
                    job.status = JobStatus::Failed;
                    job.error = Some(msg.clone());
                    self.calls.save_job(job);
                    return Err(FlintError::GenerationError(format!(
                        "Flux generation failed: {msg}"
                    )));
                }
                QueueStatus::Done => {
                    let output = match self.get_json(&response_url, "Flux result") {
                        Ok(o) => o,
                        Err(e) => return Err(fail(job, e.to_string())),
                    };
                    let result = self.download(&job.asset_name, &output, output_dir);
                    return match result {
                        Ok(mut result) => {
                            job.status = JobStatus::Complete;
                            job.progress = 100;
                            job.error = None;
                            job.output_path = Some(result.output_path.clone());
                            self.calls.save_job(job);
                            result.prompt_used = job.prompt.clone().unwrap_or_default();
                            result.duration_secs = start.elapsed().as_secs_f64();
                            if let Some(id) = &job.remote_id {
                                result.metadata.insert("request_id".into(), id.clone());
                            }
                            if self.calls.jobs.is_some() {
                                result.metadata.insert("job_id".into(), job.id.clone());
                            }
                            Ok(result)
                        }
                        Err(e) => Err(fail(job, e.to_string())),
                    };
                }
            }
        }
        Err(fail(
            job,
            format!(
                "Flux generation timed out after {} status checks",
                self.calls.max_polls
            ),
        ))
    }

    /// Download the image a model output names to `<name>.png` under
    /// `output_dir`. The result's prompt and duration are left for the
    /// caller.
    fn download(
        &self,
        name: &str,
        output: &serde_json::Value,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let image_url = image_url(output).ok_or_else(|| {
            FlintError::GenerationError(format!(
                "Unexpected Flux response format: {}",
                shorten(&output.to_string(), 400)
            ))
        })?;
        std::fs::create_dir_all(output_dir)?;
        let output_path = output_dir.join(format!("{name}.png"));
        let image = self
            .calls
            .request(&HttpRequest::get(image_url), "Flux image download")?;
        std::fs::write(&output_path, &image.body)?;

        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();
        let mut metadata = HashMap::new();
        if let Some(seed) = output.get("seed").and_then(|s| s.as_u64()) {
            metadata.insert("seed".to_string(), seed.to_string());
        }
        Ok(GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: String::new(),
            provider: "flux".to_string(),
            duration_secs: 0.0,
            content_hash: hash,
            metadata,
        })
    }
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

    fn billed_units(&self, request: &GenerateRequest) -> f64 {
        let p = request.texture_params.clone().unwrap_or_default();
        megapixels(p.width, p.height)
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let start = Instant::now();
        let prompt = self.build_prompt(request, style);
        let params = request.texture_params.clone().unwrap_or_default();
        std::fs::create_dir_all(output_dir)?;

        let units = self.billed_units(request);
        self.calls
            .meter
            .admit_call("flux", units)
            .map_err(cost_error)?;
        let response = self.submit(&prompt, params.width, params.height, params.seed)?;
        // fal took the request: it's billed, whatever happens next.
        let cost =
            self.calls
                .meter
                .record_call("flux", &endpoint_label(&self.api_url, "flux"), units);

        let unexpected = |r: &serde_json::Value| {
            FlintError::GenerationError(format!(
                "Unexpected Flux response format: {}",
                shorten(&r.to_string(), 400)
            ))
        };
        match answer(response.clone()).ok_or_else(|| unexpected(&response))? {
            Answer::Output(output) => {
                let mut result = self.download(&request.name, &output, output_dir)?;
                result.prompt_used = prompt;
                result.duration_secs = start.elapsed().as_secs_f64();
                Ok(result)
            }
            Answer::Queued(receipt) => {
                // Queued and paid for: from here a job.
                let mut job = GenerationJob::new("flux", &request.name);
                job.remote_id = Some(receipt.request_id.clone());
                job.remote_url = Some(receipt.response_url);
                job.prompt = Some(prompt);
                job.estimated_usd = Some(cost);
                self.calls.save_job(&job);
                tracing::info!(
                    "Queued Flux request {} (job {})",
                    receipt.request_id,
                    job.id
                );
                self.finish(&mut job, Some(receipt.status_url), output_dir, start)
            }
        }
    }

    fn resume(&self, job: &GenerationJob, output_dir: &Path) -> Result<GenerateResult> {
        if job.provider != "flux" {
            return Err(FlintError::GenerationError(format!(
                "job {} is {}'s, not Flux's",
                job.id, job.provider
            )));
        }
        let mut job = job.clone();
        self.finish(&mut job, None, output_dir, Instant::now())
    }

    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String {
        match style {
            Some(s) => s.enrich_prompt(&request.description),
            None => request.description.clone(),
        }
    }
}

/// The first image's URL in a Flux model output.
fn image_url(response: &serde_json::Value) -> Option<&str> {
    response
        .get("images")
        .and_then(|imgs| imgs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
}

/// Parse a Flux model output for testing
pub fn parse_flux_response(json: &str) -> Result<String> {
    let response: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| FlintError::GenerationError(format!("Invalid JSON: {}", e)))?;
    image_url(&response)
        .map(str::to_string)
        .ok_or_else(|| FlintError::GenerationError("No image URL in response".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::JobStore;
    use crate::providers::test_support::{calls_over, request, temp_dir, test_config};
    use flint_ai_core::{HttpResponse, Method, ScriptedTransport};
    use serde_json::json;

    // fal's answers, in the shapes its queue docs give
    // (fal.ai/docs/model-apis/model-endpoints/queue, read 2026-11-21). The
    // status and response URLs name the app (`fal-ai/flux`), not the
    // endpoint (`fal-ai/flux/dev`), as fal's do.
    const BASE: &str = "http://fal.test/fal-ai/flux";
    const ID: &str = "019a1b2c-3d4e-7f80-9a1b-2c3d4e5f6a7b";

    fn receipt() -> serde_json::Value {
        json!({
            "status": "IN_QUEUE",
            "request_id": ID,
            "response_url": format!("{BASE}/requests/{ID}"),
            "status_url": format!("{BASE}/requests/{ID}/status"),
            "cancel_url": format!("{BASE}/requests/{ID}/cancel"),
            "queue_position": 0,
            "logs": null,
            "metrics": {}
        })
    }

    fn in_queue(ahead: u64) -> serde_json::Value {
        json!({"status": "IN_QUEUE", "request_id": ID, "queue_position": ahead,
               "response_url": format!("{BASE}/requests/{ID}")})
    }

    fn in_progress() -> serde_json::Value {
        json!({"status": "IN_PROGRESS", "request_id": ID, "logs": [],
               "response_url": format!("{BASE}/requests/{ID}")})
    }

    fn completed() -> serde_json::Value {
        json!({"status": "COMPLETED", "request_id": ID, "logs": [],
               "metrics": {"inference_time": 3.42},
               "response_url": format!("{BASE}/requests/{ID}")})
    }

    /// flux/dev's output schema: `images`, `timings`, `seed`,
    /// `has_nsfw_concepts`, `prompt`.
    fn output() -> serde_json::Value {
        json!({
            "images": [{"url": "http://fal.test/files/img.png", "width": 1024,
                        "height": 1024, "content_type": "image/png"}],
            "timings": {"inference": 3.1},
            "seed": 7,
            "has_nsfw_concepts": [false],
            "prompt": "a brick"
        })
    }

    fn flux_over(script: &ScriptedTransport, jobs: Option<&Path>) -> (FluxProvider, Calls) {
        let (mut calls, _) = calls_over(script, None);
        if let Some(dir) = jobs {
            calls = calls.with_jobs(JobStore::new(dir));
        }
        let flux = FluxProvider::from_config(
            &test_config("flux", "http://fal.test/fal-ai/flux/dev"),
            calls.clone(),
        )
        .unwrap();
        (flux, calls)
    }

    #[test]
    fn test_parse_flux_response() {
        let url = parse_flux_response(&output().to_string()).unwrap();
        assert_eq!(url, "http://fal.test/files/img.png");
    }

    #[test]
    fn test_parse_flux_response_invalid() {
        let json = r#"{"error": "something went wrong"}"#;
        assert!(parse_flux_response(json).is_err());
    }

    #[test]
    fn megapixels_round_up() {
        assert_eq!(megapixels(1000, 1000), 1.0);
        assert_eq!(megapixels(1024, 1024), 2.0);
        assert_eq!(megapixels(512, 512), 1.0);
        assert_eq!(megapixels(2048, 2048), 5.0);
    }

    #[test]
    fn a_receipt_is_followed_and_an_output_is_read() {
        assert_eq!(
            answer(receipt()),
            Some(Answer::Queued(Receipt {
                request_id: ID.into(),
                status_url: format!("{BASE}/requests/{ID}/status"),
                response_url: format!("{BASE}/requests/{ID}"),
            }))
        );
        // A receipt without a status URL: fal's pattern.
        let mut r = receipt();
        r.as_object_mut().unwrap().remove("status_url");
        let Some(Answer::Queued(r)) = answer(r) else {
            panic!("a receipt")
        };
        assert_eq!(r.status_url, format!("{BASE}/requests/{ID}/status"));
        assert_eq!(answer(output()), Some(Answer::Output(output())));
        assert_eq!(answer(json!({"detail": "Not authenticated"})), None);
    }

    #[test]
    fn statuses_are_read() {
        assert_eq!(queue_status(&in_queue(2)), QueueStatus::Queued(Some(2)));
        assert_eq!(queue_status(&in_progress()), QueueStatus::Running);
        assert_eq!(queue_status(&completed()), QueueStatus::Done);
        let mut failed = completed();
        failed["error"] = json!("CUDA out of memory");
        failed["error_type"] = json!("runner_error");
        assert_eq!(
            queue_status(&failed),
            QueueStatus::Failed("CUDA out of memory (runner_error)".into())
        );
        failed["error"] = serde_json::Value::Null;
        assert_eq!(queue_status(&failed), QueueStatus::Done);
    }

    #[test]
    fn a_queued_generate_is_polled_fetched_and_downloaded() {
        let script = ScriptedTransport::new(vec![
            // A 503, retried, then the queue's receipt.
            Ok(HttpResponse::text_body(503, "busy")),
            ScriptedTransport::json(receipt()),
            ScriptedTransport::json(in_queue(1)),
            ScriptedTransport::json(in_progress()),
            ScriptedTransport::json(completed()),
            ScriptedTransport::json(output()),
            Ok(HttpResponse::bytes(b"\x89PNG fake".to_vec())),
        ]);
        let (calls, meter) = calls_over(&script, None);
        let jobs = temp_dir("flux-jobs");
        let flux = FluxProvider::from_config(
            &test_config("flux", "http://fal.test/fal-ai/flux/dev"),
            calls.with_jobs(JobStore::new(&jobs)),
        )
        .unwrap();
        let dir = temp_dir("flux");
        let result = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap();
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"\x89PNG fake");
        assert_eq!(result.metadata["seed"], "7");
        assert_eq!(result.metadata["request_id"], ID);
        assert_eq!(result.prompt_used, "a brick");

        let sent = script.requests();
        let lines: Vec<_> = sent
            .iter()
            .map(|r| format!("{:?} {}", r.method, r.url))
            .collect();
        assert_eq!(
            lines,
            [
                "Post http://fal.test/fal-ai/flux/dev".to_string(),
                "Post http://fal.test/fal-ai/flux/dev".to_string(),
                format!("Get {BASE}/requests/{ID}/status"),
                format!("Get {BASE}/requests/{ID}/status"),
                format!("Get {BASE}/requests/{ID}/status"),
                format!("Get {BASE}/requests/{ID}"),
                "Get http://fal.test/files/img.png".to_string(),
            ]
        );
        for r in &sent[..6] {
            assert_eq!(r.header("authorization"), Some("Key test-key"), "{}", r.url);
        }
        let body: serde_json::Value = serde_json::from_str(&sent[1].body).unwrap();
        assert_eq!(body["image_size"]["width"], 1024);

        // One call, 2 megapixels at $0.025, and a complete job.
        let entries = meter.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider, "flux");
        assert_eq!(entries[0].model, "fal-ai/flux/dev");
        assert_eq!(entries[0].units, Some(2.0));
        assert!((entries[0].estimated_usd - 0.05).abs() < 1e-12);
        let job = JobStore::new(&jobs)
            .load(&result.metadata["job_id"])
            .unwrap();
        assert_eq!(job.status, JobStatus::Complete);
        assert_eq!(job.remote_id.as_deref(), Some(ID));
        assert_eq!(job.output_path.as_deref(), Some(&*result.output_path));
        assert_eq!(job.estimated_usd, Some(0.05));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&jobs).ok();
    }

    #[test]
    fn a_synchronous_answer_is_downloaded_at_once() {
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(output()),
            Ok(HttpResponse::bytes(b"\x89PNG fake".to_vec())),
        ]);
        let (calls, meter) = calls_over(&script, None);
        let flux =
            FluxProvider::from_config(&test_config("flux", "http://fal.test/x"), calls).unwrap();
        let dir = temp_dir("flux-sync");
        let result = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap();
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"\x89PNG fake");
        assert_eq!(script.requests()[1].method, Method::Get);
        assert_eq!(meter.calls(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_call_past_the_ceiling_is_never_sent() {
        let script = ScriptedTransport::new(vec![]);
        let (calls, meter) = calls_over(&script, Some(0.04));
        let flux =
            FluxProvider::from_config(&test_config("flux", "http://fal.test/x"), calls).unwrap();
        let dir = temp_dir("flux-ceiling");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(e.to_string().contains("cost ceiling reached"), "{e}");
        assert!(script.requests().is_empty());
        assert_eq!(meter.calls(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_answer_is_an_error_and_still_recorded() {
        let script = ScriptedTransport::new(vec![ScriptedTransport::json(json!({"queued": true}))]);
        let (calls, meter) = calls_over(&script, None);
        let flux =
            FluxProvider::from_config(&test_config("flux", "http://fal.test/x"), calls).unwrap();
        let dir = temp_dir("flux-odd");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(
            e.to_string().contains("Unexpected Flux response format"),
            "{e}"
        );
        assert_eq!(meter.calls(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_request_fails_its_job() {
        let mut failed = completed();
        failed["error"] = json!("NSFW content detected");
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(receipt()),
            ScriptedTransport::json(failed),
        ]);
        let jobs = temp_dir("flux-failed-jobs");
        let (flux, _) = flux_over(&script, Some(&jobs));
        let dir = temp_dir("flux-failed");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(
            e.to_string().contains("Flux generation failed: NSFW"),
            "{e}"
        );
        let listed = JobStore::new(&jobs).list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, JobStatus::Failed);
        assert_eq!(listed[0].error.as_deref(), Some("NSFW content detected"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&jobs).ok();
    }

    #[test]
    fn a_billed_request_whose_download_fails_resumes_without_paying_again() {
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(receipt()),
            ScriptedTransport::json(completed()),
            ScriptedTransport::json(output()),
            Ok(HttpResponse::text_body(404, "gone")),
        ]);
        let jobs = temp_dir("flux-resume-jobs");
        let (flux, calls) = flux_over(&script, Some(&jobs));
        let dir = temp_dir("flux-resume");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(e.to_string().contains("Flux image download failed"), "{e}");
        assert!(e.to_string().contains("flint asset job resume"), "{e}");
        assert_eq!(calls.meter.calls(), 1);

        // The job knows where fal keeps the result: resume asks the status
        // there (no receipt now) and fetches it, with no new POST.
        let job = JobStore::new(&jobs).list().unwrap().remove(0);
        assert_eq!(job.remote_url, Some(format!("{BASE}/requests/{ID}")));
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(completed()),
            ScriptedTransport::json(output()),
            Ok(HttpResponse::bytes(b"\x89PNG later".to_vec())),
        ]);
        let (flux, calls) = flux_over(&script, Some(&jobs));
        let result = flux.resume(&job, &dir).unwrap();
        assert_eq!(
            std::fs::read(&result.output_path).unwrap(),
            b"\x89PNG later"
        );
        assert_eq!(
            script.requests()[0].url,
            format!("{BASE}/requests/{ID}/status")
        );
        assert!(script.requests().iter().all(|r| r.method == Method::Get));
        assert_eq!(calls.meter.calls(), 0);
        let job = JobStore::new(&jobs).load(&job.id).unwrap();
        assert_eq!(job.status, JobStatus::Complete);
        assert_eq!(job.error, None);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&jobs).ok();
    }

    #[test]
    fn a_wait_that_runs_out_leaves_the_job() {
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(receipt()),
            ScriptedTransport::json(in_queue(3)),
            ScriptedTransport::json(in_progress()),
        ]);
        let jobs = temp_dir("flux-timeout-jobs");
        let (calls, _) = calls_over(&script, None);
        let flux = FluxProvider::from_config(
            &test_config("flux", "http://fal.test/fal-ai/flux/dev"),
            calls
                .with_jobs(JobStore::new(&jobs))
                .with_polling(Duration::from_secs(10), 2),
        )
        .unwrap();
        let dir = temp_dir("flux-timeout");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(e.to_string().contains("timed out after 2"), "{e}");
        let job = JobStore::new(&jobs).list().unwrap().remove(0);
        assert_eq!(job.status, JobStatus::Processing);
        assert!(e.to_string().contains(&job.id), "{e}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&jobs).ok();
    }

    #[test]
    fn another_providers_job_is_refused() {
        let script = ScriptedTransport::new(vec![]);
        let (flux, _) = flux_over(&script, None);
        let job = GenerationJob::new("meshy", "chair");
        let e = flux.resume(&job, Path::new(".")).unwrap_err();
        assert!(e.to_string().contains("not Flux's"), "{e}");
    }
}
