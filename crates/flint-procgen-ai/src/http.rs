//! The HTTP seam under [`crate::AnthropicAgent`]: a one-method
//! [`Transport`] trait, the real `ureq` implementation, and the mapping from
//! a Messages API response (or error) to Rust values.
//!
//! Tests swap the transport for a scripted one, so every request the agent
//! builds can be inspected and every response, good or broken, replayed.

use std::time::Duration;

use serde_json::Value as Json;

use crate::error::{ProcGenAiError, Result};

/// An outgoing POST.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// Full URL.
    pub url: String,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// JSON body.
    pub body: String,
}

impl HttpRequest {
    /// The value of a header, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A response with any status. Transports never turn a status into an error.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status.
    pub status: u16,
    /// The `retry-after` header in seconds, if present and numeric.
    pub retry_after: Option<u64>,
    /// Response body.
    pub body: String,
}

/// Sends a POST and returns whatever came back. `Err` only for failures
/// where no response arrived (connection, TLS, timeout).
pub trait Transport: Send + Sync {
    /// Send `req`.
    fn post(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, String>;
}

/// The real transport: a blocking `ureq` agent with a long timeout, since a
/// thinking model can take minutes on a hard spec.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    /// A transport whose whole request may take up to `timeout`.
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
    fn default() -> Self {
        Self::new(Duration::from_secs(600))
    }
}

impl Transport for UreqTransport {
    fn post(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, String> {
        let mut builder = self.agent.post(&req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let mut resp = builder.send(req.body.as_str()).map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse().ok());
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok(HttpResponse {
            status,
            retry_after,
            body,
        })
    }
}

/// Token counts from one response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens (thinking included).
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_creation_input_tokens: u64,
}

impl Usage {
    fn of(v: &Json) -> Self {
        let n = |k: &str| v.get(k).and_then(Json::as_u64).unwrap_or(0);
        Self {
            input_tokens: n("input_tokens"),
            output_tokens: n("output_tokens"),
            cache_read_input_tokens: n("cache_read_input_tokens"),
            cache_creation_input_tokens: n("cache_creation_input_tokens"),
        }
    }

    /// Sum of two usages.
    pub fn plus(self, o: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + o.input_tokens,
            output_tokens: self.output_tokens + o.output_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens + o.cache_read_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens
                + o.cache_creation_input_tokens,
        }
    }
}

/// A successful Messages API reply.
#[derive(Debug, Clone)]
pub struct Reply {
    /// The `content` array exactly as returned, thinking blocks included,
    /// so it can be sent back unchanged as the assistant turn.
    pub content: Json,
    /// The text blocks joined.
    pub text: String,
    /// The model that answered (differs from the request after a fallback).
    pub model: String,
    /// Token counts.
    pub usage: Usage,
}

/// The `error.type` and `error.message` of an API error body, or the raw
/// body (shortened) when it isn't the documented shape.
fn error_parts(body: &str) -> (String, String) {
    let parsed: Option<Json> = serde_json::from_str(body).ok();
    let err = parsed.as_ref().and_then(|j| j.get("error"));
    let field = |k: &str| err.and_then(|e| e.get(k)).and_then(Json::as_str);
    match (field("type"), field("message")) {
        (Some(t), Some(m)) => (t.to_string(), m.to_string()),
        _ => ("unknown".to_string(), shorten(body, 300)),
    }
}

/// `s` cut to `max` characters with an ellipsis.
pub(crate) fn shorten(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}...")
    }
}

/// Turn a response into a [`Reply`] or a typed error. `attempts` is how many
/// requests have been sent so far, for the rate-limit message.
pub fn parse_response(resp: &HttpResponse, attempts: u32) -> Result<Reply> {
    if !(200..300).contains(&resp.status) {
        let (kind, message) = error_parts(&resp.body);
        return Err(match resp.status {
            401 | 403 => ProcGenAiError::Auth {
                status: resp.status,
                message: if resp.status == 401 {
                    format!("{message} (check ANTHROPIC_API_KEY or AgentConfig::api_key)")
                } else {
                    message
                },
            },
            429 | 529 => ProcGenAiError::RateLimited {
                status: resp.status,
                retry_after: resp.retry_after,
                attempts,
                message,
            },
            status => ProcGenAiError::Api {
                status,
                kind,
                message,
            },
        });
    }

    let v: Json = serde_json::from_str(&resp.body).map_err(|e| {
        ProcGenAiError::BadResponse(format!(
            "body is not JSON ({e}): {}",
            shorten(&resp.body, 200)
        ))
    })?;
    let stop = v.get("stop_reason").and_then(Json::as_str).unwrap_or("");
    if stop == "refusal" {
        let details = v.get("stop_details");
        let category = details
            .and_then(|d| d.get("category"))
            .and_then(Json::as_str)
            .unwrap_or("unspecified");
        let why = details
            .and_then(|d| d.get("explanation"))
            .and_then(Json::as_str)
            .unwrap_or("no explanation given");
        return Err(ProcGenAiError::BadResponse(format!(
            "the model declined (category {category}): {why}"
        )));
    }
    let content = v
        .get("content")
        .filter(|c| c.is_array())
        .cloned()
        .ok_or_else(|| {
            ProcGenAiError::BadResponse(format!("no content array: {}", shorten(&resp.body, 200)))
        })?;
    if stop == "max_tokens" {
        return Err(ProcGenAiError::BadResponse(
            "the reply hit max_tokens before it finished; raise AnthropicAgent::max_tokens"
                .to_string(),
        ));
    }
    let text = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(Json::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Json::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Reply {
        content,
        text,
        model: v
            .get("model")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        usage: v.get("usage").map(Usage::of).unwrap_or_default(),
    })
}

/// The first JSON object in `text`, allowing prose or a ```json fence
/// around it. The reply contract asks for bare JSON; this is the slack.
pub fn extract_json_object(text: &str) -> Result<serde_json::Map<String, Json>> {
    let mut last_err = None;
    for (i, _) in text.match_indices('{') {
        let mut stream = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Json>();
        match stream.next() {
            Some(Ok(Json::Object(map))) => return Ok(map),
            Some(Err(e)) => last_err = Some(e.to_string()),
            _ => {}
        }
    }
    Err(ProcGenAiError::BadResponse(match last_err {
        Some(e) => format!(
            "reply has no valid JSON object ({e}): {}",
            shorten(text, 200)
        ),
        None => format!("reply has no JSON object: {}", shorten(text, 200)),
    }))
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The image media type of `bytes` from its magic number, for the formats
/// the Messages API accepts.
pub fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            retry_after: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input}");
        }
    }

    #[test]
    fn media_types_from_magic() {
        assert_eq!(
            image_media_type(b"\x89PNG\r\n\x1a\n...."),
            Some("image/png")
        );
        assert_eq!(image_media_type(b"\xFF\xD8\xFF\xE0"), Some("image/jpeg"));
        assert_eq!(
            image_media_type(b"RIFF\0\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(image_media_type(b"BM...."), None);
    }

    #[test]
    fn json_found_inside_prose_and_fences() {
        let m = extract_json_object("Sure!\n```json\n{\"a\": {\"b\": 1}}\n```\nDone.").unwrap();
        assert_eq!(m["a"]["b"], 1);
        // A brace in prose before the object is skipped.
        let m = extract_json_object("use {braces} like {\"x\": true}").unwrap();
        assert_eq!(m["x"], true);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        for text in [
            "",
            "no json here",
            "{not json",
            "{\"a\": 1",
            "[1, 2]",
            "}}}{{{",
        ] {
            let err = extract_json_object(text).unwrap_err();
            assert!(
                matches!(err, ProcGenAiError::BadResponse(_)),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn status_codes_map_to_typed_errors() {
        let body = |t: &str, m: &str| {
            format!(r#"{{"type":"error","error":{{"type":"{t}","message":"{m}"}}}}"#)
        };
        let e = parse_response(
            &resp(401, &body("authentication_error", "invalid x-api-key")),
            1,
        )
        .unwrap_err();
        assert!(matches!(e, ProcGenAiError::Auth { status: 401, .. }));
        assert!(e.to_string().contains("invalid x-api-key"), "{e}");
        assert!(e.to_string().contains("ANTHROPIC_API_KEY"), "{e}");
        assert!(!e.is_retryable());

        let mut r = resp(429, &body("rate_limit_error", "slow down"));
        r.retry_after = Some(7);
        let e = parse_response(&r, 3).unwrap_err();
        assert_eq!(
            e.to_string(),
            "rate limited after 3 attempt(s); retry in 7s: slow down"
        );
        assert!(e.is_retryable());

        let e = parse_response(&resp(529, &body("overloaded_error", "busy")), 1).unwrap_err();
        assert!(e.to_string().starts_with("provider overloaded"), "{e}");

        let e =
            parse_response(&resp(400, &body("invalid_request_error", "bad field")), 1).unwrap_err();
        assert_eq!(
            e.to_string(),
            "API error 400 (invalid_request_error): bad field"
        );
        assert!(!e.is_retryable());

        let e = parse_response(&resp(502, "<html>bad gateway</html>"), 1).unwrap_err();
        assert!(e.to_string().contains("bad gateway"), "{e}");
        assert!(e.is_retryable());
    }

    #[test]
    fn malformed_success_bodies_are_errors() {
        for body in [
            "",
            "not json",
            "{}",
            r#"{"content": "a string"}"#,
            r#"{"content": null}"#,
        ] {
            let e = parse_response(&resp(200, body), 1).unwrap_err();
            assert!(matches!(e, ProcGenAiError::BadResponse(_)), "{body}: {e}");
        }
    }

    #[test]
    fn refusal_and_max_tokens_are_reported() {
        let e = parse_response(
            &resp(
                200,
                r#"{"content":[],"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"no"}}"#,
            ),
            1,
        )
        .unwrap_err();
        assert!(e.to_string().contains("declined (category cyber)"), "{e}");
        let e = parse_response(
            &resp(
                200,
                r#"{"content":[{"type":"text","text":"{\"par"}],"stop_reason":"max_tokens"}"#,
            ),
            1,
        )
        .unwrap_err();
        assert!(e.to_string().contains("max_tokens"), "{e}");
    }

    #[test]
    fn text_blocks_are_joined_and_thinking_kept_in_content() {
        let r = parse_response(
            &resp(
                200,
                r#"{"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"","signature":"s"},{"type":"text","text":"a"},{"type":"text","text":"b"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":90}}"#,
            ),
            1,
        )
        .unwrap();
        assert_eq!(r.text, "a\nb");
        assert_eq!(r.content.as_array().unwrap().len(), 3);
        assert_eq!(r.usage.cache_read_input_tokens, 90);
        assert_eq!(r.model, "claude-opus-5-5");
    }
}
