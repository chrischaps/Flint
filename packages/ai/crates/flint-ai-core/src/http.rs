//! The HTTP seam every paid call goes through: a one-method [`Transport`],
//! the real `ureq` implementation, and a [`ScriptedTransport`] that replays
//! canned responses so a provider can be tested end to end with no network.
//!
//! A transport never turns a status into an error: a 429 or a 500 comes back
//! as an [`HttpResponse`], and the caller (usually [`crate::retry`]) decides.
//! `Err` is only for a request that got no response at all.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// GET or POST: the two verbs the providers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// No body.
    Get,
    /// A JSON body.
    Post,
}

/// An outgoing request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// GET or POST.
    pub method: Method,
    /// Full URL.
    pub url: String,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// The body (empty for a GET).
    pub body: String,
}

impl HttpRequest {
    /// A POST of `body` (JSON) to `url`, with `content-type: application/json`.
    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
        }
    }

    /// A GET of `url`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// This request with one more header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The value of a header, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A response with any status.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    /// HTTP status.
    pub status: u16,
    /// The `retry-after` header in seconds, if present and numeric.
    pub retry_after: Option<u64>,
    /// The body's bytes (an image, a GLB, an MP3, or JSON text).
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response of `status` whose body is `text`.
    pub fn text_body(status: u16, text: impl Into<String>) -> Self {
        Self {
            status,
            retry_after: None,
            body: text.into().into_bytes(),
        }
    }

    /// A 200 whose body is `bytes`.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            retry_after: None,
            body: bytes.into(),
        }
    }

    /// The body as text (lossy: an invalid byte becomes U+FFFD).
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends a request and returns whatever came back. `Err` only when no
/// response arrived (connection, TLS, timeout).
pub trait Transport: Send + Sync {
    /// Send `req`.
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, String>;
}

impl<T: Transport + ?Sized> Transport for Arc<T> {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        (**self).send(req)
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        (**self).send(req)
    }
}

/// The real transport: a blocking `ureq` agent whose whole request may take
/// up to the timeout it was made with.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    /// A transport whose whole request (body included) may take `timeout`.
    pub fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Default for UreqTransport {
    /// Ten minutes: a thinking model can take minutes on a hard request, and
    /// a large GLB is a long download.
    fn default() -> Self {
        Self::new(Duration::from_secs(600))
    }
}

impl Transport for UreqTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        let result = match req.method {
            Method::Get => {
                let mut b = self.agent.get(&req.url);
                for (k, v) in &req.headers {
                    b = b.header(k, v);
                }
                b.call()
            }
            Method::Post => {
                let mut b = self.agent.post(&req.url);
                for (k, v) in &req.headers {
                    b = b.header(k, v);
                }
                b.send(req.body.as_str())
            }
        };
        let mut resp = result.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse().ok());
        let body = resp
            .body_mut()
            .with_config()
            .limit(512 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

/// A [`Transport`] that replays canned responses in order and keeps every
/// request it was sent: for testing code that makes paid calls, with no
/// network. Clones share the script and the record.
#[derive(Clone, Default)]
pub struct ScriptedTransport {
    responses: Arc<Mutex<VecDeque<Result<HttpResponse, String>>>>,
    sent: Arc<Mutex<Vec<HttpRequest>>>,
}

impl ScriptedTransport {
    /// A transport that answers with `responses`, one per request.
    pub fn new(responses: Vec<Result<HttpResponse, String>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            sent: Arc::default(),
        }
    }

    /// A 200 whose body is `value` as JSON.
    pub fn json(value: serde_json::Value) -> Result<HttpResponse, String> {
        Ok(HttpResponse::text_body(200, value.to_string()))
    }

    /// A 200 Messages API reply whose one text block is `text`, from
    /// `model`, with the given token counts.
    pub fn reply(
        model: &str,
        text: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<HttpResponse, String> {
        Self::json(serde_json::json!({
            "id": "msg_scripted",
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens},
        }))
    }

    /// Every request sent so far.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.sent.lock().expect("transport lock").clone()
    }

    /// Responses not yet sent.
    pub fn remaining(&self) -> usize {
        self.responses.lock().expect("transport lock").len()
    }
}

impl Transport for ScriptedTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        self.sent.lock().expect("transport lock").push(req.clone());
        self.responses
            .lock()
            .expect("transport lock")
            .pop_front()
            .unwrap_or_else(|| Err("the script has no more responses".to_string()))
    }
}

/// `s` cut to `max` characters with an ellipsis, for error messages.
pub fn shorten(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_build_and_read_headers() {
        let r = HttpRequest::post_json("http://x/y", &serde_json::json!({"a": 1}))
            .with_header("Authorization", "Key k");
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.header("authorization"), Some("Key k"));
        assert_eq!(r.header("Content-Type"), Some("application/json"));
        assert_eq!(r.body, r#"{"a":1}"#);
        assert!(HttpRequest::get("http://x").body.is_empty());
    }

    #[test]
    fn the_script_replays_in_order_then_fails() {
        let t = ScriptedTransport::new(vec![
            Ok(HttpResponse::bytes(vec![1, 2, 3])),
            Err("down".into()),
        ]);
        assert_eq!(t.send(&HttpRequest::get("a")).unwrap().body, vec![1, 2, 3]);
        assert_eq!(t.send(&HttpRequest::get("b")).unwrap_err(), "down");
        assert!(t.send(&HttpRequest::get("c")).is_err());
        let urls: Vec<_> = t.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(urls, ["a", "b", "c"]);
    }

    #[test]
    fn a_text_body_reads_back() {
        let r = HttpResponse::text_body(503, "busy");
        assert_eq!(r.text(), "busy");
        assert!(!r.is_success());
        assert_eq!(shorten("abcdef", 3), "abc...");
    }
}
