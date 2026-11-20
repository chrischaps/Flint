//! A scope trace read back (#1099): the Chrome trace-event JSON the
//! player's recorder writes (ADR 0128, #1097), as data the profiler tab
//! asks questions of. `flint profile` (#1098) reads the same files with a
//! reader of its own, `commands/profile/read.rs`; the two landed side by
//! side and are not yet one.
//!
//! The file is a list of events. A thread's scopes are `"ph":"X"` events
//! with `args: { frame, depth, detail? }`, on a track (`tid`) named by a
//! `thread_name` event; the player adds tracks of its own, recognised by
//! name: `frames` (one event a frame, begin to begin), `phases` (the F2
//! overlay's phases), `gpu` (each GPU pass, laid from its frame's render
//! phase), `hitches` and `notes` (instants). The header is `otherData`.
//! A track whose events carry a `depth` is a thread's scopes whatever its
//! name, and a trace with no `frames` track (another writer's) has its
//! frames made from the scopes' `frame` numbers.
//!
//! [`Trace`] is the file as read. [`profile::Profile`] is what is worked
//! out from it once: frames as rows, each scope's parent and path, its
//! self time and system colour, statistics over a range, a scope's time
//! across frames and the median frame each path is compared with.

pub mod profile;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;

/// One scope: a stretch of one thread's time, nested `depth` deep.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// Index into [`Trace::names`].
    pub name: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
    /// The recorder's frame index when it closed.
    pub frame: u32,
    pub depth: u16,
    /// The scope's detail (a file, a label, a count), as text.
    pub detail: Option<Box<str>>,
    /// A summed record (#1288): `calls` calls of one engine function,
    /// laid from the first for their total. A leaf, never a parent.
    pub summed: Option<Summed>,
}

/// What a summed record stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summed {
    pub calls: u32,
    /// The longest of them.
    pub worst_ns: u64,
}

impl Scope {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.dur_ns
    }

    /// The calls it stands for: one, or a summed record's count.
    pub fn calls(&self) -> u32 {
        self.summed.map_or(1, |s| s.calls)
    }

    /// Its longest single call.
    pub fn worst_ns(&self) -> u64 {
        self.summed.map_or(self.dur_ns, |s| s.worst_ns)
    }
}

/// A thread's scopes, in the order [`Trace::read`] sorted them: by frame,
/// then start, then depth (a parent before the children it starts with).
#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub name: String,
    pub scopes: Vec<Scope>,
}

/// A frame of the `frames` track: begin to begin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSpan {
    pub index: u32,
    pub start_ns: u64,
    pub end_ns: u64,
}

/// A span of one of the player's tracks (a phase, a GPU pass).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// Index into [`Trace::names`].
    pub name: u32,
    pub frame: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
}

impl Span {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.dur_ns
    }
}

/// A hitch the player's detector found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hitch {
    pub start_ns: u64,
    pub dur_ns: u64,
    pub judged_ms: f32,
    pub median_ms: f32,
    pub cause: String,
    /// Its frame in the player's history (a capture's `cpu.hitch.frame`).
    pub history_frame: Option<i64>,
    pub game_time: Option<f64>,
}

/// A frame note (#1077): what the engine said it did, at its frame's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub at_ns: u64,
    pub kind: String,
    pub detail: String,
    pub ms: Option<f32>,
    pub depth: u16,
}

/// A trace file, read.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Every scope, phase and GPU pass name, once.
    pub names: Vec<String>,
    /// The threads' scopes, in the file's track order.
    pub threads: Vec<Thread>,
    /// The `frames` track, by start.
    pub frames: Vec<FrameSpan>,
    /// The `phases` track, by frame then start.
    pub phases: Vec<Span>,
    /// The `gpu` track, by frame then start.
    pub gpu: Vec<Span>,
    pub hitches: Vec<Hitch>,
    pub notes: Vec<Note>,
    /// `otherData`: what was recorded, where and why.
    pub header: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(rename = "traceEvents", default)]
    events: Vec<RawEvent>,
    #[serde(rename = "otherData", default)]
    other: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(default)]
    ph: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    pid: Value,
    #[serde(default)]
    tid: Value,
    #[serde(default)]
    ts: f64,
    #[serde(default)]
    dur: f64,
    #[serde(default)]
    args: RawArgs,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawArgs {
    frame: Option<i64>,
    depth: Option<i64>,
    detail: Option<Value>,
    name: Option<String>,
    sort_index: Option<i64>,
    ms: Option<f64>,
    judged_ms: Option<f64>,
    median_ms: Option<f64>,
    cause: Option<String>,
    history_frame: Option<i64>,
    game_time: Option<f64>,
    calls: Option<i64>,
    worst_us: Option<f64>,
}

/// The player's own tracks, by the name `trace.rs` gives each.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scopes,
    Frames,
    Phases,
    Gpu,
    Hitches,
    Notes,
}

fn kind_of(name: &str) -> Kind {
    match name {
        "frames" => Kind::Frames,
        "phases" => Kind::Phases,
        "gpu" => Kind::Gpu,
        "hitches" => Kind::Hitches,
        "notes" => Kind::Notes,
        _ => Kind::Scopes,
    }
}

/// Microseconds, as the file writes them, to whole nanoseconds.
fn ns(us: f64) -> u64 {
    (us * 1000.0).round().max(0.0) as u64
}

fn as_frame(v: Option<i64>) -> u32 {
    v.unwrap_or(0).clamp(0, u32::MAX as i64) as u32
}

impl Trace {
    /// Read the trace at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::read(&text).with_context(|| path.display().to_string().replace('\\', "/"))
    }

    /// Whether `text` looks like a trace (an object with `traceEvents`, or
    /// a bare event list), without reading all of it.
    pub fn looks_like(text: &str) -> bool {
        let head: String = text.chars().take(4096).collect();
        head.contains("\"traceEvents\"") || head.trim_start().starts_with('[')
    }

    /// A trace file's text, read.
    pub fn read(text: &str) -> Result<Self> {
        let file: RawFile = if text.trim_start().starts_with('[') {
            RawFile {
                events: serde_json::from_str(text).context("not a trace: not JSON")?,
                other: Map::new(),
            }
        } else {
            serde_json::from_str(text).context("not a trace: not trace-event JSON")?
        };
        if file.events.is_empty() {
            bail!("not a trace: it has no events");
        }
        let mut names: Vec<String> = Vec::new();
        let mut name_ids: HashMap<String, u32> = HashMap::new();
        let mut intern = |s: &str| -> u32 {
            if let Some(&i) = name_ids.get(s) {
                return i;
            }
            let i = names.len() as u32;
            names.push(s.to_string());
            name_ids.insert(s.to_string(), i);
            i
        };

        // Tracks: named by metadata, ordered by sort index then first sight
        struct Track {
            name: Option<String>,
            sort: Option<i64>,
            first: usize,
            depths: bool,
        }
        let key = |e: &RawEvent| format!("{}:{}", e.pid, e.tid);
        let mut tracks: HashMap<String, Track> = HashMap::new();
        for (i, e) in file.events.iter().enumerate() {
            let t = tracks.entry(key(e)).or_insert(Track {
                name: None,
                sort: None,
                first: i,
                depths: false,
            });
            match (e.ph.as_str(), e.name.as_str()) {
                ("M", "thread_name") => t.name = e.args.name.clone(),
                ("M", "thread_sort_index") => t.sort = e.args.sort_index,
                ("X", _) if e.args.depth.is_some() => t.depths = true,
                _ => {}
            }
        }
        let mut order: Vec<(&String, &Track)> = tracks.iter().collect();
        order.sort_by_key(|(_, t)| (t.sort.unwrap_or(i64::MAX), t.first));
        let mut thread_of: HashMap<&str, usize> = HashMap::new();
        let mut kind: HashMap<&str, Kind> = HashMap::new();
        let mut threads: Vec<Thread> = Vec::new();
        for (k, t) in &order {
            let name = t.name.clone().unwrap_or_else(|| format!("thread {k}"));
            let kd = if t.depths {
                Kind::Scopes
            } else {
                kind_of(&name)
            };
            kind.insert(k.as_str(), kd);
            if kd == Kind::Scopes {
                thread_of.insert(k.as_str(), threads.len());
                threads.push(Thread {
                    name,
                    scopes: Vec::new(),
                });
            }
        }

        let mut trace = Trace {
            header: file.other.clone(),
            ..Default::default()
        };
        for e in &file.events {
            let k = key(e);
            let Some(&kd) = kind.get(k.as_str()) else {
                continue;
            };
            match (e.ph.as_str(), kd) {
                ("X", Kind::Scopes) => {
                    let detail = e.args.detail.as_ref().and_then(|d| match d {
                        Value::String(s) => Some(s.clone().into_boxed_str()),
                        Value::Null => None,
                        other => Some(other.to_string().into_boxed_str()),
                    });
                    let scope = Scope {
                        name: intern(&e.name),
                        start_ns: ns(e.ts),
                        dur_ns: ns(e.dur),
                        frame: as_frame(e.args.frame),
                        depth: e.args.depth.unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
                        detail,
                        summed: e.args.calls.map(|calls| Summed {
                            calls: calls.clamp(0, u32::MAX as i64) as u32,
                            worst_ns: e.args.worst_us.map_or(ns(e.dur), ns),
                        }),
                    };
                    threads[thread_of[k.as_str()]].scopes.push(scope);
                }
                ("X", Kind::Frames) => trace.frames.push(FrameSpan {
                    index: as_frame(e.args.frame),
                    start_ns: ns(e.ts),
                    end_ns: ns(e.ts) + ns(e.dur),
                }),
                ("X", Kind::Phases | Kind::Gpu) => {
                    let span = Span {
                        name: intern(&e.name),
                        frame: as_frame(e.args.frame),
                        start_ns: ns(e.ts),
                        dur_ns: ns(e.dur),
                    };
                    if kd == Kind::Phases {
                        trace.phases.push(span);
                    } else {
                        trace.gpu.push(span);
                    }
                }
                ("X", Kind::Hitches) => trace.hitches.push(Hitch {
                    start_ns: ns(e.ts),
                    dur_ns: ns(e.dur),
                    judged_ms: e.args.judged_ms.unwrap_or(e.dur / 1000.0) as f32,
                    median_ms: e.args.median_ms.unwrap_or(0.0) as f32,
                    cause: e.args.cause.clone().unwrap_or_default(),
                    history_frame: e.args.history_frame,
                    game_time: e.args.game_time,
                }),
                ("i" | "I", Kind::Notes) => trace.notes.push(Note {
                    at_ns: ns(e.ts),
                    kind: e.name.clone(),
                    detail: match &e.args.detail {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(other) => other.to_string(),
                    },
                    ms: e.args.ms.map(|m| m as f32),
                    depth: e.args.depth.unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
                }),
                _ => {}
            }
        }
        drop(intern);
        for t in &mut threads {
            t.scopes
                .sort_by_key(|s| (s.frame, s.start_ns, s.depth, u64::MAX - s.dur_ns));
        }
        // A track of no scopes is a thread that recorded nothing in the window
        threads.retain(|t| !t.scopes.is_empty());
        trace.frames.sort_by_key(|f| f.start_ns);
        trace.phases.sort_by_key(|s| (s.frame, s.start_ns));
        trace.gpu.sort_by_key(|s| (s.frame, s.start_ns));
        trace.hitches.sort_by_key(|h| h.start_ns);
        trace.notes.sort_by_key(|n| n.at_ns);
        trace.names = names;
        trace.threads = threads;
        if trace.threads.is_empty() && trace.frames.is_empty() {
            bail!("not a Flint trace: it has no scopes and no frames");
        }
        Ok(trace)
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("?", String::as_str)
    }

    /// A header field as text (`scene`, `reason`, `written_by`).
    pub fn header_str(&self, key: &str) -> Option<&str> {
        self.header
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Every scope, every thread's.
    pub fn scope_count(&self) -> usize {
        self.threads.iter().map(|t| t.scopes.len()).sum()
    }

    /// Where the scopes begin, ns, and the rings that wrapped, when one
    /// lost part of the look-back (#1288). A writer since then says
    /// (`cut_threads`, empty for a whole window, and `covered_from_ms`);
    /// for a trace from before it, a thread the header calls `wrapped`
    /// whose oldest scope closed after the second frame ended was cut
    /// there. A frame that began here or later has all its scopes.
    pub fn covered_from(&self) -> Option<(u64, String)> {
        let text = |k: &str| self.header_str(k).map(str::to_string);
        let ms_ns = |k: &str| {
            self.header
                .get(k)
                .and_then(Value::as_f64)
                .map(|ms| ns(ms * 1000.0))
        };
        if self.header.contains_key("cut_threads") {
            let at = ms_ns("covered_from_ms")?;
            return Some((at, text("cut_threads").unwrap_or_else(|| "?".into())));
        }
        let wrapped = text("wrapped")?;
        let second_end = self.frames.get(1).map(|f| f.end_ns)?;
        let mut cut: Vec<(&str, u64)> = Vec::new();
        for name in wrapped.split(',').map(str::trim) {
            let Some(t) = self.threads.iter().find(|t| t.name == name) else {
                continue;
            };
            let Some(oldest) = t.scopes.iter().map(Scope::end_ns).min() else {
                continue;
            };
            if oldest > second_end {
                cut.push((name, oldest));
            }
        }
        let from = cut.iter().map(|(_, at)| *at).max()?;
        let names: Vec<&str> = cut.iter().map(|(n, _)| *n).collect();
        Some((from, names.join(", ")))
    }

    /// The hitch this trace means by `hitch`, an index into
    /// [`Trace::hitches`]: see [`meant_hitch`].
    pub fn meant_hitch(&self) -> Option<usize> {
        let marks: Vec<HitchMark> = self
            .hitches
            .iter()
            .map(|h| HitchMark {
                game_time: h.game_time,
                judged_ms: h.judged_ms as f64,
            })
            .collect();
        meant_hitch(self.header_str("reason"), &marks)
    }
}

/// What [`meant_hitch`] needs of a hitch on a trace's `hitches` track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitchMark {
    pub game_time: Option<f64>,
    pub judged_ms: f64,
}

/// The game time a header's `reason` names, when the trace was saved for a
/// hitch: the player writes `hitch at t=6.004 s, 120.3 ms: <cause>`.
pub fn reason_hitch_time(reason: &str) -> Option<f64> {
    let rest = reason.strip_prefix("hitch at t=")?;
    rest.split_once(" s")?.0.trim().parse().ok()
}

/// Which of a trace's hitches (in track order, by start) `hitch` means
/// (#1289). One rule, so the profiler tab and `flint profile` answer the
/// same frame for the same file and word:
///
/// - a trace saved for a hitch (`--trace-on-hitch`; its `reason` says
///   `hitch at t=...`) means that hitch: the one whose game time is the
///   reason's (written to the millisecond), else the last on the track, which
///   it is unless another hitch landed in the frames written after it;
/// - any other trace (on exit, a key, a capture) means its worst hitch, the
///   one judged longest (the later of two equal ones).
pub fn meant_hitch(reason: Option<&str>, hitches: &[HitchMark]) -> Option<usize> {
    if hitches.is_empty() {
        return None;
    }
    if let Some(t) = reason.and_then(reason_hitch_time) {
        let by_time = (0..hitches.len())
            .filter_map(|i| Some((i, (hitches[i].game_time? - t).abs())))
            .filter(|&(_, d)| d <= 0.000_6)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        return by_time.or(Some(hitches.len() - 1));
    }
    (0..hitches.len()).max_by(|&a, &b| hitches[a].judged_ms.total_cmp(&hitches[b].judged_ms))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small trace in the player's shape: three frames of 10, 10 and 40
    /// ms, a `tick` each on the main thread with `scripts` and `render`
    /// inside it, the third's script stalling; a loader thread; phases,
    /// GPU passes, a hitch on the third frame and a note.
    pub(crate) fn sample() -> String {
        let mut e: Vec<String> = Vec::new();
        let meta = |tid: u32, name: &str| {
            format!(
                "{{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":{tid},\"args\":{{\"name\":\"{name}\"}}}},\n\
                 {{\"ph\":\"M\",\"name\":\"thread_sort_index\",\"pid\":1,\"tid\":{tid},\"args\":{{\"sort_index\":{tid}}}}}"
            )
        };
        let x = |tid: u32, name: &str, ts: f64, dur: f64, args: &str| {
            format!("{{\"ph\":\"X\",\"name\":\"{name}\",\"pid\":1,\"tid\":{tid},\"ts\":{ts},\"dur\":{dur},\"args\":{{{args}}}}}")
        };
        e.push(meta(1, "main"));
        e.push(meta(2, "loader"));
        let lengths = [10_000.0, 10_000.0, 40_000.0];
        let mut t = 1000.0;
        for (f, len) in lengths.iter().enumerate() {
            let script = if f == 2 { 31_000.0 } else { 1_000.0 };
            // Children first, as the recorder writes them
            e.push(x(
                1,
                "stall.rhai:on_update",
                t + 600.0,
                script,
                &format!("\"frame\":{f},\"depth\":2,\"detail\":\"scripts/stall.rhai\""),
            ));
            e.push(x(
                1,
                "script::update",
                t + 500.0,
                script + 200.0,
                &format!("\"frame\":{f},\"depth\":1"),
            ));
            e.push(x(
                1,
                "render",
                t + script + 1000.0,
                6000.0,
                &format!("\"frame\":{f},\"depth\":1,\"detail\":12"),
            ));
            e.push(x(
                1,
                "tick",
                t + 100.0,
                len - 300.0,
                &format!("\"frame\":{f},\"depth\":0"),
            ));
            t += len;
        }
        e.push(x(
            2,
            "decode_image",
            2000.0,
            5000.0,
            "\"frame\":0,\"depth\":0,\"detail\":\"textures/a.png\"",
        ));
        e.push(meta(3, "frames"));
        e.push(meta(4, "phases"));
        e.push(meta(5, "gpu"));
        e.push(meta(6, "hitches"));
        e.push(meta(7, "notes"));
        let mut t = 1000.0;
        for (f, len) in lengths.iter().enumerate() {
            let script = if f == 2 { 31_000.0 } else { 1_000.0 };
            e.push(x(
                3,
                "frame",
                t,
                *len,
                &format!("\"frame\":{f},\"ms\":{}", len / 1000.0),
            ));
            e.push(x(
                4,
                "scripts",
                t + 400.0,
                script + 400.0,
                &format!("\"frame\":{f}"),
            ));
            e.push(x(
                4,
                "render",
                t + script + 900.0,
                6200.0,
                &format!("\"frame\":{f}"),
            ));
            e.push(x(
                5,
                "Scene",
                t + script + 1000.0,
                2000.0,
                &format!("\"frame\":{f},\"gpu_frame_ms\":2.5"),
            ));
            t += len;
        }
        e.push(x(6, "hitch", 21_000.0, 40_000.0, "\"history_frame\":2,\"game_time\":3.0,\"judged_ms\":40,\"median_ms\":10,\"cause\":\"script stall.rhai:on_update 31.0 ms\""));
        e.push("{\"ph\":\"i\",\"s\":\"t\",\"name\":\"model load\",\"pid\":1,\"tid\":7,\"ts\":1000,\"args\":{\"detail\":\"models/a.glb\",\"depth\":0,\"ms\":4.5}}".into());
        format!(
            "{{\"displayTimeUnit\":\"ms\",\"traceEvents\":[\n{}\n],\"otherData\":{{\"format\":\"flint-trace\",\"scene\":\"levels/stall.scene.toml\",\"reason\":\"exit\"}}}}\n",
            e.join(",\n")
        )
    }

    /// A trace of `frames` frames of 16.7 ms at 60 fps, each a `tick` over
    /// ten systems of `per` scopes each (`10 * per + 11` scopes a frame),
    /// with frame `stall` taking 100 ms in a script: the size of trace the
    /// profiler tab is held to (#1099: 5 s at 60 fps).
    pub(crate) fn synthetic(frames: usize, per: usize, stall: usize) -> String {
        let mut e: Vec<String> = vec![
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":1,"args":{"name":"main"}}"#.into(),
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":2,"args":{"name":"frames"}}"#.into(),
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":3,"args":{"name":"phases"}}"#.into(),
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":4,"args":{"name":"hitches"}}"#.into(),
        ];
        let systems = [
            ("script::update", "scripts"),
            ("physics::step", "physics"),
            ("animation::update", "anim/fx"),
            ("particles::simulate", "anim/fx"),
            ("sync_renderer", "sync"),
            ("extract::entities", "sync"),
            ("render_shadow_pass", "render"),
            ("render_main_pass", "render"),
            ("render_postprocess", "render"),
            ("render_hud", "hud"),
        ];
        let mut t = 0.0f64;
        for f in 0..frames {
            let len = if f == stall { 100_000.0 } else { 16_667.0 };
            let normal = (16_667.0 - 1_000.0) / systems.len() as f64;
            let mut s0 = t + 500.0;
            for (k, (name, phase)) in systems.iter().enumerate() {
                // The stalled frame's script system holds the extra time
                let stalls = f == stall && k == 0;
                let slot = if stalls {
                    normal + (len - 16_667.0)
                } else {
                    normal
                };
                let child = normal * 0.8 / per as f64;
                let mut c0 = s0 + 10.0;
                for c in 0..per {
                    // The script's first callback: 0.1 ms, or the stall
                    let (child_name, dur) = match (k, c) {
                        (0, 0) if stalls => {
                            ("stall.rhai:on_update".to_string(), child + (len - 16_667.0))
                        }
                        (0, 0) => ("stall.rhai:on_update".to_string(), child * 0.9),
                        _ => (format!("{name}::part{c}"), child * 0.9),
                    };
                    e.push(format!(
                        r#"{{"ph":"X","name":"{child_name}","pid":1,"tid":1,"ts":{c0:.3},"dur":{dur:.3},"args":{{"frame":{f},"depth":2}}}}"#
                    ));
                    c0 += dur + child * 0.1;
                }
                e.push(format!(
                    r#"{{"ph":"X","name":"{name}","pid":1,"tid":1,"ts":{s0:.3},"dur":{:.3},"args":{{"frame":{f},"depth":1}}}}"#,
                    slot * 0.9
                ));
                e.push(format!(
                    r#"{{"ph":"X","name":"{phase}","pid":1,"tid":3,"ts":{s0:.3},"dur":{:.3},"args":{{"frame":{f}}}}}"#,
                    slot * 0.95
                ));
                s0 += slot;
            }
            e.push(format!(
                r#"{{"ph":"X","name":"tick","pid":1,"tid":1,"ts":{:.3},"dur":{:.3},"args":{{"frame":{f},"depth":0}}}}"#,
                t + 100.0,
                len - 200.0
            ));
            e.push(format!(
                r#"{{"ph":"X","name":"frame","pid":1,"tid":2,"ts":{t:.3},"dur":{len:.3},"args":{{"frame":{f}}}}}"#
            ));
            if f == stall {
                e.push(format!(
                    r#"{{"ph":"X","name":"hitch","pid":1,"tid":4,"ts":{t:.3},"dur":{len:.3},"args":{{"judged_ms":100,"median_ms":16.7,"cause":"script stall.rhai:on_update"}}}}"#
                ));
            }
            t += len;
        }
        format!(
            "{{\"traceEvents\":[\n{}\n],\"otherData\":{{\"scene\":\"synthetic.scene.toml\"}}}}",
            e.join(",\n")
        )
    }

    /// Twenty frames 100..119 of 16.7 ms, a `tick` each, and three hitches
    /// as `--trace-on-hitch=2` lays them (#1289): frame 105 at t=3.000 s
    /// (120 ms, the worst), frame 114 at t=6.004 s (60 ms, the one a
    /// `hitch at t=6.004 s` trace was saved for) and frame 117 at t=6.100 s
    /// (30 ms, in the frames written after it, so the last on the track).
    pub(crate) fn three_hitches(reason: &str) -> String {
        let mut e: Vec<String> = Vec::new();
        for (tid, name) in [(1, "main"), (2, "frames"), (4, "hitches")] {
            e.push(format!(
                r#"{{"ph":"M","name":"thread_name","pid":1,"tid":{tid},"args":{{"name":"{name}"}}}}"#
            ));
        }
        let hitches = [(105u32, 3.0, 120.0), (114, 6.004, 60.0), (117, 6.1, 30.0)];
        let mut t = 0.0f64;
        for f in 100..120u32 {
            let hitch = hitches.iter().find(|h| h.0 == f);
            let len = hitch.map_or(16_667.0, |h| h.2 * 1000.0);
            e.push(format!(
                r#"{{"ph":"X","name":"tick","pid":1,"tid":1,"ts":{:.3},"dur":{:.3},"args":{{"frame":{f},"depth":0}}}}"#,
                t + 100.0,
                len - 200.0
            ));
            e.push(format!(
                r#"{{"ph":"X","name":"frame","pid":1,"tid":2,"ts":{t:.3},"dur":{len:.3},"args":{{"frame":{f}}}}}"#
            ));
            if let Some(&(_, game_time, ms)) = hitch {
                e.push(format!(
                    r#"{{"ph":"X","name":"hitch","pid":1,"tid":4,"ts":{t:.3},"dur":{len:.3},"args":{{"history_frame":{f},"game_time":{game_time},"judged_ms":{ms},"median_ms":16.7,"cause":"script stall.rhai:on_update"}}}}"#
                ));
            }
            t += len;
        }
        format!(
            "{{\"traceEvents\":[\n{}\n],\"otherData\":{{\"format\":\"flint-trace\",\"reason\":\"{reason}\"}}}}",
            e.join(",\n")
        )
    }

    /// The reason line of a trace `--trace-on-hitch` saved for the t=6.004 s
    /// hitch, as the player writes it.
    pub(crate) const SAVED_ON_SECOND: &str =
        "hitch at t=6.004 s, 60.0 ms: script stall.rhai:on_update 58.2 ms";

    #[test]
    fn a_trace_saved_for_a_hitch_means_that_hitch_and_any_other_its_worst() {
        assert_eq!(reason_hitch_time(SAVED_ON_SECOND), Some(6.004));
        assert_eq!(reason_hitch_time("exit"), None);
        let mark = |game_time: Option<f64>, judged_ms: f64| HitchMark {
            game_time,
            judged_ms,
        };
        let three = [
            mark(Some(3.0), 120.0),
            mark(Some(6.0042), 60.0),
            mark(Some(6.1), 30.0),
        ];
        // Its own hitch by time: not the worst, not the last
        assert_eq!(meant_hitch(Some(SAVED_ON_SECOND), &three), Some(1));
        // No game times (an older file): the last on the track
        let untimed = three.map(|h| mark(None, h.judged_ms));
        assert_eq!(meant_hitch(Some(SAVED_ON_SECOND), &untimed), Some(2));
        // On exit, a key or a capture: the worst
        for reason in [Some("exit"), Some("key"), Some("capture"), None] {
            assert_eq!(meant_hitch(reason, &three), Some(0), "{reason:?}");
        }
        assert_eq!(meant_hitch(Some(SAVED_ON_SECOND), &[]), None);
        // And as read from a file
        let t = Trace::read(&three_hitches(SAVED_ON_SECOND)).unwrap();
        assert_eq!(t.meant_hitch(), Some(1));
        assert_eq!(
            Trace::read(&three_hitches("exit")).unwrap().meant_hitch(),
            Some(0)
        );
    }

    #[test]
    fn a_players_trace_is_read_into_threads_frames_and_its_own_tracks() {
        let t = Trace::read(&sample()).unwrap();
        assert_eq!(
            t.threads
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "loader"]
        );
        assert_eq!(t.threads[0].scopes.len(), 12);
        // Sorted by frame, start, depth: the parent before its children
        let first = &t.threads[0].scopes[0];
        assert_eq!((t.name(first.name), first.depth), ("tick", 0));
        assert_eq!(first.start_ns, 1_100_000);
        assert_eq!(t.frames.len(), 3);
        assert_eq!(t.frames[2].end_ns - t.frames[2].start_ns, 40_000_000);
        assert_eq!(t.phases.len(), 6);
        assert_eq!(t.gpu.len(), 3);
        assert_eq!(t.hitches.len(), 1);
        assert_eq!(t.hitches[0].history_frame, Some(2));
        assert_eq!(t.notes[0].kind, "model load");
        assert_eq!(t.notes[0].ms, Some(4.5));
        let render = t.threads[0]
            .scopes
            .iter()
            .find(|s| t.name(s.name) == "render")
            .unwrap();
        assert_eq!(render.detail.as_deref(), Some("12"), "a count is text");
        assert_eq!(t.header_str("scene"), Some("levels/stall.scene.toml"));
    }

    #[test]
    fn what_is_not_a_trace_is_refused() {
        assert!(Trace::read("{\"format\":\"flintframe\"}").is_err());
        assert!(Trace::read("not json").is_err());
        assert!(!Trace::looks_like(
            "{\"format\":\"flintframe\",\"passes\":[]}"
        ));
        assert!(Trace::looks_like(&sample()));
    }

    #[test]
    fn a_bare_event_list_of_another_writer_reads() {
        let text = r#"[{"ph":"X","name":"work","pid":7,"tid":"io","ts":5,"dur":3,"args":{"frame":4,"depth":0}}]"#;
        let t = Trace::read(text).unwrap();
        assert_eq!(t.threads.len(), 1);
        assert_eq!(t.threads[0].scopes[0].frame, 4);
        assert!(t.frames.is_empty());
    }
}
