//! A scope trace read back: the Chrome trace-event JSON the player's
//! recorder writes (ADR 0128, #1097), as data `flint profile` (#1098) and
//! the profiler tab of `flint frame open` (#1099) ask questions of. One
//! reader for both (#1287): the two were built side by side with a reader
//! each, which already differed (only one checked the version, they sorted
//! differently, each said what `hitch` means), so they would have drifted
//! on what a trace means.
//!
//! The file is a list of events. A thread's scopes are `"ph":"X"` events
//! with `args: { frame, depth, detail? }`, on a track (`pid:tid`) named by a
//! `thread_name` event and ordered by `thread_sort_index`; the player adds
//! tracks of its own, recognised by name: `frames` (one event a frame,
//! begin to begin), `phases` (the F2 overlay's phases), `gpu` (each GPU
//! pass, laid from its frame's render phase), `hitches` and `notes`
//! (instants). The header is `otherData`. A track whose events carry a
//! `depth` is a thread's scopes whatever its name, and a trace with no
//! `frames` track (another writer's) has its frames made from the scopes'
//! `frame` numbers (`frames_derived`).
//!
//! Records are written when a scope closes, so a parent follows its
//! children in the file: each scope's parent is found over the scopes by
//! start (a parent before a child that starts with it) from `depth`, never
//! guessed from times, and a parent that closed in a later frame than its
//! child (a worker's long load) is still its parent.
//!
//! [`Trace`] is the file as read: every frame, each thread's scopes with
//! their parents and self time, the player's tracks, where the scopes begin
//! when a ring wrapped ([`Coverage`], #1288) and which hitch `hitch` means
//! ([`meant_hitch`], #1289). [`profile::Profile`] is what is worked out from
//! it: paths, systems, statistics, and the median frame each path is held
//! against ([`profile::Baseline`]), whose walk ([`profile::Profile::carrier`])
//! is `flint profile explain`'s answer and the bar the tab's Compare marks.

pub mod profile;
pub mod stats;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;

/// The newest trace version this reader knows (the header's `version`).
pub const VERSION: i64 = 1;

/// No parent.
pub const ROOT: u32 = u32::MAX;

/// What a scope carried beside its name.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    /// A count (`render`'s draw calls): shown, never a key of its own.
    Count(i64),
    /// A file or a label (`scripts/stall.rhai`): a view may count the scope
    /// under its name and this ([`Trace::key`]).
    Text(Box<str>),
}

impl std::fmt::Display for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Detail::Count(n) => write!(f, "{n}"),
            Detail::Text(t) => f.write_str(t),
        }
    }
}

/// One scope: a stretch of one thread's time, nested `depth` deep.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// Index into [`Trace::names`].
    pub name: u32,
    pub start_ns: u64,
    pub dur_ns: u64,
    /// The recorder's frame index when it closed.
    pub frame: u32,
    /// Open scopes around it on its thread (0 = outermost).
    pub depth: u16,
    pub detail: Option<Detail>,
    /// A summed record (#1288): `calls` calls of one engine function,
    /// laid from the first for their total. A leaf: it may overlap its
    /// siblings, so it is nobody's parent.
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

    /// Its text detail, if it has one.
    pub fn text(&self) -> Option<&str> {
        match &self.detail {
            Some(Detail::Text(t)) if !t.is_empty() => Some(t),
            _ => None,
        }
    }
}

/// A thread's scopes, sorted by frame, then start, then depth (a parent
/// before the children it starts with), so each frame's are a range.
#[derive(Debug, Clone, Default)]
pub struct Thread {
    pub name: String,
    pub scopes: Vec<Scope>,
    /// Each scope's parent on this thread, an index into `scopes`, or
    /// [`ROOT`].
    pub parent: Vec<u32>,
    /// Each scope's time less its children's, ns.
    pub self_ns: Vec<u64>,
    /// The scopes by start, a parent before its children: the order a
    /// walk from the roots down takes.
    pub by_start: Vec<u32>,
}

impl Thread {
    fn new(name: String, mut scopes: Vec<Scope>) -> Thread {
        scopes.sort_by_key(|s| (s.frame, s.start_ns, s.depth, u64::MAX - s.dur_ns));
        let n = scopes.len();
        let mut by_start: Vec<u32> = (0..n as u32).collect();
        by_start.sort_by_key(|&i| {
            let s = &scopes[i as usize];
            (s.start_ns, s.depth, u64::MAX - s.dur_ns)
        });
        let mut parent = vec![ROOT; n];
        let mut children_ns = vec![0u64; n];
        let mut stack: Vec<usize> = Vec::new();
        for &i in &by_start {
            let (i, s) = (i as usize, &scopes[i as usize]);
            while let Some(&top) = stack.last() {
                let t = &scopes[top];
                if t.depth >= s.depth || t.end_ns() <= s.start_ns {
                    stack.pop();
                } else {
                    break;
                }
            }
            if let Some(&top) = stack.last() {
                parent[i] = top as u32;
                children_ns[top] += s.dur_ns;
            }
            // A summed record is a leaf (#1288): it may overlap a sibling,
            // whose children must not take it for their parent
            if s.summed.is_none() {
                stack.push(i);
            }
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
            by_start,
        }
    }

    /// Scope `i`'s parent, if it has one.
    pub fn parent_of(&self, i: usize) -> Option<usize> {
        let p = self.parent[i];
        (p != ROOT).then_some(p as usize)
    }

    /// The range of `scopes` that closed in recorder frame `frame`.
    pub fn in_frame(&self, frame: u32) -> Range<usize> {
        let a = self.scopes.partition_point(|s| s.frame < frame);
        let b = self.scopes.partition_point(|s| s.frame <= frame);
        a..b
    }
}

/// A frame: of the `frames` track (begin to begin), else made from the
/// scopes that closed in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSpan {
    /// The recorder's frame index (the scopes' `frame`).
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

/// A span of one of the player's tracks (a phase, a GPU pass).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// Index into [`Trace::names`].
    pub name: u32,
    /// The frame it was laid in (`None` when the writer didn't say).
    pub frame: Option<u32>,
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
    pub judged_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub cause: String,
    /// Its frame in the player's history (a capture's `cpu.hitch.frame`).
    pub history_frame: Option<i64>,
    pub game_time: Option<f64>,
}

impl Hitch {
    /// What the detector judged it, ms: its mark's length when it didn't say.
    pub fn judged(&self) -> f64 {
        self.judged_ms.unwrap_or(self.dur_ns as f64 / 1e6)
    }
}

/// A frame note (#1077): what the engine said it did, at its frame's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub at_ns: u64,
    pub kind: String,
    pub detail: String,
    pub ms: Option<f64>,
    pub depth: u16,
}

/// A look-back whose scopes begin late (#1288): a thread's ring wrapped
/// inside the window it was saved for, so its oldest frames lack their
/// scopes. Every view chooses only from [`Trace::covered`].
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    /// Where every frame is whole, ns: a frame that began here or later.
    pub from_ns: u64,
    /// The first frame (a position in [`Trace::frames`]) with all its scopes.
    pub first: usize,
    /// The rings that wrapped (`main`).
    pub threads: String,
    /// The look-back the trace was saved for, seconds.
    pub window_s: Option<f64>,
    /// What the scopes cover of it, seconds.
    pub covered_s: f64,
}

impl Coverage {
    /// `scopes cover frames 5729..5932 (0.85 s of 5 s): the main ring
    /// wrapped, so the 994 frames before them have no scopes`: the one
    /// line `flint profile` and the profiler tab both say.
    pub fn line(&self, frames: &[FrameSpan]) -> String {
        let kept = &frames[self.first.min(frames.len())..];
        let span = match (kept.first(), kept.last()) {
            (Some(a), Some(b)) => format!("frames {}..{}", a.frame, b.frame),
            _ => "no whole frame".to_string(),
        };
        let of = self
            .window_s
            .map_or(String::new(), |w| format!(" of {w:.0} s"));
        let before = self.first;
        format!(
            "scopes cover {span} ({:.2} s{of}): the {} ring wrapped, so the {before} frame{} \
             before them {} no scopes",
            self.covered_s,
            self.threads,
            if before == 1 { "" } else { "s" },
            if before == 1 { "has" } else { "have" },
        )
    }
}

/// A trace file, read.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// The file as given, for a view's first line and its errors.
    pub path: String,
    /// Every scope, phase and GPU pass name, once.
    pub names: Vec<String>,
    /// The threads' scopes, in the file's track order.
    pub threads: Vec<Thread>,
    /// Every frame, by start: the `frames` track, else made from the
    /// scopes (`frames_derived`). Those before [`Trace::covered`] lack
    /// their scopes.
    pub frames: Vec<FrameSpan>,
    pub frames_derived: bool,
    /// The `phases` track, by frame then start (`render` twice a frame:
    /// before and after `acquire`).
    pub phases: Vec<Span>,
    /// The `gpu` track, by frame then start.
    pub gpu: Vec<Span>,
    pub hitches: Vec<Hitch>,
    pub notes: Vec<Note>,
    /// `otherData`: what was recorded, where and why.
    pub header: Map<String, Value>,
    /// Where the scopes begin, when a ring lost part of the look-back.
    pub coverage: Option<Coverage>,
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

/// Microseconds (three decimals: the nanosecond) to whole nanoseconds.
fn ns(us: f64) -> u64 {
    (us * 1000.0).round().max(0.0) as u64
}

fn frame_of(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Trace {
    /// Read the trace at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
        Self::parse(&bytes, &path.display().to_string())
    }

    /// Whether `text` looks like a trace (an object with `traceEvents`, or
    /// a bare event list), without reading all of it.
    pub fn looks_like(text: &str) -> bool {
        let head: String = text.chars().take(4096).collect();
        head.contains("\"traceEvents\"") || head.trim_start().starts_with('[')
    }

    /// A trace's text, read; `path` names it in errors and views.
    pub fn parse(bytes: &[u8], path: &str) -> Result<Self> {
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

        // Tracks: named by metadata (which may follow the events it
        // names), ordered by sort index then first sight
        struct Track {
            name: Option<String>,
            tid: String,
            sort: Option<i64>,
            first: usize,
            depths: bool,
        }
        let key = |e: &RawEvent| format!("{}:{}", e.pid, e.tid);
        let mut tracks: HashMap<String, Track> = HashMap::new();
        for (i, e) in events.iter().enumerate() {
            let t = tracks.entry(key(e)).or_insert_with(|| Track {
                name: None,
                tid: text_of(&e.tid),
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
        let mut threads: Vec<(String, Vec<Scope>)> = Vec::new();
        for (k, t) in &order {
            let name = t.name.clone().unwrap_or_else(|| t.tid.clone());
            let kd = if t.depths {
                Kind::Scopes
            } else {
                kind_of(&name)
            };
            kind.insert(k.as_str(), kd);
            if kd == Kind::Scopes {
                thread_of.insert(k.as_str(), threads.len());
                threads.push((name, Vec::new()));
            }
        }

        let mut trace = Trace {
            path: path.to_string(),
            header,
            ..Default::default()
        };
        for e in &events {
            let k = key(e);
            let Some(&kd) = kind.get(k.as_str()) else {
                continue;
            };
            let a = &e.args;
            match (e.ph.as_str(), kd) {
                // A scope: on a thread's track, with its depth
                ("X", Kind::Scopes) if a.depth.is_some() => {
                    let detail = a.detail.as_ref().and_then(|d| match d {
                        Value::String(s) => Some(Detail::Text(s.clone().into_boxed_str())),
                        Value::Null => None,
                        v => Some(
                            v.as_i64()
                                .map(Detail::Count)
                                .unwrap_or_else(|| Detail::Text(v.to_string().into())),
                        ),
                    });
                    threads[thread_of[k.as_str()]].1.push(Scope {
                        name: intern(&e.name),
                        start_ns: ns(e.ts),
                        dur_ns: ns(e.dur),
                        frame: frame_of(a.frame.unwrap_or(0)),
                        depth: a.depth.unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
                        detail,
                        summed: a.calls.map(|calls| Summed {
                            calls: calls.clamp(0, u32::MAX as i64) as u32,
                            worst_ns: a.worst_us.map_or(ns(e.dur), ns),
                        }),
                    });
                }
                ("X", Kind::Frames) => {
                    let at = trace.frames.len() as i64;
                    trace.frames.push(FrameSpan {
                        frame: frame_of(a.frame.unwrap_or(at)),
                        start_ns: ns(e.ts),
                        dur_ns: ns(e.dur),
                    })
                }
                ("X", Kind::Phases | Kind::Gpu) => {
                    let span = Span {
                        name: intern(&e.name),
                        frame: a.frame.map(frame_of),
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
                    judged_ms: a.judged_ms,
                    median_ms: a.median_ms,
                    cause: a.cause.clone().unwrap_or_default(),
                    history_frame: a.history_frame,
                    game_time: a.game_time,
                }),
                ("i" | "I", Kind::Notes) => trace.notes.push(Note {
                    at_ns: ns(e.ts),
                    kind: e.name.clone(),
                    detail: match &a.detail {
                        Some(Value::Null) | None => String::new(),
                        Some(v) => text_of(v),
                    },
                    ms: a.ms,
                    depth: a.depth.unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
                }),
                _ => {}
            }
        }
        drop(intern);
        trace.names = names;
        // A track of no scopes is a thread that recorded nothing in the window
        trace.threads = threads
            .into_iter()
            .filter(|(_, s)| !s.is_empty())
            .map(|(name, scopes)| Thread::new(name, scopes))
            .collect();
        if trace.threads.is_empty() {
            bail!(
                "{path} holds no Flint scopes (events with `frame` and `depth` args): \
                 was it written by `flint-player --trace`?"
            );
        }

        // Frames by start, one a frame index (a frames track never repeats
        // one, but be sure); with no frames track, from the scopes
        trace.frames.sort_by_key(|f| f.start_ns);
        trace.frames_derived = trace.frames.is_empty();
        if trace.frames_derived {
            trace.frames = derived_frames(&trace.threads);
        }
        let mut seen = HashSet::new();
        trace.frames.retain(|f| seen.insert(f.frame));
        trace.phases.sort_by_key(|s| (s.frame, s.start_ns));
        trace.gpu.sort_by_key(|s| (s.frame, s.start_ns));
        trace.hitches.sort_by_key(|h| h.start_ns);
        trace.notes.sort_by_key(|n| n.at_ns);
        trace.coverage = trace.coverage_of();
        Ok(trace)
    }

    /// [`Trace::parse`] of text, as a test writes it.
    #[cfg(test)]
    pub fn read(text: &str) -> Result<Self> {
        Self::parse(text.as_bytes(), "t.json")
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

    /// The name a view counts a scope under: its name, and with `split`
    /// its text detail too (`script::call (scripts/stall.rhai)`), so one
    /// scope at many files reads as many. A count never splits.
    pub fn key(&self, s: &Scope, split: bool) -> String {
        match s.text() {
            Some(t) if split => format!("{} ({t})", self.name(s.name)),
            _ => self.name(s.name).to_string(),
        }
    }

    /// Whether scope `i` of `t` has an ancestor counted under the same
    /// key, so a scope inside itself isn't counted twice in a total.
    pub fn inside_itself(&self, t: &Thread, i: usize, split: bool) -> bool {
        let key = self.key(&t.scopes[i], split);
        let mut at = t.parent_of(i);
        while let Some(p) = at {
            if self.key(&t.scopes[p], split) == key {
                return true;
            }
            at = t.parent_of(p);
        }
        false
    }

    /// The keys from the outermost scope down to `i`'s parent.
    pub fn ancestors(&self, t: &Thread, i: usize, split: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = t.parent_of(i);
        while let Some(p) = at {
            out.push(self.key(&t.scopes[p], split));
            at = t.parent_of(p);
        }
        out.reverse();
        out
    }

    /// The frames the scopes cover, as positions in [`Trace::frames`]: all
    /// of them, unless a ring wrapped.
    pub fn covered(&self) -> Range<usize> {
        self.coverage.as_ref().map_or(0, |c| c.first)..self.frames.len()
    }

    /// The covered frames themselves.
    pub fn covered_frames(&self) -> &[FrameSpan] {
        &self.frames[self.covered()]
    }

    /// The position in [`Trace::frames`] of the frame with index `frame`.
    pub fn position(&self, frame: u32) -> Option<usize> {
        self.frames.iter().position(|f| f.frame == frame)
    }

    /// The frame (a position in [`Trace::frames`]) a moment falls in.
    pub fn frame_at(&self, at_ns: u64) -> Option<usize> {
        let i = self.frames.partition_point(|f| f.start_ns <= at_ns);
        let i = i.checked_sub(1)?;
        (at_ns < self.frames[i].end_ns() || i + 1 == self.frames.len()).then_some(i)
    }

    /// Hitch `h`'s frame: the one its mark's middle falls in (a mark spans
    /// its frame).
    pub fn hitch_frame(&self, h: usize) -> Option<usize> {
        let hitch = self.hitches.get(h)?;
        let mid = hitch.start_ns + hitch.dur_ns / 2;
        let i = self.frames.partition_point(|f| f.start_ns <= mid);
        (i > 0 && mid < self.frames[i - 1].end_ns()).then(|| i - 1)
    }

    /// Each hitch's frame, in track order.
    pub fn hitch_frames(&self) -> Vec<Option<usize>> {
        (0..self.hitches.len())
            .map(|h| self.hitch_frame(h))
            .collect()
    }

    /// The hitch this trace means by `hitch`, an index into
    /// [`Trace::hitches`]: see [`meant_hitch`].
    pub fn meant_hitch(&self) -> Option<usize> {
        let marks: Vec<HitchMark> = self
            .hitches
            .iter()
            .map(|h| HitchMark {
                game_time: h.game_time,
                judged_ms: h.judged(),
            })
            .collect();
        meant_hitch(self.header_str("reason"), &marks)
    }

    /// The frame `hitch` names, a covered position in [`Trace::frames`]:
    /// the hitch the trace means, unless it is before the scopes begin
    /// (#1288), when the worst hitch after them stands in. The tab's
    /// default frame and `flint profile`'s `hitch` alike.
    pub fn meant_hitch_frame(&self) -> Option<usize> {
        let covered = self.covered();
        let held = |f: Option<usize>| f.filter(|f| covered.contains(f));
        self.meant_hitch()
            .and_then(|h| held(self.hitch_frame(h)))
            .or_else(|| {
                (0..self.hitches.len())
                    .filter(|&h| held(self.hitch_frame(h)).is_some())
                    .max_by(|&a, &b| {
                        self.hitches[a]
                            .judged()
                            .total_cmp(&self.hitches[b].judged())
                    })
                    .and_then(|h| self.hitch_frame(h))
            })
    }

    /// The longest covered frame.
    pub fn worst_frame(&self) -> Option<usize> {
        self.covered()
            .max_by(|&a, &b| self.frames[a].dur_ns.cmp(&self.frames[b].dur_ns))
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

    /// The spans of `list` laid in recorder frame `frame`.
    pub fn spans_in<'a>(&self, list: &'a [Span], frame: u32) -> Range<usize> {
        let a = list.partition_point(|s| s.frame < Some(frame));
        let b = list.partition_point(|s| s.frame <= Some(frame));
        a..b
    }

    /// Each phase's ms in the frame at `pos` (`render` summed, as the
    /// player's history keeps it).
    pub fn phases_of(&self, pos: usize) -> BTreeMap<String, f64> {
        let frame = self.frames[pos].frame;
        let mut out = BTreeMap::new();
        for p in &self.phases[self.spans_in(&self.phases, frame)] {
            *out.entry(self.name(p.name).to_string()).or_insert(0.0) += p.dur_ns as f64 / 1e6;
        }
        out
    }

    /// The GPU passes of the frame at `pos`, as measured.
    pub fn gpu_of(&self, pos: usize) -> &[Span] {
        &self.gpu[self.spans_in(&self.gpu, self.frames[pos].frame)]
    }

    /// The frame notes laid at the frame at `pos`.
    pub fn notes_of(&self, pos: usize) -> Vec<&Note> {
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
            let name = self.name(p.name);
            if !order.iter().any(|o| o == name) {
                order.push(name.to_string());
            }
        }
        order
    }

    /// Where the scopes begin, when a ring lost part of the look-back
    /// (#1288). A writer since then says (`cut_threads`, empty for a whole
    /// window, and `covered_from_ms`); for a trace from before it, a thread
    /// the header calls `wrapped` whose oldest scope closed after the
    /// file's second frame ended was cut there (a whole copy's first frame
    /// has scopes that close inside it). A frame that began there or later
    /// has all its scopes.
    fn coverage_of(&self) -> Option<Coverage> {
        let text = |k: &str| self.header_str(k).map(str::to_string);
        let (from_ns, threads) = if self.header.contains_key("cut_threads") {
            let ms = self.header.get("covered_from_ms").and_then(Value::as_f64)?;
            (
                ns(ms * 1000.0),
                text("cut_threads").unwrap_or_else(|| "?".into()),
            )
        } else {
            let wrapped = text("wrapped")?;
            let second_end = self.frames.get(1).map(FrameSpan::end_ns)?;
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
            (from, names.join(", "))
        };
        let first = self.frames.partition_point(|f| f.start_ns < from_ns);
        if first == 0 {
            return None;
        }
        // A cut so late no frame is whole: the last frame, as it is
        let first = first.min(self.frames.len() - 1);
        let to_ns = self
            .header
            .get("window_to_ms")
            .and_then(Value::as_f64)
            .map(|ms| ns(ms * 1000.0))
            .or_else(|| self.frames.last().map(FrameSpan::end_ns))
            .unwrap_or(from_ns);
        Some(Coverage {
            from_ns,
            first,
            threads,
            window_s: self
                .header
                .get("seconds")
                .and_then(Value::as_f64)
                .filter(|s| *s > 0.0),
            covered_s: to_ns.saturating_sub(from_ns) as f64 / 1e9,
        })
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
    use crate::commands::profile::{pick_frame, stats::per_frame};

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

    /// A trace of `events` on one `main` thread.
    fn on_main(events: &str) -> Trace {
        Trace::read(&format!(
            r#"{{"traceEvents":[{{"ph":"M","name":"thread_name","pid":1,"tid":1,"args":{{"name":"main"}}}},{events}],"otherData":{{"format":"flint-trace","version":1}}}}"#
        ))
        .unwrap()
    }

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
        // And as read from a file, by hitch and by frame
        let t = Trace::read(&three_hitches(SAVED_ON_SECOND)).unwrap();
        assert_eq!(t.meant_hitch(), Some(1));
        assert_eq!(t.frames[t.meant_hitch_frame().unwrap()].frame, 114);
        let t = Trace::read(&three_hitches("exit")).unwrap();
        assert_eq!(t.meant_hitch(), Some(0));
        assert_eq!(t.frames[t.meant_hitch_frame().unwrap()].frame, 105);
    }

    #[test]
    fn a_players_trace_is_read_into_threads_frames_and_its_own_tracks() {
        let t = Trace::read(&sample()).unwrap();
        assert_eq!(t.thread_names(), ["main", "loader"]);
        assert_eq!(t.threads[0].scopes.len(), 12);
        // Sorted by frame, start, depth: the parent before its children
        let first = &t.threads[0].scopes[0];
        assert_eq!((t.name(first.name), first.depth), ("tick", 0));
        assert_eq!(first.start_ns, 1_100_000);
        assert_eq!(t.frames.len(), 3);
        assert!(!t.frames_derived);
        assert_eq!(t.frames[2].dur_ns, 40_000_000);
        assert_eq!(t.phases.len(), 6);
        assert_eq!(t.gpu.len(), 3);
        assert_eq!(t.hitches.len(), 1);
        assert_eq!(t.hitches[0].history_frame, Some(2));
        assert_eq!(t.hitch_frames(), vec![Some(2)]);
        assert_eq!(t.notes[0].kind, "model load");
        assert_eq!(t.notes[0].ms, Some(4.5));
        let render = t.threads[0]
            .scopes
            .iter()
            .find(|s| t.name(s.name) == "render")
            .unwrap();
        assert_eq!(render.detail, Some(Detail::Count(12)), "a count, not text");
        assert_eq!(t.key(render, true), "render", "a count never splits");
        assert_eq!(t.header_str("scene"), Some("levels/stall.scene.toml"));
        assert_eq!(t.phases_of(2)["scripts"], 31.4);
        assert_eq!(t.phase_order(), ["scripts", "render"]);
        assert_eq!(t.covered(), 0..3);
    }

    #[test]
    fn a_parent_written_after_its_children_is_their_parent_and_self_time_is_what_they_leave() {
        let t = on_main(
            r#"{"ph":"X","name":"child","pid":1,"tid":1,"ts":10.0,"dur":3.0,"args":{"frame":0,"depth":1}},
               {"ph":"X","name":"child","pid":1,"tid":1,"ts":14.0,"dur":2.0,"args":{"frame":0,"depth":1}},
               {"ph":"X","name":"tick","pid":1,"tid":1,"ts":10.0,"dur":10.0,"args":{"frame":0,"depth":0}}"#,
        );
        let main = &t.threads[0];
        assert_eq!(
            t.name(main.scopes[0].name),
            "tick",
            "the parent sorts first"
        );
        assert_eq!(main.parent, vec![ROOT, 0, 0]);
        assert_eq!(main.self_ns[0], 5_000);
        assert!(t.frames_derived);
        assert_eq!(t.frames[0].dur_ns, 10_000);
    }

    #[test]
    fn a_parent_that_closed_in_a_later_frame_is_still_its_childrens_parent() {
        // A worker's load across frames 3 to 5: its children closed in 3
        // and 4, sorted before it, and are still its own
        let t = on_main(
            r#"{"ph":"X","name":"decode","pid":1,"tid":1,"ts":10.0,"dur":5.0,"args":{"frame":3,"depth":1}},
               {"ph":"X","name":"upload","pid":1,"tid":1,"ts":20.0,"dur":5.0,"args":{"frame":4,"depth":1}},
               {"ph":"X","name":"load","pid":1,"tid":1,"ts":5.0,"dur":40.0,"args":{"frame":5,"depth":0}}"#,
        );
        let main = &t.threads[0];
        let at = |n: &str| {
            main.scopes
                .iter()
                .position(|s| t.name(s.name) == n)
                .unwrap()
        };
        assert_eq!(main.parent_of(at("decode")), Some(at("load")));
        assert_eq!(main.parent_of(at("upload")), Some(at("load")));
        assert_eq!(main.self_ns[at("load")], 30_000);
        assert_eq!(main.in_frame(4), at("upload")..at("upload") + 1);
    }

    #[test]
    fn a_summed_record_is_a_leaf_its_overlapped_siblings_children_keep_their_parent() {
        // on_update holds helper() and get_field()'s summed record, which
        // overlaps helper(); helper()'s child is helper()'s
        let t = on_main(
            r#"{"ph":"X","name":"get_field()","pid":1,"tid":1,"ts":5.0,"dur":30.0,"args":{"frame":0,"depth":1,"calls":180,"worst_us":0.5}},
               {"ph":"X","name":"clamp()","pid":1,"tid":1,"ts":20.0,"dur":2.0,"args":{"frame":0,"depth":2,"calls":4,"worst_us":0.75}},
               {"ph":"X","name":"helper()","pid":1,"tid":1,"ts":10.0,"dur":50.0,"args":{"frame":0,"depth":1}},
               {"ph":"X","name":"on_update","pid":1,"tid":1,"ts":0.0,"dur":100.0,"args":{"frame":0,"depth":0}}"#,
        );
        let main = &t.threads[0];
        let at = |name: &str| {
            main.scopes
                .iter()
                .position(|s| t.name(s.name) == name)
                .unwrap()
        };
        assert_eq!(main.parent_of(at("get_field()")), Some(at("on_update")));
        assert_eq!(main.parent_of(at("helper()")), Some(at("on_update")));
        assert_eq!(
            main.parent_of(at("clamp()")),
            Some(at("helper()")),
            "not the summed record it lies inside"
        );
        let s = &main.scopes[at("get_field()")];
        assert_eq!((s.calls(), s.worst_ns()), (180, 500));
        assert_eq!(main.self_ns[at("on_update")], 20_000, "100 - 50 - 30 us");
        let per = per_frame(&t, &[main], false);
        assert_eq!(per[0]["get_field()"].calls, 180, "a view counts its calls");
        assert_eq!(per[0]["get_field()"].worst_ns, 500, "and its longest");
    }

    /// Five frames, of which the first two began before the main ring's
    /// oldest record: the second is the longest, and has no scopes.
    fn wrapped(header: &str) -> Trace {
        let mut events = vec![
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":1,"args":{"name":"main"}}"#.to_string(),
            r#"{"ph":"M","name":"thread_name","pid":1,"tid":2,"args":{"name":"frames"}}"#
                .to_string(),
        ];
        let lengths = [10.0, 90.0, 10.0, 12.0, 10.0];
        let mut ts = 0.0;
        for (f, len) in lengths.iter().enumerate() {
            events.push(format!(
                r#"{{"ph":"X","name":"frame","pid":1,"tid":2,"ts":{ts},"dur":{len},"args":{{"frame":{f},"ms":{}}}}}"#,
                len / 1000.0
            ));
            if f >= 2 {
                events.push(format!(
                    r#"{{"ph":"X","name":"tick","pid":1,"tid":1,"ts":{ts},"dur":{},"args":{{"frame":{f},"depth":0}}}}"#,
                    len - 1.0
                ));
            }
            ts += len;
        }
        Trace::read(&format!(
            r#"{{"traceEvents":[{}],"otherData":{{"format":"flint-trace","version":1,{header}}}}}"#,
            events.join(",")
        ))
        .unwrap()
    }

    #[test]
    fn a_wrapped_trace_chooses_only_from_the_frames_its_scopes_cover_and_says_so() {
        for (header, kept, said) in [
            // What the player writes since #1288: frame 2 began at 0.1 ms
            (
                r#""seconds":5,"wrapped":"main","covered_from_ms":0.1,"cut_threads":"main""#,
                vec![2, 3, 4],
                "scopes cover frames 2..4",
            ),
            // A trace from before it: from the wrapped ring's oldest close
            // (frame 2's tick, 0.109 ms), so frame 2, which began before
            // it and may have lost a scope, is left out too
            (
                r#""seconds":5,"wrapped":"main","window_from_ms":0"#,
                vec![3, 4],
                "scopes cover frames 3..4",
            ),
        ] {
            let t = wrapped(header);
            let c = t.coverage.as_ref().expect("the scopes begin late");
            assert_eq!(
                t.covered_frames()
                    .iter()
                    .map(|f| f.frame)
                    .collect::<Vec<_>>(),
                kept,
                "{header}"
            );
            assert_eq!(t.frames.len(), 5, "every frame is kept, the covered named");
            let line = c.line(&t.frames);
            let before = 5 - kept.len();
            assert!(
                line.starts_with(said)
                    && line.contains("of 5 s")
                    && line.contains(&format!(
                        "the main ring wrapped, so the {before} frames before them have no scopes"
                    )),
                "{line}"
            );
            assert_eq!(t.frames[t.worst_frame().unwrap()].frame, 3);
            let worst = pick_frame(&t, "worst").unwrap();
            assert_eq!(t.frames[worst].frame, 3, "the worst covered frame, not 1");
            let err = pick_frame(&t, "1").unwrap_err().to_string();
            assert!(err.contains("before its scopes begin"), "{err}");
        }
        // Whole: a ring that never wrapped, and one that wrapped but held
        // the window, which the writer says with an empty `cut_threads`
        for header in [
            r#""seconds":5,"wrapped":"""#,
            r#""seconds":5,"wrapped":"main","cut_threads":"""#,
        ] {
            let whole = wrapped(header);
            assert!(whole.coverage.is_none(), "{header}");
            assert_eq!(whole.covered(), 0..5);
        }
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
    fn what_is_not_a_trace_is_refused_saying_what_it_wants() {
        for (text, says) in [
            ("{\"format\":\"flintframe\"}", "is not a trace"),
            ("not json", "is not a trace"),
            (
                r#"{"traceEvents":[{"ph":"X","name":"x","ts":1,"dur":1}]}"#,
                "no Flint scopes",
            ),
            (
                r#"{"traceEvents":[],"otherData":{"format":"flintframe"}}"#,
                "not a flint-trace",
            ),
        ] {
            let err = format!("{:#}", Trace::read(text).err().unwrap());
            assert!(err.contains(says), "{text}: {err}");
        }
        assert!(!Trace::looks_like(
            "{\"format\":\"flintframe\",\"passes\":[]}"
        ));
        assert!(Trace::looks_like(&sample()));
    }

    #[test]
    fn a_bare_event_list_of_another_writer_reads() {
        let text = r#"[{"ph":"X","name":"work","pid":7,"tid":"io","ts":5,"dur":3,"args":{"frame":4,"depth":0}}]"#;
        let t = Trace::read(text).unwrap();
        assert_eq!(t.thread_names(), ["io"]);
        assert_eq!(t.threads[0].scopes[0].frame, 4);
        assert!(t.frames_derived);
        assert_eq!(t.frames[0].frame, 4);
    }

    #[test]
    fn text_detail_splits_a_key_and_a_count_does_not() {
        let t = on_main(
            r#"{"ph":"X","name":"script::call","pid":1,"tid":1,"ts":0,"dur":1,"args":{"frame":0,"depth":0,"detail":"scripts/a.rhai"}},
               {"ph":"X","name":"script::call","pid":1,"tid":1,"ts":2,"dur":1,"args":{"frame":0,"depth":0,"detail":3}}"#,
        );
        let s = &t.threads[0].scopes;
        assert_eq!(t.key(&s[0], true), "script::call (scripts/a.rhai)");
        assert_eq!(t.key(&s[0], false), "script::call");
        assert_eq!(t.key(&s[1], true), "script::call");
        assert_eq!(s[1].detail.as_ref().unwrap().to_string(), "3");
    }
}
