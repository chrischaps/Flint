//! Flux texture generation provider (fal.ai)
//!
//! Generates textures via the Flux image generation API. Textures are fast
//! (~10s), so `generate()` blocks. fal bills per megapixel, each image
//! rounded up ([`megapixels`]); the call is checked against the budget
//! before it is sent and recorded once fal answers.

use crate::calls::{cost_error, endpoint_label, Calls};
use crate::config::FlintConfig;
use crate::provider::*;
use crate::style::StyleGuide;
use flint_ai_core::HttpRequest;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_FLUX_URL: &str = "https://queue.fal.run/fal-ai/flux/dev";

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
        let req = HttpRequest::post_json(&self.api_url, &payload)
            .with_header("Authorization", format!("Key {}", self.api_key));
        self.calls.request_json(&req, "Flux API request")
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
        let start = std::time::Instant::now();
        let prompt = self.build_prompt(request, style);
        let params = request.texture_params.clone().unwrap_or_default();
        std::fs::create_dir_all(output_dir)?;

        let units = self.billed_units(request);
        self.calls
            .meter
            .admit_call("flux", units)
            .map_err(cost_error)?;
        let response = self.submit(&prompt, params.width, params.height, params.seed)?;
        // fal answered with an image: it's billed, whatever happens next.
        self.calls
            .meter
            .record_call("flux", &endpoint_label(&self.api_url, "flux"), units);

        let image_url = image_url(&response).ok_or_else(|| {
            FlintError::GenerationError(format!(
                "Unexpected Flux response format: {}",
                serde_json::to_string_pretty(&response).unwrap_or_default()
            ))
        })?;

        let output_path = output_dir.join(format!("{}.png", request.name));
        let image = self
            .calls
            .request(&HttpRequest::get(image_url), "Flux image download")?;
        std::fs::write(&output_path, &image.body)?;

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
            duration_secs: start.elapsed().as_secs_f64(),
            content_hash: hash,
            metadata,
        })
    }

    fn build_prompt(&self, request: &GenerateRequest, style: Option<&StyleGuide>) -> String {
        match style {
            Some(s) => s.enrich_prompt(&request.description),
            None => request.description.clone(),
        }
    }
}

/// The first image's URL in a Flux response.
fn image_url(response: &serde_json::Value) -> Option<&str> {
    response
        .get("images")
        .and_then(|imgs| imgs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
}

/// Parse a Flux API response for testing
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
    use crate::providers::test_support::{calls_over, request, temp_dir, test_config};
    use flint_ai_core::{HttpResponse, Method, ScriptedTransport};

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
    fn megapixels_round_up() {
        assert_eq!(megapixels(1000, 1000), 1.0);
        assert_eq!(megapixels(1024, 1024), 2.0);
        assert_eq!(megapixels(512, 512), 1.0);
        assert_eq!(megapixels(2048, 2048), 5.0);
    }

    #[test]
    fn a_generate_is_admitted_sent_recorded_and_downloaded() {
        let script = ScriptedTransport::new(vec![
            // A 503, retried, then fal's answer and the image.
            Ok(HttpResponse::text_body(503, "busy")),
            ScriptedTransport::json(serde_json::json!({
                "images": [{"url": "http://fal.test/img.png"}], "seed": 7
            })),
            Ok(HttpResponse::bytes(b"\x89PNG fake".to_vec())),
        ]);
        let (calls, meter) = calls_over(&script, None);
        let flux = FluxProvider::from_config(
            &test_config("flux", "http://fal.test/fal-ai/flux/dev"),
            calls,
        )
        .unwrap();
        let dir = temp_dir("flux");
        let result = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap();
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"\x89PNG fake");
        assert_eq!(result.metadata["seed"], "7");

        let sent = script.requests();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[1].header("authorization"), Some("Key test-key"));
        let body: serde_json::Value = serde_json::from_str(&sent[1].body).unwrap();
        assert_eq!(body["image_size"]["width"], 1024);
        assert_eq!(sent[2].method, Method::Get);
        assert_eq!(sent[2].url, "http://fal.test/img.png");

        // One call, 2 megapixels at $0.025.
        let entries = meter.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider, "flux");
        assert_eq!(entries[0].model, "fal-ai/flux/dev");
        assert_eq!(entries[0].units, Some(2.0));
        assert!((entries[0].estimated_usd - 0.05).abs() < 1e-12);
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
    fn a_billed_call_whose_download_fails_is_still_recorded() {
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::json(serde_json::json!({"images": [{"url": "http://fal.test/i"}]})),
            Ok(HttpResponse::text_body(404, "gone")),
        ]);
        let (calls, meter) = calls_over(&script, None);
        let flux =
            FluxProvider::from_config(&test_config("flux", "http://fal.test/x"), calls).unwrap();
        let dir = temp_dir("flux-gone");
        let e = flux
            .generate(&request("brick", AssetKind::Texture), None, &dir)
            .unwrap_err();
        assert!(e.to_string().contains("Flux image download failed"), "{e}");
        assert_eq!(meter.calls(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
