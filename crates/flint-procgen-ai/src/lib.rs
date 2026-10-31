//! AI-assisted procedural generation: agents that write and refine
//! [`flint_procgen::ProcGenSpec`]s.
//!
//! Tool-time only: nothing in the player links it. `flint-cli` does, for
//! `flint gen --refine`, and supplies the mesh [`Previewer`] this crate
//! can't (it depends on `flint-procgen` only, not on `flint-render`).
//!
//! - [`ProcGenAgent`] is the object-safe trait every agent implements.
//! - [`AnthropicAgent`] asks Claude through the Messages API, and holds
//!   every reply to the generator's schema and an actual build before it
//!   accepts it, sending failures back for another round. Its refine can
//!   see what a spec builds (a [`Previewer`] draws meshes) and keeps
//!   refining while the [`measure`]d distance to the reference falls.
//! - [`MockAgent`] is a local, deterministic agent for tests and pipelines
//!   with no network: its specs always fit the generator's param schema.
//! - [`schema`] checks and repairs a spec's params against that schema.

pub mod agent;
pub mod anthropic;
pub mod config;
pub mod error;
pub mod http;
pub mod measure;
pub mod mock;
pub mod preview;
pub mod prompt;
pub mod schema;

pub use agent::{ProcGenAgent, SpecFeedback};
pub use anthropic::{AnthropicAgent, Report, Round};
pub use config::AgentConfig;
pub use error::{ProcGenAiError, Result};
pub use http::{Transport, Usage};
pub use measure::Distance;
pub use mock::MockAgent;
pub use preview::Previewer;
pub use schema::{check_params, repair_params, IssueKind, ParamIssue};
