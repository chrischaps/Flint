//! ElevenLabs audio generation provider
//!
//! Generates sound effects via the ElevenLabs sound generation API. Audio
//! is fast (~3-10s), so `generate()` blocks. A call is priced by the
//! duration it asks for, checked against the budget before it is sent and
//! recorded once ElevenLabs answers.

use crate::calls::{cost_error, endpoint_label, Calls};
use crate::config::FlintConfig;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_ai_core::HttpRequest;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_ELEVENLABS_URL: &str = "https://api.elevenlabs.io/v1/sound-generation";

/// The duration asked for when a request names none, in seconds.
const DEFAULT_DURATION: f64 = 3.0;

/// ElevenLabs provider for AI audio/sound effect generation
pub struct ElevenLabsProvider {
    api_key: String,
    api_url: String,
    calls: Calls,
}

impl ElevenLabsProvider {
    /// Create a new ElevenLabsProvider from config, its calls through `calls`
    pub fn from_config(config: &FlintConfig, calls: Calls) -> Result<Self> {
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

        Ok(Self {
            api_key,
            api_url,
            calls,
        })
    }

    fn duration(request: &GenerateRequest) -> f64 {
        request
            .audio_params
            .as_ref()
            .map(|p| p.duration)
            .unwrap_or(DEFAULT_DURATION)
    }
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

    fn billed_units(&self, request: &GenerateRequest) -> f64 {
        Self::duration(request)
    }

    fn generate(
        &self,
        request: &GenerateRequest,
        style: Option<&StyleGuide>,
        output_dir: &Path,
    ) -> Result<GenerateResult> {
        let start = std::time::Instant::now();
        let prompt = self.build_prompt(request, style);
        let duration = Self::duration(request);
        std::fs::create_dir_all(output_dir)?;

        self.calls
            .meter
            .admit_call("elevenlabs", duration)
            .map_err(cost_error)?;
        let req = HttpRequest::post_json(
            &self.api_url,
            &serde_json::json!({ "text": prompt, "duration_seconds": duration }),
        )
        .with_header("xi-api-key", &self.api_key);
        let audio = self.calls.request(&req, "ElevenLabs API request")?;
        self.calls.meter.record_call(
            "elevenlabs",
            &endpoint_label(&self.api_url, "elevenlabs"),
            duration,
        );

        // ElevenLabs returns MP3 by default
        let output_path = output_dir.join(format!("{}.mp3", request.name));
        std::fs::write(&output_path, &audio.body)?;

        let hash = flint_core::ContentHash::from_file(&output_path)
            .map(|h| h.to_prefixed_hex())
            .ok();

        Ok(GenerateResult {
            output_path: output_path.to_string_lossy().to_string(),
            prompt_used: prompt,
            provider: "elevenlabs".to_string(),
            duration_secs: start.elapsed().as_secs_f64(),
            content_hash: hash,
            metadata: HashMap::new(),
        })
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
    use crate::providers::test_support::{calls_over, request, temp_dir, test_config};
    use flint_ai_core::{HttpResponse, ScriptedTransport};

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

    #[test]
    fn a_sound_is_priced_by_its_duration() {
        let script = ScriptedTransport::new(vec![Ok(HttpResponse::bytes(b"ID3 fake".to_vec()))]);
        let (calls, meter) = calls_over(&script, None);
        let el = ElevenLabsProvider::from_config(
            &test_config("elevenlabs", "http://el.test/v1/sound-generation"),
            calls,
        )
        .unwrap();
        let dir = temp_dir("eleven");
        let mut req = request("door_creak", AssetKind::Audio);
        req.audio_params = Some(AudioParams {
            duration: 5.0,
            seed: None,
        });
        let result = el.generate(&req, None, &dir).unwrap();
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"ID3 fake");
        let sent = &script.requests()[0];
        assert_eq!(sent.header("xi-api-key"), Some("test-key"));
        let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(body["duration_seconds"], 5.0);
        // 5 s at $0.002.
        let e = &meter.entries()[0];
        assert_eq!((e.units, e.unit.as_deref()), (Some(5.0), Some("second")));
        assert!((e.estimated_usd - 0.01).abs() < 1e-12);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_auth_failure_is_not_retried_or_recorded() {
        let script = ScriptedTransport::new(vec![Ok(HttpResponse::text_body(
            401,
            r#"{"detail":{"message":"Invalid API key"}}"#,
        ))]);
        let (calls, meter) = calls_over(&script, None);
        let el =
            ElevenLabsProvider::from_config(&test_config("elevenlabs", "http://el.test"), calls)
                .unwrap();
        let dir = temp_dir("eleven-401");
        let e = el
            .generate(&request("x", AssetKind::Audio), None, &dir)
            .unwrap_err();
        assert!(e.to_string().contains("HTTP 401 after 1 attempt(s)"), "{e}");
        assert!(e.to_string().contains("Invalid API key"), "{e}");
        assert_eq!(meter.calls(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
