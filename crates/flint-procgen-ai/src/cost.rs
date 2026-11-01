//! Cost tracking for AI calls: what each model charges, an append-only
//! ledger of every call made, a report over it, and a [`CostMeter`] that sits
//! in front of each request and refuses one the budget can't cover.
//!
//! The ledger is `.flint/procgen_costs.json`, one JSON object per line
//! (JSON Lines), so a call is recorded by appending and nothing already
//! written is ever rewritten. A line that doesn't parse is skipped and
//! counted, never fatal: the ledger is a record, not a dependency.
//!
//! Prices are USD per million tokens, from Anthropic's published rates
//! (2026-10). A model not in the table is priced as the most expensive one
//! and marked `priced_as` in the ledger, so an unknown model can make the
//! budget too careful, never too loose.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::error::{ProcGenAiError, Result};
use crate::http::Usage;

/// Where the ledger lives, relative to the project root (the working dir).
pub const LEDGER_PATH: &str = ".flint/procgen_costs.json";

/// Fraction of the ceiling past which the meter warns once.
pub const WARN_FRACTION: f64 = 0.8;

/// The smallest reply worth asking for. When the budget left can't pay for
/// at least this much output, the call is refused rather than sent with a
/// cap so low the reply would be cut off (and paid for anyway).
pub const MIN_OUTPUT_TOKENS: u32 = 2048;

/// Input tokens assumed for one image block. A high-resolution image on a
/// current model is up to ~4,800 tokens; assuming the most keeps the
/// estimate on the safe side.
const IMAGE_TOKENS: u64 = 4_800;

/// What a model charges, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Uncached input.
    pub input: f64,
    /// Output (thinking included).
    pub output: f64,
    /// Input read from the prompt cache.
    pub cache_read: f64,
    /// Input written to the prompt cache (5-minute TTL: 1.25x input).
    pub cache_write: f64,
}

impl Price {
    const fn new(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write: input * 1.25,
        }
    }

    /// Free: the mock provider.
    pub const FREE: Self = Self {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    };

    /// What `usage` cost at these prices.
    pub fn usd(&self, usage: &Usage) -> f64 {
        (usage.input_tokens as f64 * self.input
            + usage.output_tokens as f64 * self.output
            + usage.cache_read_input_tokens as f64 * self.cache_read
            + usage.cache_creation_input_tokens as f64 * self.cache_write)
            / 1_000_000.0
    }
}

/// Published first-party rates (USD / MTok: input, output, cache read).
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", Price::new(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", Price::new(10.0, 50.0, 0.25)),
    ("claude-fable-5", Price::new(10.0, 50.0, 1.0)),
    ("claude-mythos-5", Price::new(10.0, 50.0, 1.0)),
    ("claude-opus-5-5", Price::new(4.0, 20.0, 0.20)),
    ("claude-opus-5", Price::new(5.0, 25.0, 0.50)),
    ("claude-opus-4-8", Price::new(5.0, 25.0, 0.50)),
    ("claude-opus-4-7", Price::new(5.0, 25.0, 0.50)),
    ("claude-opus-4-6", Price::new(5.0, 25.0, 0.50)),
    ("claude-sonnet-5-5", Price::new(2.0, 10.0, 0.20)),
    ("claude-sonnet-5", Price::new(2.0, 10.0, 0.20)),
    ("claude-sonnet-4-6", Price::new(3.0, 15.0, 0.30)),
    // Up to 100K-token prompts; this agent's prompts are far below that.
    ("claude-haiku-5-5", Price::new(0.10, 0.50, 0.01)),
    ("claude-haiku-4-5", Price::new(1.0, 5.0, 0.10)),
];

/// The model an unknown one is priced as: the dearest in the table.
const FALLBACK_PRICE_MODEL: &str = "claude-fable-5-1";

/// The price for `model` on `provider`, and the model it was priced as when
/// that isn't `model` itself (an unknown model). A dated or suffixed id
/// (`claude-opus-5-5-xyz`) is priced as its longest known prefix.
pub fn price_for(provider: &str, model: &str) -> (Price, Option<&'static str>) {
    if provider == "mock" {
        return (Price::FREE, None);
    }
    if let Some((_, p)) = PRICES.iter().find(|(m, _)| *m == model) {
        return (*p, None);
    }
    let prefix = PRICES
        .iter()
        .filter(|(m, _)| model.starts_with(m) && model[m.len()..].starts_with(['-', '@']))
        .max_by_key(|(m, _)| m.len());
    if let Some((m, p)) = prefix {
        return (*p, Some(m));
    }
    let (m, p) = PRICES
        .iter()
        .find(|(m, _)| *m == FALLBACK_PRICE_MODEL)
        .expect("the fallback model is in the table");
    (*p, Some(m))
}

/// One API call, as recorded in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
    /// When the reply arrived, UTC, RFC 3339 (`2026-10-31T14:05:09Z`).
    pub timestamp: String,
    /// Provider (`anthropic`).
    pub provider: String,
    /// The model that answered.
    pub model: String,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the prompt cache.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// What the call cost at the table's prices.
    pub estimated_usd: f64,
    /// The spec the call was about.
    pub spec_name: String,
    /// `create`, `interpret` or `refine`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation: String,
    /// The model whose price was used, when `model` wasn't in the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_as: Option<String>,
}

/// The append-only ledger file.
#[derive(Debug, Clone)]
pub struct CostLedger {
    path: PathBuf,
}

/// What reading a ledger found.
#[derive(Debug, Clone, Default)]
pub struct LedgerContents {
    /// Every entry that parsed, in file order.
    pub entries: Vec<CostEntry>,
    /// Lines that didn't (1-based line numbers).
    pub skipped: Vec<usize>,
}

impl CostLedger {
    /// A ledger at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The project ledger, [`LEDGER_PATH`].
    pub fn open_default() -> Self {
        Self::new(LEDGER_PATH)
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one entry as a line, creating the file and its directory.
    pub fn append(&self, entry: &CostEntry) -> Result<()> {
        let io = |e: std::io::Error| {
            ProcGenAiError::ConfigError(format!("cost ledger {}: {e}", self.path.display()))
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let line = serde_json::to_string(entry).expect("a CostEntry always serializes");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io)?;
        writeln!(f, "{line}").map_err(io)
    }

    /// Every entry. A missing file is an empty ledger.
    pub fn read(&self) -> Result<LedgerContents> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(ProcGenAiError::ConfigError(format!(
                    "cost ledger {}: {e}",
                    self.path.display()
                )))
            }
        };
        Ok(parse_ledger(&text))
    }
}

/// Parse ledger text: one entry per non-blank line.
pub fn parse_ledger(text: &str) -> LedgerContents {
    let mut out = LedgerContents::default();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<CostEntry>(line) {
            Ok(e) => out.entries.push(e),
            Err(_) => out.skipped.push(i + 1),
        }
    }
    out
}

/// Calls and dollars for one group of a report.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tally {
    /// API calls.
    pub calls: u64,
    /// Input tokens of every kind (uncached + cache read + cache write).
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Dollars.
    pub usd: f64,
}

impl Tally {
    fn add(&mut self, e: &CostEntry) {
        self.calls += 1;
        self.input_tokens +=
            e.input_tokens + e.cache_read_input_tokens + e.cache_creation_input_tokens;
        self.output_tokens += e.output_tokens;
        self.usd += e.estimated_usd;
    }
}

/// A ledger summed up: the total, and per provider, model and spec.
#[derive(Debug, Clone, Default)]
pub struct CostSummary {
    /// Everything in range.
    pub total: Tally,
    /// Per provider.
    pub by_provider: BTreeMap<String, Tally>,
    /// Per model.
    pub by_model: BTreeMap<String, Tally>,
    /// Per spec.
    pub by_spec: BTreeMap<String, Tally>,
    /// Earliest and latest timestamps in range.
    pub span: Option<(String, String)>,
    /// The `since` the summary was made with.
    pub since: Option<String>,
}

/// Check a `--since` date: `YYYY-MM-DD`, optionally with a time
/// (`YYYY-MM-DDTHH:MM:SSZ`). Returns it unchanged, for comparison against
/// ledger timestamps (which sort as strings).
pub fn parse_since(s: &str) -> Result<String> {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let date_ok = b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10);
    let month = s.get(5..7).and_then(|m| m.parse::<u32>().ok());
    let day = s.get(8..10).and_then(|d| d.parse::<u32>().ok());
    if !date_ok
        || !matches!(month, Some(1..=12))
        || !matches!(day, Some(1..=31))
        || (b.len() > 10 && b[10] != b'T')
    {
        return Err(ProcGenAiError::ConfigError(format!(
            "--since '{s}' isn't a date; use YYYY-MM-DD"
        )));
    }
    Ok(s.to_string())
}

impl CostSummary {
    /// Sum `entries` from `since` on (inclusive; `None` = everything).
    pub fn from_entries(entries: &[CostEntry], since: Option<&str>) -> Self {
        let mut s = Self {
            since: since.map(str::to_string),
            ..Self::default()
        };
        for e in entries {
            if since.is_some_and(|d| e.timestamp.as_str() < d) {
                continue;
            }
            s.total.add(e);
            s.by_provider.entry(e.provider.clone()).or_default().add(e);
            s.by_model.entry(e.model.clone()).or_default().add(e);
            let spec = if e.spec_name.is_empty() {
                "(unnamed)"
            } else {
                &e.spec_name
            };
            s.by_spec.entry(spec.to_string()).or_default().add(e);
            s.span = Some(match s.span.take() {
                None => (e.timestamp.clone(), e.timestamp.clone()),
                Some((a, b)) => (a.min(e.timestamp.clone()), b.max(e.timestamp.clone())),
            });
        }
        s
    }

    /// The report as text, for a terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let range = match (&self.since, &self.span) {
            (Some(d), _) => format!(" since {d}"),
            (None, Some((a, b))) => {
                format!(" ({} .. {})", &a[..a.len().min(10)], &b[..b.len().min(10)])
            }
            (None, None) => String::new(),
        };
        out.push_str(&format!("AI cost report{range}\n"));
        if self.total.calls == 0 {
            out.push_str("  no AI calls recorded\n");
            return out;
        }
        out.push_str(&format!(
            "  Total:  {}  over {} call{}  ({} in / {} out tokens)\n",
            usd(self.total.usd),
            self.total.calls,
            if self.total.calls == 1 { "" } else { "s" },
            count(self.total.input_tokens),
            count(self.total.output_tokens),
        ));
        for (title, map) in [
            ("By provider", &self.by_provider),
            ("By model", &self.by_model),
            ("By spec", &self.by_spec),
        ] {
            out.push_str(&format!("\n  {title}:\n"));
            let width = map.keys().map(|k| k.chars().count()).max().unwrap_or(0);
            let mut rows: Vec<_> = map.iter().collect();
            rows.sort_by(|a, b| b.1.usd.total_cmp(&a.1.usd).then(a.0.cmp(b.0)));
            for (k, t) in rows {
                let share = if self.total.usd > 0.0 {
                    t.usd / self.total.usd * 100.0
                } else {
                    0.0
                };
                out.push_str(&format!(
                    "    {k:<width$}  {:>9}  {:>5.1}%  {:>4} call{}\n",
                    usd(t.usd),
                    share,
                    t.calls,
                    if t.calls == 1 { "" } else { "s" },
                ));
            }
        }
        out
    }
}

/// `$0.1234` (four places under a dollar, two above).
pub fn usd(v: f64) -> String {
    if v.abs() < 1.0 {
        format!("${v:.4}")
    } else {
        format!("${v:.2}")
    }
}

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Now, UTC, as RFC 3339 to the second.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

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

/// What the meter's owner hears: the 80% warning, a capped reply, a ledger
/// that couldn't be written.
pub type Notify = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Default)]
struct MeterState {
    spent: f64,
    calls: u64,
    spec_name: String,
    operation: String,
    warned: bool,
    entries: Vec<CostEntry>,
}

/// The cost-tracking middleware every API call goes through.
///
/// Before a request, [`CostMeter::admit`] prices its worst case (estimated
/// input plus a full `max_tokens` reply) against what's left of the ceiling:
/// if it fits, it's sent; if a smaller reply would fit, `max_tokens` is
/// lowered so the ceiling still holds; if not even [`MIN_OUTPUT_TOKENS`]
/// would, the call is refused with [`ProcGenAiError::CostCeiling`]. After a
/// reply, [`CostMeter::record`] prices its real usage and appends it to the
/// ledger.
pub struct CostMeter {
    provider: String,
    ledger: Option<CostLedger>,
    max_usd: Option<f64>,
    state: Mutex<MeterState>,
    notify: Notify,
}

impl std::fmt::Debug for CostMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostMeter")
            .field("provider", &self.provider)
            .field("ledger", &self.ledger)
            .field("max_usd", &self.max_usd)
            .field("spent", &self.spent())
            .finish_non_exhaustive()
    }
}

impl CostMeter {
    /// A meter for `provider`'s calls with no ledger, no ceiling and
    /// notices printed to stderr.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            ledger: None,
            max_usd: None,
            state: Mutex::default(),
            notify: Box::new(|m| eprintln!("{m}")),
        }
    }

    /// Record each call in `ledger`.
    pub fn with_ledger(mut self, ledger: CostLedger) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// Refuse any call that could take the total past `max_usd`.
    pub fn with_ceiling(mut self, max_usd: f64) -> Self {
        self.max_usd = Some(max_usd);
        self
    }

    /// Send notices somewhere other than stderr.
    pub fn with_notify(mut self, notify: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.notify = Box::new(notify);
        self
    }

    /// Name the spec and operation the next calls are for, for the ledger.
    pub fn set_context(&self, spec_name: &str, operation: &str) {
        let mut s = self.state.lock().expect("meter lock");
        s.spec_name = spec_name.to_string();
        s.operation = operation.to_string();
    }

    /// Dollars spent through this meter so far.
    pub fn spent(&self) -> f64 {
        self.state.lock().expect("meter lock").spent
    }

    /// Calls recorded so far.
    pub fn calls(&self) -> u64 {
        self.state.lock().expect("meter lock").calls
    }

    /// Every call recorded through this meter, in order.
    pub fn entries(&self) -> Vec<CostEntry> {
        self.state.lock().expect("meter lock").entries.clone()
    }

    /// The ceiling, if any.
    pub fn ceiling(&self) -> Option<f64> {
        self.max_usd
    }

    /// Check a request before it's sent. Returns the `max_tokens` to send
    /// (`max_tokens`, or less so the worst case fits the ceiling).
    pub fn admit(&self, model: &str, input_tokens: u64, max_tokens: u32) -> Result<u32> {
        let Some(max) = self.max_usd else {
            return Ok(max_tokens);
        };
        let (price, _) = price_for(&self.provider, model);
        let spent = self.spent();
        let left = max - spent;
        let input_usd = input_tokens as f64 * price.input / 1e6;
        let worst = |out: u32| input_usd + f64::from(out) * price.output / 1e6;
        if worst(max_tokens) <= left {
            return Ok(max_tokens);
        }
        let affordable = if price.output > 0.0 {
            ((left - input_usd) / price.output * 1e6).floor()
        } else {
            f64::from(max_tokens)
        };
        if affordable >= f64::from(MIN_OUTPUT_TOKENS) {
            let capped = (affordable as u32).min(max_tokens);
            (self.notify)(&format!(
                "cost: capping this reply at {capped} output tokens (from {max_tokens}) so the \
                 run stays under --max-cost {}",
                usd(max)
            ));
            return Ok(capped);
        }
        Err(ProcGenAiError::CostCeiling {
            spent,
            next: worst(MIN_OUTPUT_TOKENS.min(max_tokens)),
            max,
        })
    }

    /// Record a call's real usage: price it, add it to the total, append it
    /// to the ledger (a ledger that can't be written is reported, not
    /// fatal: the call is already paid for), and warn once past
    /// [`WARN_FRACTION`] of the ceiling. Returns the call's cost.
    pub fn record(&self, model: &str, usage: &Usage) -> f64 {
        let (price, priced_as) = price_for(&self.provider, model);
        let cost = price.usd(usage);
        let (entry, warn) = {
            let mut s = self.state.lock().expect("meter lock");
            s.spent += cost;
            s.calls += 1;
            let entry = CostEntry {
                timestamp: now_rfc3339(),
                provider: self.provider.clone(),
                model: model.to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                estimated_usd: cost,
                spec_name: s.spec_name.clone(),
                operation: s.operation.clone(),
                priced_as: priced_as.map(str::to_string),
            };
            s.entries.push(entry.clone());
            let warn = match self.max_usd {
                Some(max) if !s.warned && s.spent > max * WARN_FRACTION => {
                    s.warned = true;
                    Some((s.spent, max))
                }
                _ => None,
            };
            (entry, warn)
        };
        if let Some(m) = priced_as {
            (self.notify)(&format!(
                "cost: no price known for model '{model}'; counted at {m}'s rates"
            ));
        }
        if let Some(ledger) = &self.ledger {
            if let Err(e) = ledger.append(&entry) {
                (self.notify)(&format!("cost: could not record the call: {e}"));
            }
        }
        if let Some((spent, max)) = warn {
            (self.notify)(&format!(
                "cost: warning: {} spent, {:.0}% of --max-cost {}",
                usd(spent),
                spent / max * 100.0,
                usd(max)
            ));
        }
        cost
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn usage(i: u64, o: u64) -> Usage {
        Usage {
            input_tokens: i,
            output_tokens: o,
            ..Usage::default()
        }
    }

    fn entry(ts: &str, provider: &str, model: &str, spec: &str, usd: f64) -> CostEntry {
        CostEntry {
            timestamp: ts.into(),
            provider: provider.into(),
            model: model.into(),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            estimated_usd: usd,
            spec_name: spec.into(),
            operation: "create".into(),
            priced_as: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flint-cost-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn prices_follow_the_published_rates() {
        let (p, as_) = price_for("anthropic", "claude-opus-5-5");
        assert_eq!(as_, None);
        // 1M in + 1M out on Opus 5.5 = $4 + $20.
        assert!((p.usd(&usage(1_000_000, 1_000_000)) - 24.0).abs() < 1e-9);
        let cached = Usage {
            cache_read_input_tokens: 1_000_000,
            cache_creation_input_tokens: 1_000_000,
            ..Usage::default()
        };
        assert!((p.usd(&cached) - (0.20 + 5.0)).abs() < 1e-9);
        assert_eq!(price_for("mock", "mock").0, Price::FREE);
    }

    #[test]
    fn an_unknown_model_is_priced_as_the_dearest() {
        let (p, as_) = price_for("anthropic", "claude-new-thing");
        assert_eq!(as_, Some(FALLBACK_PRICE_MODEL));
        assert_eq!(p.output, 50.0);
        // A suffixed id finds its family, not a shorter neighbour.
        assert_eq!(
            price_for("anthropic", "claude-opus-5-5-preview").1,
            Some("claude-opus-5-5")
        );
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_793_458_509), "2026-10-31T14:55:09Z");
    }

    #[test]
    fn the_ledger_appends_lines_and_reads_them_back() {
        let dir = tmp("ledger");
        let ledger = CostLedger::new(dir.join(".flint/procgen_costs.json"));
        assert!(ledger.read().unwrap().entries.is_empty());
        let a = entry(
            "2026-10-01T00:00:00Z",
            "anthropic",
            "claude-opus-5-5",
            "oak",
            0.01,
        );
        let b = entry(
            "2026-10-02T00:00:00Z",
            "anthropic",
            "claude-opus-5-5",
            "elm",
            0.02,
        );
        ledger.append(&a).unwrap();
        ledger.append(&b).unwrap();
        let text = std::fs::read_to_string(ledger.path()).unwrap();
        assert_eq!(text.lines().count(), 2);
        // Every line is a JSON object with the documented fields.
        for line in text.lines() {
            let v: Json = serde_json::from_str(line).unwrap();
            for k in [
                "timestamp",
                "provider",
                "model",
                "input_tokens",
                "output_tokens",
                "estimated_usd",
                "spec_name",
            ] {
                assert!(v.get(k).is_some(), "{k} missing from {line}");
            }
        }
        // A damaged line is skipped, not fatal.
        std::fs::write(ledger.path(), format!("{text}not json\n")).unwrap();
        let back = ledger.read().unwrap();
        assert_eq!(back.entries, vec![a, b]);
        assert_eq!(back.skipped, vec![3]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_summary_adds_up() {
        let entries = vec![
            entry(
                "2026-09-30T23:59:59Z",
                "anthropic",
                "claude-opus-5-5",
                "oak",
                0.10,
            ),
            entry(
                "2026-10-01T00:00:00Z",
                "anthropic",
                "claude-opus-5-5",
                "oak",
                0.20,
            ),
            entry(
                "2026-10-02T00:00:00Z",
                "anthropic",
                "claude-sonnet-5-5",
                "elm",
                0.05,
            ),
            entry("2026-10-03T00:00:00Z", "mock", "mock", "elm", 0.0),
        ];
        let all = CostSummary::from_entries(&entries, None);
        assert_eq!(all.total.calls, 4);
        assert!((all.total.usd - 0.35).abs() < 1e-9);
        assert!((all.by_spec["oak"].usd - 0.30).abs() < 1e-9);
        assert_eq!(all.by_provider["anthropic"].calls, 3);
        assert_eq!(all.total.input_tokens, 4000);

        let since = parse_since("2026-10-01").unwrap();
        let recent = CostSummary::from_entries(&entries, Some(&since));
        assert_eq!(recent.total.calls, 3);
        assert!((recent.total.usd - 0.25).abs() < 1e-9);
        let text = recent.render();
        assert!(text.contains("since 2026-10-01"), "{text}");
        assert!(text.contains("$0.2500"), "{text}");
        assert!(text.contains("By spec"), "{text}");

        assert!(CostSummary::from_entries(&[], None)
            .render()
            .contains("no AI calls"));
    }

    #[test]
    fn since_must_be_a_date() {
        assert!(parse_since("2024-01-01").is_ok());
        assert!(parse_since("2024-01-01T12:00:00Z").is_ok());
        for bad in ["yesterday", "2024-13-01", "2024-1-1", "2024-01-01 12:00"] {
            assert!(parse_since(bad).is_err(), "{bad}");
        }
    }

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

    fn quiet(meter: CostMeter) -> (CostMeter, Arc<Mutex<Vec<String>>>) {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        (
            meter.with_notify(move |m| h.lock().unwrap().push(m.to_string())),
            heard,
        )
    }

    #[test]
    fn the_ceiling_admits_caps_and_refuses() {
        let (meter, heard) = quiet(CostMeter::new("anthropic").with_ceiling(0.50));
        // Opus 5.5: 10K in ($0.04) + 16K out ($0.32) fits $0.50.
        assert_eq!(
            meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap(),
            16_000
        );
        meter.record("claude-opus-5-5", &usage(10_000, 10_000)); // $0.24
                                                                 // $0.26 left: 10K in ($0.04) leaves $0.22 = 11,000 output tokens.
        let capped = meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap();
        assert!((10_990..=11_000).contains(&capped), "{capped}");
        assert!(heard.lock().unwrap().iter().any(|m| m.contains("capping")));
        meter.record("claude-opus-5-5", &usage(10_000, 9_000)); // $0.22 -> $0.46
        assert!(
            heard.lock().unwrap().iter().any(|m| m.contains("92%")),
            "{:?}",
            heard.lock().unwrap()
        );
        // $0.04 left can't pay for 10K in + 2K out.
        let err = meter.admit("claude-opus-5-5", 10_000, 16_000).unwrap_err();
        assert!(matches!(err, ProcGenAiError::CostCeiling { .. }));
        let msg = err.to_string();
        assert!(msg.contains("$0.50") && msg.contains("$0.46"), "{msg}");
        assert_eq!(meter.calls(), 2);
    }

    #[test]
    fn no_ceiling_admits_everything_and_mock_is_free() {
        let meter = CostMeter::new("anthropic");
        assert_eq!(
            meter.admit("claude-fable-5-1", 1 << 30, 128_000).unwrap(),
            128_000
        );
        let mock = CostMeter::new("mock").with_ceiling(0.0);
        assert_eq!(mock.admit("mock", 1 << 30, 16_000).unwrap(), 16_000);
        assert_eq!(mock.record("mock", &usage(100, 100)), 0.0);
    }

    #[test]
    fn record_writes_the_ledger_with_context() {
        let dir = tmp("meter");
        let ledger = CostLedger::new(dir.join("costs.json"));
        let (meter, _) = quiet(CostMeter::new("anthropic").with_ledger(ledger.clone()));
        meter.set_context("twisted_dead_tree", "create");
        let cost = meter.record("claude-opus-5-5", &usage(1_000, 2_000));
        assert!((cost - (0.004 + 0.04)).abs() < 1e-9);
        let back = ledger.read().unwrap().entries;
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].spec_name, "twisted_dead_tree");
        assert_eq!(back[0].operation, "create");
        assert_eq!(back[0].output_tokens, 2_000);
        assert!((back[0].estimated_usd - cost).abs() < 1e-12);
        let _ = std::fs::remove_dir_all(dir);
    }
}
