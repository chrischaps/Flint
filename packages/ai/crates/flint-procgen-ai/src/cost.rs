//! Cost tracking for the agent's calls. The price table, the ledger, the
//! report and the [`CostMeter`] are [`flint_ai_core::cost`]'s, shared with
//! asset generation since #721, and re-exported here; what stays is how a
//! Messages API body is estimated before it is sent.

pub use flint_ai_core::cost::*;
use serde_json::Value as Json;

/// Input tokens assumed for one image block. A high-resolution image on a
/// current model is up to ~4,800 tokens; assuming the most keeps the
/// estimate on the safe side.
pub const IMAGE_TOKENS: u64 = 4_800;

/// A conservative count of the input tokens a request body will bill: about
/// three characters a token for text (real English runs nearer four), and
/// [`IMAGE_TOKENS`] per image, whose base64 data isn't counted as text.
pub fn estimate_input_tokens(body: &Json) -> u64 {
    fn walk(v: &Json, chars: &mut u64, images: &mut u64) {
        match v {
            Json::String(s) => *chars += s.len() as u64,
            Json::Array(a) => a.iter().for_each(|x| walk(x, chars, images)),
            Json::Object(o) => {
                if o.get("type").and_then(Json::as_str) == Some("image") {
                    *images += 1;
                    return;
                }
                o.values().for_each(|x| walk(x, chars, images));
            }
            _ => {}
        }
    }
    let (mut chars, mut images) = (0, 0);
    for key in ["system", "messages"] {
        if let Some(v) = body.get(key) {
            walk(v, &mut chars, &mut images);
        }
    }
    chars.div_ceil(3) + images * IMAGE_TOKENS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_estimate_counts_text_and_images_but_not_image_bytes() {
        let body = serde_json::json!({
            "system": [{"type": "text", "text": "x".repeat(300)}],
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "data": "A".repeat(1_000_000)}},
                {"type": "text", "text": "y".repeat(30)},
            ]}],
        });
        let n = estimate_input_tokens(&body);
        // 330 characters of text plus a few role/type strings, and one image.
        assert!((110 + IMAGE_TOKENS..200 + IMAGE_TOKENS).contains(&n), "{n}");
    }
}
