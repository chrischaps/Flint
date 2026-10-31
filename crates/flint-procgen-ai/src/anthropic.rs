//! [`AnthropicAgent`]: a [`ProcGenAgent`] that asks Claude.
//!
//! Every reply goes through the same gate the engine would apply: its params
//! are checked against the generator's `param_schema()` and then actually
//! generated. A reply that fails goes back to Claude with the validator's
//! report, up to `max_iterations` rounds, so what comes out is a spec the
//! generator builds and not just JSON that parses.

use std::time::Duration;

use flint_procgen::{
    Generator, GeneratorOutput, GeneratorRegistry, OutputKind, ProcGenSpec, SeedConfig, SeedMode,
    SpecMeta,
};
use serde_json::{json, Map, Value as Json};

use crate::agent::{ProcGenAgent, SpecFeedback};
use crate::config::AgentConfig;
use crate::error::{ProcGenAiError, Result};
use crate::http::{
    base64, extract_json_object, image_media_type, parse_response, shorten, HttpRequest, Reply,
    Transport, UreqTransport, Usage,
};
use crate::mock::{bump_patch, fnv1a, props, registry, resolve_name, slug, unknown_generator};
use crate::prompt;
use crate::schema::{check_params, json_to_toml, prop_type, repair_params};

/// The model used when a config names none.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// The Messages API endpoint.
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// The `anthropic-version` header sent with every request.
pub const API_VERSION: &str = "2026-10-21";
/// The environment variable read for the key.
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Beta that lets a request declined by a safety classifier be re-run on
/// another model inside the same call (`fallbacks: "default"`).
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Models that take `fallbacks: "default"`.
const FALLBACK_MODELS: &[&str] = &[
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-fable-5-1",
    "claude-sonnet-5-5",
];
/// The API's limit on one base64 image is 5 MB of encoded data.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024 * 3 / 4;
/// Images and textures are generated at most this wide when only checked or
/// shown to the model: the check is for errors, not pixels.
const PROBE_SIZE: i64 = 256;

/// The key to use: the environment's when it is set and non-empty, else
/// the config's. The environment wins so a key saved in a project file can
/// be overridden per shell without editing the file.
pub fn resolve_api_key(config_key: &str, env_key: Option<&str>) -> Option<String> {
    env_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .or_else(|| Some(config_key.trim()).filter(|k| !k.is_empty()))
        .map(str::to_string)
}

/// One request/response round of a conversation.
#[derive(Debug, Clone, Default)]
pub struct Round {
    /// What the validator found in this round's reply; empty when accepted.
    pub issues: Vec<String>,
    /// Tokens this round cost.
    pub usage: Usage,
    /// The model that answered.
    pub model: String,
}

/// How a conversation went.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Each round, in order.
    pub rounds: Vec<Round>,
    /// Params the schema repair had to fix after the last round (0 when
    /// Claude converged on its own).
    pub repaired: usize,
    /// The model's own account of its choices (`rationale` / `changes`).
    pub notes: Vec<String>,
}

impl Report {
    /// Whether the last round's reply passed validation unaided.
    pub fn converged(&self) -> bool {
        self.rounds.last().is_some_and(|r| r.issues.is_empty())
    }

    /// Tokens across all rounds.
    pub fn usage(&self) -> Usage {
        self.rounds
            .iter()
            .fold(Usage::default(), |a, r| a.plus(r.usage))
    }
}

/// A [`ProcGenAgent`] backed by the Claude Messages API.
pub struct AnthropicAgent {
    config: AgentConfig,
    transport: Box<dyn Transport>,
    url: String,
    /// Output token ceiling per reply (thinking included).
    pub max_tokens: u32,
    /// Extra attempts for a 429, 529, 5xx or network failure.
    pub max_retries: u32,
    sleep: fn(Duration),
}

impl std::fmt::Debug for AnthropicAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicAgent")
            .field("config", &self.config)
            .field("url", &self.url)
            .field("max_tokens", &self.max_tokens)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl AnthropicAgent {
    /// An agent for `config`, with the key from `ANTHROPIC_API_KEY` when
    /// that is set (see [`resolve_api_key`]) and the model defaulted to
    /// [`DEFAULT_MODEL`] when empty.
    pub fn new(config: AgentConfig) -> Result<Self> {
        let env = std::env::var(API_KEY_ENV).ok();
        Self::with_env_key(config, env.as_deref())
    }

    /// [`Self::new`] with the default config: provider `anthropic`,
    /// [`DEFAULT_MODEL`], three iterations.
    pub fn from_env() -> Result<Self> {
        Self::new(AgentConfig::anthropic())
    }

    /// [`Self::new`] with the environment's key passed in, for tests.
    pub fn with_env_key(mut config: AgentConfig, env_key: Option<&str>) -> Result<Self> {
        if config.provider.trim().is_empty() {
            config.provider = "anthropic".to_string();
        }
        if config.provider != "anthropic" {
            return Err(ProcGenAiError::ConfigError(format!(
                "AnthropicAgent needs provider \"anthropic\", not \"{}\"",
                config.provider
            )));
        }
        if config.model.trim().is_empty() {
            config.model = DEFAULT_MODEL.to_string();
        }
        config.api_key = resolve_api_key(&config.api_key, env_key).ok_or_else(|| {
            ProcGenAiError::ConfigError(format!(
                "no API key: set {API_KEY_ENV} or AgentConfig::api_key"
            ))
        })?;
        config.validate()?;
        Ok(Self {
            config,
            transport: Box::new(UreqTransport::default()),
            url: API_URL.to_string(),
            max_tokens: 16_000,
            max_retries: 2,
            sleep: std::thread::sleep,
        })
    }

    /// Replace the HTTP transport (tests, proxies).
    pub fn with_transport(mut self, transport: impl Transport + 'static) -> Self {
        self.transport = Box::new(transport);
        self
    }

    /// Send to another endpoint (a gateway or a local mock server).
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Don't actually wait between retries (tests).
    pub fn without_backoff(mut self) -> Self {
        self.sleep = |_| {};
        self
    }

    /// The resolved config. Its key is redacted in `Debug`.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// The request body for one call.
    fn body(&self, system: &str, messages: &[Json]) -> Json {
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.max_tokens,
            "system": [{
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"},
            }],
            "messages": messages,
        });
        if let Some(effort) = self.config.effort.as_deref().filter(|e| !e.is_empty()) {
            body["output_config"] = json!({ "effort": effort });
        }
        if self.uses_fallback() {
            body["fallbacks"] = json!("default");
        }
        body
    }

    fn uses_fallback(&self) -> bool {
        FALLBACK_MODELS.contains(&self.config.model.as_str())
    }

    /// One Messages API call, retried on 429 / 529 / 5xx / network errors.
    fn call(&self, system: &str, messages: &[Json]) -> Result<Reply> {
        let mut headers = vec![
            ("x-api-key".to_string(), self.config.api_key.clone()),
            ("anthropic-version".to_string(), API_VERSION.to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        if self.uses_fallback() {
            headers.push(("anthropic-beta".to_string(), FALLBACK_BETA.to_string()));
        }
        let req = HttpRequest {
            url: self.url.clone(),
            headers,
            body: self.body(system, messages).to_string(),
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = match self.transport.post(&req) {
                Ok(resp) => parse_response(&resp, attempt),
                Err(e) => Err(ProcGenAiError::Network(e)),
            };
            match result {
                Err(e) if e.is_retryable() && attempt <= self.max_retries => {
                    let wait = match &e {
                        ProcGenAiError::RateLimited {
                            retry_after: Some(s),
                            ..
                        } => *s,
                        _ => 1 << attempt,
                    };
                    (self.sleep)(Duration::from_secs(wait.min(60)));
                }
                other => return other,
            }
        }
    }

    /// Ask, validate, and on failure send the validator's report back, up to
    /// `max_iterations` rounds. Returns the accepted reply object and its
    /// params (`None` when the reply set `"params": null` and that's
    /// allowed). `shape` adds task-specific checks on the reply object.
    fn converse(
        &self,
        ctx: &Ctx,
        first_turn: Vec<Json>,
        params_required: bool,
        shape: impl Fn(&Map<String, Json>) -> Vec<String>,
    ) -> Result<(Map<String, Json>, Option<toml::Value>, Report)> {
        let system = prompt::system_prompt(&ctx.name, ctx.generator());
        let mut messages = vec![json!({"role": "user", "content": first_turn})];
        let mut report = Report::default();
        let mut last: Option<(Map<String, Json>, Option<toml::Value>)> = None;

        for round in 1..=self.config.max_iterations {
            let reply = self.call(&system, &messages)?;
            messages.push(json!({"role": "assistant", "content": reply.content}));

            let mut issues = Vec::new();
            match extract_json_object(&reply.text) {
                Err(e) => issues.push(format!("{e}; reply with a single JSON object")),
                Ok(obj) => {
                    issues.extend(shape(&obj));
                    let params = match obj.get("params") {
                        None | Some(Json::Null) if !params_required => None,
                        None | Some(Json::Null) => {
                            issues.push("the reply has no `params` object".to_string());
                            None
                        }
                        Some(v) => match ctx.params_from_json(v) {
                            Some(p) => {
                                issues.extend(ctx.issues(&p));
                                Some(p)
                            }
                            None => {
                                issues.push("`params` must be a JSON object".to_string());
                                None
                            }
                        },
                    };
                    last = Some((obj, params));
                }
            }
            report.rounds.push(Round {
                issues: issues.clone(),
                usage: reply.usage,
                model: reply.model,
            });
            if issues.is_empty() {
                let (obj, params) = last.expect("an accepted round parsed");
                return Ok((obj, params, report));
            }
            if round < self.config.max_iterations {
                messages.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": prompt::feedback(round, &issues)}],
                }));
            }
        }

        // Out of rounds. Repair what the schema can fix; refuse what it can't.
        let remaining = report
            .rounds
            .last()
            .map(|r| r.issues.join("; "))
            .unwrap_or_default();
        let rounds = report.rounds.len();
        let fail = || {
            ProcGenAiError::AgentError(format!(
                "no valid reply after {rounds} round(s); last issues: {remaining}"
            ))
        };
        let Some((obj, params)) = last else {
            return Err(fail());
        };
        if !shape(&obj).is_empty() {
            return Err(fail());
        }
        let params = match params {
            Some(mut p) => {
                report.repaired = repair_params(&ctx.schema, &mut p);
                if !ctx.generation_issues(&p).is_empty() {
                    return Err(fail());
                }
                Some(p)
            }
            None if params_required => return Err(fail()),
            None => None,
        };
        Ok((obj, params, report))
    }

    /// [`ProcGenAgent::create_spec_from_prompt`], with the conversation's
    /// [`Report`].
    pub fn create_spec_reported(
        &self,
        prompt_text: &str,
        generator: &str,
    ) -> Result<(ProcGenSpec, Report)> {
        let ctx = Ctx::new(generator)?;
        let turn = vec![text_block(&prompt::create_task(prompt_text))];
        let (obj, params, mut report) = self.converse(&ctx, turn, true, |_| Vec::new())?;
        let params = params.expect("create requires params");

        let name = obj
            .get("name")
            .and_then(Json::as_str)
            .map(slug)
            .filter(|s| s != "untitled")
            .unwrap_or_else(|| slug(prompt_text));
        let mut tags = vec!["ai-generated".to_string(), "anthropic".to_string()];
        for t in strings(&obj, "tags").into_iter().take(8) {
            let t = slug(&t);
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        report.notes = strings(&obj, "rationale");
        let seed = fnv1a(&[ctx.name.as_bytes(), prompt_text.as_bytes()]) % 100_000;
        let spec = ProcGenSpec {
            meta: SpecMeta {
                name,
                description: Some(prompt_text.to_string()),
                version: "1.0.0".to_string(),
                tags,
            },
            generator: ctx.name.clone(),
            seed: SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(seed),
                derive_from: None,
            },
            params,
            lod: None,
        };
        Ok((spec, report))
    }

    /// [`ProcGenAgent::interpret_spec`], with the conversation's [`Report`].
    pub fn interpret_spec_reported(&self, spec: &ProcGenSpec) -> Result<(SpecFeedback, Report)> {
        let ctx = match Ctx::new(&spec.generator) {
            Ok(ctx) => ctx,
            // Nothing to ask Claude about: the spec names no generator.
            Err(e) => {
                return Ok((
                    SpecFeedback {
                        suggestions: vec![e.to_string()],
                        confidence: 0.0,
                        revised_spec: None,
                    },
                    Report::default(),
                ))
            }
        };
        let mut findings = Vec::new();
        if ctx.name != spec.generator {
            findings.push(format!(
                "generator \"{}\" is registered as \"{}\"",
                spec.generator, ctx.name
            ));
        }
        findings.extend(ctx.issues(&spec.params));
        if spec.seed.mode == SeedMode::Random {
            findings.push("the seed is random, so every run builds a different asset".into());
        }

        let turn = vec![text_block(&prompt::interpret_task(spec, &findings))];
        let shape = |obj: &Map<String, Json>| {
            if strings(obj, "suggestions").is_empty() {
                vec!["`suggestions` must be a non-empty array of strings".to_string()]
            } else {
                Vec::new()
            }
        };
        let (obj, params, report) = self.converse(&ctx, turn, false, shape)?;

        let confidence = obj
            .get("confidence")
            .and_then(Json::as_f64)
            .map_or(0.5, |c| c.clamp(0.0, 1.0)) as f32;
        let mut revised = spec.clone();
        revised.generator = ctx.name.clone();
        if let Some(p) = params {
            revised.params = p;
        }
        if spec.seed.mode == SeedMode::Random {
            revised.seed = SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(fnv1a(&[spec.meta.name.as_bytes()]) % 100_000),
                derive_from: None,
            };
        }
        let changed = revised != *spec;
        if changed {
            revised.meta.version = bump_patch(&revised.meta.version);
        }
        Ok((
            SpecFeedback {
                suggestions: strings(&obj, "suggestions"),
                confidence,
                revised_spec: changed.then_some(revised),
            },
            report,
        ))
    }

    /// [`ProcGenAgent::refine_spec`], with the conversation's [`Report`].
    ///
    /// For a texture generator Claude also sees what the spec builds now,
    /// beside the reference, so it can compare instead of guess.
    pub fn refine_spec_reported(
        &self,
        spec: &ProcGenSpec,
        reference: Option<&[u8]>,
    ) -> Result<(ProcGenSpec, Report)> {
        let ctx = Ctx::new(&spec.generator)?;
        let mut turn = Vec::new();
        if let Some(bytes) = reference {
            turn.push(image_block(bytes)?);
        }
        let current = match (reference, ctx.generator().output_kind()) {
            (Some(_), OutputKind::Image) => ctx.preview_png(&spec.params),
            _ => None,
        };
        if let Some(png) = &current {
            turn.push(image_block(png)?);
        }
        // Eyes misjudge the overall tone of a small image; numbers don't.
        let measured = match (
            reference.and_then(mean_rgb),
            current.as_deref().and_then(mean_rgb),
        ) {
            (Some(r), Some(c)) => Some((r, c)),
            _ => None,
        };
        turn.push(text_block(&prompt::refine_task(
            spec,
            reference.is_some(),
            current.is_some(),
            measured,
        )));
        let (obj, params, mut report) = self.converse(&ctx, turn, true, |_| Vec::new())?;

        let mut refined = spec.clone();
        refined.generator = ctx.name.clone();
        refined.params = params.expect("refine requires params");
        refined.meta.version = bump_patch(&refined.meta.version);
        report.notes = strings(&obj, "changes");
        Ok((refined, report))
    }
}

impl ProcGenAgent for AnthropicAgent {
    fn interpret_spec(&self, spec: &ProcGenSpec) -> Result<SpecFeedback> {
        self.interpret_spec_reported(spec).map(|(f, _)| f)
    }

    fn create_spec_from_prompt(&self, prompt: &str, generator: &str) -> Result<ProcGenSpec> {
        self.create_spec_reported(prompt, generator).map(|(s, _)| s)
    }

    fn refine_spec(&self, spec: &ProcGenSpec, reference: Option<&[u8]>) -> Result<ProcGenSpec> {
        self.refine_spec_reported(spec, reference).map(|(s, _)| s)
    }
}

/// The generator a conversation is about, and how to validate against it.
struct Ctx {
    registry: GeneratorRegistry,
    name: String,
    schema: Json,
}

impl Ctx {
    fn new(generator: &str) -> Result<Self> {
        let registry = registry();
        let name = resolve_name(&registry, generator)
            .ok_or_else(|| unknown_generator(&registry, generator))?;
        let schema = registry
            .get(&name)
            .expect("resolved names are registered")
            .param_schema();
        Ok(Self {
            registry,
            name,
            schema,
        })
    }

    fn generator(&self) -> &dyn Generator {
        self.registry
            .get(&self.name)
            .expect("resolved names are registered")
    }

    /// Claude's JSON params as TOML, with whole numbers written for `number`
    /// params turned into floats (generators read those with `as_float`).
    fn params_from_json(&self, v: &Json) -> Option<toml::Value> {
        let obj = v.as_object()?;
        let props = props(&self.schema);
        let mut table = toml::map::Map::new();
        for (k, v) in obj {
            let Some(mut t) = json_to_toml(v) else {
                continue;
            };
            let is_number = props
                .and_then(|p| p.get(k))
                .is_some_and(|p| prop_type(p) == "number");
            if let (true, toml::Value::Integer(i)) = (is_number, &t) {
                t = toml::Value::Float(*i as f64);
            }
            table.insert(k.clone(), t);
        }
        Some(toml::Value::Table(table))
    }

    /// Schema issues, then (if there are none) what generating them reveals.
    fn issues(&self, params: &toml::Value) -> Vec<String> {
        let schema: Vec<String> = check_params(&self.schema, params)
            .iter()
            .map(ToString::to_string)
            .collect();
        if !schema.is_empty() {
            return schema;
        }
        self.generation_issues(params)
    }

    /// A spec for these params, small enough to build quickly.
    fn probe(&self, params: &toml::Value, size: i64) -> ProcGenSpec {
        let mut params = params.clone();
        if self.generator().output_kind() == OutputKind::Image {
            if let Some(t) = params.as_table_mut() {
                for key in ["width", "height"] {
                    let w = t.get(key).and_then(toml::Value::as_integer).unwrap_or(size);
                    t.insert(key.to_string(), toml::Value::Integer(w.min(size)));
                }
            }
        }
        ProcGenSpec {
            meta: SpecMeta {
                name: "probe".to_string(),
                description: None,
                version: "1.0.0".to_string(),
                tags: Vec::new(),
            },
            generator: self.name.clone(),
            seed: SeedConfig {
                mode: SeedMode::Fixed,
                value: Some(1),
                derive_from: None,
            },
            params,
            lod: None,
        }
    }

    /// Build the params and report a failure or an empty result.
    fn generation_issues(&self, params: &toml::Value) -> Vec<String> {
        let empty = "the generator built an empty mesh (0 vertices); the params describe nothing";
        match self
            .registry
            .generate_from_spec(&self.probe(params, PROBE_SIZE))
        {
            Err(e) => vec![format!("the generator rejected the params: {e}")],
            Ok(GeneratorOutput::Mesh(m)) if m.vertices.is_empty() => vec![empty.to_string()],
            Ok(GeneratorOutput::MeshWithLods(l))
                if l.first().is_none_or(|m| m.vertices.is_empty()) =>
            {
                vec![empty.to_string()]
            }
            Ok(GeneratorOutput::SkinnedMesh(m)) if m.vertices.is_empty() => {
                vec![empty.to_string()]
            }
            Ok(_) => Vec::new(),
        }
    }

    /// The first image these params build, as a PNG, for the model to see.
    fn preview_png(&self, params: &toml::Value) -> Option<Vec<u8>> {
        let image = match self
            .registry
            .generate_from_spec(&self.probe(params, PROBE_SIZE))
            .ok()?
        {
            GeneratorOutput::Image(i) => i,
            GeneratorOutput::ImageSet(mut s) if !s.is_empty() => s.swap_remove(0),
            _ => return None,
        };
        let rgba = image::RgbaImage::from_raw(image.width, image.height, image.pixels)?;
        let mut png = std::io::Cursor::new(Vec::new());
        rgba.write_to(&mut png, image::ImageFormat::Png).ok()?;
        Some(png.into_inner())
    }
}

/// The average sRGB colour of an encoded image, ignoring alpha.
fn mean_rgb(bytes: &[u8]) -> Option<[u8; 3]> {
    let img = image::load_from_memory(bytes).ok()?.to_rgb8();
    let n = u64::from(img.width()) * u64::from(img.height());
    if n == 0 {
        return None;
    }
    let mut sum = [0u64; 3];
    for p in img.pixels() {
        for (s, c) in sum.iter_mut().zip(p.0) {
            *s += u64::from(c);
        }
    }
    Some(sum.map(|s| ((s + n / 2) / n) as u8))
}

fn text_block(text: &str) -> Json {
    json!({"type": "text", "text": text})
}

fn image_block(bytes: &[u8]) -> Result<Json> {
    let media_type = image_media_type(bytes).ok_or_else(|| {
        ProcGenAiError::SpecError("the reference image is not PNG, JPEG, GIF or WebP".to_string())
    })?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ProcGenAiError::SpecError(format!(
            "the reference image is {:.1} MB; the API takes at most {:.1} MB",
            bytes.len() as f64 / 1_048_576.0,
            MAX_IMAGE_BYTES as f64 / 1_048_576.0
        )));
    }
    Ok(json!({
        "type": "image",
        "source": {"type": "base64", "media_type": media_type, "data": base64(bytes)},
    }))
}

/// A string field, or each string in an array field.
fn strings(obj: &Map<String, Json>, key: &str) -> Vec<String> {
    match obj.get(key) {
        Some(Json::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Some(Json::Array(a)) => a
            .iter()
            .filter_map(Json::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| shorten(s, 600))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
