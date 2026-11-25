//! The one place a command gets what its paid AI calls go through (#721):
//! the project's ledger (`.flint/ai_costs.jsonl` at its root), its prices
//! (the built-in table under `.flint/prices.toml`), a meter with the run's
//! `--max-cost`, and the report over the ledger that `flint gen
//! --cost-report` and `flint asset costs` both print.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use flint_ai_core::cost::{self, CostLedger, CostMeter, CostSummary, PriceTable};

/// The project root: the working directory's project, or the working
/// directory when it is in none.
pub fn project_root() -> PathBuf {
    flint_asset::Project::current()
        .map(|p| p.root)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// The working directory's project's ledger.
pub fn ledger() -> CostLedger {
    CostLedger::in_project(&project_root())
}

/// The working directory's project's prices.
pub fn prices() -> Result<Arc<PriceTable>> {
    PriceTable::for_project(&project_root()).map_err(|e| anyhow!("{e}"))
}

/// [`meter_at`] the working directory's project.
pub fn meter(provider: &str, max_cost: Option<f64>) -> Result<CostMeter> {
    meter_at(&project_root(), provider, max_cost)
}

/// A meter for `provider`'s calls (the token-billed provider; a per-call
/// provider names itself when it records), writing the ledger of the
/// project at `root`, priced by its table, refusing past `max_cost`.
pub fn meter_at(root: &Path, provider: &str, max_cost: Option<f64>) -> Result<CostMeter> {
    let prices = PriceTable::for_project(root).map_err(|e| anyhow!("{e}"))?;
    let mut meter = CostMeter::new(provider)
        .with_ledger(CostLedger::in_project(root))
        .with_prices(prices);
    if let Some(max) = max_cost {
        meter = meter.with_ceiling(max);
    }
    Ok(meter)
}

/// `$0.0500 spent over 2 calls, recorded in .flint/ai_costs.jsonl`.
pub fn spent_line(meter: &CostMeter) -> String {
    let ceiling = meter
        .ceiling()
        .map(|c| format!(" of --max-cost {}", cost::usd(c)))
        .unwrap_or_default();
    let refused = match meter.refused() {
        0 => String::new(),
        n => format!(", {n} refused"),
    };
    format!(
        "{} spent{ceiling} over {} call{}{refused}, recorded in {}",
        cost::usd(meter.spent()),
        meter.calls(),
        if meter.calls() == 1 { "" } else { "s" },
        cost::LEDGER_PATH
    )
}

/// What AI calls have cost, from `ledger`: total, per provider, per model,
/// per spec or asset, from `since` on.
pub fn cost_report(ledger: &CostLedger, since: Option<&str>) -> Result<()> {
    let since = since
        .map(cost::parse_since)
        .transpose()
        .map_err(|e| anyhow!("{e}"))?;
    let contents = ledger.read().map_err(|e| anyhow!("{e}"))?;
    if !contents.skipped.is_empty() {
        crate::output::progress!(
            "warning: skipped {} unreadable ledger line(s): {}",
            contents.skipped.len(),
            contents.skipped.join(", ")
        );
    }
    let summary = CostSummary::from_entries(&contents.entries, since.as_deref());
    let ledger_path = ledger.path().display().to_string().replace('\\', "/");
    let mut document = serde_json::to_value(&summary)?;
    document["ledger"] = ledger_path.clone().into();
    crate::output::emit(&crate::output::Answer::new(
        format!("{}\nLedger: {ledger_path}", summary.render()),
        document,
    ))
}
