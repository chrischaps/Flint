//! One retry policy for every provider (#721: Flux, Meshy and ElevenLabs had
//! a copy each, and the Anthropic agent a fourth).
//!
//! A request is sent again after a network failure or a status that says
//! "later" ([`is_retryable_status`]), waiting `base_delay`, then twice that,
//! and so on, or the `retry-after` the provider named, never more than
//! `max_delay`. [`RetryPolicy::run`] is the loop for a caller that does more
//! than send per attempt (the Anthropic agent checks its budget and rebuilds
//! the body each time); [`send`] is the loop for a plain request.

use std::time::Duration;

use crate::http::{HttpRequest, HttpResponse, Transport};

/// A status worth sending the same request again for: timeout, rate
/// limits, server errors and Anthropic's 529 (overloaded).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// What a failed attempt says about the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Don't: the failure is final.
    No,
    /// Do, after the policy's backoff, or after this many seconds if the
    /// provider said.
    After(Option<u64>),
}

/// How many times to send, and how long to wait between.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Sends in all, the first included (at least 1).
    pub attempts: u32,
    /// The wait before the first retry; each later one doubles it.
    pub base_delay: Duration,
    /// No wait is longer, a provider's `retry-after` included.
    pub max_delay: Duration,
    /// How to wait (a no-op in tests).
    pub sleep: fn(Duration),
}

impl Default for RetryPolicy {
    /// Three sends, half a second then a second apart: what the asset
    /// providers always did.
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(60),
            sleep: std::thread::sleep,
        }
    }
}

impl RetryPolicy {
    /// `attempts` sends, `base_delay` before the first retry.
    pub fn new(attempts: u32, base_delay: Duration) -> Self {
        Self {
            attempts: attempts.max(1),
            base_delay,
            ..Self::default()
        }
    }

    /// Send once and never retry.
    pub fn once() -> Self {
        Self::new(1, Duration::ZERO)
    }

    /// The same policy, but waiting doesn't wait (tests).
    pub fn without_sleep(mut self) -> Self {
        self.sleep = |_| {};
        self
    }

    /// The wait after the `failed`-th failure (1-based).
    pub fn delay(&self, failed: u32, retry_after: Option<u64>) -> Duration {
        let d = match retry_after {
            Some(s) => Duration::from_secs(s),
            None => self
                .base_delay
                .saturating_mul(1u32 << failed.saturating_sub(1).min(16)),
        };
        d.min(self.max_delay)
    }

    /// Run `attempt` (given the 1-based attempt number) until it succeeds,
    /// `retry` says a failure is final, or the attempts run out; the last
    /// result is returned either way.
    pub fn run<T, E>(
        &self,
        mut attempt: impl FnMut(u32) -> Result<T, E>,
        mut retry: impl FnMut(&E) -> Retry,
    ) -> Result<T, E> {
        let mut n = 0;
        loop {
            n += 1;
            let result = attempt(n);
            let err = match result {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            match retry(&err) {
                Retry::After(after) if n < self.attempts.max(1) => {
                    (self.sleep)(self.delay(n, after));
                }
                _ => return Err(err),
            }
        }
    }
}

/// Why [`send`] gave up.
#[derive(Debug, Clone, PartialEq)]
pub enum SendError {
    /// No response arrived, `attempts` times.
    Network {
        /// The last failure.
        message: String,
        /// Sends made.
        attempts: u32,
    },
    /// A response that isn't a success, after `attempts` sends (one, for a
    /// status not worth retrying).
    Status {
        /// The last response.
        response: HttpResponse,
        /// Sends made.
        attempts: u32,
    },
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network { message, attempts } => {
                write!(f, "no response after {attempts} attempt(s): {message}")
            }
            Self::Status { response, attempts } => write!(
                f,
                "HTTP {} after {attempts} attempt(s): {}",
                response.status,
                crate::http::shorten(&response.text(), 300)
            ),
        }
    }
}

impl std::error::Error for SendError {}

/// Send `req` through `transport` under `policy`, returning the first 2xx.
pub fn send(
    transport: &dyn Transport,
    req: &HttpRequest,
    policy: &RetryPolicy,
) -> Result<HttpResponse, SendError> {
    policy.run(
        |attempts| match transport.send(req) {
            Ok(r) if r.is_success() => Ok(r),
            Ok(response) => Err(SendError::Status { response, attempts }),
            Err(message) => Err(SendError::Network { message, attempts }),
        },
        |e| match e {
            SendError::Network { .. } => Retry::After(None),
            SendError::Status { response, .. } if is_retryable_status(response.status) => {
                Retry::After(response.retry_after)
            }
            SendError::Status { .. } => Retry::No,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::http::ScriptedTransport;

    static WAITS: Mutex<Vec<Duration>> = Mutex::new(Vec::new());

    fn record(d: Duration) {
        WAITS.lock().unwrap().push(d);
    }

    #[test]
    fn delays_double_and_respect_retry_after_and_the_cap() {
        let p = RetryPolicy::new(5, Duration::from_millis(500));
        assert_eq!(p.delay(1, None), Duration::from_millis(500));
        assert_eq!(p.delay(2, None), Duration::from_secs(1));
        assert_eq!(p.delay(3, None), Duration::from_secs(2));
        assert_eq!(p.delay(1, Some(7)), Duration::from_secs(7));
        assert_eq!(p.delay(1, Some(600)), Duration::from_secs(60));
        assert_eq!(p.delay(40, None), Duration::from_secs(60));
    }

    #[test]
    fn send_retries_what_says_later_and_stops_on_what_says_no() {
        WAITS.lock().unwrap().clear();
        let t = ScriptedTransport::new(vec![
            Err("reset".into()),
            Ok(HttpResponse {
                retry_after: Some(3),
                ..HttpResponse::text_body(429, "slow down")
            }),
            Ok(HttpResponse::text_body(200, "ok")),
        ]);
        let policy = RetryPolicy {
            sleep: record,
            ..RetryPolicy::new(3, Duration::from_millis(500))
        };
        let r = send(&t, &HttpRequest::get("u"), &policy).unwrap();
        assert_eq!(r.text(), "ok");
        assert_eq!(t.requests().len(), 3);
        assert_eq!(
            *WAITS.lock().unwrap(),
            [Duration::from_millis(500), Duration::from_secs(3)]
        );

        // A 400 is final at once; a 503 gives up when the attempts run out.
        let t = ScriptedTransport::new(vec![Ok(HttpResponse::text_body(400, "bad"))]);
        let e = send(&t, &HttpRequest::get("u"), &policy.without_sleep()).unwrap_err();
        assert!(matches!(e, SendError::Status { attempts: 1, .. }), "{e}");
        let t = ScriptedTransport::new(vec![
            Ok(HttpResponse::text_body(503, "busy")),
            Ok(HttpResponse::text_body(503, "busy")),
            Ok(HttpResponse::text_body(503, "busy")),
        ]);
        let e = send(&t, &HttpRequest::get("u"), &policy.without_sleep()).unwrap_err();
        assert_eq!(e.to_string(), "HTTP 503 after 3 attempt(s): busy");
        assert_eq!(t.remaining(), 0);
    }

    #[test]
    fn once_sends_once() {
        let t = ScriptedTransport::new(vec![Err("down".into()), Err("down".into())]);
        let e = send(&t, &HttpRequest::get("u"), &RetryPolicy::once()).unwrap_err();
        assert!(matches!(e, SendError::Network { attempts: 1, .. }));
        assert_eq!(t.remaining(), 1);
    }
}
