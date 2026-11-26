use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{ProcGenAiError, Result};

/// Configuration for a procedural generation AI agent.
///
/// The API key is never written out: `Debug` redacts it and serialization
/// skips it, so a config can be logged or saved beside a project without
/// leaking the secret. Load the key from the environment instead.
#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub struct AgentConfig {
    /// Provider name (e.g. "anthropic", "mock").
    pub provider: String,
    /// Model identifier (e.g. "claude-sonnet-5-5").
    pub model: String,
    /// Maximum refinement iterations before stopping.
    pub max_iterations: u32,
    /// API key for the provider. Not serialized.
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// Reasoning effort (`low` ... `max`) for providers that take one;
    /// `None` uses the model's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl AgentConfig {
    /// A config for [`crate::MockAgent`]: no key, three iterations.
    pub fn mock() -> Self {
        Self {
            provider: "mock".to_string(),
            model: "mock".to_string(),
            max_iterations: 3,
            api_key: String::new(),
            effort: None,
        }
    }

    /// A config for [`crate::AnthropicAgent`]: [`crate::anthropic::DEFAULT_MODEL`],
    /// three iterations, and no key (the agent reads `ANTHROPIC_API_KEY`).
    pub fn anthropic() -> Self {
        Self {
            provider: "anthropic".to_string(),
            model: crate::anthropic::DEFAULT_MODEL.to_string(),
            max_iterations: 3,
            api_key: String::new(),
            effort: None,
        }
    }

    /// Check the config is usable: a provider and model are named, at least
    /// one iteration is allowed, and any provider but `mock` has a key.
    pub fn validate(&self) -> Result<()> {
        let err = |m: &str| Err(ProcGenAiError::ConfigError(m.to_string()));
        if self.provider.trim().is_empty() {
            return err("provider is empty");
        }
        if self.model.trim().is_empty() {
            return err("model is empty");
        }
        if self.max_iterations == 0 {
            return err("max_iterations must be at least 1");
        }
        if self.provider != "mock" && self.api_key.trim().is_empty() {
            return Err(ProcGenAiError::ConfigError(format!(
                "provider '{}' needs an api_key",
                self.provider
            )));
        }
        Ok(())
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::mock()
    }
}

impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = if self.api_key.is_empty() {
            "<none>"
        } else {
            "<redacted>"
        };
        f.debug_struct("AgentConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("max_iterations", &self.max_iterations)
            .field("api_key", &key)
            .field("effort", &self.effort)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> AgentConfig {
        AgentConfig {
            provider: "anthropic".into(),
            model: "claude-sonnet-5-5".into(),
            max_iterations: 4,
            api_key: "sk-secret-123".into(),
            effort: None,
        }
    }

    #[test]
    fn debug_never_shows_the_key() {
        let shown = format!("{:?}", real());
        assert!(!shown.contains("sk-secret"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }

    #[test]
    fn serialization_drops_the_key() {
        let text = toml::to_string(&real()).unwrap();
        assert!(!text.contains("sk-secret"), "{text}");
        let back: AgentConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.api_key, "");
        assert_eq!(back.model, "claude-sonnet-5-5");
    }

    #[test]
    fn validation() {
        assert!(AgentConfig::mock().validate().is_ok());
        assert!(real().validate().is_ok());
        let mut keyless = real();
        keyless.api_key.clear();
        assert!(keyless.validate().is_err());
        let mut zero = AgentConfig::mock();
        zero.max_iterations = 0;
        assert!(zero.validate().is_err());
    }
}
