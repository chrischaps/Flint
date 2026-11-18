//! The player's one output channel (#716): everything it says goes through
//! `tracing`, under a short target that names what it is about, and one
//! subscriber writes it to stderr as text or as JSON lines. Stdout is left
//! for what a flag asks the player to answer (`--list-gpus`).
//!
//! | target       | what                                                    |
//! |--------------|---------------------------------------------------------|
//! | `load`       | each file a scene loads (debug), the end-of-load summary (info) |
//! | `transition` | scene transitions and chunks                            |
//! | `perf`       | `--perf`'s line every 10 s                              |
//! | `report`     | the exit report (`--exit-after`, `quit_game()`)         |
//! | `script`     | what scripts print and log, reloads                     |
//! | `input`      | the controls banner                                     |
//! | `music`      | music and rhythm sessions                               |
//! | `player`     | the player's own notices: debug keys, looks, bundles    |
//! | `listen`     | `--expression-listen`'s socket opening, after the first frame (#584) |
//!
//! `RUST_LOG` replaces the filter whole (`RUST_LOG=load=debug` lists every
//! file again). A load's summary is counted by [`LoadTallyLayer`], which
//! hears each `load` event's `kind` field and every warning, whatever the
//! filter shows.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::{EnvFilter, Layer};
use web_time::Instant;

/// What the player shows by default: warnings, and each target's info.
pub const DEFAULT_FILTER: &str = "warn,load=info,transition=info,perf=info,report=info,\
     script=info,input=info,music=info,player=info,inspect=info,listen=info";

/// `--quiet`: warnings, errors and what a flag asked for (`--perf`'s lines,
/// the exit report, `--inspect-listen`'s and `--dump-world`'s, #713, and
/// `--expression-listen`'s opening, #584).
pub const QUIET_FILTER: &str = "warn,perf=info,report=info,inspect=info,listen=info";

/// How log lines are written.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// One line a message, for a person
    #[default]
    Text,
    /// One JSON object a line: timestamp, level, target, message and the
    /// event's fields
    Json,
}

/// `--log-format` and `--quiet`, flattened into [`crate::PlayerArgs`].
#[derive(clap::Args, Clone, Debug, Default)]
pub struct LogArgs {
    /// How the player writes what it says (to stderr): `text`, or `json`,
    /// one object a line with `timestamp`, `level`, `target`, `message`
    /// and the event's fields. RUST_LOG sets the filter in either
    #[arg(long, value_enum, value_name = "FORMAT", default_value_t = LogFormat::Text)]
    pub log_format: LogFormat,

    /// Say only warnings, errors and what a flag asked for (`--perf`, the
    /// `--exit-after` report): no load summary, controls or transitions
    #[arg(long)]
    pub quiet: bool,
}

impl LogArgs {
    /// The filter these flags ask for, when RUST_LOG doesn't name one
    pub fn default_filter(&self) -> &'static str {
        if self.quiet {
            QUIET_FILTER
        } else {
            DEFAULT_FILTER
        }
    }
}

/// Install the player's subscriber for the process: the format and filter
/// `args` ask for (RUST_LOG over the filter), to stderr, with colour only
/// on a terminal, and the load tally. Does nothing when a subscriber is
/// already installed (a game binary that set its own).
pub fn init(args: &LogArgs) {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|f| !f.trim().is_empty());
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let (subscriber, tally) = subscriber(args, filter.as_deref(), std::io::stderr, ansi);
    // `try_init`, not `set_global_default`: it routes the `log` crate's
    // records (flint-animation's) through the same subscriber
    if tracing_subscriber::util::SubscriberInitExt::try_init(subscriber).is_ok() {
        let _ = INSTALLED.set(tally);
    }
}

/// The player's subscriber, writing to `writer`, and the tally it counts
/// loads into. `filter` (RUST_LOG's syntax) replaces the flags' filter.
pub fn subscriber<W>(
    args: &LogArgs,
    filter: Option<&str>,
    writer: W,
    ansi: bool,
) -> (impl Subscriber + Send + Sync, LoadTally)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let filter = filter
        .and_then(|f| EnvFilter::try_new(f).ok())
        .unwrap_or_else(|| EnvFilter::new(args.default_filter()));
    let fmt: Box<dyn Layer<Registry> + Send + Sync> = match args.log_format {
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(ansi)
            .boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(writer)
            .boxed(),
    };
    let tally = LoadTally::default();
    let subscriber = Registry::default()
        .with(fmt.with_filter(filter))
        .with(tally.layer());
    (subscriber, tally)
}

static INSTALLED: OnceLock<LoadTally> = OnceLock::new();

/// The tally [`init`] installed (or [`tally_layer`] made), if any: what
/// loaded and what went wrong since the last [`LoadTally::take`].
pub fn installed_tally() -> Option<&'static LoadTally> {
    INSTALLED.get()
}

/// The load tally's layer, for a game binary that builds its own
/// subscriber: add it, and the player's load summary counts files and
/// warnings as under [`init`].
pub fn tally_layer<S>() -> impl Layer<S> + Send + Sync
where
    S: Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    INSTALLED.get_or_init(LoadTally::default).layer()
}

/// How many of the first warnings a summary repeats
const FIRST_WARNINGS: usize = 3;

/// What a scene's load said, counted: each `load` event's `kind`, and the
/// warnings and errors of every target, the first few kept.
#[derive(Clone, Default)]
pub struct LoadTally(Arc<Mutex<Counts>>);

#[derive(Default)]
struct Counts {
    since: Option<Instant>,
    kinds: BTreeMap<String, usize>,
    warnings: usize,
    errors: usize,
    first: Vec<String>,
}

impl LoadTally {
    /// The layer that counts into this tally. It hears `load` at every
    /// level and the rest at warn and error, whatever the shown filter.
    pub fn layer<S>(&self) -> impl Layer<S> + Send + Sync
    where
        S: Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
    {
        LoadTallyLayer(self.clone()).with_filter(filter_fn(|meta| {
            meta.target() == "load" || *meta.level() <= Level::WARN
        }))
    }

    /// Everything counted since the last take (or since the tally was
    /// made), and a fresh count from now.
    pub fn take(&self) -> LoadSummary {
        let mut counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let since = counts.since.replace(Instant::now());
        let taken = std::mem::take(&mut *counts);
        counts.since = taken.since;
        LoadSummary {
            kinds: taken.kinds.into_iter().collect(),
            warnings: taken.warnings,
            errors: taken.errors,
            first: taken.first,
            ms: since.map(|s| s.elapsed().as_secs_f64() * 1000.0),
        }
    }
}

/// A [`LoadTally`]'s layer
pub struct LoadTallyLayer(LoadTally);

/// The targets whose warnings a load summary counts: the engine's own. A
/// library's (wgpu's Vulkan loader, gilrs' pad mappings) is shown as it
/// happens and left out of the count, so the summary names the warnings a
/// game can do something about.
fn flint_target(target: &str) -> bool {
    const SHORT: [&str; 11] = [
        "load",
        "transition",
        "perf",
        "report",
        "script",
        "input",
        "music",
        "player",
        "expression",
        "inspect",
        "loopback",
    ];
    target.starts_with("flint") || SHORT.contains(&target)
}

/// How much of a warning a summary quotes: its first line, this many
/// characters at most
const QUOTED_CHARS: usize = 120;

impl<S: Subscriber> Layer<S> for LoadTallyLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let problem = *meta.level() <= Level::WARN;
        if problem && !flint_target(meta.target()) {
            return;
        }
        let mut heard = Heard::default();
        event.record(&mut heard);
        let mut counts = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        if counts.since.is_none() {
            counts.since = Some(Instant::now());
        }
        if problem {
            if *meta.level() == Level::ERROR {
                counts.errors += 1;
            } else {
                counts.warnings += 1;
            }
            if counts.first.len() < FIRST_WARNINGS {
                let line = heard.message.lines().next().unwrap_or("").trim();
                let quoted = match line.char_indices().nth(QUOTED_CHARS) {
                    Some((cut, _)) => format!("{}...", &line[..cut]),
                    None => line.to_string(),
                };
                counts.first.push(quoted);
            }
        } else if meta.target() == "load" {
            let n = heard.count.unwrap_or(1) as usize;
            if let Some(kind) = heard.kind.filter(|_| n > 0) {
                *counts.kinds.entry(kind).or_default() += n;
            }
        }
    }
}

#[derive(Default)]
struct Heard {
    message: String,
    kind: Option<String>,
    /// How many files one event stands for (`[procgen] 3 assets ...`)
    count: Option<u64>,
}

impl Visit for Heard {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "count" {
            self.count = Some(value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "count" {
            self.count = u64::try_from(value).ok();
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "kind" => self.kind = Some(value.to_string()),
            "message" => self.message = value.to_string(),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => {
                self.message.clear();
                let _ = write!(self.message, "{value:?}");
            }
            "kind" => self.kind = Some(format!("{value:?}")),
            _ => {}
        }
    }
}

/// One load, counted: [`LoadTally::take`]'s answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadSummary {
    /// Each kind of file and how many loaded, by name
    pub kinds: Vec<(String, usize)>,
    pub warnings: usize,
    pub errors: usize,
    /// The first few warnings' and errors' messages
    pub first: Vec<String>,
    /// Since the count started
    pub ms: Option<f64>,
}

impl LoadSummary {
    /// `12 models, 3 scripts`: each kind, in name order, plural when it is
    /// not one (`particle_effect` reads `particle effects`)
    pub fn loaded(&self) -> String {
        let parts: Vec<String> = self
            .kinds
            .iter()
            .map(|(kind, n)| {
                let words = kind.replace('_', " ");
                if *n == 1 {
                    format!("1 {words}")
                } else {
                    format!("{n} {}", plural(&words))
                }
            })
            .collect();
        if parts.is_empty() {
            "nothing from files".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// `2 warnings: a; b` (`no warnings` for none), errors counted first
    pub fn problems(&self) -> String {
        let mut said = Vec::new();
        if self.errors > 0 {
            said.push(count(self.errors, "error"));
        }
        if self.warnings > 0 || self.errors == 0 {
            said.push(count(self.warnings, "warning"));
        }
        let mut line = said.join(", ");
        if line == "0 warnings" {
            line = "no warnings".to_string();
        }
        if !self.first.is_empty() {
            line.push_str(": ");
            line.push_str(&self.first.join("; "));
            let more = self.errors + self.warnings - self.first.len();
            if more > 0 {
                let _ = write!(line, "; {more} more");
            }
        }
        line
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn plural(words: &str) -> String {
    if words.ends_with('s') || words.ends_with('x') {
        format!("{words}es")
    } else {
        format!("{words}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A writer the tests read back
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'w> MakeWriter<'w> for Buffer {
        type Writer = Buffer;
        fn make_writer(&'w self) -> Buffer {
            self.clone()
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn a_load() {
        tracing::debug!(target: "load", kind = "model", "Loaded model: crate (1 meshes)");
        tracing::debug!(target: "load", kind = "model", "Loaded model: lamp (2 meshes)");
        tracing::debug!(target: "load", kind = "script", "Loaded: spinner -> spinner.rhai");
        tracing::debug!(target: "load", kind = "particle_effect", "Loaded particle effect: fire");
        tracing::debug!(target: "load", kind = "procgen_asset", count = 3u64, "[procgen] 3 asset(s)");
        tracing::warn!(target: "wgpu_hal::vulkan", "Loader Message\n\tregistry lookup failed");
        tracing::warn!("texture 'moss.png' not found\n  tried: textures/moss.png");
        tracing::info!(target: "transition", "Scene loaded: b.scene.toml");
        tracing::info!(target: "load", scene = "Test", "Loaded scene 'Test'");
        tracing::info!(target: "perf", fps = 59.9, "[perf] 120 frames");
    }

    fn run(args: LogArgs, filter: Option<&str>) -> (String, LoadSummary) {
        let buffer = Buffer::default();
        let (subscriber, tally) = subscriber(&args, filter, buffer.clone(), false);
        tracing::subscriber::with_default(subscriber, a_load);
        (buffer.text(), tally.take())
    }

    #[test]
    fn json_is_one_object_a_line_with_the_target_and_fields() {
        let (text, _) = run(
            LogArgs {
                log_format: LogFormat::Json,
                quiet: false,
            },
            None,
        );
        let lines: Vec<&str> = text.lines().collect();
        // The debug `load` lines are counted, not shown; a message's own
        // newline stays inside its string
        assert_eq!(lines.len(), 5, "{text}");
        let objects: Vec<serde_json::Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}")))
            .collect();
        let targets: Vec<&str> = objects
            .iter()
            .map(|o| o["target"].as_str().unwrap())
            .collect();
        assert_eq!(
            targets,
            [
                "wgpu_hal::vulkan",
                "flint_player::logging::tests",
                "transition",
                "load",
                "perf"
            ]
        );
        assert_eq!(objects[1]["level"], "WARN");
        assert_eq!(
            objects[1]["message"],
            "texture 'moss.png' not found\n  tried: textures/moss.png"
        );
        assert_eq!(objects[3]["scene"], "Test");
        assert_eq!(objects[4]["fps"], 59.9);
        assert!(objects[4]["timestamp"].is_string());
    }

    #[test]
    fn quiet_keeps_warnings_and_perf() {
        let (text, _) = run(
            LogArgs {
                log_format: LogFormat::Text,
                quiet: true,
            },
            None,
        );
        assert!(text.contains("moss.png"), "{text}");
        assert!(text.contains("[perf] 120 frames"), "{text}");
        assert!(!text.contains("Scene loaded"), "{text}");
        assert!(!text.contains("Loaded scene"), "{text}");
    }

    #[test]
    fn the_expression_sockets_opening_is_said_quiet_or_not() {
        for quiet in [false, true] {
            let buffer = Buffer::default();
            let args = LogArgs {
                log_format: LogFormat::Text,
                quiet,
            };
            let (subscriber, _) = subscriber(&args, None, buffer.clone(), false);
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(target: "listen", "[listen] expression: listening on 127.0.0.1:7777");
                tracing::info!(target: "expression", "   0.000s socket: {{}}");
            });
            let text = buffer.text();
            assert!(
                text.contains("[listen] expression"),
                "quiet {quiet}: {text}"
            );
            assert!(
                !text.contains("socket:"),
                "each state is still opt-in: {text}"
            );
        }
    }

    #[test]
    fn a_filter_lists_every_file_again() {
        let (text, _) = run(LogArgs::default(), Some("load=debug"));
        assert!(text.contains("Loaded model: crate"), "{text}");
        assert!(
            !text.contains("[perf]"),
            "the filter replaces the flags': {text}"
        );
    }

    #[test]
    fn the_tally_counts_each_kind_and_the_warnings_whatever_is_shown() {
        let (_, summary) = run(
            LogArgs {
                log_format: LogFormat::Text,
                quiet: true,
            },
            None,
        );
        assert_eq!(
            summary.kinds,
            [
                ("model".to_string(), 2),
                ("particle_effect".to_string(), 1),
                ("procgen_asset".to_string(), 3),
                ("script".to_string(), 1)
            ]
        );
        assert_eq!(
            summary.loaded(),
            "2 models, 1 particle effect, 3 procgen assets, 1 script"
        );
        // A library's warning is shown, not counted; the engine's is
        // quoted by its first line
        assert_eq!(summary.warnings, 1);
        assert_eq!(
            summary.problems(),
            "1 warning: texture 'moss.png' not found"
        );
    }

    #[test]
    fn a_take_starts_a_fresh_count() {
        let buffer = Buffer::default();
        let (subscriber, tally) = subscriber(&LogArgs::default(), None, buffer, false);
        tracing::subscriber::with_default(subscriber, || {
            a_load();
            let first = tally.take();
            assert_eq!(first.warnings, 1);
            let second = tally.take();
            assert_eq!(second.kinds, []);
            assert_eq!(second.problems(), "no warnings");
            assert_eq!(second.loaded(), "nothing from files");
        });
    }

    #[test]
    fn many_problems_name_the_first_few() {
        let summary = LoadSummary {
            warnings: 4,
            errors: 1,
            first: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        assert_eq!(summary.problems(), "1 error, 4 warnings: a; b; c; 2 more");
    }
}
