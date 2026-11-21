//! Every `FLINT_*` environment switch, in one table (#715).
//!
//! A switch is a [`Var`]: its name, what it accepts ([`Kind`]), what happens
//! when it's unset, a line of help and the [`Group`] of runs it changes.
//! [`ALL`] lists them; `flint env [--json]` prints it with each switch's
//! current value, the `--help` of `flint render`, `flint play` and
//! `flint-player` ends with [`help_section`], and the book's
//! `cli-reference/environment.md` is [`book_page`].
//!
//! This is the one module that reads the environment for a `FLINT_*` name:
//! code calls a typed getter on the switch's constant
//! (`env::VSYNC.choice()`, `env::LOD_SCALE.f32()`), never `std::env::var`.
//! `scripts/ci.sh` refuses a `var("FLINT_` anywhere else, and the
//! `env_table` test refuses a `"FLINT_..."` literal that names no switch
//! here, so a new switch can't be read without being listed.
//!
//! A value the switch doesn't accept (`FLINT_VSYNC=fasle`) is said once per
//! process, naming the forms it takes and what is used instead, and the
//! getter returns `None`, as when it's unset: a typo can't silently measure
//! the wrong thing. An empty value is unset.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

/// What runs a switch changes; `--help` lists the groups a command reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// A window's frames: how they're presented and paced (`flint play`,
    /// `flint-player`).
    Window,
    /// The renderer, in a window or headless (`flint render` too).
    Render,
    /// The player: its quality, caches and perf readouts.
    Player,
    /// AI asset generation's provider keys (`flint gen`, `flint asset`).
    Assets,
    /// The test harnesses (`cargo test`), not a shipped run.
    Tests,
}

impl Group {
    pub const ALL: [Group; 5] = [
        Group::Window,
        Group::Render,
        Group::Player,
        Group::Assets,
        Group::Tests,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Group::Window => "window",
            Group::Render => "render",
            Group::Player => "player",
            Group::Assets => "assets",
            Group::Tests => "tests",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Group::Window => "Window",
            Group::Render => "Renderer",
            Group::Player => "Player",
            Group::Assets => "Asset generation",
            Group::Tests => "Test harnesses",
        }
    }
}

/// What a switch accepts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `1`/`on`/`true`/`yes` or `0`/`off`/`false`/`no`, any case.
    Flag,
    /// A number from `min` to `max`, both included; `whole`: an integer.
    Number { min: f64, max: f64, whole: bool },
    /// One of these words, any case.
    Choice(&'static [&'static str]),
    /// A file or folder.
    Path,
    /// A folder, or `0`/`off`/`false` for none.
    PathOrOff,
    /// A key: listed as set or not, its value never shown.
    Secret,
}

const FLAG_ON: [&str; 4] = ["1", "on", "true", "yes"];
const FLAG_OFF: [&str; 4] = ["0", "off", "false", "no"];

impl Kind {
    /// The forms it accepts, as a warning and the help name them.
    pub fn accepts(&self) -> String {
        match *self {
            Kind::Flag => "1|on|true|yes or 0|off|false|no".into(),
            Kind::Number { min, max, whole } => {
                let what = if whole { "a whole number" } else { "a number" };
                match (min.is_finite(), max.is_finite()) {
                    (true, true) => format!("{what} from {min} to {max}"),
                    (true, false) => format!("{what} of at least {min}"),
                    (false, true) => format!("{what} of at most {max}"),
                    (false, false) => what.into(),
                }
            }
            Kind::Choice(words) => words.join("|"),
            Kind::Path => "a path".into(),
            Kind::PathOrOff => "a folder, or 0|off|false for none".into(),
            Kind::Secret => "a key".into(),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Kind::Flag => "flag",
            Kind::Number { whole: true, .. } => "integer",
            Kind::Number { .. } => "number",
            Kind::Choice(_) => "choice",
            Kind::Path => "path",
            Kind::PathOrOff => "path-or-off",
            Kind::Secret => "secret",
        }
    }
}

/// A value a switch accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Flag(bool),
    Number(f64),
    /// The word, lowercased, as the table spells it.
    Choice(&'static str),
    Path(PathBuf),
    /// A [`Kind::PathOrOff`] turned off.
    Off,
    Secret(String),
}

/// One `FLINT_*` switch.
#[derive(Debug)]
pub struct Var {
    pub name: &'static str,
    pub kind: Kind,
    /// What an unset switch means.
    pub default: &'static str,
    /// What it does, in a line.
    pub help: &'static str,
    pub group: Group,
    /// The flag or quality key this switch is the alias of (#1149):
    /// `--vsync`, `--quality lod_scale=`. Given, the flag or key wins.
    pub alias_of: Option<&'static str>,
}

impl Var {
    /// What `raw`, the variable's text, gives: the accepted value, or the
    /// warning a bad one is said with. `raw` is trimmed first; call it only
    /// with a non-empty value (empty is unset).
    pub fn parse(&self, raw: &str) -> Result<Value, String> {
        let v = raw.trim();
        let lower = v.to_ascii_lowercase();
        let ok = match self.kind {
            Kind::Flag => {
                if FLAG_ON.contains(&lower.as_str()) {
                    Some(Value::Flag(true))
                } else if FLAG_OFF.contains(&lower.as_str()) {
                    Some(Value::Flag(false))
                } else {
                    None
                }
            }
            Kind::Number { min, max, whole } => v
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= min && *n <= max)
                .filter(|n| !whole || n.fract() == 0.0)
                .map(Value::Number),
            Kind::Choice(words) => words
                .iter()
                .find(|w| **w == lower)
                .map(|w| Value::Choice(w)),
            Kind::Path => Some(Value::Path(PathBuf::from(v))),
            Kind::PathOrOff => Some(if FLAG_OFF.contains(&lower.as_str()) {
                Value::Off
            } else {
                Value::Path(PathBuf::from(v))
            }),
            Kind::Secret => Some(Value::Secret(v.to_string())),
        };
        ok.ok_or_else(|| {
            format!(
                "{}={raw:?} is not {}: using the default ({})",
                self.name,
                self.kind.accepts(),
                self.default
            )
        })
    }

    /// The variable's text, `None` when it's unset or empty. The one read
    /// of a `FLINT_*` variable in the engine.
    pub fn raw(&self) -> Option<String> {
        let os = std::env::var_os(self.name)?;
        let text = match os.into_string() {
            Ok(s) => s,
            Err(os) => {
                warn_once(self.name, || {
                    format!(
                        "{}={os:?} is not unicode: using the default ({})",
                        self.name, self.default
                    )
                });
                return None;
            }
        };
        (!text.trim().is_empty()).then_some(text)
    }

    /// Whether it holds anything.
    pub fn is_set(&self) -> bool {
        self.raw().is_some()
    }

    /// Its value: `None` when unset, or when the value isn't one it
    /// accepts, which is said once per process.
    pub fn get(&self) -> Option<Value> {
        let raw = self.raw()?;
        match self.parse(&raw) {
            Ok(v) => Some(v),
            Err(warning) => {
                warn_once(self.name, || warning);
                None
            }
        }
    }

    /// A [`Kind::Flag`]'s value.
    pub fn flag(&self) -> Option<bool> {
        debug_assert_eq!(self.kind, Kind::Flag, "{} is not a flag", self.name);
        match self.get()? {
            Value::Flag(b) => Some(b),
            _ => None,
        }
    }

    /// A [`Kind::Number`]'s value.
    pub fn number(&self) -> Option<f64> {
        debug_assert!(
            matches!(self.kind, Kind::Number { .. }),
            "{} is not a number",
            self.name
        );
        match self.get()? {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn f32(&self) -> Option<f32> {
        self.number().map(|n| n as f32)
    }

    /// A whole [`Kind::Number`] (its `min` is at least 0).
    pub fn u32(&self) -> Option<u32> {
        self.number().map(|n| n as u32)
    }

    /// A [`Kind::Choice`]'s word, lowercased as the table spells it.
    pub fn choice(&self) -> Option<&'static str> {
        debug_assert!(
            matches!(self.kind, Kind::Choice(_)),
            "{} is not a choice",
            self.name
        );
        match self.get()? {
            Value::Choice(w) => Some(w),
            _ => None,
        }
    }

    /// A [`Kind::Path`]'s path; a [`Kind::PathOrOff`]'s when it isn't off.
    pub fn path(&self) -> Option<PathBuf> {
        match self.get()? {
            Value::Path(p) => Some(p),
            _ => None,
        }
    }

    /// A [`Kind::Secret`]'s key.
    pub fn secret(&self) -> Option<String> {
        match self.get()? {
            Value::Secret(s) => Some(s),
            _ => None,
        }
    }

    /// What `flint env` says of it now.
    pub fn status(&self) -> Status {
        let raw = self.raw();
        let problem = raw.as_deref().and_then(|r| self.parse(r).err());
        let value = raw.map(|r| {
            if self.kind == Kind::Secret {
                "(set)".to_string()
            } else {
                r
            }
        });
        Status {
            name: self.name,
            group: self.group,
            kind: self.kind.name(),
            accepts: self.kind.accepts(),
            default: self.default,
            help: self.help,
            alias_of: self.alias_of,
            value,
            problem,
        }
    }
}

/// A switch as `flint env --json` lists it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub name: &'static str,
    pub group: Group,
    pub kind: &'static str,
    pub accepts: String,
    pub default: &'static str,
    pub help: &'static str,
    /// The flag or quality key that wins over it, for an alias (#1149).
    pub alias_of: Option<&'static str>,
    /// What it's set to (a secret's is `(set)`); `None` when unset.
    pub value: Option<String>,
    /// Why the value isn't used, when it isn't one it accepts.
    pub problem: Option<String>,
}

/// The names already warned about.
static WARNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

fn warn_once(name: &'static str, warning: impl FnOnce() -> String) {
    let first = WARNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name);
    if first {
        tracing::warn!("{}", warning());
    }
}

const UNBOUNDED: f64 = f64::INFINITY;

macro_rules! alias {
    () => {
        None
    };
    ($alias:literal) => {
        Some($alias)
    };
}

/// The words `FLINT_VSYNC` and `--vsync` take (`flint_render::Vsync`).
pub const VSYNC_WORDS: [&str; 7] = ["1", "on", "fifo", "0", "off", "immediate", "mailbox"];

macro_rules! vars {
    ($($id:ident = $name:literal, $group:ident, $kind:expr, $(alias $alias:literal,)? default $default:literal, $help:literal;)+) => {
        $(
            #[doc = $help]
            pub static $id: Var = Var {
                name: $name,
                kind: $kind,
                default: $default,
                help: $help,
                group: Group::$group,
                alias_of: alias!($($alias)?),
            };
        )+
        /// Every switch, in the order `flint env` lists them.
        pub static ALL: &[&Var] = &[$(&$id),+];
    };
}

vars! {
    VSYNC = "FLINT_VSYNC", Window,
        Kind::Choice(&VSYNC_WORDS),
        alias "--vsync",
        default "vsync (fifo)",
        "How frames are presented: 1|on|fifo waits for vsync, 0|off|immediate is unlocked (real frame times for perf runs), mailbox renders uncapped and shows the newest frame each refresh";
    FRAME_LATENCY = "FLINT_FRAME_LATENCY", Window,
        Kind::Number { min: 1.0, max: 3.0, whole: true },
        alias "--frame-latency",
        default "2",
        "Frames the CPU may queue ahead of the display";
    PACING = "FLINT_PACING", Window,
        Kind::Flag,
        alias "--no-pacing",
        default "on",
        "Pace the game clock to the monitor's refresh when frames wait for vsync; 0 turns it off";

    LOD_SCALE = "FLINT_LOD_SCALE", Render,
        Kind::Number { min: 0.05, max: UNBOUNDED, whole: false },
        alias "--quality lod_scale=",
        default "1",
        "Multiplies every LOD switch distance (under 1 drops to coarser levels sooner)";
    DRAW_DISTANCE_SCALE = "FLINT_DRAW_DISTANCE_SCALE", Render,
        Kind::Number { min: 0.05, max: UNBOUNDED, whole: false },
        alias "--quality draw_distance_scale=",
        default "1",
        "Multiplies every entity's draw distance";
    STYLE_SCISSOR = "FLINT_STYLE_SCISSOR", Render,
        Kind::Flag,
        default "on",
        "Per-entity styles scissor their passes to their entities; 0 draws them full-frame, to check the two match byte for byte";
    STYLE_SHADER_DIR = "FLINT_STYLE_SHADER_DIR", Render,
        Kind::Path,
        default "the built-in shaders",
        "Read the style shaders from this folder (crates/flint-render/src/stylize) and rebuild on save, without a rebuild of the engine";
    SHADER_CHECKS = "FLINT_SHADER_CHECKS", Render,
        Kind::Choice(&["all", "default"]),
        default "every check but loop bounding",
        "all also keeps loop bounding in the engine's own shaders (it costs ASCII's pass 2.6x GPU)";
    GPU_LABELS = "FLINT_GPU_LABELS", Render,
        Kind::Choice(&["0", "off", "groups", "1", "on", "draws"]),
        default "draws under a capture tool, else groups",
        "How much of a frame is named for GPU debuggers (RenderDoc, PIX): off, groups (each part of the frame) or draws (a marker per draw too)";
    GPU_WAIT_TIMEOUT = "FLINT_GPU_WAIT_TIMEOUT", Render,
        Kind::Number { min: 0.0, max: UNBOUNDED, whole: false },
        default "no limit",
        "Seconds any wait for the GPU may take before the process panics naming a hung device; 0 is no limit";

    FRAME_HISTORY = "FLINT_FRAME_HISTORY", Player,
        Kind::Flag,
        default "the build's frame-history feature (on)",
        "Record every frame for the F2 frame history";
    HITCH_FACTOR = "FLINT_HITCH_FACTOR", Player,
        Kind::Number { min: 1.0, max: UNBOUNDED, whole: false },
        default "2",
        "A frame is a hitch over this many times the rolling median frame time";
    HITCH_FLOOR_MS = "FLINT_HITCH_FLOOR_MS", Player,
        Kind::Number { min: 0.0, max: UNBOUNDED, whole: false },
        default "24",
        "...and at least this many milliseconds over it, at any refresh rate";
    PERF_LOG = "FLINT_PERF_LOG", Player,
        Kind::Path,
        default "stderr only",
        "Also append each perf summary line to this file (Android sets it beside a perf profile)";
    SHADOWS = "FLINT_SHADOWS", Player,
        Kind::Flag,
        alias "--quality shadows=",
        default "on",
        "0 starts with the shadow pass off";
    SHADOW_RES = "FLINT_SHADOW_RES", Player,
        Kind::Number { min: 256.0, max: 4096.0, whole: true },
        alias "--quality shadow_resolution=",
        default "2048",
        "Each shadow cascade's map, texels on a side";
    PROCGEN_CACHE = "FLINT_PROCGEN_CACHE", Player,
        Kind::PathOrOff,
        default ".flint/procgen-cache/ at the project root",
        "Where procedural generation caches what it built on disk; 0 keeps no disk cache";

    FLUX_API_KEY = "FLINT_FLUX_API_KEY", Assets,
        Kind::Secret,
        default "the key in flint.toml's [providers.flux]",
        "The Flux provider's API key, over the config file's";
    MESHY_API_KEY = "FLINT_MESHY_API_KEY", Assets,
        Kind::Secret,
        default "the key in flint.toml's [providers.meshy]",
        "The Meshy provider's API key, over the config file's";
    ELEVENLABS_API_KEY = "FLINT_ELEVENLABS_API_KEY", Assets,
        Kind::Secret,
        default "the key in flint.toml's [providers.elevenlabs]",
        "The ElevenLabs provider's API key, over the config file's";

    REQUIRE_GPU = "FLINT_REQUIRE_GPU", Tests,
        Kind::Flag,
        default "off",
        "A GPU test that finds no adapter fails instead of passing as skipped (CI's GPU job sets it)";
    BLESS = "FLINT_BLESS", Tests,
        Kind::Flag,
        default "off",
        "Golden tests rewrite their goldens instead of comparing against them";
    GOLDENS_OUT = "FLINT_GOLDENS_OUT", Tests,
        Kind::Path,
        default "a temporary folder, removed on a pass",
        "Where render_goldens writes its renders and diffs, kept";
    GPU_TEST_PARALLEL = "FLINT_GPU_TEST_PARALLEL", Tests,
        Kind::Flag,
        default "on",
        "GPU tests share the device alongside each other; 0 lends it one test at a time";
    GPU_TEST_PIPELINE_CACHE = "FLINT_GPU_TEST_PIPELINE_CACHE", Tests,
        Kind::Flag,
        default "on",
        "GPU tests share built pipelines; 0 builds each test's own (an A/B of build cost)";
    GPU_TEST_TIMES = "FLINT_GPU_TEST_TIMES", Tests,
        Kind::Flag,
        default "off",
        "Print a line per GPU test: how long it held the device and built pipelines";
    GPU_TEST_TIMEOUT = "FLINT_GPU_TEST_TIMEOUT", Tests,
        Kind::Number { min: 1.0, max: UNBOUNDED, whole: true },
        default "the harness's DEFAULT_TIMEOUT_S",
        "Seconds a GPU test may hold the device before the binary is called hung and aborted";
    QUALITY_PNG = "FLINT_QUALITY_PNG", Tests,
        Kind::Path,
        default "none",
        "flint-render's quality test writes its frames to this folder";
    PANELS_DUMP = "FLINT_PANELS_DUMP", Tests,
        Kind::Path,
        default "none",
        "flint-render's panels test writes a frame to this folder";
    CUBIC_MODEL = "FLINT_CUBIC_MODEL", Tests,
        Kind::Path,
        default "skipped",
        "A glTF with CUBICSPLINE animation for flint-animation's opt-in cubicspline_model test";
    SAVE_194_DIR = "FLINT_194_SAVE_DIR", Tests,
        Kind::Path,
        default "none",
        "gen_preview's spec-save test writes the old and new saves to this folder (#194)";
    LIVING_PRINT_DIGEST = "FLINT_LIVING_PRINT_DIGEST", Tests,
        Kind::Flag,
        default "off",
        "Set by Living Props' determinism test for the copy of itself it runs";
    FORM_PRINT_DIGEST = "FLINT_FORM_PRINT_DIGEST", Tests,
        Kind::Flag,
        default "off",
        "Set by the form generator's determinism test for the copy of itself it runs";
}

/// What a run of the player reads: `flint play`'s and `flint-player`'s
/// `--help` list these groups.
pub const PLAYER_GROUPS: [Group; 3] = [Group::Window, Group::Render, Group::Player];

/// The switch named `name`.
pub fn find(name: &str) -> Option<&'static Var> {
    ALL.iter().copied().find(|v| v.name == name)
}

/// Every switch as `flint env` lists it now.
pub fn statuses() -> Vec<Status> {
    ALL.iter().map(|v| v.status()).collect()
}

/// The end of a command's `--help`: the switches of `groups`, a line each.
pub fn help_section(groups: &[Group]) -> String {
    let vars: Vec<&Var> = ALL
        .iter()
        .copied()
        .filter(|v| groups.contains(&v.group))
        .collect();
    let width = vars.iter().map(|v| v.name.len()).max().unwrap_or(0);
    let mut out = String::from(
        "Environment (`flint env` lists every switch, what it accepts and its value now):\n",
    );
    for v in vars {
        let alias = v
            .alias_of
            .map(|a| format!(" (alias of {a})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {:width$}  {}{alias}\n",
            v.name,
            first_clause(v.help),
            width = width
        ));
    }
    out
}

/// The line `flint env` and the book give an alias: what wins over it.
pub fn alias_line(alias_of: &str) -> String {
    format!("Alias of `{alias_of}`, which wins when given")
}

/// The help up to its first `:` or `;`, for a line of `--help`.
fn first_clause(help: &str) -> &str {
    help.split([':', ';']).next().unwrap_or(help).trim_end()
}

/// The book's `cli-reference/environment.md`.
pub fn book_page() -> String {
    let mut out = String::from(
        "# Environment Switches\n\n\
         <!-- Generated by `flint env --book`; edit crates/flint-core/src/env.rs, not this page.\n     \
         CI's headless job runs `flint env --check-book`. -->\n\n\
         Flint reads a few `FLINT_*` environment variables, for what a flag can't reach easily \
         (a perf A/B, a test harness, a device profile). Each is listed here from the one table \
         the engine reads them through (`flint_core::env`). `flint env` prints the same list \
         with each switch's value in this shell, and `flint env --json` as data.\n\n\
         A value a switch doesn't accept is said once, as a warning naming the forms it takes, \
         and the default is used: `FLINT_VSYNC=fasle` warns rather than measuring with vsync on. \
         An empty value is the same as unset.\n\n\
         A switch that is the alias of a flag or a quality key (`FLINT_VSYNC` of `--vsync`, \
         `FLINT_LOD_SCALE` of `--quality lod_scale=`) says so: the flag or key wins when given, \
         and the variable is read only when it isn't.\n",
    );
    for group in Group::ALL {
        out.push_str(&format!("\n## {}\n\n", group.title()));
        out.push_str("| Variable | Accepts | Unset | What it does |\n|---|---|---|---|\n");
        for v in ALL.iter().filter(|v| v.group == group) {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                v.name,
                v.kind.accepts().replace('|', "\\|"),
                v.default,
                match v.alias_of {
                    Some(a) => format!("{}. {}", v.help, alias_line(a)),
                    None => v.help.to_string(),
                }
                .replace('|', "\\|")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_flint_prefixed() {
        let mut seen = BTreeSet::new();
        for v in ALL {
            assert!(v.name.starts_with("FLINT_"), "{}", v.name);
            assert!(seen.insert(v.name), "{} twice", v.name);
            assert!(!v.help.is_empty() && !v.default.is_empty(), "{}", v.name);
        }
    }

    #[test]
    fn flags_take_every_spelling_and_refuse_a_typo() {
        for on in ["1", "on", "TRUE", " yes "] {
            assert_eq!(PACING.parse(on), Ok(Value::Flag(true)), "{on}");
        }
        for off in ["0", "Off", "false", "no"] {
            assert_eq!(PACING.parse(off), Ok(Value::Flag(false)), "{off}");
        }
        let warning = PACING.parse("fasle").unwrap_err();
        assert!(warning.contains("FLINT_PACING=\"fasle\""), "{warning}");
        assert!(
            warning.contains("1|on|true|yes or 0|off|false|no"),
            "{warning}"
        );
        assert!(warning.contains("the default (on)"), "{warning}");
    }

    #[test]
    fn numbers_hold_their_range() {
        assert_eq!(FRAME_LATENCY.parse(" 3 "), Ok(Value::Number(3.0)));
        for bad in ["0", "4", "2.5", "two", "inf", "NaN"] {
            let w = FRAME_LATENCY.parse(bad).unwrap_err();
            assert!(w.contains("a whole number from 1 to 3"), "{bad}: {w}");
        }
        assert_eq!(LOD_SCALE.parse("0.5"), Ok(Value::Number(0.5)));
        assert!(LOD_SCALE
            .parse("0")
            .unwrap_err()
            .contains("a number of at least 0.05"));
        assert_eq!(HITCH_FLOOR_MS.parse("0"), Ok(Value::Number(0.0)));
        assert!(HITCH_FACTOR.parse("0.5").is_err());
    }

    #[test]
    fn choices_are_any_case_and_name_their_words() {
        assert_eq!(VSYNC.parse("Mailbox"), Ok(Value::Choice("mailbox")));
        let w = VSYNC.parse("fasle").unwrap_err();
        assert!(
            w.contains(
                "is not 1|on|fifo|0|off|immediate|mailbox: using the default (vsync (fifo))"
            ),
            "{w}"
        );
    }

    #[test]
    fn a_path_can_be_off_and_a_secret_is_never_shown() {
        assert_eq!(PROCGEN_CACHE.parse("off"), Ok(Value::Off));
        assert_eq!(
            PROCGEN_CACHE.parse(" C:/cache "),
            Ok(Value::Path(PathBuf::from("C:/cache")))
        );
        // Statuses redact a secret's value; parse keeps it for the reader
        assert_eq!(FLUX_API_KEY.parse("k"), Ok(Value::Secret("k".to_string())));
    }

    #[test]
    fn help_lists_only_the_groups_asked_for() {
        let help = help_section(&[Group::Render]);
        assert!(help.contains("FLINT_LOD_SCALE"));
        assert!(!help.contains("FLINT_VSYNC"));
        assert!(!help.contains("FLINT_BLESS"));
        // A line each, cut at its first clause
        assert!(help.contains("How much of a frame is named for GPU debuggers (RenderDoc, PIX)\n"));
    }

    /// The issue's third box: every switch a flag or quality key took over
    /// names it, in `--help` and the book, and the quality keys are real
    #[test]
    fn each_alias_names_its_flag_or_key() {
        let aliases: Vec<(&str, &str)> = ALL
            .iter()
            .filter_map(|v| Some((v.name, v.alias_of?)))
            .collect();
        assert_eq!(
            aliases,
            [
                ("FLINT_VSYNC", "--vsync"),
                ("FLINT_FRAME_LATENCY", "--frame-latency"),
                ("FLINT_PACING", "--no-pacing"),
                ("FLINT_LOD_SCALE", "--quality lod_scale="),
                (
                    "FLINT_DRAW_DISTANCE_SCALE",
                    "--quality draw_distance_scale="
                ),
                ("FLINT_SHADOWS", "--quality shadows="),
                ("FLINT_SHADOW_RES", "--quality shadow_resolution="),
            ]
        );
        let all_groups = [
            Group::Window,
            Group::Render,
            Group::Player,
            Group::Assets,
            Group::Tests,
        ];
        let help = help_section(&all_groups);
        let page = book_page();
        for (name, alias) in aliases {
            if let Some(key) = alias.strip_prefix("--quality ") {
                let key = key.trim_end_matches('=');
                assert!(
                    crate::quality::QualitySettings::KEYS.contains(&key),
                    "{name}: {key}"
                );
            }
            let line = help.lines().find(|l| l.contains(name)).unwrap();
            assert!(line.ends_with(&format!("(alias of {alias})")), "{line}");
            let cell = format!("| `{name}` |");
            let row = page.lines().find(|l| l.starts_with(&cell)).unwrap();
            assert!(row.contains(&alias_line(alias)), "{row}");
        }
    }

    #[test]
    fn the_book_page_escapes_pipes_in_cells() {
        let page = book_page();
        assert!(page.contains("| `FLINT_VSYNC` | 1\\|on\\|fifo"));
        for line in page.lines().filter(|l| l.starts_with("| `FLINT_")) {
            let cells = line.replace("\\|", "").matches('|').count();
            assert_eq!(cells, 5, "{line}");
        }
    }
}
