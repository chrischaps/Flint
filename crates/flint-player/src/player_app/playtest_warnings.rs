//! A playtest's count of the engine's warnings (#714), as a `tracing`
//! layer the host installs beside its log ([`warning_layer`]).
//!
//! Each warning is counted by its message, and by whether it came while
//! the scene loaded or while it ran, so `--strict` can fail a load that
//! warned (a missing sound, script or file). What scripts' test functions
//! say and script errors are left out: the report has its own lists of
//! those.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// One warning's message and how often it came.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Warning {
    pub message: String,
    /// `warn` or `error`.
    pub level: &'static str,
    /// Times while the scene loaded.
    pub at_load: u64,
    /// Times while it ran.
    pub while_running: u64,
}

/// Where a playtest is, for the count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Phase {
    /// Not counting (no playtest running).
    Off,
    Load,
    Run,
}

struct Tally {
    phase: Phase,
    warnings: Vec<Warning>,
    /// Message -> index into `warnings`, so a repeat is a lookup.
    seen: BTreeMap<(String, &'static str), usize>,
}

fn tally() -> &'static Mutex<Tally> {
    static TALLY: OnceLock<Mutex<Tally>> = OnceLock::new();
    TALLY.get_or_init(|| {
        Mutex::new(Tally {
            phase: Phase::Off,
            warnings: Vec::new(),
            seen: BTreeMap::new(),
        })
    })
}

fn installed() -> &'static OnceLock<()> {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    &INSTALLED
}

/// The layer that counts warnings for a playtest: add it to the process's
/// subscriber (`flint` does, in its `main`). It hears warnings and errors
/// only, and counts nothing while no playtest runs.
pub fn warning_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    let _ = installed().set(());
    WarningLayer.with_filter(tracing_subscriber::filter::LevelFilter::WARN)
}

/// Whether [`warning_layer`] was installed, so a report can say it didn't
/// count rather than report none.
pub(crate) fn counting() -> bool {
    installed().get().is_some()
}

/// Start counting afresh, in `phase`.
pub(crate) fn begin(phase: Phase) {
    let mut t = lock();
    t.phase = phase;
    t.warnings.clear();
    t.seen.clear();
}

pub(crate) fn set_phase(phase: Phase) {
    lock().phase = phase;
}

/// Stop counting and hand over what was counted, in the order each first
/// came.
pub(crate) fn end() -> Vec<Warning> {
    let mut t = lock();
    t.phase = Phase::Off;
    t.seen.clear();
    std::mem::take(&mut t.warnings)
}

fn lock() -> std::sync::MutexGuard<'static, Tally> {
    tally().lock().unwrap_or_else(|e| e.into_inner())
}

struct WarningLayer;

/// Targets whose warnings the report lists elsewhere: scripts' test
/// failures and script errors.
fn listed_elsewhere(target: &str) -> bool {
    target == "script::test" || target == "flint_script::errors"
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for WarningLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if listed_elsewhere(meta.target()) {
            return;
        }
        let level = match *meta.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            _ => return,
        };
        let mut t = lock();
        if t.phase == Phase::Off {
            return;
        }
        let mut message = Message::default();
        event.record(&mut message);
        let phase = t.phase;
        let key = (message.0, level);
        let i = match t.seen.get(&key) {
            Some(i) => *i,
            None => {
                let i = t.warnings.len();
                t.warnings.push(Warning {
                    message: key.0.clone(),
                    level,
                    at_load: 0,
                    while_running: 0,
                });
                t.seen.insert(key, i);
                i
            }
        };
        let w = &mut t.warnings[i];
        match phase {
            Phase::Load => w.at_load += 1,
            Phase::Run => w.while_running += 1,
            Phase::Off => {}
        }
    }
}

/// Keep the count's one place shared between tests that run playtests.
pub(crate) fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    static ONE: Mutex<()> = Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}
