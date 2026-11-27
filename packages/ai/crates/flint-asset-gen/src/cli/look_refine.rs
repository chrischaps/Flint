//! `flint render --look <x> --reference ref.png --ai-refine` (#357): a
//! measured refine loop for looks, the way `flint gen --ai-refine` refines
//! a procgen spec (#183). The ai package's since #1572: its flags
//! ([`RefineArgs`]) go in the slot the kernel's `render` declares, and the
//! render runs it through the [`LookRefines`] point this package registers,
//! lending a [`RefineHost`] that draws and measures.
//!
//! Each round an agent sees the reference, the look's current render, its
//! `[post_process]` block, the keys it may change (with their ranges) and
//! every edit tried so far with the distance it scored. It proposes a few
//! candidate edits in `--sweep` key syntax (`ascii.brightness=0.25`,
//! `post.exposure=1.3`). Each candidate is drawn and measured against the
//! reference by the host (#183's metric, or with the reference's glyph
//! grid, #495's, which reads an ASCII frame as letters of some ink rather
//! than as pixel texture); the best one is kept if it moves the distance
//! down, and the loop stops when no candidate does, or after
//! `--refine-rounds`. What's kept is written as a new `.look.toml`, the run
//! as a JSON log beside it, and the rounds as a contact sheet (reference,
//! start, each kept round).
//!
//! Agents: `anthropic` asks Claude, every call through a [`CostMeter`]
//! (priced, ledgered in `.flint/procgen_costs.json`, refused before sending
//! past `--max-cost`). `mock` is free and offline: it walks the look's own
//! `tune` sweeps, one untried value at a time, so the loop is a measured
//! coordinate search, the same loop with a simpler proposer.

use anyhow::{bail, Context, Result};
use clap::ArgMatches;
use flint_package::cli::look::{
    DistanceReport, ImageMeasures, Look, LookRefine, LookRefines, Metric, RefineHost,
};
use flint_package::cli::render::RenderPlace;
use flint_package::cli::{flags, say, Flags};
use flint_procgen_ai::cost::CostMeter;
use flint_procgen_ai::{AgentConfig, AnthropicAgent};
use image::{RgbImage, RgbaImage};
use serde::Serialize;
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `--ai-refine`: a look refined toward a reference. Added to the kernel's
/// `render` at its slot (`RenderFlags`), so `flint render --help` lists
/// them under the heading they have always had.
#[derive(clap::Args)]
#[command(next_help_heading = "Refine a look")]
pub struct RefineArgs {
    /// Refine --look toward --reference (#357): an agent proposes edits,
    /// each is rendered and measured, the best is kept while the distance
    /// falls. Writes the refined look (--refine-out), a JSON log beside it
    /// and a sheet of the rounds (-o). Calls go through the cost meter and
    /// ledger (.flint/procgen_costs.json)
    #[arg(long, requires_all = ["look", "reference"],
          conflicts_with_all = ["sweep", "frames", "mesh", "watch", "strip", "variant", "variants"])]
    pub ai_refine: bool,

    /// Most refine rounds
    #[arg(long, default_value_t = 4)]
    pub refine_rounds: u32,

    /// Candidates rendered a round
    #[arg(long, default_value_t = 4)]
    pub refine_candidates: usize,

    /// Where the refined look goes [default: <look>_refined.look.toml
    /// beside -o]; its log is the same path with .json
    #[arg(long, value_name = "FILE.look.toml")]
    pub refine_out: Option<String>,

    /// Who proposes refine edits: `anthropic` (Claude; ANTHROPIC_API_KEY)
    /// or `mock` (free, offline: walks the look's tune sweeps)
    #[arg(long, default_value = "anthropic")]
    pub ai_provider: String,

    /// The Claude model for --ai-refine
    #[arg(long)]
    pub ai_model: Option<String>,

    /// Refuse any call that could take the run's spend past this (USD)
    #[arg(long)]
    pub max_cost: Option<f64>,
}

/// The group on `render`, as the package's extension returns it.
pub fn flags() -> Flags {
    Flags::of::<RefineArgs>(<RenderPlace as flags::FlagPlace>::PATH)
}

/// The point this package registers: `render --ai-refine`.
pub fn point() -> LookRefines {
    LookRefines(Box::new(Refine))
}

/// The refine, as the render asks it.
struct Refine;

impl Refine {
    fn args(flags: Option<&ArgMatches>) -> Option<RefineArgs> {
        flags::parsed::<RefineArgs>(flags, "ai_refine")
    }
}

impl LookRefine for Refine {
    fn asked(&self, flags: Option<&ArgMatches>) -> bool {
        Self::args(flags).is_some_and(|a| a.ai_refine)
    }

    fn run(&self, flags: Option<&ArgMatches>, host: &mut dyn RefineHost) -> Result<()> {
        let args = Self::args(flags).context("the render's line has no --ai-refine")?;
        refine_look(&args, host)
    }
}

/// A distance must fall by more than this (score points out of 100) for a
/// round to count as a step: below it is render noise and rounding.
pub const MIN_GAIN: f64 = 0.05;

/// Rounds in a row without a gain before the loop stops.
pub const PATIENCE: u32 = 2;

/// One change to a look: a `--sweep` key and one value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edit {
    pub key: String,
    pub value: String,
}

impl std::fmt::Display for Edit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.key, self.value)
    }
}

/// A set of edits tried together.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub edits: Vec<Edit>,
    /// The agent's reason, if it gave one.
    pub why: String,
}

impl Candidate {
    pub fn label(&self) -> String {
        self.edits
            .iter()
            .map(Edit::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A candidate after it was rendered (or refused).
#[derive(Debug, Clone, Serialize)]
pub struct Tried {
    pub round: u32,
    pub candidate: Candidate,
    /// The distance it scored, or `None` when it was refused.
    pub distance: Option<DistanceReport>,
    /// Why it was refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    pub image: Option<ImageMeasures>,
    pub kept: bool,
}

/// What an agent is shown each round.
pub struct Ask<'a> {
    pub round: u32,
    pub look: &'a Look,
    /// The current (best so far) `[post_process]`.
    pub current: &'a toml::Table,
    pub current_png: &'a [u8],
    pub current_distance: &'a DistanceReport,
    /// The current frame's measures, as one line.
    pub current_measures: &'a str,
    /// The current frame's glyph grid, when the ASCII style drew.
    pub current_ink: Option<&'a str>,
    pub reference_png: &'a [u8],
    pub reference_measures: &'a str,
    /// The reference's glyph grid, when it was given (`--reference-text`).
    pub reference_ink: Option<&'a str>,
    pub history: &'a [Tried],
    /// How many candidates to propose.
    pub want: usize,
    /// The render, for what `--sweep` knows of a key.
    pub host: &'a dyn RefineHost,
}

/// Something that proposes edits.
pub trait LookAgent {
    fn name(&self) -> String;
    fn propose(&self, ask: &Ask) -> Result<Vec<Candidate>>;
}

// --- The mock agent ---------------------------------------------------------

/// Walks the look's `tune` sweeps: each round, the next untried value of
/// each tune key in turn, so a round tries several keys. Free, offline,
/// deterministic.
pub struct TuneWalker;

impl LookAgent for TuneWalker {
    fn name(&self) -> String {
        "mock (tune walk)".into()
    }

    fn propose(&self, ask: &Ask) -> Result<Vec<Candidate>> {
        let host = ask.host;
        let sweeps: Vec<_> = ask.look.tune.iter().filter_map(|t| host.tune(t)).collect();
        // Tried over the current block: a keep changes the base, so what
        // missed before it is worth another look
        let kept_round = ask.history.iter().filter(|t| t.kept).map(|t| t.round).max();
        let since = ask
            .history
            .iter()
            .position(|t| kept_round.is_none_or(|r| t.round > r))
            .unwrap_or(ask.history.len());
        let tried = |e: &Edit| {
            ask.history[since..]
                .iter()
                .any(|t| t.candidate.edits.len() == 1 && &t.candidate.edits[0] == e)
        };
        let mut out = Vec::new();
        // A round each value deep, across every key, until `want` are found
        let depth = sweeps.iter().map(|s| s.values.len()).max().unwrap_or(0);
        for d in 0..depth {
            for s in &sweeps {
                if out.len() >= ask.want {
                    return Ok(out);
                }
                let Some(v) = s.values.get(d) else { continue };
                let edit = Edit {
                    key: s.key.clone(),
                    value: v.clone(),
                };
                // The value it has now, or started from, needs no render
                let v_toml = host.value(v);
                let same = [
                    host.current(ask.current, &s.key),
                    host.current(&ask.look.post_process, &s.key),
                ]
                .iter()
                .flatten()
                .any(|c| same_value(c, &v_toml));
                if same || tried(&edit) || out.iter().any(|c: &Candidate| c.edits[0] == edit) {
                    continue;
                }
                out.push(Candidate {
                    edits: vec![edit],
                    why: format!("tune sweep {}", s.key),
                });
            }
        }
        Ok(out)
    }
}

/// Equal as TOML, with `9` and `9.0` the same number.
fn same_value(a: &toml::Value, b: &toml::Value) -> bool {
    let num = |v: &toml::Value| match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        _ => None,
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

// --- Claude -----------------------------------------------------------------

/// Asks Claude, through the meter.
pub struct ClaudeLooks {
    pub agent: AnthropicAgent,
}

const SYSTEM: &str = "You tune post-processing looks for Flint, a 3D engine. A look is a \
[post_process] block (TOML) merged over a scene's own. You are shown a REFERENCE image (the \
target), the CURRENT render of the scene under the look, the look's block, the keys you may \
change with their ranges, measures of both pictures, and every edit tried so far with the \
distance it scored to the reference: a score out of 100, lower is closer, the sum of the terms \
listed beside it (the message says what each term measures). The scene, camera and models never \
change: \
only the look does, so aim at tone, colour, contrast, cell size, line work and grain, not \
content. Propose candidate edits that should move the CURRENT picture toward the REFERENCE. \
Each candidate is one to three edits applied together over the current block. Keys: \
`post.<field>` for a [post_process] field, `<style>.<key>` for a style's param or option \
(e.g. `ascii.brightness`, `ascii.ramp`); a style key does nothing unless that style is \
enabled (`<style>.enabled=true`). Values: a number, a bool, a string for an option, or an \
array of three numbers for a colour or vector. Stay inside the ranges. Don't repeat an edit \
already tried at the same value; learn from the scores. Reply with ONE JSON object and \
nothing else: {\"observations\": \"what differs between the pictures\", \"candidates\": \
[{\"edits\": {\"<key>\": <value>, ...}, \"why\": \"...\"}, ...]}";

impl LookAgent for ClaudeLooks {
    fn name(&self) -> String {
        format!("anthropic {}", self.agent.config().model)
    }

    fn propose(&self, ask: &Ask) -> Result<Vec<Candidate>> {
        let text = ask_text(ask);
        let messages = [json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "REFERENCE:"},
                AnthropicAgent::image_content(ask.reference_png)?,
                {"type": "text", "text": "CURRENT render under the look:"},
                AnthropicAgent::image_content(ask.current_png)?,
                {"type": "text", "text": text},
            ],
        })];
        let reply = self.agent.ask(
            &format!("look:{}", ask.look.name),
            "look-refine",
            SYSTEM,
            &messages,
        )?;
        let obj = flint_procgen_ai::http::extract_json_object(&reply.text)
            .with_context(|| format!("round {}: the reply wasn't a JSON object", ask.round))?;
        if let Some(o) = obj.get("observations").and_then(Json::as_str) {
            say(format!("  claude: {o}"));
        }
        Ok(parse_candidates(obj.get("candidates")))
    }
}

/// The candidates in a reply. A malformed entry is skipped, not fatal: the
/// rest are still worth rendering.
pub fn parse_candidates(v: Option<&Json>) -> Vec<Candidate> {
    let Some(list) = v.and_then(Json::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let edits = c.get("edits")?.as_object()?;
            let edits: Vec<Edit> = edits
                .iter()
                .filter_map(|(k, v)| {
                    let value = match v {
                        Json::String(s) => s.clone(),
                        Json::Number(n) => n.to_string(),
                        Json::Bool(b) => b.to_string(),
                        Json::Array(a) => a
                            .iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                        _ => return None,
                    };
                    Some(Edit {
                        key: k.trim().to_string(),
                        value,
                    })
                })
                .collect();
            (!edits.is_empty()).then(|| Candidate {
                edits,
                why: c
                    .get("why")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// The keys an agent may change: every param and option of each style the
/// block names, then the [post_process] fields.
pub fn key_catalogue(current: &toml::Table) -> String {
    let mut out = String::new();
    let styles: Vec<String> = current
        .get("styles")
        .and_then(toml::Value::as_table)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    for name in &styles {
        // A look's styles are the built-ins' (#742: a look carries no
        // registry of its own)
        let Some(d) = flint_render::stylize::builtin(name) else {
            continue;
        };
        out.push_str(&format!("style {name}:\n"));
        for p in &d.params {
            out.push_str(&format!(
                "  {name}.{} = {} (range {}..{})\n",
                p.name, p.default, p.min, p.max
            ));
        }
        for o in &d.options {
            let choices = if o.choices.is_empty() {
                String::new()
            } else {
                format!(" one of {}", o.choices.join("|"))
            };
            out.push_str(&format!(
                "  {name}.{} = \"{}\"{choices}\n",
                o.name, o.default
            ));
        }
    }
    let others: Vec<&str> = flint_render::stylize::builtins()
        .iter()
        .map(|d| d.name)
        .filter(|n| !styles.iter().any(|s| s == n))
        .collect();
    out.push_str(&format!(
        "other styles (enable with <style>.enabled=true): {}\n",
        others.join(", ")
    ));
    out.push_str(&format!(
        "post fields: {}\n",
        flint_scene::PostProcessDef::field_names()
            .iter()
            .filter(|f| *f != "styles")
            .map(|f| format!("post.{f}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out
}

/// What the score's terms measure, for the agent.
fn metric_text(metric: Metric) -> &'static str {
    match metric {
        Metric::Pixels => {
            "The distance reads both pictures as pixels: mean colour, a 16-bin luminance \
             histogram and a 16x16 luminance layout."
        }
        Metric::Glyphs => {
            "Both pictures are ASCII glyph grids, so the distance reads them as glyphs, each \
             cell the ink of the letter it shows (0 blank, 1 the densest letter): mean colour \
             (pixels), glyph coverage (how far the mix of blank, light and dense letters is \
             from the reference's) and glyph layout (where the dense letters sit, over 16x16 \
             regions). A halo or blur that smears ink into the ground doesn't change a cell's \
             letter; the ramp, cell size, brightness, contrast and tone keys do."
        }
    }
}

fn ask_text(ask: &Ask) -> String {
    let block = toml::to_string(ask.current).unwrap_or_default();
    let mut history = String::new();
    for t in ask.history {
        let score = match (&t.distance, &t.refused) {
            (Some(d), _) => format!("{:.2} ({})", d.score, d.terms_line()),
            (None, Some(why)) => format!("refused: {why}"),
            _ => "-".into(),
        };
        history.push_str(&format!(
            "  round {}: {} -> {}{}\n",
            t.round,
            t.candidate.label(),
            score,
            if t.kept { " (kept)" } else { "" }
        ));
    }
    if history.is_empty() {
        history.push_str("  (none yet)\n");
    }
    let glyphs = match (ask.reference_ink, ask.current_ink) {
        (None, None) => String::new(),
        (r, c) => {
            let line = |g: Option<&str>| g.unwrap_or("no glyph grid").to_string();
            format!(
                "CURRENT glyphs:     {}\nREFERENCE glyphs:   {}\n",
                line(c),
                line(r)
            )
        }
    };
    format!(
        "Round {round}. Look `{name}`: {desc}\nIts notes: {notes}\nIts suggested sweeps: {tune}\n\n\
         {metric}\nCURRENT distance to the reference: {d:.2} = {terms}\n\
         CURRENT measures:   {cm}\nREFERENCE measures: {rm}\n{glyphs}\n\
         The look's [post_process] block now:\n```toml\n{block}```\n\nKeys you may change:\n{keys}\n\
         Tried so far:\n{history}\nPropose up to {want} candidates.",
        round = ask.round,
        name = ask.look.name,
        desc = ask.look.description,
        notes = ask.look.notes.trim(),
        tune = ask.look.tune.join("; "),
        metric = metric_text(ask.current_distance.metric),
        d = ask.current_distance.score,
        terms = ask.current_distance.terms_line(),
        cm = ask.current_measures,
        rm = ask.reference_measures,
        keys = key_catalogue(ask.current),
        want = ask.want,
    )
}

// --- The loop ---------------------------------------------------------------

/// What the run is asked to do: how long it may go on. The look and the
/// reference are the host's.
pub struct Options {
    pub rounds: u32,
    pub candidates: usize,
}

/// The run, as logged.
#[derive(Debug, Clone, Serialize)]
pub struct RefineLog {
    pub look: String,
    pub look_source: String,
    pub reference: String,
    pub agent: String,
    /// Which terms the scores are made of.
    pub metric: Metric,
    pub start: DistanceReport,
    pub end: DistanceReport,
    pub rounds_run: u32,
    pub stopped: String,
    /// Every candidate rendered (or refused), in order.
    pub tried: Vec<Tried>,
    /// The edits kept, in order.
    pub kept: Vec<Edit>,
    pub calls: u64,
    pub cost_usd: f64,
    #[serde(skip)]
    pub post_process: toml::Table,
    /// The pictures for the sheet: start, then each kept round's.
    #[serde(skip)]
    pub frames: Vec<(String, RgbaImage)>,
}

fn png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn rgba(img: &RgbImage) -> RgbaImage {
    image::DynamicImage::ImageRgb8(img.clone()).to_rgba8()
}

/// A candidate's score line: the score, then its terms in points.
fn scored(d: &DistanceReport) -> String {
    format!("{:.2}  ({})", d.score, d.terms_line())
}

/// Run the loop over what `host` lends: its look toward its reference,
/// each candidate drawn and scored by it. `meter` is read for the log.
pub fn run(
    opts: Options,
    agent: &dyn LookAgent,
    meter: &CostMeter,
    host: &mut dyn RefineHost,
) -> Result<RefineLog> {
    let Options { rounds, candidates } = opts;
    let look = host.look().clone();
    let reference_path = host.reference_path().to_string();
    let reference_rgba = rgba(host.reference());
    let reference_png = png(&reference_rgba)?;
    let reference_measures = host.summary(&host.measure(&reference_rgba));
    let reference_ink = host.reference_ink();

    let mut best = look.post_process.clone();
    let first = host.draw(&best)?;
    let mut best_img = first.image;
    let mut best_ink = first.ink;
    let mut best_d = first.distance;
    let start = best_d.clone();
    say(format!(
        "Refine {} toward {reference_path} with {}, by {}: start {}",
        look.name,
        agent.name(),
        start.metric.name(),
        scored(&start)
    ));
    let mut log = RefineLog {
        look: look.name.clone(),
        look_source: look.source.clone(),
        reference: reference_path.clone(),
        agent: agent.name(),
        metric: start.metric,
        start: start.clone(),
        end: start,
        rounds_run: 0,
        stopped: format!("ran all {rounds} rounds"),
        tried: Vec::new(),
        kept: Vec::new(),
        calls: 0,
        cost_usd: 0.0,
        post_process: toml::Table::new(),
        frames: vec![(format!("start {:.1}", best_d.score), best_img.clone())],
    };

    let mut misses = 0;
    for round in 1..=rounds {
        let best_png = png(&best_img)?;
        let best_measures = host.summary(&host.measure(&best_img));
        let proposed = agent.propose(&Ask {
            round,
            look: &look,
            current: &best,
            current_png: &best_png,
            current_distance: &best_d,
            current_measures: &best_measures,
            current_ink: best_ink.as_deref(),
            reference_png: &reference_png,
            reference_measures: &reference_measures,
            reference_ink: reference_ink.as_deref(),
            history: &log.tried,
            want: candidates,
            host: &*host,
        })?;
        log.rounds_run = round;
        if proposed.is_empty() {
            log.stopped = format!("round {round}: the agent proposed nothing");
            say(format!("  {}", log.stopped));
            break;
        }
        let first = log.tried.len();
        let mut winner: Option<(
            usize,
            toml::Table,
            RgbaImage,
            Option<String>,
            DistanceReport,
        )> = None;
        for candidate in proposed.into_iter().take(candidates) {
            let mut table = best.clone();
            let refused = candidate.edits.iter().find_map(|e| {
                host.apply_edit(&mut table, &e.key, &e.value)
                    .err()
                    .map(|why| format!("{e}: {why}"))
            });
            if let Some(why) = refused {
                say(format!("  {:<40} refused: {why}", candidate.label()));
                log.tried.push(Tried {
                    round,
                    candidate,
                    distance: None,
                    refused: Some(why),
                    image: None,
                    kept: false,
                });
                continue;
            }
            let drawn = host.draw(&table)?;
            let d = drawn.distance;
            say(format!("  {:<40} {}", candidate.label(), scored(&d)));
            let index = log.tried.len();
            log.tried.push(Tried {
                round,
                candidate,
                distance: Some(d.clone()),
                refused: None,
                image: Some(host.measure(&drawn.image)),
                kept: false,
            });
            if winner.as_ref().is_none_or(|w| d.score < w.4.score) {
                winner = Some((index, table, drawn.image, drawn.ink, d));
            }
        }
        match winner {
            Some((index, table, image, ink, d)) if d.score < best_d.score - MIN_GAIN => {
                log.tried[index].kept = true;
                log.kept.extend(log.tried[index].candidate.edits.clone());
                say(format!(
                    "  round {round}: kept {} ({:.2} -> {:.2})",
                    log.tried[index].candidate.label(),
                    best_d.score,
                    d.score
                ));
                log.frames
                    .push((format!("round {round} {:.1}", d.score), image.clone()));
                best = table;
                best_img = image;
                best_ink = ink;
                best_d = d;
                misses = 0;
            }
            _ => {
                let tried = log.tried.len() - first;
                log.stopped = format!(
                    "round {round}: none of {tried} candidate(s) moved the distance below {:.2}",
                    best_d.score
                );
                say(format!("  {}", log.stopped));
                // A miss teaches the agent what doesn't work; give it
                // another round with that history before stopping
                misses += 1;
                if misses >= PATIENCE {
                    break;
                }
            }
        }
    }
    log.end = best_d;
    log.calls = meter.calls();
    log.cost_usd = meter.spent();
    log.post_process = best;
    log.frames.insert(
        0,
        ("reference".into(), {
            let (w, h) = log.frames[0].1.dimensions();
            image::imageops::resize(&reference_rgba, w, h, image::imageops::FilterType::Triangle)
        }),
    );
    Ok(log)
}

/// The refined look as a `.look.toml`: the original's description, tune
/// and notes, a line saying what the refine did, and the kept block.
pub fn refined_look_toml(log: &RefineLog, base: &Look, name: &str) -> Result<String> {
    let mut meta = toml::Table::new();
    meta.insert("name".into(), name.into());
    meta.insert(
        "description".into(),
        format!("{} (refined toward {})", base.description, log.reference).into(),
    );
    meta.insert(
        "tune".into(),
        toml::Value::Array(base.tune.iter().map(|t| t.as_str().into()).collect()),
    );
    let kept = if log.kept.is_empty() {
        "nothing".to_string()
    } else {
        log.kept
            .iter()
            .map(Edit::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let notes = format!(
        "{}\n\nRefined from `{}` toward {} by `flint render --ai-refine` ({}, {} call(s), ${:.4}): \
         distance {:.2} -> {:.2} in {} round(s), kept {kept}. Stopped: {}.\n",
        base.notes.trim(),
        base.name,
        log.reference,
        log.agent,
        log.calls,
        log.cost_usd,
        log.start.score,
        log.end.score,
        log.rounds_run,
        log.stopped
    );
    meta.insert("notes".into(), notes.trim_start().to_string().into());
    let mut root = toml::Table::new();
    root.insert("look".into(), meta.into());
    root.insert("post_process".into(), log.post_process.clone().into());
    let text = format!(
        "# Written by `flint render --look {} --reference {} --ai-refine` (#357).\n\n{}",
        base.name,
        log.reference,
        toml::to_string(&root)?
    );
    // It must load as a look
    Look::parse(name, &text, "refined").map_err(|e| anyhow::anyhow!(e))?;
    Ok(text)
}

/// Where the refined look goes by default: beside the sheet, named for it.
pub fn default_out(output: &str, name: &str) -> PathBuf {
    Path::new(output)
        .parent()
        .unwrap_or(Path::new(""))
        .join(format!("{name}.look.toml"))
}

/// The agent for `provider`, and the meter its calls go through.
pub fn build_agent(
    provider: &str,
    model: Option<&str>,
    max_cost: Option<f64>,
) -> Result<(Box<dyn LookAgent>, Arc<CostMeter>)> {
    let meter = Arc::new(super::costs::meter(provider, max_cost)?);
    let agent: Box<dyn LookAgent> = match provider {
        "mock" => Box::new(TuneWalker),
        "anthropic" => {
            let mut config = AgentConfig::anthropic();
            if let Some(m) = model {
                config.model = m.to_string();
            }
            let agent = AnthropicAgent::new(config)
                .map_err(|e| anyhow::anyhow!("--ai-refine: {e}"))?
                .with_meter(meter.clone());
            Box::new(ClaudeLooks { agent })
        }
        other => bail!("unknown --ai-provider '{other}'; known: anthropic, mock"),
    };
    Ok((agent, meter))
}

/// `--ai-refine` (#357): run the loop, then write the refined look, its log
/// and the sheet of rounds.
pub fn refine_look(args: &RefineArgs, host: &mut dyn RefineHost) -> Result<()> {
    let (agent, meter) = build_agent(&args.ai_provider, args.ai_model.as_deref(), args.max_cost)?;
    let look = host.look().clone();
    let output = host.output().to_string();
    let name = format!("{}_refined", look.name);
    let out = args
        .refine_out
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_out(&output, &name));
    let log = run(
        Options {
            rounds: args.refine_rounds.max(1),
            candidates: args.refine_candidates.max(1),
        },
        agent.as_ref(),
        &meter,
        host,
    )?;
    let text = refined_look_toml(&log, &look, &name)?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, text).context(format!("Failed to write {}", out.display()))?;
    let log_path = out.with_extension("json");
    std::fs::write(&log_path, serde_json::to_string_pretty(&log)?)
        .context(format!("Failed to write {}", log_path.display()))?;
    let (labels, cells): (Vec<String>, Vec<RgbaImage>) = log.frames.iter().cloned().unzip();
    let sheet = host.sheet(
        &cells,
        format!("refine {} toward {}", look.name, log.reference),
        format!(
            "{}: distance {:.2} -> {:.2}, {} round(s), {} call(s), ${:.4}",
            log.agent, log.start.score, log.end.score, log.rounds_run, log.calls, log.cost_usd
        ),
        labels,
    );
    sheet
        .save(&output)
        .context(format!("Failed to save image to {output}"))?;
    say(format!(
        "Refined {}: distance {:.2} -> {:.2} ({}), {} call(s), ${:.4}",
        look.name, log.start.score, log.end.score, log.stopped, log.calls, log.cost_usd
    ));
    say(format!(
        "  by {}: start {}; end {}",
        log.metric.name(),
        log.start.terms_line(),
        log.end.terms_line()
    ));
    say(format!(
        "Wrote {} (load it with --look {})",
        out.display(),
        out.display()
    ));
    say(format!(
        "Wrote {} and the rounds' sheet {}",
        log_path.display(),
        output
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_replies_become_candidates() {
        let reply = json!([
            {"edits": {"ascii.brightness": 0.3, "post.fog_color": [0.1, 0.2, 0.3]}, "why": "lift"},
            {"edits": {"ascii.ramp": "blocks"}},
            {"nope": 1}
        ]);
        let c = parse_candidates(Some(&reply));
        assert_eq!(c.len(), 2);
        assert_eq!(
            c[0].label(),
            "ascii.brightness=0.3 post.fog_color=0.1,0.2,0.3"
        );
        assert_eq!(c[1].edits[0].value, "blocks");
    }

    #[test]
    fn the_refine_group_reads_from_renders_matches_only_with_its_flag() {
        let cmd = clap::Command::new("render")
            .arg(clap::Arg::new("look").long("look"))
            .arg(clap::Arg::new("reference").long("reference"))
            .arg(clap::Arg::new("sweep").long("sweep"))
            .arg(clap::Arg::new("frames").long("frames"))
            .arg(clap::Arg::new("mesh").long("mesh"))
            .arg(clap::Arg::new("watch").long("watch"))
            .arg(clap::Arg::new("strip").long("strip"))
            .arg(clap::Arg::new("variant").long("variant"))
            .arg(clap::Arg::new("variants").long("variants"));
        let cmd = <RefineArgs as clap::Args>::augment_args(cmd);
        let m = cmd
            .clone()
            .try_get_matches_from(["render", "--look", "l", "--reference", "r", "--ai-refine"])
            .unwrap();
        assert!(Refine.asked(Some(&m)));
        let plain = cmd.try_get_matches_from(["render"]).unwrap();
        assert!(!Refine.asked(Some(&plain)));
        assert!(!Refine.asked(None));
    }
}
