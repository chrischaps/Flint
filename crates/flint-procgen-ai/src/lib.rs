//! AI-assisted procedural generation: agents that write and refine
//! [`flint_procgen::ProcGenSpec`]s.
//!
//! Tool-time only. This crate is not a default workspace member and nothing
//! in the player links it; build it with `cargo build -p flint-procgen-ai`.
//!
//! - [`ProcGenAgent`] is the object-safe trait every agent implements.
//! - [`MockAgent`] is a local, deterministic agent for tests and pipelines
//!   with no network: its specs always fit the generator's param schema.
//! - [`schema`] checks and repairs a spec's params against that schema.

pub mod agent;
pub mod config;
pub mod error;
pub mod mock;
pub mod schema;

pub use agent::{ProcGenAgent, SpecFeedback};
pub use config::AgentConfig;
pub use error::{ProcGenAiError, Result};
pub use mock::MockAgent;
pub use schema::{check_params, repair_params, IssueKind, ParamIssue};
