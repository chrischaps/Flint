//! AI-assisted procedural generation: agents that write and refine
//! [`flint_procgen::ProcGenSpec`]s.
//!
//! Tool-time only. This crate is not a default workspace member and nothing
//! in the player links it (the CLI does, for `flint gen --ai-*`); build it
//! alone with `cargo build -p flint-procgen-ai`.
//!
//! - [`ProcGenAgent`] is the object-safe trait every agent implements.
//! - [`AnthropicAgent`] asks Claude through the Messages API, and holds
//!   every reply to the generator's schema and an actual build before it
//!   accepts it, sending failures back for another round.
//! - [`MockAgent`] is a local, deterministic agent for tests and pipelines
//!   with no network: its specs always fit the generator's param schema.
//! - [`cost`] prices every call, keeps the append-only ledger at
//!   `.flint/procgen_costs.json`, and enforces a cost ceiling before each
//!   request ([`CostMeter`]).
//! - [`schema`] checks and repairs a spec's params against that schema.

pub mod agent;
pub mod anthropic;
pub mod config;
pub mod cost;
pub mod error;
pub mod http;
pub mod mock;
pub mod prompt;
pub mod schema;

pub use agent::{ProcGenAgent, Report, Round, SpecFeedback};
pub use anthropic::AnthropicAgent;
pub use config::AgentConfig;
pub use cost::{CostEntry, CostLedger, CostMeter, CostSummary};
pub use error::{ProcGenAiError, Result};
pub use http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, Usage};
pub use mock::MockAgent;
pub use schema::{check_params, repair_params, IssueKind, ParamIssue};
