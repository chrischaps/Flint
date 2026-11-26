//! What AI calls cost: the price table, an append-only ledger of every paid
//! call, a report over it, and a [`CostMeter`] that sits in front of each
//! call and refuses one the budget can't cover.
//!
//! **Prices** are data ([`PriceTable`]): `prices.toml` in this crate,
//! compiled in, under a project's `.flint/prices.toml`. A token-billed model
//! not in the table is priced as the dearest one and marked `priced_as`, so
//! an unknown model makes a budget too careful, never too loose.
//!
//! **The ledger** is `.flint/ai_costs.jsonl`, one JSON object per line, so a
//! call is recorded by appending and nothing already written is rewritten. A
//! line that doesn't parse is skipped and counted, never fatal: the ledger
//! is a record, not a dependency. The procgen agent's older ledger,
//! `.flint/procgen_costs.json`, is read beside it and never written.
//!
//! **The meter** checks before a call ([`CostMeter::admit`] for a token-billed
//! reply, [`CostMeter::admit_call`] for a call priced per unit) and records
//! after it ([`CostMeter::record`], [`CostMeter::record_call`]).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// The ledger, relative to the project root.
pub const LEDGER_PATH: &str = ".flint/ai_costs.jsonl";

/// The procgen agent's ledger before #721, read beside [`LEDGER_PATH`].
pub const LEGACY_LEDGER_PATH: &str = ".flint/procgen_costs.json";

/// A project's price overrides, relative to its root.
pub const PRICES_PATH: &str = ".flint/prices.toml";

/// The built-in price table.
pub const BUILTIN_PRICES: &str = include_str!("../prices.toml");

/// Fraction of the ceiling past which the meter warns once.
pub const WARN_FRACTION: f64 = 0.8;

/// The smallest reply worth asking for. When the budget left can't pay for
/// at least this much output, the call is refused rather than sent with a
/// cap so low the reply would be cut off (and paid for anyway).
pub const MIN_OUTPUT_TOKENS: u32 = 2048;

/// Why the meter, the ledger or the price table said no.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CostError {
    /// The next call could take the run past its ceiling, so it was not sent.
    #[error("cost ceiling reached: {} spent, and the next call could cost up to {}, past --max-cost {}; nothing more was sent", usd(*spent), usd(*next), usd(*max))]
    Ceiling {
        /// Dollars spent so far this run.
        spent: f64,
        /// The next call's estimated worst case.
        next: f64,
        /// The ceiling.
        max: f64,
    },
    /// A ceiling is set and the provider has no price, so it can't be kept.
    #[error("no price for provider '{0}', so --max-cost can't be kept; add [calls.{0}] to .flint/prices.toml")]
    Unpriced(String),
    /// The ledger couldn't be read or written.
    #[error("cost ledger {0}")]
    Ledger(String),
    /// A price file is malformed.
    #[error("price table {0}")]
    Prices(String),
}

// ---------------------------------------------------------------- prices

/// Token counts from one reply.
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
    /// The Messages API's `usage` object.
    pub fn of(v: &serde_json::Value) -> Self {
        let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
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

/// What a model charges, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    /// Uncached input.
    pub input: f64,
    /// Output (thinking included).
    pub output: f64,
    /// Input read from the prompt cache.
    pub cache_read: f64,
    /// Input written to the prompt cache (5-minute TTL).
    pub cache_write: f64,
}

impl Price {
    /// Free: the mock providers.
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

/// What a provider billed per call charges: `usd` for each `unit` a call
/// counts (a megapixel, a second, a task).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallPrice {
    /// What a call counts.
    pub unit: String,
    /// USD per unit.
    pub usd: f64,
    /// Where the rate was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl CallPrice {
    /// What `units` of this cost.
    pub fn cost(&self, units: f64) -> f64 {
        self.usd * units
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenPriceFile {
    input: f64,
    output: f64,
    cache_read: f64,
    #[serde(default)]
    cache_write: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokensFile {
    #[serde(default)]
    fallback: Option<String>,
    #[serde(default)]
    models: BTreeMap<String, TokenPriceFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PricesFile {
    #[serde(default)]
    checked: Option<String>,
    #[serde(default)]
    tokens: TokensFile,
    #[serde(default)]
    calls: BTreeMap<String, CallPrice>,
}

/// Every price the meter knows: per-token rates by model, per-call rates by
/// provider. See `prices.toml` for the shape.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceTable {
    /// When the rates were checked (the last file's `checked`).
    pub checked: String,
    /// The model an unknown one is priced as.
    pub fallback: String,
    /// Token-billed models.
    pub models: BTreeMap<String, Price>,
    /// Per-call providers.
    pub calls: BTreeMap<String, CallPrice>,
    /// The files laid over the built-in table, in order.
    pub overrides: Vec<PathBuf>,
}

impl PriceTable {
    /// The table compiled into this crate.
    pub fn builtin() -> Arc<PriceTable> {
        static TABLE: OnceLock<Arc<PriceTable>> = OnceLock::new();
        TABLE
            .get_or_init(|| {
                let empty = PriceTable {
                    checked: String::new(),
                    fallback: String::new(),
                    models: BTreeMap::new(),
                    calls: BTreeMap::new(),
                    overrides: Vec::new(),
                };
                Arc::new(
                    empty
                        .overlaid(BUILTIN_PRICES, None)
                        .expect("the built-in price table parses"),
                )
            })
            .clone()
    }

    /// The built-in table under `<root>/.flint/prices.toml`, if there is one.
    pub fn for_project(root: &Path) -> Result<Arc<PriceTable>, CostError> {
        let path = root.join(PRICES_PATH);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Arc::new(
                Self::builtin().overlaid(&text, Some(path.clone()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::builtin()),
            Err(e) => Err(CostError::Prices(format!("{}: {e}", path.display()))),
        }
    }

    /// This table with `text` (a price file) laid over it: each model or
    /// provider it names replaces the one here.
    pub fn overlaid(&self, text: &str, origin: Option<PathBuf>) -> Result<PriceTable, CostError> {
        let name = origin
            .as_ref()
            .map_or_else(|| "built-in".to_string(), |p| p.display().to_string());
        let file: PricesFile =
            toml::from_str(text).map_err(|e| CostError::Prices(format!("{name}: {e}")))?;
        let mut out = self.clone();
        if let Some(c) = file.checked {
            out.checked = c;
        }
        if let Some(f) = file.tokens.fallback {
            out.fallback = f;
        }
        let bad = |what: &str, v: f64| -> Result<(), CostError> {
            if v.is_finite() && v >= 0.0 {
                Ok(())
            } else {
                Err(CostError::Prices(format!(
                    "{name}: {what} is {v}; a price is a non-negative number"
                )))
            }
        };
        for (model, p) in file.tokens.models {
            let cache_write = p.cache_write.unwrap_or(p.input * 1.25);
            for (k, v) in [
                ("input", p.input),
                ("output", p.output),
                ("cache_read", p.cache_read),
                ("cache_write", cache_write),
            ] {
                bad(&format!("tokens.models.\"{model}\".{k}"), v)?;
            }
            out.models.insert(
                model,
                Price {
                    input: p.input,
                    output: p.output,
                    cache_read: p.cache_read,
                    cache_write,
                },
            );
        }
        for (provider, p) in file.calls {
            bad(&format!("calls.{provider}.usd"), p.usd)?;
            out.calls.insert(provider, p);
        }
        if !out.models.contains_key(&out.fallback) {
            return Err(CostError::Prices(format!(
                "{name}: tokens.fallback '{}' is not one of tokens.models",
                out.fallback
            )));
        }
        out.overrides.extend(origin);
        Ok(out)
    }

    /// The price for `model` on `provider`, and the model it was priced as
    /// when that isn't `model` itself. A dated or suffixed id
    /// (`claude-opus-5-5-xyz`) is priced as its longest listed prefix; an
    /// unknown one as [`Self::fallback`]. `mock` is free.
    pub fn token_price(&self, provider: &str, model: &str) -> (Price, Option<String>) {
        if provider == "mock" {
            return (Price::FREE, None);
        }
        if let Some(p) = self.models.get(model) {
            return (*p, None);
        }
        let prefix = self
            .models
            .iter()
            .filter(|(m, _)| {
                model.starts_with(m.as_str()) && model[m.len()..].starts_with(['-', '@'])
            })
            .max_by_key(|(m, _)| m.len());
        if let Some((m, p)) = prefix {
            return (*p, Some(m.clone()));
        }
        (self.models[&self.fallback], Some(self.fallback.clone()))
    }

    /// The per-call price of `provider`, if it has one.
    pub fn call_price(&self, provider: &str) -> Option<&CallPrice> {
        self.calls.get(provider)
    }
}

/// [`PriceTable::token_price`] on the built-in table.
pub fn price_for(provider: &str, model: &str) -> (Price, Option<String>) {
    PriceTable::builtin().token_price(provider, model)
}

// ---------------------------------------------------------------- ledger

/// One paid call, as recorded in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
    /// When the reply arrived, UTC, RFC 3339 (`2026-10-31T14:05:09Z`).
    pub timestamp: String,
    /// Provider (`anthropic`, `flux`, `meshy`, `elevenlabs`).
    pub provider: String,
    /// The model or endpoint that answered.
    pub model: String,
    /// Uncached input tokens (0 for a call priced per unit).
    #[serde(default)]
    pub input_tokens: u64,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the prompt cache.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// What the call cost at the table's prices.
    pub estimated_usd: f64,
    /// What the call was for: a spec, a look, an asset.
    #[serde(default)]
    pub spec_name: String,
    /// `create`, `interpret`, `refine`, `generate`, `resolve`, ...
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation: String,
    /// The model whose price was used, when `model` wasn't in the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_as: Option<String>,
    /// For a call priced per unit: how many units it counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<f64>,
    /// ...and what a unit is (`megapixel`, `second`, `task`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// The append-only ledger file, and the older one read beside it.
#[derive(Debug, Clone)]
pub struct CostLedger {
    path: PathBuf,
    legacy: Option<PathBuf>,
}

/// What reading a ledger found.
#[derive(Debug, Clone, Default)]
pub struct LedgerContents {
    /// Every entry that parsed, the legacy file's first, in file order.
    pub entries: Vec<CostEntry>,
    /// Lines that didn't, as `file:line`.
    pub skipped: Vec<String>,
}

impl CostLedger {
    /// A ledger at `path`, alone.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            legacy: None,
        }
    }

    /// The ledger of the project rooted at `root`, with its legacy file.
    pub fn in_project(root: &Path) -> Self {
        Self {
            path: root.join(LEDGER_PATH),
            legacy: Some(root.join(LEGACY_LEDGER_PATH)),
        }
    }

    /// [`Self::in_project`] of the working directory.
    pub fn open_default() -> Self {
        Self::in_project(Path::new(""))
    }

    /// The file written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one entry as a line, creating the file and its directory.
    pub fn append(&self, entry: &CostEntry) -> Result<(), CostError> {
        let io = |e: std::io::Error| CostError::Ledger(format!("{}: {e}", self.path.display()));
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
    pub fn read(&self) -> Result<LedgerContents, CostError> {
        let mut out = LedgerContents::default();
        for path in self.legacy.iter().chain([&self.path]) {
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(CostError::Ledger(format!("{}: {e}", path.display()))),
            };
            let part = parse_ledger(&text);
            out.entries.extend(part.entries);
            out.skipped.extend(
                part.skipped
                    .into_iter()
                    .map(|line| format!("{}:{line}", path.display())),
            );
        }
        Ok(out)
    }
}

/// What one ledger file's text holds.
#[derive(Debug, Clone, Default)]
pub struct ParsedLedger {
    /// Entries, in order.
    pub entries: Vec<CostEntry>,
    /// 1-based numbers of the lines that didn't parse.
    pub skipped: Vec<usize>,
}

/// Parse ledger text: one entry per non-blank line.
pub fn parse_ledger(text: &str) -> ParsedLedger {
    let mut out = ParsedLedger::default();
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

// ---------------------------------------------------------------- report

/// Calls and dollars for one group of a report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Tally {
    /// Calls.
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

/// A ledger summed up: the total, and per provider, model and subject.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CostSummary {
    /// Everything in range.
    pub total: Tally,
    /// Per provider.
    pub by_provider: BTreeMap<String, Tally>,
    /// Per model or endpoint.
    pub by_model: BTreeMap<String, Tally>,
    /// Per spec, look or asset (the entries' `spec_name`).
    pub by_spec: BTreeMap<String, Tally>,
    /// Earliest and latest timestamps in range.
    pub span: Option<(String, String)>,
    /// The `since` the summary was made with.
    pub since: Option<String>,
}

/// Check a `--since` date: `YYYY-MM-DD`, optionally with a time
/// (`YYYY-MM-DDTHH:MM:SSZ`). Returns it unchanged, for comparison against
/// ledger timestamps (which sort as strings).
pub fn parse_since(s: &str) -> Result<String, String> {
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
        return Err(format!("--since '{s}' isn't a date; use YYYY-MM-DD"));
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
            let subject = if e.spec_name.is_empty() {
                "(unnamed)"
            } else {
                &e.spec_name
            };
            s.by_spec.entry(subject.to_string()).or_default().add(e);
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
            ("By spec or asset", &self.by_spec),
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

// ---------------------------------------------------------------- meter

/// What the meter's owner hears: the 80% warning, a capped reply, a ledger
/// that couldn't be written.
pub type Notify = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Default)]
struct MeterState {
    spent: f64,
    calls: u64,
    refused: u64,
    spec_name: String,
    operation: String,
    warned: bool,
    entries: Vec<CostEntry>,
}

/// The cost-tracking middleware every paid call goes through.
///
/// Before a call, the meter prices its worst case against what's left of the
/// ceiling. For a token-billed reply ([`CostMeter::admit`]): estimated input
/// plus a full `max_tokens` reply; if a smaller reply would fit,
/// `max_tokens` is lowered so the ceiling still holds; if not even
/// [`MIN_OUTPUT_TOKENS`] would, the call is refused. For a call priced per
/// unit ([`CostMeter::admit_call`]): its units at the provider's rate. After
/// the call, [`CostMeter::record`] / [`CostMeter::record_call`] prices what
/// it really cost and appends it to the ledger.
pub struct CostMeter {
    provider: String,
    ledger: Option<CostLedger>,
    max_usd: Option<f64>,
    prices: Arc<PriceTable>,
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
    /// A meter whose token-billed calls are `provider`'s, with no ledger, no
    /// ceiling, the built-in prices and notices printed to stderr.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            ledger: None,
            max_usd: None,
            prices: PriceTable::builtin(),
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

    /// Price calls with `prices` (a project's table).
    pub fn with_prices(mut self, prices: Arc<PriceTable>) -> Self {
        self.prices = prices;
        self
    }

    /// Send notices somewhere other than stderr.
    pub fn with_notify(mut self, notify: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.notify = Box::new(notify);
        self
    }

    /// Name the spec, look or asset and the operation the next calls are
    /// for, for the ledger.
    pub fn set_context(&self, subject: &str, operation: &str) {
        let mut s = self.state.lock().expect("meter lock");
        s.spec_name = subject.to_string();
        s.operation = operation.to_string();
    }

    /// The prices this meter charges by.
    pub fn prices(&self) -> &PriceTable {
        &self.prices
    }

    /// The ledger, if any.
    pub fn ledger(&self) -> Option<&CostLedger> {
        self.ledger.as_ref()
    }

    /// Dollars spent through this meter so far.
    pub fn spent(&self) -> f64 {
        self.state.lock().expect("meter lock").spent
    }

    /// Calls recorded so far.
    pub fn calls(&self) -> u64 {
        self.state.lock().expect("meter lock").calls
    }

    /// Calls refused for the ceiling so far.
    pub fn refused(&self) -> u64 {
        self.state.lock().expect("meter lock").refused
    }

    /// Every call recorded through this meter, in order.
    pub fn entries(&self) -> Vec<CostEntry> {
        self.state.lock().expect("meter lock").entries.clone()
    }

    /// The ceiling, if any.
    pub fn ceiling(&self) -> Option<f64> {
        self.max_usd
    }

    fn refuse(&self, spent: f64, next: f64, max: f64) -> CostError {
        self.state.lock().expect("meter lock").refused += 1;
        CostError::Ceiling { spent, next, max }
    }

    /// Check a token-billed request before it's sent. Returns the
    /// `max_tokens` to send (`max_tokens`, or less so the worst case fits).
    pub fn admit(&self, model: &str, input_tokens: u64, max_tokens: u32) -> Result<u32, CostError> {
        let Some(max) = self.max_usd else {
            return Ok(max_tokens);
        };
        let (price, _) = self.prices.token_price(&self.provider, model);
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
        Err(self.refuse(spent, worst(MIN_OUTPUT_TOKENS.min(max_tokens)), max))
    }

    /// What `units` of `provider`'s call cost at this meter's prices. A
    /// provider with no price costs nothing, unless a ceiling is set, when
    /// it is [`CostError::Unpriced`]: an unknown price can't be kept under one.
    pub fn price_call(&self, provider: &str, units: f64) -> Result<f64, CostError> {
        match self.prices.call_price(provider) {
            Some(p) => Ok(p.cost(units)),
            None if provider == "mock" => Ok(0.0),
            None if self.max_usd.is_some() => Err(CostError::Unpriced(provider.to_string())),
            None => Ok(0.0),
        }
    }

    /// Check a call priced per unit before it's sent: refused when what's
    /// been spent plus its price would pass the ceiling. Returns the price.
    pub fn admit_call(&self, provider: &str, units: f64) -> Result<f64, CostError> {
        let cost = self.price_call(provider, units)?;
        self.admit_usd(cost)?;
        Ok(cost)
    }

    /// Refuse when `next` dollars on top of what's been spent would pass the
    /// ceiling.
    pub fn admit_usd(&self, next: f64) -> Result<(), CostError> {
        let Some(max) = self.max_usd else {
            return Ok(());
        };
        let spent = self.spent();
        // A hair of slack, so $0.25 + $0.25 fits a $0.50 ceiling.
        if spent + next > max + 1e-9 {
            return Err(self.refuse(spent, next, max));
        }
        Ok(())
    }

    /// Record a token-billed call's real usage: price it, add it to the
    /// total, append it to the ledger (a ledger that can't be written is
    /// reported, not fatal: the call is already paid for), and warn once
    /// past [`WARN_FRACTION`] of the ceiling. Returns the call's cost.
    pub fn record(&self, model: &str, usage: &Usage) -> f64 {
        let (price, priced_as) = self.prices.token_price(&self.provider, model);
        let cost = price.usd(usage);
        if let Some(m) = &priced_as {
            (self.notify)(&format!(
                "cost: no price known for model '{model}'; counted at {m}'s rates"
            ));
        }
        let provider = self.provider.clone();
        self.push(|s| CostEntry {
            timestamp: now_rfc3339(),
            provider,
            model: model.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            estimated_usd: cost,
            spec_name: s.spec_name.clone(),
            operation: s.operation.clone(),
            priced_as,
            units: None,
            unit: None,
        });
        cost
    }

    /// Record a call priced per unit: `units` of `provider`'s rate, on
    /// `model` (the endpoint). Returns the call's cost.
    pub fn record_call(&self, provider: &str, model: &str, units: f64) -> f64 {
        let price = self.prices.call_price(provider).cloned();
        let cost = price.as_ref().map_or(0.0, |p| p.cost(units));
        if price.is_none() && provider != "mock" {
            (self.notify)(&format!(
                "cost: no price known for provider '{provider}'; recorded at $0"
            ));
        }
        self.push(|s| CostEntry {
            timestamp: now_rfc3339(),
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            estimated_usd: cost,
            spec_name: s.spec_name.clone(),
            operation: s.operation.clone(),
            priced_as: None,
            units: Some(units),
            unit: price.map(|p| p.unit),
        });
        cost
    }

    fn push(&self, make: impl FnOnce(&MeterState) -> CostEntry) {
        let (entry, warn) = {
            let mut s = self.state.lock().expect("meter lock");
            let entry = make(&s);
            s.spent += entry.estimated_usd;
            s.calls += 1;
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
    }
}

#[cfg(test)]
mod tests;
