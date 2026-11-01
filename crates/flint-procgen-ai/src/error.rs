use thiserror::Error;

/// Errors specific to AI-assisted procedural generation.
#[derive(Debug, Error)]
pub enum ProcGenAiError {
    /// Invalid or malformed spec.
    #[error("spec error: {0}")]
    SpecError(String),

    /// Agent communication or processing failure.
    #[error("agent error: {0}")]
    AgentError(String),

    /// Configuration issue (missing API key, bad model name, etc.).
    #[error("config error: {0}")]
    ConfigError(String),

    /// The provider rejected the credentials (HTTP 401 / 403).
    #[error("authentication failed ({status}): {message}")]
    Auth {
        /// HTTP status.
        status: u16,
        /// The provider's message.
        message: String,
    },

    /// Rate limited (HTTP 429) or overloaded (HTTP 529), after retries.
    #[error("{} after {attempts} attempt(s){}: {message}", if *status == 529 { "provider overloaded" } else { "rate limited" }, retry_after.map(|s| format!("; retry in {s}s")).unwrap_or_default())]
    RateLimited {
        /// HTTP status (429 or 529).
        status: u16,
        /// Seconds the provider asked us to wait, if it said.
        retry_after: Option<u64>,
        /// How many requests were sent.
        attempts: u32,
        /// The provider's message.
        message: String,
    },

    /// Any other non-success HTTP response.
    #[error("API error {status} ({kind}): {message}")]
    Api {
        /// HTTP status.
        status: u16,
        /// The provider's error type (`invalid_request_error`, ...).
        kind: String,
        /// The provider's message.
        message: String,
    },

    /// The request never got a response (DNS, TLS, timeout...).
    #[error("network error: {0}")]
    Network(String),

    /// The response arrived but could not be used: not JSON, no JSON
    /// object in the text, cut off at `max_tokens`, or a refusal.
    #[error("unusable response: {0}")]
    BadResponse(String),

    /// The next call could take the run past its cost ceiling, so it was
    /// not sent.
    #[error("cost ceiling reached: {} spent, and the next call could cost up to {}, past --max-cost {}; nothing more was sent", crate::cost::usd(*spent), crate::cost::usd(*next), crate::cost::usd(*max))]
    CostCeiling {
        /// Dollars spent so far this run.
        spent: f64,
        /// The next call's estimated worst case (smallest useful reply).
        next: f64,
        /// The ceiling.
        max: f64,
    },

    /// An error from the procgen subsystem itself.
    #[error(transparent)]
    ProcGen(#[from] flint_procgen::ProcGenError),
}

impl ProcGenAiError {
    /// Whether sending the same request again might succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Network(_))
            || matches!(self, Self::Api { status, .. } if *status >= 500)
    }
}

pub type Result<T> = std::result::Result<T, ProcGenAiError>;
