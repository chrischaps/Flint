//! What every paid AI call in Flint's tools goes through (#721).
//!
//! Tool-time only: the CLI, `flint-procgen-ai` and `flint-asset-gen` link
//! it; the player does not.
//!
//! - [`http`]: the [`Transport`] seam (GET and POST, byte bodies), the real
//!   [`UreqTransport`], and a [`ScriptedTransport`] that replays canned
//!   responses, so a provider is tested end to end with no network.
//! - [`retry`]: one [`RetryPolicy`] for every provider: what is retried
//!   ([`retry::is_retryable_status`]), how long to wait, and the loop.
//! - [`cost`]: the [`PriceTable`] (data: `prices.toml`, under a project's
//!   `.flint/prices.toml`), the append-only [`CostLedger`]
//!   (`.flint/ai_costs.jsonl`), its [`CostSummary`], and the [`CostMeter`]
//!   that refuses a call before it could pass `--max-cost`.

pub mod cost;
pub mod http;
pub mod retry;

pub use cost::{
    CallPrice, CostEntry, CostError, CostLedger, CostMeter, CostSummary, Price, PriceTable, Usage,
};
pub use http::{HttpRequest, HttpResponse, Method, ScriptedTransport, Transport, UreqTransport};
pub use retry::{Retry, RetryPolicy, SendError};
