//! Mock-HTTP tests: a scripted [`Transport`] replays canned API responses
//! and records every request, so the agent's prompts, parsing, retries and
//! validation loop run end to end with no network.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::*;
use crate::http::HttpResponse;
use crate::MockAgent;

/// Replays `responses` in order and records each request.
#[derive(Clone, Default)]
struct Scripted {
    responses: Arc<Mutex<VecDeque<std::result::Result<HttpResponse, String>>>>,
    sent: Arc<Mutex<Vec<HttpRequest>>>,
}

impl Scripted {
    fn new(responses: Vec<std::result::Result<HttpResponse, String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            sent: Arc::default(),
        }
    }

    fn bodies(&self) -> Vec<Json> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_str(&r.body).unwrap())
            .collect()
    }

    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
}

impl Transport for Scripted {
    fn post(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, String> {
        self.sent.lock().unwrap().push(req.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("the agent sent more requests than the test scripted")
    }
}

/// A 200 whose single text block is `text`, after a thinking block.
fn ok(text: &str) -> std::result::Result<HttpResponse, String> {
    let body = json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "sig"},
            {"type": "text", "text": text},
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 120, "output_tokens": 400, "cache_read_input_tokens": 3000},
    });
    Ok(HttpResponse {
        status: 200,
        retry_after: None,
        body: body.to_string(),
    })
}

fn status(
    code: u16,
    kind: &str,
    retry_after: Option<u64>,
) -> std::result::Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: code,
        retry_after,
        body:
            json!({"type": "error", "error": {"type": kind, "message": format!("{kind} message")}})
                .to_string(),
    })
}

fn agent(script: &Scripted) -> AnthropicAgent {
    let mut config = AgentConfig::anthropic();
    config.api_key = "sk-test".into();
    AnthropicAgent::with_env_key(config, None)
        .unwrap()
        .with_transport(script.clone())
        .without_backoff()
}

const PINE: &str = r##"{
  "name": "tall_sparse_pine",
  "tags": ["pine", "conifer", "Tall Tree", "sparse"],
  "params": {
    "trunk_height": 14,
    "trunk_radius_base": 0.35,
    "crown_shape": "ellipsoid",
    "crown_radius": 1.8,
    "crown_height_ratio": 3.0,
    "leaf_density": 0.3,
    "leaf_color_base": "#1F4D2B"
  },
  "rationale": "A tall trunk and a narrow, tall crown read as a pine; low leaf density keeps it sparse."
}"##;

#[test]
fn create_returns_a_spec_the_generator_builds() {
    let script = Scripted::new(vec![ok(PINE)]);
    let (spec, report) = agent(&script)
        .create_spec_reported("tall pine tree with sparse branches", "tree")
        .unwrap();

    assert_eq!(spec.generator, "tree_v1");
    assert_eq!(spec.meta.name, "tall_sparse_pine");
    assert_eq!(
        spec.meta.description.as_deref(),
        Some("tall pine tree with sparse branches")
    );
    assert!(
        spec.meta.tags.contains(&"tall_tree".to_string()),
        "{:?}",
        spec.meta.tags
    );
    assert_eq!(spec.seed.mode, SeedMode::Fixed);
    // A whole number for a `number` param arrives as a float.
    assert_eq!(spec.params["trunk_height"].as_float(), Some(14.0));
    assert_eq!(spec.params["leaf_density"].as_float(), Some(0.3));

    let schema = registry().get("tree_v1").unwrap().param_schema();
    assert!(check_params(&schema, &spec.params).is_empty());
    registry().generate_from_spec(&spec).unwrap();

    assert_eq!(report.rounds.len(), 1);
    assert!(report.converged());
    assert_eq!(report.repaired, 0);
    assert!(report.notes[0].contains("pine"));
    assert_eq!(report.usage().cache_read_input_tokens, 3000);
}

#[test]
fn the_request_follows_the_messages_api() {
    let script = Scripted::new(vec![ok(PINE)]);
    agent(&script)
        .create_spec_from_prompt("tall pine tree with sparse branches", "tree")
        .unwrap();
    let sent = script.sent.lock().unwrap()[0].clone();
    assert_eq!(sent.url, API_URL);
    assert_eq!(sent.header("x-api-key"), Some("sk-test"));
    assert_eq!(sent.header("anthropic-version"), Some(API_VERSION));
    assert_eq!(sent.header("anthropic-beta"), Some(FALLBACK_BETA));

    let body: Json = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["model"], DEFAULT_MODEL);
    assert_eq!(body["fallbacks"], "default");
    // Removed on API version 2026-10-21; sending them is a 400.
    for gone in ["temperature", "top_p", "top_k"] {
        assert!(body.get(gone).is_none(), "{gone}");
    }
    let system = &body["system"][0];
    assert_eq!(system["cache_control"]["type"], "ephemeral");
    let system = system["text"].as_str().unwrap();
    assert!(
        system.contains("\"trunk_height\""),
        "schema in system prompt"
    );
    assert!(system.contains("## Spec format"));
    assert!(system.contains("## A valid example"));
    let user = body["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(user.contains("tall pine tree with sparse branches"));
    // The prompt never leaks into the cached system prefix.
    assert!(!system.contains("sparse branches"));
}

#[test]
fn effort_goes_in_output_config() {
    let script = Scripted::new(vec![ok(PINE)]);
    let mut config = AgentConfig::anthropic();
    config.api_key = "k".into();
    config.effort = Some("high".into());
    config.model = "claude-haiku-5-5".into();
    let agent = AnthropicAgent::with_env_key(config, None)
        .unwrap()
        .with_transport(script.clone());
    agent.create_spec_from_prompt("pine", "tree").unwrap();
    let body = &script.bodies()[0];
    assert_eq!(body["output_config"]["effort"], "high");
    // Haiku takes no server-side fallback.
    assert!(body.get("fallbacks").is_none());
    assert!(script.sent.lock().unwrap()[0]
        .header("anthropic-beta")
        .is_none());
}

#[test]
fn the_loop_sends_validator_reports_back_until_the_spec_is_valid() {
    let broken = r##"{"name": "pine", "tags": [], "params": {
        "trunk_height": -3.0, "trunk_radius": 0.3, "crown_shape": "cone", "leaf_density": 0.3
    }}"##;
    let half = r##"{"name": "pine", "tags": [], "params": {
        "trunk_height": 12.0, "crown_shape": "cone", "leaf_density": 0.3
    }}"##;
    let script = Scripted::new(vec![ok(broken), ok(half), ok(PINE)]);
    let (spec, report) = agent(&script)
        .create_spec_reported("tall pine tree with sparse branches", "tree")
        .unwrap();

    let counts: Vec<usize> = report.rounds.iter().map(|r| r.issues.len()).collect();
    assert_eq!(counts, vec![3, 1, 0], "{:#?}", report.rounds);
    assert!(report.converged());
    assert_eq!(spec.params["crown_shape"].as_str(), Some("ellipsoid"));

    let bodies = script.bodies();
    assert_eq!(bodies.len(), 3);
    // Round 2 carries the whole conversation: task, reply, report.
    let msgs = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1]["role"], "assistant");
    // The assistant turn goes back unchanged, thinking block included.
    assert_eq!(msgs[1]["content"][0]["type"], "thinking");
    assert_eq!(msgs[1]["content"][0]["signature"], "sig");
    let report_text = msgs[2]["content"][0]["text"].as_str().unwrap();
    assert!(
        report_text.contains("`trunk_radius` is not a parameter"),
        "{report_text}"
    );
    assert!(report_text.contains("`trunk_height` = -3"), "{report_text}");
    assert!(
        report_text.contains("\"cone\" is not one of"),
        "{report_text}"
    );
    // The system prompt is byte-identical every round (cache-friendly).
    assert_eq!(bodies[0]["system"], bodies[2]["system"]);
}

#[test]
fn a_mock_agent_that_fails_once_takes_two_rounds() {
    let first = r##"{"name": "x", "params": {"leaf_density": 1.7}}"##;
    let second = r##"{"name": "x", "params": {"leaf_density": 0.7}}"##;
    let script = Scripted::new(vec![ok(first), ok(second)]);
    let (_, report) = agent(&script).create_spec_reported("x", "tree").unwrap();
    assert_eq!(report.rounds.len(), 2);
    assert_eq!(script.count(), 2);
}

#[test]
fn out_of_rounds_the_schema_repairs_what_it_can() {
    let bad = r##"{"name": "x", "params": {"leaf_density": 1.7, "trunk_height": 8.0}}"##;
    let script = Scripted::new(vec![ok(bad), ok(bad), ok(bad)]);
    let (spec, report) = agent(&script).create_spec_reported("x", "tree").unwrap();
    assert_eq!(report.rounds.len(), 3, "max_iterations rounds, then stop");
    assert!(!report.converged());
    assert_eq!(report.repaired, 1);
    assert_eq!(spec.params["leaf_density"].as_float(), Some(1.0));
    assert_eq!(spec.params["trunk_height"].as_float(), Some(8.0));
}

#[test]
fn replies_that_never_hold_json_are_an_error() {
    let script = Scripted::new(vec![
        ok("I think a pine tree would be lovely."),
        ok("{oops"),
        ok(""),
    ]);
    let err = agent(&script)
        .create_spec_from_prompt("pine", "tree")
        .unwrap_err();
    assert!(matches!(err, ProcGenAiError::AgentError(_)), "{err}");
    assert!(err.to_string().contains("after 3 round(s)"), "{err}");
}

#[test]
fn a_params_value_that_is_not_an_object_is_fed_back() {
    let script = Scripted::new(vec![ok(r#"{"params": "tall"}"#), ok(PINE)]);
    let (_, report) = agent(&script).create_spec_reported("pine", "tree").unwrap();
    assert_eq!(
        report.rounds[0].issues,
        vec!["`params` must be a JSON object"]
    );
}

#[test]
fn params_that_fit_the_schema_but_build_nothing_are_issues() {
    // A creature with no skeleton passes the schema and builds 0 vertices.
    let ctx = Ctx::new("creature").unwrap();
    let empty = ctx.params_from_json(&json!({})).unwrap();
    assert!(check_params(&ctx.schema, &empty).is_empty());
    let issues = ctx.issues(&empty);
    assert!(
        issues.iter().any(|i| i.contains("empty mesh")),
        "{issues:?}"
    );
    // The mock's template builds a beetle.
    let beetle = MockAgent
        .create_spec_from_prompt("beetle", "creature")
        .unwrap();
    assert!(ctx.issues(&beetle.params).is_empty());
}

#[test]
fn malformed_api_bodies_are_errors_not_panics() {
    for body in [
        "",
        "<html>",
        "{}",
        r#"{"content": 5}"#,
        r#"{"content": [{"type": "text"}]}"#,
    ] {
        let script = Scripted::new(vec![
            Ok(HttpResponse {
                status: 200,
                retry_after: None,
                body: body.to_string(),
            });
            3
        ]);
        let result = agent(&script).create_spec_from_prompt("pine", "tree");
        assert!(result.is_err(), "{body:?} gave a spec");
    }
}

#[test]
fn rate_limits_are_retried_then_reported() {
    let script = Scripted::new(vec![status(429, "rate_limit_error", Some(2)), ok(PINE)]);
    agent(&script)
        .create_spec_from_prompt("pine", "tree")
        .unwrap();
    assert_eq!(script.count(), 2);

    let script = Scripted::new(vec![
        status(529, "overloaded_error", None),
        status(429, "rate_limit_error", None),
        status(429, "rate_limit_error", Some(30)),
    ]);
    let err = agent(&script)
        .create_spec_from_prompt("pine", "tree")
        .unwrap_err();
    assert_eq!(script.count(), 3, "one try plus max_retries");
    assert_eq!(
        err.to_string(),
        "rate limited after 3 attempt(s); retry in 30s: rate_limit_error message"
    );
}

#[test]
fn auth_failures_are_not_retried() {
    let script = Scripted::new(vec![status(401, "authentication_error", None)]);
    let err = agent(&script)
        .create_spec_from_prompt("pine", "tree")
        .unwrap_err();
    assert_eq!(script.count(), 1);
    assert!(
        matches!(err, ProcGenAiError::Auth { status: 401, .. }),
        "{err}"
    );
    assert!(err.to_string().contains("ANTHROPIC_API_KEY"));
}

#[test]
fn network_failures_are_retried_then_reported() {
    let script = Scripted::new(vec![
        Err("connection refused".into()),
        Err("connection refused".into()),
        Err("connection refused".into()),
    ]);
    let err = agent(&script)
        .create_spec_from_prompt("pine", "tree")
        .unwrap_err();
    assert_eq!(err.to_string(), "network error: connection refused");
    assert_eq!(script.count(), 3);
}

#[test]
fn an_unknown_generator_never_reaches_the_network() {
    let script = Scripted::new(vec![]);
    let err = agent(&script)
        .create_spec_from_prompt("x", "spaceship")
        .unwrap_err();
    assert!(err.to_string().contains("tree_v1"), "{err}");
    let mut spec = MockAgent.create_spec_from_prompt("x", "tree").unwrap();
    spec.generator = "spaceship".into();
    let fb = agent(&script).interpret_spec(&spec).unwrap();
    assert_eq!(fb.confidence, 0.0);
    assert_eq!(script.count(), 0);
}

#[test]
fn interpret_starts_from_the_validators_findings() {
    let spec = ProcGenSpec::parse(
        r#"
generator = "tree"
[meta]
name = "pine"
description = "tall pine tree with sparse branches"
version = "1.0.0"
[seed]
mode = "random"
[params]
trunk_height = 3.0
leaf_density = 1.4
"#,
    )
    .unwrap();
    let reply = r#"{
      "suggestions": [
        "`trunk_height` 3.0 -> 13.0: a pine is tall; 3 m reads as a shrub",
        "`leaf_density` 1.4 -> 0.3: out of range, and 'sparse' asks for a thin crown"
      ],
      "confidence": 0.2,
      "params": {"trunk_height": 13.0, "leaf_density": 0.3, "crown_shape": "ellipsoid"}
    }"#;
    let script = Scripted::new(vec![ok(reply)]);
    let fb = agent(&script).interpret_spec(&spec).unwrap();

    let sent = script.bodies()[0]["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(sent.contains("registered as \"tree_v1\""), "{sent}");
    assert!(sent.contains("`leaf_density` = 1.4 is outside"), "{sent}");
    assert!(sent.contains("seed is random"), "{sent}");

    assert_eq!(fb.suggestions.len(), 2);
    assert!(fb.suggestions[0].contains("trunk_height"));
    assert!((fb.confidence - 0.2).abs() < 1e-6);
    let revised = fb.revised_spec.unwrap();
    assert_eq!(revised.generator, "tree_v1");
    assert_eq!(revised.seed.mode, SeedMode::Fixed);
    assert_eq!(revised.meta.version, "1.0.1");
    assert_eq!(revised.params["trunk_height"].as_float(), Some(13.0));
}

#[test]
fn interpret_needs_suggestions_and_allows_null_params() {
    let spec = MockAgent
        .create_spec_from_prompt("tall oak", "tree")
        .unwrap();
    let script = Scripted::new(vec![
        ok(r#"{"confidence": 0.9, "params": null}"#),
        ok(
            r#"{"suggestions": ["`crown_radius` 3.0 -> 4.2: an oak's crown is wider than it is tall"], "confidence": 0.8, "params": null}"#,
        ),
    ]);
    let (fb, report) = agent(&script).interpret_spec_reported(&spec).unwrap();
    assert_eq!(report.rounds.len(), 2);
    assert!(report.rounds[0].issues[0].contains("suggestions"));
    assert!(fb.revised_spec.is_none());
    assert_eq!(fb.suggestions.len(), 1);
}

/// A tiny real PNG.
fn png(r: u8, g: u8, b: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([r, g, b, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn refine_sends_the_reference_as_a_base64_image() {
    let spec = MockAgent.create_spec_from_prompt("pine", "tree").unwrap();
    let reference = png(30, 80, 40);
    let script = Scripted::new(vec![ok(
        r##"{"params": {"trunk_height": 12.0, "leaf_color_base": "#1E5028"}, "changes": ["`leaf_color_base` -> #1E5028: the reference's darker green"]}"##,
    )]);
    let (refined, report) = agent(&script)
        .refine_spec_reported(&spec, Some(&reference))
        .unwrap();

    let content = script.bodies()[0]["messages"][0]["content"].clone();
    let blocks = content.as_array().unwrap();
    assert_eq!(blocks.len(), 2, "a tree has no preview: reference + text");
    assert_eq!(blocks[0]["type"], "image");
    assert_eq!(blocks[0]["source"]["media_type"], "image/png");
    assert_eq!(blocks[0]["source"]["data"], base64(&reference));
    assert!(blocks[1]["text"]
        .as_str()
        .unwrap()
        .contains("The image is the reference"));

    assert_eq!(refined.params["leaf_color_base"].as_str(), Some("#1E5028"));
    assert_eq!(refined.meta.version, "1.0.1");
    assert!(report.notes[0].contains("darker green"));
}

#[test]
fn refining_a_texture_also_shows_what_it_builds_now() {
    let mut spec = MockAgent
        .create_spec_from_prompt("old brick wall", "texture")
        .unwrap();
    let t = spec.params.as_table_mut().unwrap();
    t.insert("width".into(), toml::Value::Integer(64));
    t.insert("height".into(), toml::Value::Integer(64));
    let reply = json!({"params": spec.params.clone(), "changes": []}).to_string();
    let script = Scripted::new(vec![ok(&reply)]);
    agent(&script)
        .refine_spec(&spec, Some(&png(150, 60, 40)))
        .unwrap();
    let content = script.bodies()[0]["messages"][0]["content"].clone();
    let blocks = content.as_array().unwrap();
    assert_eq!(blocks.len(), 3, "reference, current, text");
    assert_eq!(blocks[1]["type"], "image");
    let text = blocks[2]["text"].as_str().unwrap();
    assert!(text.contains("reference #963C28 [150, 60, 40]"), "{text}");
    assert!(blocks[2]["text"]
        .as_str()
        .unwrap()
        .contains("The second is what this spec generates now"));
}

/// A stand-in for the CLI's model render: a solid image in the spec's leaf
/// colour, so a refine that moves `leaf_color_base` moves the "render".
fn leaf_render(spec: &ProcGenSpec) -> Option<Vec<u8>> {
    let hex = spec.params.get("leaf_color_base")?.as_str()?;
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()?;
    Some(png((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn tree_with_leaves(hex: &str) -> ProcGenSpec {
    let mut spec = MockAgent.create_spec_from_prompt("pine", "tree").unwrap();
    spec.params
        .as_table_mut()
        .unwrap()
        .insert("leaf_color_base".into(), toml::Value::String(hex.into()));
    spec
}

fn leaves_reply(hex: &str) -> std::result::Result<HttpResponse, String> {
    ok(&json!({
        "params": {"leaf_color_base": hex},
        "changes": [format!("`leaf_color_base` -> {hex}")],
    })
    .to_string())
}

#[test]
fn refining_a_tree_with_a_previewer_sends_a_render_of_the_current_tree() {
    let spec = tree_with_leaves("#808080");
    let reference = png(30, 120, 40);
    let script = Scripted::new(vec![leaves_reply("#808080")]);
    agent(&script)
        .with_previewer(leaf_render)
        .refine_spec(&spec, Some(&reference))
        .unwrap();

    let content = script.bodies()[0]["messages"][0]["content"].clone();
    let blocks = content.as_array().unwrap();
    assert_eq!(blocks.len(), 3, "reference, render, text");
    assert_eq!(blocks[0]["source"]["data"], base64(&reference));
    let render = leaf_render(&spec).unwrap();
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["source"]["data"], base64(&render));
    let text = blocks[2]["text"].as_str().unwrap();
    assert!(
        text.contains("a render of the mesh this spec builds now"),
        "{text}"
    );
    assert!(text.contains("current output #808080"), "{text}");
    assert!(text.contains("out of 100"), "{text}");
}

#[test]
fn the_measured_loop_runs_while_the_distance_falls_and_keeps_the_closest() {
    // Reference green; rounds go grey -> olive -> near green -> red.
    let spec = tree_with_leaves("#808080");
    let script = Scripted::new(vec![
        leaves_reply("#607040"),
        leaves_reply("#28782C"),
        leaves_reply("#C03020"),
    ]);
    let (refined, report) = agent(&script)
        .with_previewer(leaf_render)
        .with_refine_rounds(5)
        .refine_spec_reported(&spec, Some(&png(30, 120, 40)))
        .unwrap();

    assert_eq!(
        script.count(),
        3,
        "the third round got further: the loop stops"
    );
    let scores: Vec<f64> = report.distances.iter().map(|d| d.score).collect();
    assert_eq!(scores.len(), 4, "start + three rounds: {scores:?}");
    assert!(scores[1] < scores[0] && scores[2] < scores[1], "{scores:?}");
    assert!(scores[3] > scores[2], "{scores:?}");
    assert_eq!(report.kept, 2);
    assert_eq!(refined.params["leaf_color_base"].as_str(), Some("#28782C"));
    assert_eq!(refined.meta.version, "1.0.1");
    assert_eq!(
        report.notes,
        vec!["`leaf_color_base` -> #28782C".to_string()]
    );
    assert_eq!(report.rounds.len(), 3);

    // Round 2 was asked with round 1's render and its measured progress.
    let second = script.bodies()[1]["messages"].clone();
    let msgs = second.as_array().unwrap();
    assert_eq!(msgs.len(), 3, "user, assistant, user");
    let progress = msgs[2]["content"].as_array().unwrap();
    assert_eq!(
        progress[0]["source"]["data"],
        base64(&leaf_render(&tree_with_leaves("#607040")).unwrap())
    );
    let text = progress[1]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("from {:.1} to {:.1}", scores[0], scores[1])),
        "{text}"
    );
}

#[test]
fn the_measured_loop_stops_at_refine_rounds() {
    let spec = tree_with_leaves("#808080");
    let script = Scripted::new(vec![leaves_reply("#607040"), leaves_reply("#28782C")]);
    let (refined, report) = agent(&script)
        .with_previewer(leaf_render)
        .with_refine_rounds(2)
        .refine_spec_reported(&spec, Some(&png(30, 120, 40)))
        .unwrap();
    assert_eq!(script.count(), 2);
    assert_eq!(report.distances.len(), 3);
    assert_eq!(report.kept, 2);
    assert_eq!(refined.params["leaf_color_base"].as_str(), Some("#28782C"));
}

#[test]
fn a_first_round_that_gets_further_returns_the_spec_unchanged() {
    let spec = tree_with_leaves("#28782C");
    let script = Scripted::new(vec![leaves_reply("#E0E0E0")]);
    let (refined, report) = agent(&script)
        .with_previewer(leaf_render)
        .refine_spec_reported(&spec, Some(&png(30, 120, 40)))
        .unwrap();
    assert_eq!(script.count(), 1);
    assert_eq!(report.kept, 0);
    assert_eq!(refined, spec);
    assert!(report.notes[0].contains("no closer"), "{:?}", report.notes);
}

#[test]
fn without_a_picture_a_tree_refine_is_one_unmeasured_shot() {
    let spec = tree_with_leaves("#808080");
    let script = Scripted::new(vec![leaves_reply("#607040")]);
    let (refined, report) = agent(&script)
        .with_previewer(|_: &ProcGenSpec| None)
        .refine_spec_reported(&spec, Some(&png(30, 120, 40)))
        .unwrap();
    assert_eq!(script.count(), 1);
    assert!(report.distances.is_empty());
    assert_eq!(report.kept, 1);
    assert_eq!(refined.params["leaf_color_base"].as_str(), Some("#607040"));
}

#[test]
fn a_reference_that_is_not_an_image_is_refused_before_sending() {
    let spec = MockAgent.create_spec_from_prompt("pine", "tree").unwrap();
    let script = Scripted::new(vec![]);
    let err = agent(&script)
        .refine_spec(&spec, Some(b"definitely not a png"))
        .unwrap_err();
    assert!(matches!(err, ProcGenAiError::SpecError(_)), "{err}");
    assert_eq!(script.count(), 0);
}

#[test]
fn the_api_key_comes_from_the_environment_first() {
    assert_eq!(
        resolve_api_key("from-config", Some("from-env")).as_deref(),
        Some("from-env")
    );
    assert_eq!(
        resolve_api_key("from-config", Some("  ")).as_deref(),
        Some("from-config")
    );
    assert_eq!(
        resolve_api_key("from-config", None).as_deref(),
        Some("from-config")
    );
    assert_eq!(resolve_api_key("", None), None);

    let agent = AnthropicAgent::with_env_key(AgentConfig::anthropic(), Some("env-key")).unwrap();
    assert_eq!(agent.config().api_key, "env-key");
    assert!(
        !format!("{agent:?}").contains("env-key"),
        "Debug redacts the key"
    );

    let err = AnthropicAgent::with_env_key(AgentConfig::anthropic(), None).unwrap_err();
    assert!(err.to_string().contains(API_KEY_ENV), "{err}");
    let err = AnthropicAgent::with_env_key(AgentConfig::mock(), Some("k")).unwrap_err();
    assert!(err.to_string().contains("provider"), "{err}");
}

#[test]
fn works_as_a_trait_object() {
    let script = Scripted::new(vec![ok(PINE)]);
    let boxed: Box<dyn ProcGenAgent> = Box::new(agent(&script));
    let spec = boxed
        .create_spec_from_prompt("tall pine tree with sparse branches", "tree")
        .unwrap();
    assert_eq!(spec.generator, "tree_v1");
}
