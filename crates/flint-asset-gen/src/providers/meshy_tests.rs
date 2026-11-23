//! Meshy's tests: the parsers, and a model as a preview, then a refine of
//! it, over a scripted transport shaped as the v2 API's docs say (#1187).

use super::*;
use crate::job::JobStore;
use crate::providers::test_support::{calls_over, request, temp_dir, test_config};
use flint_ai_core::cost::PriceTable;
use flint_ai_core::{CostMeter, HttpResponse, Method, RetryPolicy, ScriptedTransport};
use std::sync::Arc;

const URL: &str = "http://meshy.test/openapi/v2/text-to-3d";

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

#[test]
fn a_canceled_task_is_a_failure() {
    assert_eq!(
        task_status(&serde_json::json!({"status": "CANCELED", "task_error": null})),
        MeshyTaskStatus::Failed("the task was canceled".into())
    );
}

fn json(v: serde_json::Value) -> std::result::Result<HttpResponse, String> {
    ScriptedTransport::json(v)
}

fn body(req: &HttpRequest) -> serde_json::Value {
    serde_json::from_str(&req.body).unwrap()
}

fn succeeded(glb: &str) -> std::result::Result<HttpResponse, String> {
    json(serde_json::json!({"status": "SUCCEEDED", "progress": 100,
        "model_urls": {"glb": glb}}))
}

fn meshy(calls: Calls) -> MeshyProvider {
    MeshyProvider::from_config(&test_config("meshy", URL), calls).unwrap()
}

#[test]
fn a_generate_is_a_preview_then_a_refine_of_it() {
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "preview-1"})),
        json(serde_json::json!({"status": "IN_PROGRESS", "progress": 40})),
        // A preview's GLB is the bare mesh: not what's downloaded.
        succeeded("http://meshy.test/preview.glb"),
        json(serde_json::json!({"result": "refine-1"})),
        json(serde_json::json!({"status": "IN_PROGRESS", "progress": 50})),
        succeeded("http://meshy.test/refined.glb"),
        Ok(HttpResponse::bytes(b"glTF textured".to_vec())),
    ]);
    let dir = temp_dir("meshy");
    let store = JobStore::new(dir.join("jobs"));
    let (calls, meter) = calls_over(&script, None);
    let result = meshy(calls.with_jobs(store.clone()))
        .generate(&request("chair", AssetKind::Model), None, &dir.join("out"))
        .unwrap();
    assert_eq!(
        std::fs::read(&result.output_path).unwrap(),
        b"glTF textured"
    );

    let sent = script.requests();
    let urls: Vec<_> = sent
        .iter()
        .map(|r| (r.method, r.url.trim_start_matches(URL)))
        .collect();
    assert_eq!(
        urls,
        vec![
            (Method::Post, ""),
            (Method::Get, "/preview-1"),
            (Method::Get, "/preview-1"),
            (Method::Post, ""),
            (Method::Get, "/refine-1"),
            (Method::Get, "/refine-1"),
            (Method::Get, "http://meshy.test/refined.glb"),
        ]
    );
    assert_eq!(sent[0].header("authorization"), Some("Bearer test-key"));
    assert_eq!(sent[3].header("authorization"), Some("Bearer test-key"));
    // The prompt goes with the preview; the refine names the preview.
    let preview = body(&sent[0]);
    assert_eq!(preview["mode"], "preview");
    assert_eq!(preview["prompt"], "a chair");
    assert!(preview.get("preview_task_id").is_none());
    let refine = body(&sent[3]);
    assert_eq!(refine["mode"], "refine");
    assert_eq!(refine["preview_task_id"], "preview-1");
    assert!(refine.get("prompt").is_none(), "{refine}");

    let jobs = store.list().unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.remote_id.as_deref(), Some("preview-1"));
    assert_eq!(job.refine_id.as_deref(), Some("refine-1"));
    assert_eq!(job.status, JobStatus::Complete);
    assert_eq!(job.progress, 100);
    assert_eq!(
        job.output_path.as_deref(),
        Some(result.output_path.as_str())
    );
    assert_eq!(result.metadata["job_id"], job.id);
    assert_eq!(result.metadata["task_id"], "refine-1");
    assert_eq!(result.metadata["preview_task_id"], "preview-1");

    // Two billed requests, 20 + 10 credits at $0.02: the ledger has each,
    // and the job their sum.
    let entries = meter.entries();
    let billed: Vec<_> = entries
        .iter()
        .map(|e| (e.model.as_str(), e.units, e.unit.as_deref()))
        .collect();
    assert_eq!(
        billed,
        vec![
            ("openapi/v2/text-to-3d preview", Some(20.0), Some("credit")),
            ("openapi/v2/text-to-3d refine", Some(10.0), Some("credit")),
        ]
    );
    assert!((entries[0].estimated_usd - 0.40).abs() < 1e-12);
    assert!((entries[1].estimated_usd - 0.20).abs() < 1e-12);
    assert!((job.estimated_usd.unwrap() - 0.60).abs() < 1e-12);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_run_stopped_in_the_preview_resumes_without_paying_for_it_again() {
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "preview-2"})),
        json(serde_json::json!({"status": "IN_PROGRESS", "progress": 10})),
        json(serde_json::json!({"status": "IN_PROGRESS", "progress": 60})),
    ]);
    let dir = temp_dir("meshy-resume-preview");
    let store = JobStore::new(dir.join("jobs"));
    let (calls, meter) = calls_over(&script, None);
    let calls = calls
        .with_jobs(store.clone())
        .with_polling(std::time::Duration::ZERO, 2);
    let e = meshy(calls)
        .generate(&request("table", AssetKind::Model), None, &dir.join("out"))
        .unwrap_err();
    let job = store.list().unwrap().remove(0);
    assert!(e.to_string().contains("Meshy preview timed out"), "{e}");
    assert!(
        e.to_string()
            .contains(&format!("flint asset job resume {}", job.id)),
        "{e}"
    );
    assert_eq!(job.status, JobStatus::Processing);
    assert_eq!(job.progress, 30, "the preview is the first half");
    assert_eq!(job.refine_id, None);
    assert_eq!(meter.calls(), 1);

    // A later run waits for the same preview, then pays for the refine alone.
    let script = ScriptedTransport::new(vec![
        succeeded("http://meshy.test/preview.glb"),
        json(serde_json::json!({"result": "refine-2"})),
        succeeded("http://meshy.test/t.glb"),
        Ok(HttpResponse::bytes(b"glTF".to_vec())),
    ]);
    let (calls, meter2) = calls_over(&script, None);
    let result = meshy(calls.with_jobs(store.clone()))
        .resume(&job, &dir.join("out"))
        .unwrap();
    assert!(result.output_path.ends_with("table.glb"));
    let sent = script.requests();
    assert_eq!(sent[0].url, format!("{URL}/preview-2"));
    let posts: Vec<_> = sent.iter().filter(|r| r.method == Method::Post).collect();
    assert_eq!(posts.len(), 1, "one submit, the refine");
    assert_eq!(body(posts[0])["preview_task_id"], "preview-2");
    let entries = meter2.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model, "openapi/v2/text-to-3d refine");

    let done = store.load(&job.id).unwrap();
    assert_eq!(done.status, JobStatus::Complete);
    assert_eq!(done.refine_id.as_deref(), Some("refine-2"));
    assert!((done.estimated_usd.unwrap() - 0.60).abs() < 1e-12);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_refused_refine_keeps_the_paid_preview_for_a_resume() {
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "preview-3"})),
        succeeded("http://meshy.test/preview.glb"),
        Ok(HttpResponse::text_body(
            402,
            r#"{"message":"Insufficient credits"}"#,
        )),
    ]);
    let dir = temp_dir("meshy-refine-refused");
    let store = JobStore::new(dir.join("jobs"));
    let (calls, meter) = calls_over(&script, None);
    let e = meshy(calls.with_jobs(store.clone()))
        .generate(&request("lamp", AssetKind::Model), None, &dir.join("out"))
        .unwrap_err();
    let job = store.list().unwrap().remove(0);
    let text = e.to_string();
    assert!(
        text.starts_with("Generation error: Meshy refine request failed: HTTP 402"),
        "{text}"
    );
    assert_eq!(text.matches("Generation error").count(), 1, "{text}");
    assert!(text.contains("Insufficient credits"), "{text}");
    assert!(
        text.contains(&format!(
            "the preview is paid for: `flint asset job resume {}` refines it",
            job.id
        )),
        "{text}"
    );
    assert_eq!(job.refine_id, None);
    assert_eq!(job.progress, 50);
    assert!(job
        .error
        .as_deref()
        .unwrap()
        .contains("Insufficient credits"));
    // The preview was paid for and is in the ledger; the refine was not.
    assert_eq!(meter.calls(), 1);
    assert!((job.estimated_usd.unwrap() - 0.40).abs() < 1e-12);

    let script = ScriptedTransport::new(vec![
        succeeded("http://meshy.test/preview.glb"),
        json(serde_json::json!({"result": "refine-3"})),
        succeeded("http://meshy.test/lamp.glb"),
        Ok(HttpResponse::bytes(b"glTF lamp".to_vec())),
    ]);
    let (calls, meter2) = calls_over(&script, None);
    meshy(calls.with_jobs(store.clone()))
        .resume(&job, &dir.join("out"))
        .unwrap();
    assert_eq!(meter2.calls(), 1, "the refine alone");
    let done = store.load(&job.id).unwrap();
    assert_eq!(done.status, JobStatus::Complete);
    assert_eq!(done.error, None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_run_stopped_in_the_refine_resumes_paying_for_nothing() {
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "preview-4"})),
        succeeded("http://meshy.test/preview.glb"),
        json(serde_json::json!({"result": "refine-4"})),
        json(serde_json::json!({"status": "IN_PROGRESS", "progress": 20})),
    ]);
    let dir = temp_dir("meshy-resume-refine");
    let store = JobStore::new(dir.join("jobs"));
    let (calls, meter) = calls_over(&script, None);
    let calls = calls
        .with_jobs(store.clone())
        .with_polling(std::time::Duration::ZERO, 1);
    let e = meshy(calls)
        .generate(&request("stool", AssetKind::Model), None, &dir.join("out"))
        .unwrap_err();
    let job = store.list().unwrap().remove(0);
    assert!(e.to_string().contains("Meshy refine timed out"), "{e}");
    assert!(e.to_string().contains("it is paid for"), "{e}");
    assert_eq!(job.refine_id.as_deref(), Some("refine-4"));
    assert_eq!(job.progress, 60, "the refine is the second half");
    assert_eq!(meter.calls(), 2);

    let script = ScriptedTransport::new(vec![
        succeeded("http://meshy.test/stool.glb"),
        Ok(HttpResponse::bytes(b"glTF".to_vec())),
    ]);
    let (calls, meter2) = calls_over(&script, None);
    meshy(calls.with_jobs(store.clone()))
        .resume(&job, &dir.join("out"))
        .unwrap();
    assert_eq!(store.load(&job.id).unwrap().status, JobStatus::Complete);
    assert_eq!(meter2.calls(), 0, "a resume of a refine pays for nothing");
    let sent = script.requests();
    assert_eq!(sent[0].url, format!("{URL}/refine-4"));
    assert!(sent.iter().all(|r| r.method == Method::Get));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_preview_marks_its_job_failed_and_is_never_refined() {
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "preview-5"})),
        json(serde_json::json!({"status": "FAILED", "task_error": {"message": "policy"}})),
    ]);
    let dir = temp_dir("meshy-fail");
    let store = JobStore::new(dir.join("jobs"));
    let (calls, meter) = calls_over(&script, None);
    let e = meshy(calls.with_jobs(store.clone()))
        .generate(&request("x", AssetKind::Model), None, &dir)
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Generation error: Meshy preview failed: policy"
    );
    let job = store.list().unwrap().remove(0);
    assert_eq!(job.status, JobStatus::Failed);
    assert_eq!(job.error.as_deref(), Some("policy"));
    assert_eq!(script.requests().len(), 2);
    assert_eq!(meter.calls(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_ceiling_under_the_whole_model_refuses_before_the_preview() {
    let script = ScriptedTransport::default();
    let (calls, meter) = calls_over(&script, Some(0.50));
    let dir = temp_dir("meshy-ceiling");
    let e = meshy(calls)
        .generate(&request("x", AssetKind::Model), None, &dir)
        .unwrap_err();
    assert!(e.to_string().contains("cost ceiling reached"), "{e}");
    assert!(script.requests().is_empty());
    assert_eq!(meter.calls(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_projects_task_price_is_split_across_the_two_requests() {
    // #721 told projects to price Meshy as `unit = "task"`: a whole model.
    let prices = PriceTable::builtin()
        .overlaid("[calls.meshy]\nunit = \"task\"\nusd = 0.45\n", None)
        .unwrap();
    let meter = Arc::new(
        CostMeter::new("asset-gen")
            .with_notify(|_| {})
            .with_prices(Arc::new(prices)),
    );
    let script = ScriptedTransport::new(vec![
        json(serde_json::json!({"result": "p"})),
        succeeded("http://meshy.test/p.glb"),
        json(serde_json::json!({"result": "r"})),
        succeeded("http://meshy.test/r.glb"),
        Ok(HttpResponse::bytes(b"glTF".to_vec())),
    ]);
    let calls = Calls::new(meter.clone())
        .with_transport(script.clone())
        .with_retry(RetryPolicy::default().without_sleep());
    let meshy = meshy(calls);
    let req = request("crate", AssetKind::Model);
    assert_eq!(meshy.billed_units(&req), 1.0);
    let dir = temp_dir("meshy-task-price");
    meshy.generate(&req, None, &dir).unwrap();
    let usd: Vec<_> = meter.entries().iter().map(|e| e.estimated_usd).collect();
    assert_eq!(usd.len(), 2);
    assert!((usd[0] - 0.30).abs() < 1e-12, "{usd:?}");
    assert!((usd[1] - 0.15).abs() < 1e-12, "{usd:?}");
    std::fs::remove_dir_all(&dir).ok();
}
