//! Reading a trace: the Chrome trace-event JSON `flint-player --trace`
//! writes (ADR 0128, #1097), into scopes by thread and the player's own
//! tracks, with nothing else looked at.
//!
//! A scope is a `"ph":"X"` event whose `args` carry `frame` and `depth`;
//! every thread track holds only those. The player's tracks are found by
//! name: `frames` (one event a frame, begin to begin), `phases`, `gpu`,
//! `hitches` and `notes`. Records are written when a scope closes, so a
//! parent follows its children in the file: each thread's scopes are sorted
//! by start (a parent before a child that starts with it) and the tree is
//! rebuilt from `depth`, never guessed from times.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The newest trace version this reader knows (the header's `version`).
pub(crate) const VERSION: i64 = 1;

/// One trace, read.
pub(crate) struct Trace {
    /// The file as given, for the views' first line.
    pub path: String,
    /// `otherData`: the scene, the reason, the build, the adapter ...
    pub header: Map<String, Value>,
    /// Every thread that holds scopes, in the file's order.
    pub threads: Vec<Thread>,
    /// Every frame, by start. From the `frames` track, else from the
    /// scopes' own frame indices (`frames_derived`).
    pub frames: Vec<FrameSpan>,
    pub frames_derived: bool,
    /// The player's phases between their marks (`render` twice a frame:
    /// before and after `acquire`).
    pub phases: Vec<Span>,
    /// Each pass the GPU timer measured, by the frame that drew it.
    pub gpu: Vec<Span>,
    pub hitches: Vec<HitchMark>,
    pub notes: Vec<NoteMark>,
}

pub(crate) struct Thread {
    pub name: String,
    /// Sorted by start, a parent before the children that start with it.
    pub scopes: Vec<Scope>,
    /// Each scope's parent on this thread, by index into `scopes`.
    pub parent: Vec<Option<usize>>,
    /// Each scope's time less its children's, ns.
    pub self_ns: Vec<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Scope {
    pub name: String,
    pub start_ns: u64,
    pub dur_ns: u64,
    /// The frame (`profile::frame`) the scope closed in.
    pub frame: u32,
    /// Open scopes around it on its thread (0 = outermost).
    pub depth: u16,
    pub detail: Option<Detail>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Detail {
    Count(i64),
    Text(String),
}

impl Scope {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.dur_ns
    }

    /// The name a view counts this scope under: its name, and with
    /// `split` its text detail too (`script::call (scripts/stall.rhai)`),
    /// so one scope at many files reads as many. A count never splits.
    pub fn key(&self, split: bool) -> String {
        match &self.detail {
            Some(Detail::Text(t)) if split && !t.is_empty() => format!("{} ({t})", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FrameSpan {
    pub frame: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
}

impl FrameSpan {
    pub fn ms(&self) -> f64 {
        self.dur_ns as f64 / 1e6
    }
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.dur_ns
    }
}

/// An event of the player's own tracks.
#[derive(Clone, Debug)]
pub(crate) struct Span {
    pub name: String,
    pub frame: Option<u32>,
    pub start_ns: u64,
    pub dur_ns: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct HitchMark {
    pub start_ns: u64,
    pub game_time: Option<f64>,
    pub judged_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub cause: String,
}

#[derive(Clone, Debug)]
pub(crate) struct NoteMark {
    pub kind: String,
    pub at_ns: u64,
    pub detail: String,
    pub ms: Option<f64>,
    pub depth: i64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawTop {
    Object {
        #[serde(rename = "traceEvents")]
        events: Vec<RawEvent>,
        #[serde(rename = "otherData", default)]
        other: Map<String, Value>,
    },
    Array(Vec<RawEvent>),
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(default)]
    ph: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tid: Value,
    #[serde(default)]
    ts: f64,
    #[serde(default)]
    dur: f64,
    #[serde(default)]
    args: Map<String, Value>,
}

/// The player's tracks, by the name `trace.rs` gives each.
const FRAMES: &str = "frames";
const PHASES: &str = "phases";
const GPU: &str = "gpu";
const HITCHES: &str = "hitches";
const NOTES: &str = "notes";

/// Microseconds (three decimals: the nanosecond) to nanoseconds.
fn ns(us: f64) -> u64 {
    (us * 1000.0).round().max(0.0) as u64
}

fn tid_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Trace {
    pub fn open(path: &Path) -> Result<Trace> {
        let bytes =
            std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
        Self::parse(&bytes, &path.display().to_string())
    }

    pub fn parse(bytes: &[u8], path: &str) -> Result<Trace> {
        let top: RawTop = serde_json::from_slice(bytes).with_context(|| {
            format!(
                "{path} is not a trace: Chrome trace-event JSON is an object with `traceEvents`"
            )
        })?;
        let (events, header) = match top {
            RawTop::Object { events, other } => (events, other),
            RawTop::Array(events) => (events, Map::new()),
        };
        if let Some(format) = header.get("format").and_then(Value::as_str) {
            if format != "flint-trace" {
                bail!("{path} is a `{format}` file, not a flint-trace");
            }
        }
        if let Some(version) = header.get("version").and_then(Value::as_i64) {
            if version > VERSION {
                bail!(
                    "{path} is trace version {version}; this flint reads up to version {VERSION}: \
                     read it with a newer flint"
                );
            }
        }

        // Track names first: metadata may follow the events it names
        let mut names: HashMap<String, String> = HashMap::new();
        for e in &events {
            if e.ph == "M" && e.name == "thread_name" {
                if let Some(n) = e.args.get("name").and_then(Value::as_str) {
                    names.insert(tid_key(&e.tid), n.to_string());
                }
            }
        }

        let mut threads: Vec<(String, Vec<Scope>)> = Vec::new();
        let mut thread_of: HashMap<String, usize> = HashMap::new();
        let mut frames = Vec::new();
        let mut phases = Vec::new();
        let mut gpu = Vec::new();
        let mut hitches = Vec::new();
        let mut notes = Vec::new();
        let int = |args: &Map<String, Value>, k: &str| args.get(k).and_then(Value::as_i64);
        let num = |args: &Map<String, Value>, k: &str| args.get(k).and_then(Value::as_f64);
        let text = |args: &Map<String, Value>, k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };

        for e in events {
            let tid = tid_key(&e.tid);
            let track = names.get(&tid).map(String::as_str).unwrap_or("");
            match e.ph.as_str() {
                "X" => {
                    let (start_ns, dur_ns) = (ns(e.ts), ns(e.dur));
                    let frame = int(&e.args, "frame").map(|f| f.max(0) as u32);
                    if let (Some(frame), Some(depth)) = (frame, int(&e.args, "depth")) {
                        let detail = match e.args.get("detail") {
                            Some(Value::String(s)) => Some(Detail::Text(s.clone())),
                            Some(v) => v.as_i64().map(Detail::Count),
                            None => None,
                        };
                        let at = *thread_of.entry(tid.clone()).or_insert_with(|| {
                            let name = names.get(&tid).cloned().unwrap_or_else(|| tid.clone());
                            threads.push((name, Vec::new()));
                            threads.len() - 1
                        });
                        threads[at].1.push(Scope {
                            name: e.name,
                            start_ns,
                            dur_ns,
                            frame,
                            depth: depth.clamp(0, u16::MAX as i64) as u16,
                            detail,
                        });
                        continue;
                    }
                    match track {
                        FRAMES => frames.push(FrameSpan {
                            frame: frame.unwrap_or(frames.len() as u32),
                            start_ns,
                            dur_ns,
                        }),
                        PHASES | GPU => {
                            let span = Span {
                                name: e.name,
                                frame,
                                start_ns,
                                dur_ns,
                            };
                            if track == PHASES {
                                phases.push(span)
                            } else {
                                gpu.push(span)
                            }
                        }
                        HITCHES => hitches.push(HitchMark {
                            start_ns,
                            game_time: num(&e.args, "game_time"),
                            judged_ms: num(&e.args, "judged_ms"),
                            median_ms: num(&e.args, "median_ms"),
                            cause: text(&e.args, "cause"),
                        }),
                        _ => {}
                    }
                }
                "i" | "I" if track == NOTES => notes.push(NoteMark {
                    kind: e.name,
                    at_ns: ns(e.ts),
                    detail: text(&e.args, "detail"),
                    ms: num(&e.args, "ms"),
                    depth: int(&e.args, "depth").unwrap_or(0),
                }),
                _ => {}
            }
        }

        if threads.iter().all(|(_, s)| s.is_empty()) {
            bail!(
                "{path} holds no Flint scopes (events with `frame` and `depth` args): \
                 was it written by `flint-player --trace`?"
            );
        }
        let threads: Vec<Thread> = threads
            .into_iter()
            .map(|(name, scopes)| Thread::new(name, scopes))
            .collect();

        frames.sort_by_key(|f| f.start_ns);
        let frames_derived = frames.is_empty();
        if frames_derived {
            frames = derived_frames(&threads);
        }
        hitches.sort_by_key(|h| h.start_ns);
        notes.sort_by_key(|n| n.at_ns);
        Ok(Trace {
            path: path.to_string(),
            header,
            threads,
            frames,
            frames_derived,
            phases,
            gpu,
            hitches,
            notes,
        })
    }

    /// A header field as text, if there.
    pub fn header_str(&self, key: &str) -> Option<&str> {
        self.header
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }

    /// The frame (by position in `frames`) a moment falls in.
    pub fn frame_at(&self, at_ns: u64) -> Option<usize> {
        let i = self.frames.partition_point(|f| f.start_ns <= at_ns);
        let i = i.checked_sub(1)?;
        (at_ns < self.frames[i].end_ns() || i + 1 == self.frames.len()).then_some(i)
    }

    /// The position in `frames` of the frame with index `frame`.
    pub fn position(&self, frame: u32) -> Option<usize> {
        self.frames.iter().position(|f| f.frame == frame)
    }

    /// Each hitch's frame, by position in `frames`.
    pub fn hitch_frames(&self) -> Vec<Option<usize>> {
        self.hitches
            .iter()
            .map(|h| self.frame_at(h.start_ns))
            .collect()
    }

    /// The thread names, for a `--thread` that names none.
    pub fn thread_names(&self) -> Vec<&str> {
        self.threads.iter().map(|t| t.name.as_str()).collect()
    }

    /// The threads `--thread` keeps (all of them without it).
    pub fn threads_named(&self, name: Option<&str>) -> Result<Vec<&Thread>> {
        let Some(name) = name else {
            return Ok(self.threads.iter().collect());
        };
        let kept: Vec<&Thread> = self.threads.iter().filter(|t| t.name == name).collect();
        if kept.is_empty() {
            bail!(
                "no thread named `{name}` in {}: it has {}",
                self.path,
                self.thread_names().join(", ")
            );
        }
        Ok(kept)
    }

    /// Each phase's ms in the frame at `pos` (`render` summed, as the
    /// player's history keeps it).
    pub fn phases_of(&self, pos: usize) -> BTreeMap<String, f64> {
        let frame = self.frames[pos].frame;
        let mut out = BTreeMap::new();
        for p in self.phases.iter().filter(|p| p.frame == Some(frame)) {
            *out.entry(p.name.clone()).or_insert(0.0) += p.dur_ns as f64 / 1e6;
        }
        out
    }

    /// The GPU passes of the frame at `pos`, as measured.
    pub fn gpu_of(&self, pos: usize) -> Vec<&Span> {
        let frame = self.frames[pos].frame;
        self.gpu.iter().filter(|g| g.frame == Some(frame)).collect()
    }

    /// The frame notes laid at the frame at `pos`.
    pub fn notes_of(&self, pos: usize) -> Vec<&NoteMark> {
        self.notes
            .iter()
            .filter(|n| self.frame_at(n.at_ns) == Some(pos))
            .collect()
    }

    /// The phases in the order a frame runs them: by start, in the frame
    /// with the most of them (the look-back's first frame may begin late).
    pub fn phase_order(&self) -> Vec<String> {
        let mut count: HashMap<u32, usize> = HashMap::new();
        for p in &self.phases {
            if let Some(f) = p.frame {
                *count.entry(f).or_default() += 1;
            }
        }
        let Some(fullest) = count
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(f, _)| f)
        else {
            return Vec::new();
        };
        let mut spans: Vec<&Span> = self
            .phases
            .iter()
            .filter(|p| p.frame == Some(fullest))
            .collect();
        spans.sort_by_key(|p| p.start_ns);
        let mut order: Vec<String> = Vec::new();
        for p in spans.into_iter().chain(&self.phases) {
            if !order.contains(&p.name) {
                order.push(p.name.clone());
            }
        }
        order
    }
}

impl Thread {
    fn new(name: String, mut scopes: Vec<Scope>) -> Thread {
        scopes.sort_by(|a, b| {
            a.start_ns
                .cmp(&b.start_ns)
                .then(a.depth.cmp(&b.depth))
                .then(b.dur_ns.cmp(&a.dur_ns))
        });
        let mut parent = vec![None; scopes.len()];
        let mut children_ns = vec![0u64; scopes.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, s) in scopes.iter().enumerate() {
            while let Some(&top) = stack.last() {
                let t = &scopes[top];
                if t.depth >= s.depth || t.end_ns() <= s.start_ns {
                    stack.pop();
                } else {
                    break;
                }
            }
            if let Some(&top) = stack.last() {
                parent[i] = Some(top);
                children_ns[top] += s.dur_ns;
            }
            stack.push(i);
        }
        let self_ns = scopes
            .iter()
            .zip(&children_ns)
            .map(|(s, c)| s.dur_ns.saturating_sub(*c))
            .collect();
        Thread {
            name,
            scopes,
            parent,
            self_ns,
        }
    }

    /// Whether scope `i` has an ancestor counted under the same key, so a
    /// scope inside itself isn't counted twice in a total.
    pub fn inside_itself(&self, i: usize, split: bool) -> bool {
        let key = self.scopes[i].key(split);
        let mut at = self.parent[i];
        while let Some(p) = at {
            if self.scopes[p].key(split) == key {
                return true;
            }
            at = self.parent[p];
        }
        false
    }

    /// The chain of scope names from the outermost down to `i`'s parent.
    pub fn ancestors(&self, i: usize, split: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = self.parent[i];
        while let Some(p) = at {
            out.push(self.scopes[p].key(split));
            at = self.parent[p];
        }
        out.reverse();
        out
    }
}

/// Frames from the scopes alone (a trace with no `frames` track): each
/// frame index from its first scope's start to its last one's end.
fn derived_frames(threads: &[Thread]) -> Vec<FrameSpan> {
    let mut spans: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    for s in threads.iter().flat_map(|t| &t.scopes) {
        let e = spans.entry(s.frame).or_insert((u64::MAX, 0));
        e.0 = e.0.min(s.start_ns);
        e.1 = e.1.max(s.end_ns());
    }
    let mut frames: Vec<FrameSpan> = spans
        .into_iter()
        .map(|(frame, (start, end))| FrameSpan {
            frame,
            start_ns: start,
            dur_ns: end - start,
        })
        .collect();
    frames.sort_by_key(|f| f.start_ns);
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(events: &str) -> Trace {
        let text = format!(
            r#"{{"traceEvents":[{{"ph":"M","name":"thread_name","pid":1,"tid":1,"args":{{"name":"main"}}}},{events}],"otherData":{{"format":"flint-trace","version":1}}}}"#
        );
        Trace::parse(text.as_bytes(), "t.json").unwrap()
    }

    #[test]
    fn a_parent_written_after_its_children_is_their_parent_and_self_time_is_what_they_leave() {
        let t = trace(
            r#"{"ph":"X","name":"child","pid":1,"tid":1,"ts":10.0,"dur":3.0,"args":{"frame":0,"depth":1}},
               {"ph":"X","name":"child","pid":1,"tid":1,"ts":14.0,"dur":2.0,"args":{"frame":0,"depth":1}},
               {"ph":"X","name":"tick","pid":1,"tid":1,"ts":10.0,"dur":10.0,"args":{"frame":0,"depth":0}}"#,
        );
        let main = &t.threads[0];
        assert_eq!(main.scopes[0].name, "tick", "the parent sorts first");
        assert_eq!(main.parent, vec![None, Some(0), Some(0)]);
        assert_eq!(main.self_ns[0], 5_000);
        assert!(t.frames_derived);
        assert_eq!(t.frames[0].dur_ns, 10_000);
    }

    #[test]
    fn a_newer_version_is_refused_naming_both() {
        let err = Trace::parse(
            br#"{"traceEvents":[],"otherData":{"format":"flint-trace","version":9}}"#,
            "new.json",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(
            err.contains("version 9") && err.contains("up to version 1"),
            "{err}"
        );
    }

    #[test]
    fn a_trace_with_no_scopes_says_what_it_wants() {
        let err = Trace::parse(
            br#"{"traceEvents":[{"ph":"X","name":"x","ts":1,"dur":1}]}"#,
            "x",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("no Flint scopes"), "{err}");
    }

    #[test]
    fn text_detail_splits_a_key_and_a_count_does_not() {
        let mut s = Scope {
            name: "script::call".into(),
            start_ns: 0,
            dur_ns: 1,
            frame: 0,
            depth: 0,
            detail: Some(Detail::Text("scripts/a.rhai".into())),
        };
        assert_eq!(s.key(true), "script::call (scripts/a.rhai)");
        assert_eq!(s.key(false), "script::call");
        s.detail = Some(Detail::Count(3));
        assert_eq!(s.key(true), "script::call");
    }
}
