//! `flint render --look <x> --reference ref.png --ai-refine` (#357): a
//! measured refine loop for looks, the way `flint gen --ai-refine` refines
//! a procgen spec (#183).
//!
//! Each round an agent sees the reference, the look's current render, its
//! `[post_process]` block, the keys it may change (with their ranges) and
//! every edit tried so far with the distance it scored. It proposes a few
//! candidate edits in `--sweep` key syntax (`ascii.brightness=0.25`,
//! `post.exposure=1.3`). Each candidate is rendered and measured against the
//! reference ([`look_measure::distance_to`], #183's metric); the best one is
//! kept if it moves the distance down, and the loop stops when no candidate
//! does, or after `--refine-rounds`. What's kept is written as a new
//! `.look.toml`, the run as a JSON log beside it, and the rounds as a
//! contact sheet (reference, start, each kept round).
//!
//! Agents: `anthropic` asks Claude, every call through a [`CostMeter`]
//! (priced, ledgered in `.flint/procgen_costs.json`, refused before sending
//! past `--max-cost`). `mock` is free and offline: it walks the look's own
//! `tune` sweeps, one untried value at a time, so the loop is a measured
//! coordinate search, the same loop with a simpler proposer.

use super::look::{self, Look, Sweep, SweepKey};
use super::look_measure::{self, DistanceReport, ImageMeasures};
use anyhow::{bail, Context, Result};
use flint_procgen_ai::cost::{CostLedger, CostMeter};
use flint_procgen_ai::{AgentConfig, AnthropicAgent};
use image::{RgbImage, RgbaImage};
use serde::Serialize;
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

impl Edit {
    /// Check `key=value` the way `--sweep` checks a column (key known,
    /// value in range), and put it into `table` (a look's `[post_process]`).
    pub fn apply(&self, table: &mut toml::Table) -> Result<(), String> {
        // The sweep parser knows every key and range; one edit is a sweep
        // of a single value, written twice because a sweep takes two.
        let sweep = Sweep::parse(&format!("{}={}|{}", self.key, self.value, self.value))?;
        let value = look::parse_value(&self.value);
        match sweep.key {
            SweepKey::Look => return Err("`look` can't be an edit; edit its keys".into()),
            SweepKey::Post { field } => {
                table.insert(field, value);
            }
            SweepKey::Style { style, key } => {
                let styles = table
                    .entry("styles")
                    .or_insert_with(|| toml::Value::Table(Default::default()));
                let Some(styles) = styles.as_table_mut() else {
                    return Err("[post_process] styles is not a table".into());
                };
                let t = styles
                    .entry(style)
                    .or_insert_with(|| toml::Value::Table(Default::default()));
                let Some(t) = t.as_table_mut() else {
                    return Err(format!("[post_process.styles] {} is not a table", self.key));
                };
                t.insert(key, value);
            }
        }
        look::overlay(None, table).map_err(|e| format!("{e:#}"))?;
        Ok(())
    }
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
    fn label(&self) -> String {
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
    pub current_distance: f64,
    pub current_measures: &'a ImageMeasures,
    pub reference_png: &'a [u8],
    pub reference_measures: &'a ImageMeasures,
    pub history: &'a [Tried],
    /// How many candidates to propose.
    pub want: usize,
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
        let sweeps: Vec<Sweep> = ask
            .look
            .tune
            .iter()
            .filter_map(|t| Sweep::parse(t).ok())
            .collect();
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
        let current = |s: &Sweep| current_value(ask.current, &s.key);
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
                    key: s.name(),
                    value: v.clone(),
                };
                // The value it has now, or started from, needs no render
                let v_toml = look::parse_value(v);
                let same = [current(s), current_value(&ask.look.post_process, &s.key)]
                    .iter()
                    .flatten()
                    .any(|c| same_value(c, &v_toml));
                if same || tried(&edit) || out.iter().any(|c: &Candidate| c.edits[0] == edit) {
                    continue;
                }
                out.push(Candidate {
                    edits: vec![edit],
                    why: format!("tune sweep {}", s.name()),
                });
            }
        }
        Ok(out)
    }
}

/// Equal as TOML, with `9` and `9.0` the same number.
fn same_value(a: &toml::Value, b: &toml::Value) -> bool {
    let num = |v: &toml::Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    match (num(a), num(b)) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

fn current_value(table: &toml::Table, key: &SweepKey) -> Option<toml::Value> {
    match key {
        SweepKey::Post { field } => table.get(field).cloned(),
        SweepKey::Style { style, key } => table.get("styles")?.get(style)?.get(key).cloned(),
        SweepKey::Look => None,
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
change with their ranges, image measures of both pictures, and every edit tried so far with the \
distance it scored to the reference (a score out of 100, lower is closer: mean colour, a \
luminance histogram and a 16x16 luminance layout). The scene, camera and models never change: \
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
            println!("  claude: {o}");
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
        let Some(d) = flint_render::stylize::find(name) else {
            continue;
        };
        out.push_str(&format!("style {name}:\n"));
        for p in d.params {
            out.push_str(&format!(
                "  {name}.{} = {} (range {}..{})\n",
                p.name, p.default, p.min, p.max
            ));
        }
        for o in d.options {
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
    let others: Vec<&str> = flint_render::stylize::registry()
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

fn ask_text(ask: &Ask) -> String {
    let block = toml::to_string(ask.current).unwrap_or_default();
    let mut history = String::new();
    for t in ask.history {
        let score = match (&t.distance, &t.refused) {
            (Some(d), _) => format!("{:.2}", d.score),
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
    let m = |x: &ImageMeasures| look_measure::summary(x, None);
    format!(
        "Round {round}. Look `{name}`: {desc}\nIts notes: {notes}\nIts suggested sweeps: {tune}\n\n\
         CURRENT distance to the reference: {d:.2}\nCURRENT measures:   {cm}\nREFERENCE measures: {rm}\n\n\
         The look's [post_process] block now:\n```toml\n{block}```\n\nKeys you may change:\n{keys}\n\
         Tried so far:\n{history}\nPropose up to {want} candidates.",
        round = ask.round,
        name = ask.look.name,
        desc = ask.look.description,
        notes = ask.look.notes.trim(),
        tune = ask.look.tune.join("; "),
        d = ask.current_distance,
        cm = m(ask.current_measures),
        rm = m(ask.reference_measures),
        keys = key_catalogue(ask.current),
        want = ask.want,
    )
}

// --- The loop ---------------------------------------------------------------

/// What the run is asked to do.
pub struct Options {
    pub look: Look,
    pub reference_path: String,
    pub reference: RgbImage,
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

/// Run the loop. `draw` renders a look's `[post_process]` block over the
/// scene; `meter` is read for the log.
pub fn run(
    opts: Options,
    agent: &dyn LookAgent,
    meter: &CostMeter,
    draw: &mut dyn FnMut(&toml::Table) -> Result<RgbaImage>,
) -> Result<RefineLog> {
    let Options {
        look,
        reference_path,
        reference,
        rounds,
        candidates,
    } = opts;
    let reference_rgba = rgba(&reference);
    let reference_png = png(&reference_rgba)?;
    let reference_measures = look_measure::measure(&reference_rgba);

    let mut best = look.post_process.clone();
    let mut best_img = draw(&best)?;
    let mut best_d = look_measure::distance_to(&reference, &best_img);
    let start = DistanceReport::from(best_d);
    println!(
        "Refine {} toward {reference_path} with {}: start {:.2}",
        look.name,
        agent.name(),
        best_d.score
    );
    let mut log = RefineLog {
        look: look.name.clone(),
        look_source: look.source.clone(),
        reference: reference_path.clone(),
        agent: agent.name(),
        start,
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
        let best_measures = look_measure::measure(&best_img);
        let proposed = agent.propose(&Ask {
            round,
            look: &look,
            current: &best,
            current_png: &best_png,
            current_distance: best_d.score,
            current_measures: &best_measures,
            reference_png: &reference_png,
            reference_measures: &reference_measures,
            history: &log.tried,
            want: candidates,
        })?;
        log.rounds_run = round;
        if proposed.is_empty() {
            log.stopped = format!("round {round}: the agent proposed nothing");
            println!("  {}", log.stopped);
            break;
        }
        let first = log.tried.len();
        let mut winner: Option<(usize, toml::Table, RgbaImage, flint_procgen_ai::Distance)> = None;
        for candidate in proposed.into_iter().take(candidates) {
            let mut table = best.clone();
            let refused = candidate
                .edits
                .iter()
                .find_map(|e| e.apply(&mut table).err().map(|why| format!("{e}: {why}")));
            if let Some(why) = refused {
                println!("  {:<40} refused: {why}", candidate.label());
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
            let img = draw(&table)?;
            let d = look_measure::distance_to(&reference, &img);
            println!("  {:<40} {:.2}", candidate.label(), d.score);
            let index = log.tried.len();
            log.tried.push(Tried {
                round,
                candidate,
                distance: Some(d.into()),
                refused: None,
                image: Some(look_measure::measure(&img)),
                kept: false,
            });
            if winner.as_ref().is_none_or(|w| d.score < w.3.score) {
                winner = Some((index, table, img, d));
            }
        }
        match winner {
            Some((index, table, img, d)) if d.score < best_d.score - MIN_GAIN => {
                log.tried[index].kept = true;
                log.kept.extend(log.tried[index].candidate.edits.clone());
                println!(
                    "  round {round}: kept {} ({:.2} -> {:.2})",
                    log.tried[index].candidate.label(),
                    best_d.score,
                    d.score
                );
                log.frames
                    .push((format!("round {round} {:.1}", d.score), img.clone()));
                best = table;
                best_img = img;
                best_d = d;
                misses = 0;
            }
            _ => {
                let tried = log.tried.len() - first;
                log.stopped = format!(
                    "round {round}: none of {tried} candidate(s) moved the distance below {:.2}",
                    best_d.score
                );
                println!("  {}", log.stopped);
                // A miss teaches the agent what doesn't work; give it
                // another round with that history before stopping
                misses += 1;
                if misses >= PATIENCE {
                    break;
                }
            }
        }
    }
    log.end = best_d.into();
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
    Look::parse(name, &text, "refined")?;
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
    let mut meter = CostMeter::new(provider).with_ledger(CostLedger::open_default());
    if let Some(max) = max_cost {
        meter = meter.with_ceiling(max);
    }
    let meter = Arc::new(meter);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn noir() -> Look {
        look::find("ascii_noir", "").unwrap()
    }

    #[test]
    fn an_edit_lands_in_the_block_and_is_checked() {
        let mut t = noir().post_process;
        Edit {
            key: "ascii.brightness".into(),
            value: "0.3".into(),
        }
        .apply(&mut t)
        .unwrap();
        assert_eq!(t["styles"]["ascii"]["brightness"].as_float(), Some(0.3));
        Edit {
            key: "post.fog_color".into(),
            value: "0.1,0.2,0.3".into(),
        }
        .apply(&mut t)
        .unwrap();
        assert!(t["fog_color"].is_array());
        let out_of_range = Edit {
            key: "ascii.cell".into(),
            value: "900".into(),
        };
        assert!(out_of_range.apply(&mut t).is_err());
        let unknown = Edit {
            key: "post.exposur".into(),
            value: "1".into(),
        };
        assert!(unknown.apply(&mut t).unwrap_err().contains("exposure"));
    }

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
    fn the_walker_skips_current_and_tried_values() {
        let look = noir();
        let m = look_measure::measure(&RgbaImage::new(1, 1));
        let ask = |history: &[Tried]| {
            TuneWalker
                .propose(&Ask {
                    round: 1,
                    look: &look,
                    current: &look.post_process,
                    current_png: &[],
                    current_distance: 0.0,
                    current_measures: &m,
                    reference_png: &[],
                    reference_measures: &m,
                    history,
                    want: 3,
                })
                .unwrap()
        };
        let first = ask(&[]);
        let labels: Vec<String> = first.iter().map(Candidate::label).collect();
        // contrast 1.6 and brightness 0.15 are the look's own values
        assert_eq!(
            labels,
            ["ascii.contrast=1.2", "ascii.cell=7", "ascii.brightness=0"]
        );
        let history: Vec<Tried> = first
            .into_iter()
            .map(|candidate| Tried {
                round: 1,
                candidate,
                distance: None,
                refused: None,
                image: None,
                kept: false,
            })
            .collect();
        let next: Vec<String> = ask(&history).iter().map(Candidate::label).collect();
        // cell 9 is the look's own 9.0
        assert_eq!(
            next,
            [
                "ascii.contrast=2.2",
                "ascii.cell=12",
                "ascii.brightness=0.3"
            ]
        );
    }

    /// A fake renderer: the picture's grey level is the look's exposure,
    /// so the loop's job is to walk exposure toward the reference's grey.
    #[test]
    fn the_loop_keeps_what_moves_the_distance_down_and_stops() {
        let look = Look::parse(
            "t",
            "[look]\ndescription = \"test\"\ntune = [\"post.exposure=0.5,1,1.5,2\"]\n\
             [post_process]\nexposure = 0.5",
            "test",
        )
        .unwrap();
        let reference = RgbImage::from_pixel(16, 16, image::Rgb([150, 150, 150]));
        let mut draw = |t: &toml::Table| -> Result<RgbaImage> {
            let e = t.get("exposure").and_then(|v| v.as_float()).unwrap_or(1.0);
            let g = (e * 100.0).clamp(0.0, 255.0) as u8;
            Ok(RgbaImage::from_pixel(16, 16, image::Rgba([g, g, g, 255])))
        };
        let meter = CostMeter::new("mock");
        let log = run(
            Options {
                look: look.clone(),
                reference_path: "ref.png".into(),
                reference,
                rounds: 5,
                candidates: 1,
            },
            &TuneWalker,
            &meter,
            &mut draw,
        )
        .unwrap();
        // 0.5 (50) -> 1 (100) -> 1.5 (150, the reference) -> 2 is further,
        // then nothing is left to try: stop
        assert_eq!(
            log.kept.iter().map(Edit::to_string).collect::<Vec<_>>(),
            ["post.exposure=1", "post.exposure=1.5"]
        );
        assert!(log.end.score < 1e-9, "{:?}", log.end);
        assert!(log.stopped.contains("round 4"), "{}", log.stopped);
        assert_eq!(log.frames.len(), 4, "reference, start and two kept rounds");
        let text = refined_look_toml(&log, &look, "t_refined").unwrap();
        let back = Look::parse("t_refined", &text, "test").unwrap();
        assert_eq!(back.post_process["exposure"].as_float(), Some(1.5));
        assert!(back
            .notes
            .contains("kept post.exposure=1, post.exposure=1.5"));
    }
}
