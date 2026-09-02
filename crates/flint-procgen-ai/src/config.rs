use serde::{Deserialize, Serialize};

/// Configuration for a procedural generation AI agent.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentConfig {
    /// Provider name (e.g. "anthropic", "mock").
    pub provider: String,
    /// Model identifier (e.g. "claude-sonnet-4-6").
    pub model: String,
    /// Maximum refinement iterations before stopping.
    pub max_iterations: u32,
    /// API key for the provider.
    pub api_key: String,
}
