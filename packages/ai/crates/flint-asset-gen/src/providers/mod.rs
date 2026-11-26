//! Provider registry
//!
//! Maps provider names to concrete implementations.

pub mod elevenlabs;
pub mod flux;
pub mod meshy;
pub mod mock;

use crate::calls::Calls;
use crate::config::FlintConfig;
use crate::provider::GenerationProvider;
use flint_core::{FlintError, Result};

/// Create a provider by name with configuration, every request it makes
/// going through `calls` (the mock makes none).
pub fn create_provider(
    name: &str,
    config: &FlintConfig,
    calls: &Calls,
) -> Result<Box<dyn GenerationProvider>> {
    match name {
        "mock" => Ok(Box::new(mock::MockProvider::new())),
        "flux" => Ok(Box::new(flux::FluxProvider::from_config(
            config,
            calls.clone(),
        )?)),
        "meshy" => Ok(Box::new(meshy::MeshyProvider::from_config(
            config,
            calls.clone(),
        )?)),
        "elevenlabs" => Ok(Box::new(elevenlabs::ElevenLabsProvider::from_config(
            config,
            calls.clone(),
        )?)),
        _ => Err(FlintError::GenerationError(format!(
            "Unknown provider '{}'. Available: mock, flux, meshy, elevenlabs",
            name
        ))),
    }
}

/// List all available provider names
pub fn available_providers() -> Vec<&'static str> {
    vec!["mock", "flux", "meshy", "elevenlabs"]
}

/// What the providers' tests share: a config naming a test key and URL,
/// calls over a scripted transport with no waiting, a request.
#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;
    use std::sync::Arc;

    use flint_ai_core::{CostMeter, RetryPolicy, ScriptedTransport};

    use crate::calls::Calls;
    use crate::config::{FlintConfig, ProviderConfig};
    use crate::provider::{AssetKind, GenerateRequest};

    pub fn test_config(provider: &str, url: &str) -> FlintConfig {
        let mut providers = HashMap::new();
        providers.insert(
            provider.to_string(),
            ProviderConfig {
                api_key: Some("test-key".into()),
                api_url: Some(url.into()),
                enabled: true,
            },
        );
        FlintConfig {
            providers,
            generation: Default::default(),
        }
    }

    /// Calls over `script`, a meter with `ceiling` and no ledger, and no
    /// waits; the meter, to read what it recorded.
    pub fn calls_over(script: &ScriptedTransport, ceiling: Option<f64>) -> (Calls, Arc<CostMeter>) {
        let mut meter = CostMeter::new("asset-gen").with_notify(|_| {});
        if let Some(c) = ceiling {
            meter = meter.with_ceiling(c);
        }
        let meter = Arc::new(meter);
        let calls = Calls::new(meter.clone())
            .with_transport(script.clone())
            .with_retry(RetryPolicy::default().without_sleep());
        (calls, meter)
    }

    pub fn request(name: &str, kind: AssetKind) -> GenerateRequest {
        GenerateRequest {
            name: name.into(),
            description: format!("a {name}"),
            kind,
            texture_params: (kind == AssetKind::Texture).then(Default::default),
            model_params: (kind == AssetKind::Model).then(Default::default),
            audio_params: (kind == AssetKind::Audio).then(Default::default),
            tags: vec![],
        }
    }

    pub fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint-asset-gen-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
