//! Provider registry
//!
//! Maps provider names to concrete implementations.

pub mod elevenlabs;
pub mod flux;
pub mod meshy;
pub mod mock;

use crate::config::FlintConfig;
use crate::provider::GenerationProvider;
use flint_core::{FlintError, Result};

/// Create a provider by name with configuration
pub fn create_provider(name: &str, config: &FlintConfig) -> Result<Box<dyn GenerationProvider>> {
    match name {
        "mock" => Ok(Box::new(mock::MockProvider::new())),
        "flux" => Ok(Box::new(flux::FluxProvider::from_config(config)?)),
        "meshy" => Ok(Box::new(meshy::MeshyProvider::from_config(config)?)),
        "elevenlabs" => Ok(Box::new(elevenlabs::ElevenLabsProvider::from_config(config)?)),
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
