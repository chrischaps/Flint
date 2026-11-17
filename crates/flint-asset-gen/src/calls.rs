//! What a provider's paid calls go through (#721): one transport, one retry
//! policy, one cost meter and, for a provider whose generations take
//! minutes, the job store.
//!
//! A provider never builds an HTTP client of its own. It asks the meter
//! before the call that costs money ([`CostMeter::admit_call`], with the
//! units the call counts: megapixels, seconds, tasks), sends through
//! [`Calls::request`], and records what it spent the moment the provider has
//! accepted the call ([`CostMeter::record_call`]), so a generation that fails
//! after it was billed is still in the ledger.

use std::sync::Arc;
use std::time::Duration;

use flint_ai_core::http::{shorten, HttpRequest, HttpResponse, ScriptedTransport, Transport};
use flint_ai_core::retry::{self, RetryPolicy};
use flint_ai_core::{CostError, CostMeter, UreqTransport};
use flint_core::{FlintError, Result};

use crate::job::JobStore;

/// Seconds between a long job's status checks.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Status checks before a long job is given up on (half an hour at
/// [`POLL_INTERVAL`]). The job record stays, so it can be resumed.
pub const MAX_POLLS: u32 = 180;

/// The shared context of every provider's calls.
#[derive(Clone)]
pub struct Calls {
    /// Where requests go.
    pub transport: Arc<dyn Transport>,
    /// What every paid call is checked against and recorded in.
    pub meter: Arc<CostMeter>,
    /// How a request is retried.
    pub retry: RetryPolicy,
    /// Where a long job's record is kept, if anywhere.
    pub jobs: Option<JobStore>,
    /// Between a long job's status checks.
    pub poll_interval: Duration,
    /// Status checks before giving up.
    pub max_polls: u32,
}

impl std::fmt::Debug for Calls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Calls")
            .field("meter", &self.meter)
            .field("retry", &self.retry)
            .field("jobs", &self.jobs)
            .finish_non_exhaustive()
    }
}

impl Calls {
    /// The real network, `meter`, the default retry policy, no job store.
    pub fn new(meter: Arc<CostMeter>) -> Self {
        Self {
            transport: Arc::new(UreqTransport::default()),
            meter,
            retry: RetryPolicy::default(),
            jobs: None,
            poll_interval: POLL_INTERVAL,
            max_polls: MAX_POLLS,
        }
    }

    /// No network and no ledger: every request fails. For the mock provider
    /// and tests that never reach a paid call.
    pub fn offline() -> Self {
        Self::new(Arc::new(CostMeter::new("mock").with_notify(|_| {})))
            .with_transport(ScriptedTransport::default())
    }

    /// Send through `transport` instead (a scripted one in tests).
    pub fn with_transport(mut self, transport: impl Transport + 'static) -> Self {
        self.transport = Arc::new(transport);
        self
    }

    /// Keep long jobs' records in `jobs`.
    pub fn with_jobs(mut self, jobs: JobStore) -> Self {
        self.jobs = Some(jobs);
        self
    }

    /// Retry by `retry`.
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Check a long job every `interval`, at most `max_polls` times.
    pub fn with_polling(mut self, interval: Duration, max_polls: u32) -> Self {
        self.poll_interval = interval;
        self.max_polls = max_polls.max(1);
        self
    }

    /// Wait as the retry policy waits (not at all in a test).
    pub fn sleep(&self, d: Duration) {
        (self.retry.sleep)(d);
    }

    /// Send `req` under the retry policy; anything but a 2xx in the end is
    /// an error naming `what`.
    pub fn request(&self, req: &HttpRequest, what: &str) -> Result<HttpResponse> {
        retry::send(&*self.transport, req, &self.retry)
            .map_err(|e| FlintError::GenerationError(format!("{what} failed: {e}")))
    }

    /// [`Self::request`], its body parsed as JSON.
    pub fn request_json(&self, req: &HttpRequest, what: &str) -> Result<serde_json::Value> {
        let resp = self.request(req, what)?;
        serde_json::from_slice(&resp.body).map_err(|e| {
            FlintError::GenerationError(format!(
                "{what}: the response is not JSON ({e}): {}",
                shorten(&resp.text(), 200)
            ))
        })
    }
}

/// A refusal from the meter as the error a provider returns.
pub fn cost_error(e: CostError) -> FlintError {
    FlintError::GenerationError(e.to_string())
}

/// The path of an endpoint URL, for the ledger's `model`
/// (`https://fal.run/fal-ai/flux/dev` -> `fal-ai/flux/dev`); `fallback` when
/// it has none.
pub fn endpoint_label(url: &str, fallback: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.split_once('/').map_or("", |(_, p)| p);
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    if path.is_empty() {
        fallback.to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_labelled_by_their_path() {
        assert_eq!(
            endpoint_label("https://queue.fal.run/fal-ai/flux/dev", "flux"),
            "fal-ai/flux/dev"
        );
        assert_eq!(
            endpoint_label("http://127.0.0.1:9/v1/sound-generation?x=1", "e"),
            "v1/sound-generation"
        );
        assert_eq!(endpoint_label("http://127.0.0.1:9", "meshy"), "meshy");
    }

    #[test]
    fn offline_calls_fail_without_a_network() {
        let e = Calls::offline()
            .with_retry(RetryPolicy::once())
            .request(&HttpRequest::get("http://x"), "Probe")
            .unwrap_err();
        assert!(e.to_string().contains("Probe failed"), "{e}");
    }
}
