//! #721's done-when, through the binary and real HTTP: a local server
//! stands in for fal.ai and Meshy, and the project's `.flint/config.toml`
//! points the providers at it.
//!
//! - `asset resolve --max-cost` refuses the call that would pass it, before
//!   it is sent (the server never sees it);
//! - every call that was sent is in the ledger, and `asset costs` adds it up;
//! - a Meshy generate is a job `asset job list` shows.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use serde_json::Value;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// What the fake providers were asked: `POST /fal-ai/flux/dev`, ...
type Log = Arc<Mutex<Vec<String>>>;

/// A server answering as fal.ai (Flux) and Meshy do, forever, on a port of
/// its own. Returns its address and the log of requests.
fn fake_providers() -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let (log2, base2) = (log.clone(), base.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, path) = (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            );
            let mut length = 0;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            log2.lock().unwrap().push(format!("{method} {path}"));
            let (kind, bytes): (&str, Vec<u8>) = match (method.as_str(), path.as_str()) {
                ("POST", "/fal-ai/flux/dev") => (
                    "application/json",
                    serde_json::json!({"images": [{"url": format!("{base2}/img.png")}], "seed": 1})
                        .to_string()
                        .into_bytes(),
                ),
                ("GET", "/img.png") => ("image/png", b"\x89PNG\r\n\x1a\nfake".to_vec()),
                ("POST", "/meshy") => ("application/json", br#"{"result":"task-7"}"#.to_vec()),
                ("GET", "/meshy/task-7") => (
                    "application/json",
                    serde_json::json!({"status": "SUCCEEDED", "progress": 100,
                        "model_urls": {"glb": format!("{base2}/m.glb")}})
                    .to_string()
                    .into_bytes(),
                ),
                ("GET", "/m.glb") => ("model/gltf-binary", b"glTF fake".to_vec()),
                _ => ("text/plain", b"not found".to_vec()),
            };
            let status = if kind == "text/plain" {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&bytes);
        }
    });
    (base, log)
}

/// A project whose providers are the fake ones, with a scene of three
/// textured walls.
fn project(name: &str, base: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("asset_cost_cli_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".flint")).unwrap();
    // The engine's schemas: they are what says `material.texture` names an
    // asset (#968)
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .canonicalize()
        .unwrap();
    let schemas = schemas.to_string_lossy().replace('\\', "/");
    let schemas = schemas.trim_start_matches("//?/");
    std::fs::write(
        dir.join("flint.toml"),
        format!("[project]\nid = \"budget\"\nschemas = [\"{schemas}\"]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join(".flint/config.toml"),
        format!(
            "[providers.flux]\napi_key = \"k\"\napi_url = \"{base}/fal-ai/flux/dev\"\n\n\
             [providers.meshy]\napi_key = \"k\"\napi_url = \"{base}/meshy\"\n"
        ),
    )
    .unwrap();
    let mut scene = String::from("[scene]\nname = \"walls\"\n");
    for wall in ["wall_a", "wall_b", "wall_c"] {
        scene.push_str(&format!(
            "\n[entities.{wall}]\n\n[entities.{wall}.material]\ntexture = \"{wall}_tex\"\n"
        ));
    }
    std::fs::write(dir.join("walls.scene.toml"), scene).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> (Output, Value) {
    let out = Command::new(FLINT)
        .args(args)
        .arg("--json")
        .current_dir(dir)
        // The project's config, not the machine's
        .env_remove("FLINT_FLUX_API_KEY")
        .env_remove("FLINT_MESHY_API_KEY")
        .env_remove("FLINT_ELEVENLABS_API_KEY")
        .output()
        .expect("flint runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "`flint {}`: not one JSON document ({e})\n{stdout}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}

#[test]
fn resolve_refuses_the_call_that_would_pass_max_cost_and_records_the_rest() {
    let (base, log) = fake_providers();
    let dir = project("resolve", &base);
    // Each 1024 x 1024 texture is 2 megapixels at $0.025: $0.05.
    let (out, report) = flint(
        &dir,
        &[
            "asset",
            "resolve",
            "walls.scene.toml",
            "--strategy",
            "ai_generate",
            "--max-cost",
            "0.12",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "an unresolved asset fails\n{stderr}"
    );
    assert_eq!(report["generated"], 2, "{report:#}");
    assert_eq!(report["over_budget"], 1, "{report:#}");
    assert_eq!(report["failed"], 0, "{report:#}");
    assert!((report["cost_usd"].as_f64().unwrap() - 0.10).abs() < 1e-9);
    assert!(
        stderr.contains("NOT SENT: cost ceiling reached"),
        "{stderr}"
    );

    // The third submit never reached the provider.
    let posts = log
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("POST"))
        .count();
    assert_eq!(posts, 2, "{:?}", log.lock().unwrap());

    // Every paid call is in the ledger, and the report adds them up.
    let ledger = std::fs::read_to_string(dir.join(".flint/ai_costs.jsonl")).unwrap();
    let entries: Vec<Value> = ledger
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries.len(), 2);
    for (e, name) in entries.iter().zip(["wall_a_tex", "wall_b_tex"]) {
        assert_eq!(e["provider"], "flux");
        assert_eq!(e["spec_name"], name);
        assert_eq!(e["operation"], "resolve");
        assert_eq!(e["unit"], "megapixel");
    }
    let (out, costs) = flint(&dir, &["asset", "costs"]);
    assert!(out.status.success());
    assert_eq!(costs["total"]["calls"], 2);
    assert!((costs["total"]["usd"].as_f64().unwrap() - 0.10).abs() < 1e-9);
    assert_eq!(costs["by_provider"]["flux"]["calls"], 2);
    // `flint gen --cost-report` reads the same ledger.
    let (_, report) = flint(&dir, &["gen", "--cost-report"]);
    assert_eq!(report["total"]["calls"], 2);

    // With room for all three, the third is generated too.
    let (out, report) = flint(
        &dir,
        &[
            "asset",
            "resolve",
            "walls.scene.toml",
            "--strategy",
            "ai_generate",
            "--max-cost",
            "1.00",
        ],
    );
    assert!(out.status.success(), "{report:#}");
    assert_eq!(
        report["generated"], 1,
        "two are in the catalog now: {report:#}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_generate_over_max_cost_sends_nothing() {
    let (base, log) = fake_providers();
    let dir = project("generate", &base);
    let (out, report) = flint(
        &dir,
        &[
            "asset",
            "generate",
            "texture",
            "-d",
            "red brick",
            "--name",
            "brick",
            "--max-cost",
            "0.01",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let message = report["error"]["message"].as_str().unwrap();
    assert!(message.contains("cost ceiling reached"), "{message}");
    assert!(message.contains("--max-cost $0.0100"), "{message}");
    assert!(log.lock().unwrap().is_empty());
    assert!(!dir.join(".flint/ai_costs.jsonl").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_meshy_generate_is_a_job_the_job_list_shows() {
    let (base, _) = fake_providers();
    let dir = project("meshy", &base);
    let (out, generated) = flint(
        &dir,
        &[
            "asset",
            "generate",
            "model",
            "-d",
            "a tavern chair",
            "--name",
            "chair",
            "--provider",
            "meshy",
        ],
    );
    assert!(
        out.status.success(),
        "{generated:#}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let job_id = generated["job"]
        .as_str()
        .expect("the generate names its job");
    assert!((generated["cost_usd"].as_f64().unwrap() - 0.60).abs() < 1e-9);

    let (_, list) = flint(&dir, &["asset", "job", "list"]);
    let jobs = list["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1, "{list:#}");
    assert_eq!(jobs[0]["id"], job_id);
    assert_eq!(jobs[0]["provider"], "meshy");
    assert_eq!(jobs[0]["remote_id"], "task-7");
    assert_eq!(jobs[0]["status"], "complete");
    assert_eq!(jobs[0]["estimated_usd"], 0.60);

    // A complete job resumes to itself.
    let (out, resumed) = flint(&dir, &["asset", "job", "resume", job_id]);
    assert!(out.status.success(), "{resumed:#}");
    assert_eq!(resumed["status"], "complete");
    let _ = std::fs::remove_dir_all(dir);
}
