//! What a trace is worked into once it is read: frames as rows, each
//! scope's parent, path, self time and system, and the questions the
//! profiler tab asks of a selection of frames (#1099).
//!
//! - A **row** is a frame of the `frames` track (or, for a trace without
//!   one, of the scopes' `frame` numbers), with its phases folded into the
//!   F2 overlay's bands and the hitch on it, if any.
//! - A scope's **path** is its thread and the names from its root down
//!   (`main / tick / script::update / stall.rhai:on_update`): what the
//!   median frame is compared by, since one name can be called from more
//!   than one place.
//! - A scope's **system** is the overlay's band of the phase it ran inside
//!   on the frame thread (so the two tools colour one moment alike), else
//!   one read from its name, else none (`tick`, which spans the frame).
//! - [`Profile::stats`] is every scope name over a range of rows: count,
//!   total, mean, p90, worst. [`Profile::series`] is one name's time in
//!   each row. [`Profile::baseline`] is each path's median time a frame,
//!   which [`Profile::excess_ns`] compares a frame's with: the visual twin
//!   of `flint profile explain` (#1098).

use super::{Scope, Trace};
use crate::commands::frame::open::timing::band_of;
use flint_debug_ui::widgets::{Band, BANDS};
use std::collections::HashMap;
use std::ops::Range;

/// No parent.
pub const ROOT: u32 = u32::MAX;

/// A frame, as a row of the strip.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRow {
    /// The recorder's frame index (the scopes' `frame`).
    pub index: u32,
    pub start_ns: u64,
    pub end_ns: u64,
    /// Its phases' time in each of the overlay's bands, ms; what no phase
    /// accounts for is `Elsewhere`.
    pub bands: [f32; BANDS],
    /// The hitch on it, an index into [`Trace::hitches`].
    pub hitch: Option<usize>,
    /// Its phases and GPU passes, as ranges of [`Trace::phases`] and
    /// [`Trace::gpu`].
    pub phases: Range<usize>,
    pub gpu: Range<usize>,
}

impl FrameRow {
    pub fn ms(&self) -> f32 {
        (self.end_ns - self.start_ns) as f32 / 1e6
    }
}

/// A distinct path: a name under a parent path, on one thread.
#[derive(Debug, Clone, PartialEq)]
pub struct PathInfo {
    pub name: u32,
    pub parent: u32,
    pub thread: u16,
}

/// What is known of each scope beyond the file, parallel to a thread's
/// `scopes`.
#[derive(Debug, Clone, Default)]
pub struct ScopeFacts {
    /// Its parent, an index into the same thread's scopes, or [`ROOT`].
    pub parent: Vec<u32>,
    /// Its path, an index into [`Profile::paths`].
    pub path: Vec<u32>,
    /// Its duration less its children's, ns.
    pub self_ns: Vec<u64>,
    /// Its system, or `None` (a scope across phases).
    pub band: Vec<Option<Band>>,
    /// Each row's scopes, as a range of the thread's (they are sorted by
    /// frame first).
    pub rows: Vec<Range<usize>>,
    /// The deepest scope's depth.
    pub max_depth: u16,
}

/// A scope name's statistics over a selection of frames, ns.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeStat {
    pub name: u32,
    pub count: u32,
    pub total_ns: u64,
    pub mean_ns: u64,
    pub p90_ns: u64,
    pub worst_ns: u64,
    /// Where the worst call is: thread, scope.
    pub worst_at: (usize, usize),
}

/// The columns statistics sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatSort {
    Name,
    Count,
    Total,
    Mean,
    P90,
    Worst,
}

/// Each path's median time a frame over every row, ns (absent counts as 0).
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    pub median_ns: Vec<u64>,
    /// The median frame's length, ms.
    pub median_frame_ms: f32,
    /// Each scope's path's excess in its frame, ns, by thread then scope:
    /// worked out once, so a chart of every frame tints at no cost.
    pub excess_ns: Vec<Vec<i64>>,
    /// Each scope's heat, 0 (as usual) to 1 (the frame's excess): what
    /// Compare tints a bar by. Laid out as `excess_ns`.
    pub heat: Vec<Vec<f32>>,
}

/// A trace worked out.
pub struct Profile {
    pub trace: Trace,
    pub rows: Vec<FrameRow>,
    /// The recorder's frame index to its row.
    row_of: HashMap<u32, usize>,
    /// The thread the frames run on (the one with `tick` at its root).
    pub frame_thread: Option<usize>,
    pub facts: Vec<ScopeFacts>,
    pub paths: Vec<PathInfo>,
    /// Each path's time in each row it appears in, (row, ns), by row.
    path_rows: Vec<Vec<(u32, u64)>>,
    baseline: Option<Baseline>,
    /// The first row with all its scopes: 0 unless a ring lost part of
    /// the look-back (#1288), when the rows before it lack theirs.
    pub first_covered: usize,
    /// What the strip and the tab say when it isn't 0: `scopes cover
    /// frames 5729..5932 (0.85 s of 5 s): the main ring wrapped`.
    pub coverage: Option<String>,
}

/// A system read from a scope's name, for scopes off the frame thread and
/// those no phase holds.
pub fn band_by_name(name: &str) -> Option<Band> {
    if name.contains(".rhai:") || name.starts_with("script") {
        return Some(Band::Scripts);
    }
    if name.starts_with("physics") || name == "tick_fixed_physics" {
        return Some(Band::Physics);
    }
    if name.starts_with("egui") || name == "render_hud" || name == "present" {
        return Some(Band::HudPresent);
    }
    const RENDER: [&str; 12] = [
        "render",
        "encode",
        "extract::",
        "upload",
        "cull",
        "prepare",
        "submit",
        "sync_renderer",
        "grass_compute",
        "bind_scene",
        "update_per_frame",
        "sort_blended",
    ];
    if RENDER.iter().any(|p| name.starts_with(p)) {
        return Some(Band::Render);
    }
    const OTHER: [&str; 9] = [
        "animation::",
        "audio::",
        "particles::",
        "living::",
        "procgen::",
        "terrain::",
        "decode_image",
        "load_",
        "import_model",
    ];
    if OTHER.iter().any(|p| name.starts_with(p)) {
        return Some(Band::Other);
    }
    None
}

/// Whether a scope is a script callback (`<source>:<callback>`, #1097).
pub fn is_script(name: &str) -> bool {
    name.contains(".rhai:")
}

impl Profile {
    pub fn new(trace: Trace) -> Self {
        let rows = rows_of(&trace);
        let row_of: HashMap<u32, usize> =
            rows.iter().enumerate().map(|(i, r)| (r.index, i)).collect();
        let frame_thread = trace
            .threads
            .iter()
            .position(|t| {
                t.scopes
                    .iter()
                    .any(|s| s.depth == 0 && trace.name(s.name) == "tick")
            })
            .or_else(|| {
                // Else the thread with the most scopes
                (0..trace.threads.len()).max_by_key(|&i| trace.threads[i].scopes.len())
            });

        let mut paths: Vec<PathInfo> = Vec::new();
        let mut path_ids: HashMap<(u32, u32, u16), u32> = HashMap::new();
        let mut path_rows: Vec<Vec<(u32, u64)>> = Vec::new();
        let mut facts = Vec::with_capacity(trace.threads.len());
        for (ti, thread) in trace.threads.iter().enumerate() {
            let n = thread.scopes.len();
            let mut f = ScopeFacts {
                parent: vec![ROOT; n],
                path: vec![0; n],
                self_ns: thread.scopes.iter().map(|s| s.dur_ns).collect(),
                band: vec![None; n],
                rows: Vec::new(),
                max_depth: 0,
            };
            // Parents: the scopes are by frame, start, depth, so the
            // innermost open scope that holds a scope is its parent
            let mut stack: Vec<usize> = Vec::new();
            for (i, s) in thread.scopes.iter().enumerate() {
                while let Some(&top) = stack.last() {
                    let p = &thread.scopes[top];
                    if p.frame == s.frame && p.depth < s.depth && s.start_ns < p.end_ns() {
                        break;
                    }
                    stack.pop();
                }
                let parent = stack.last().copied();
                f.max_depth = f.max_depth.max(s.depth);
                let parent_path = parent.map_or(ROOT, |p| f.path[p]);
                if let Some(p) = parent {
                    f.parent[i] = p as u32;
                    f.self_ns[p] = f.self_ns[p].saturating_sub(s.dur_ns);
                }
                let key = (parent_path, s.name, ti as u16);
                let path = *path_ids.entry(key).or_insert_with(|| {
                    paths.push(PathInfo {
                        name: s.name,
                        parent: parent_path,
                        thread: ti as u16,
                    });
                    path_rows.push(Vec::new());
                    (paths.len() - 1) as u32
                });
                f.path[i] = path;
                if let Some(&row) = row_of.get(&s.frame) {
                    let list = &mut path_rows[path as usize];
                    match list.last_mut() {
                        Some((r, ns)) if *r == row as u32 => *ns += s.dur_ns,
                        _ => list.push((row as u32, s.dur_ns)),
                    }
                }
                // A summed record is a leaf (#1288): it may overlap a
                // sibling, whose children must not take it for a parent
                if s.summed.is_none() {
                    stack.push(i);
                }
            }
            // Each row's range of scopes
            f.rows = rows
                .iter()
                .map(|r| {
                    let a = thread.scopes.partition_point(|s| s.frame < r.index);
                    let b = thread.scopes.partition_point(|s| s.frame <= r.index);
                    a..b
                })
                .collect();
            // Systems: the phase it ran inside, on the frame thread
            for (i, s) in thread.scopes.iter().enumerate() {
                let name = trace.name(s.name);
                let by_phase = if Some(ti) == frame_thread {
                    row_of.get(&s.frame).and_then(|&r| {
                        trace.phases[rows[r].phases.clone()]
                            .iter()
                            .find(|p| p.start_ns <= s.start_ns && s.end_ns() <= p.end_ns() + 1_000)
                            .map(|p| band_of(trace.name(p.name)))
                    })
                } else {
                    None
                };
                f.band[i] = by_phase.or_else(|| band_by_name(name)).or(
                    // A worker's own work, with no phase to be in
                    (Some(ti) != frame_thread).then_some(Band::Other),
                );
            }
            facts.push(f);
        }
        let (first_covered, coverage) = match trace.covered_from() {
            Some((from_ns, threads)) => {
                let first = rows.partition_point(|r| r.start_ns < from_ns);
                if first == 0 {
                    (0, None)
                } else {
                    // A cut so late no row is whole: the last, as it is
                    let first = first.min(rows.len().saturating_sub(1));
                    let last = rows.len() - 1;
                    let to = trace
                        .header
                        .get("window_to_ms")
                        .and_then(serde_json::Value::as_f64)
                        .map(|ms| (ms * 1e6) as u64)
                        .unwrap_or(rows[last].end_ns);
                    let covered_s = to.saturating_sub(from_ns) as f64 / 1e9;
                    let of = trace
                        .header
                        .get("seconds")
                        .and_then(serde_json::Value::as_f64)
                        .filter(|s| *s > 0.0)
                        .map_or(String::new(), |s| format!(" of {s:.0} s"));
                    let line = format!(
                        "scopes cover frames {}..{} ({covered_s:.2} s{of}): the {threads} ring wrapped, \
                         so the {first} frame{} before them {} none",
                        rows[first].index,
                        rows[last].index,
                        if first == 1 { "" } else { "s" },
                        if first == 1 { "has" } else { "have" },
                    );
                    (first, Some(line))
                }
            }
            None => (0, None),
        };
        Self {
            trace,
            rows,
            row_of,
            frame_thread,
            facts,
            paths,
            path_rows,
            baseline: None,
            first_covered,
            coverage,
        }
    }

    /// The rows the scopes cover (all of them, unless a ring wrapped).
    pub fn covered(&self) -> Range<usize> {
        self.first_covered..self.rows.len()
    }

    pub fn name(&self, id: u32) -> &str {
        self.trace.name(id)
    }

    pub fn scope(&self, thread: usize, i: usize) -> &Scope {
        &self.trace.threads[thread].scopes[i]
    }

    /// The row a recorder frame index is.
    pub fn row_of(&self, frame: u32) -> Option<usize> {
        self.row_of.get(&frame).copied()
    }

    /// The row holding `ns`, or the nearest.
    pub fn row_at(&self, ns: u64) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let i = self.rows.partition_point(|r| r.start_ns <= ns);
        Some(i.saturating_sub(1).min(self.rows.len() - 1))
    }

    /// The row a hitch is on.
    pub fn hitch_row(&self, hitch: usize) -> Option<usize> {
        self.rows.iter().position(|r| r.hitch == Some(hitch))
    }

    /// The rows that hitched, in order.
    pub fn hitch_rows(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&r| self.rows[r].hitch.is_some())
            .collect()
    }

    /// The longest row the scopes cover.
    pub fn worst_row(&self) -> Option<usize> {
        self.covered()
            .max_by_key(|&r| self.rows[r].end_ns - self.rows[r].start_ns)
    }

    /// A path written out: `tick / script::update / stall.rhai:on_update`.
    pub fn path_text(&self, path: u32) -> String {
        let mut names = Vec::new();
        let mut p = path;
        while p != ROOT {
            let info = &self.paths[p as usize];
            names.push(self.name(info.name));
            p = info.parent;
        }
        names.reverse();
        names.join(" / ")
    }

    /// Every name's statistics over `rows`, every thread's.
    ///
    /// A summed record (#1288) counts as its calls: `count` is calls,
    /// `mean` a call's, `worst` the longest call, and its share of the p90
    /// is its mean call, one value a record (a call's own time isn't kept).
    pub fn stats(&self, rows: Range<usize>) -> Vec<ScopeStat> {
        #[derive(Default)]
        struct Acc {
            durs: Vec<u64>,
            calls: u64,
            total: u64,
            worst: u64,
        }
        let mut by_name: HashMap<u32, (Acc, (usize, usize))> = HashMap::new();
        for (ti, thread) in self.trace.threads.iter().enumerate() {
            let f = &self.facts[ti];
            let Some(first) = f.rows.get(rows.start) else {
                continue;
            };
            let last = &f.rows[rows.end.saturating_sub(1).min(f.rows.len() - 1)];
            for i in first.start..last.end {
                let s = &thread.scopes[i];
                let entry = by_name.entry(s.name).or_insert((Acc::default(), (ti, i)));
                if s.worst_ns() > self.scope(entry.1 .0, entry.1 .1).worst_ns() {
                    entry.1 = (ti, i);
                }
                let acc = &mut entry.0;
                acc.durs.push(s.dur_ns / s.calls().max(1) as u64);
                acc.calls += s.calls() as u64;
                acc.total += s.dur_ns;
                acc.worst = acc.worst.max(s.worst_ns());
            }
        }
        let mut out: Vec<ScopeStat> = by_name
            .into_iter()
            .map(|(name, (mut acc, worst_at))| {
                acc.durs.sort_unstable();
                let n = acc.durs.len();
                // Nearest rank
                let p90 = acc.durs[((n as f64 * 0.9).ceil() as usize).clamp(1, n) - 1];
                let count = acc.calls.max(1);
                ScopeStat {
                    name,
                    count: count.min(u32::MAX as u64) as u32,
                    total_ns: acc.total,
                    mean_ns: acc.total / count,
                    p90_ns: p90,
                    worst_ns: acc.worst,
                    worst_at,
                }
            })
            .collect();
        self.sort_stats(&mut out, StatSort::Total, true);
        out
    }

    /// Sort statistics by `by`, largest first when `descending` (names
    /// A to Z when not).
    pub fn sort_stats(&self, stats: &mut [ScopeStat], by: StatSort, descending: bool) {
        let key = |s: &ScopeStat| -> u64 {
            match by {
                StatSort::Name | StatSort::Total => s.total_ns,
                StatSort::Count => s.count as u64,
                StatSort::Mean => s.mean_ns,
                StatSort::P90 => s.p90_ns,
                StatSort::Worst => s.worst_ns,
            }
        };
        stats.sort_by(|a, b| {
            let o = if by == StatSort::Name {
                self.name(a.name).cmp(self.name(b.name))
            } else {
                key(a).cmp(&key(b))
            };
            let o = if descending { o.reverse() } else { o };
            o.then_with(|| self.name(a.name).cmp(self.name(b.name)))
        });
    }

    /// One name's time in each row, ms (every call on every thread).
    pub fn series(&self, name: u32) -> Vec<f32> {
        let mut out = vec![0.0f32; self.rows.len()];
        for (p, info) in self.paths.iter().enumerate() {
            if info.name != name {
                continue;
            }
            for &(row, ns) in &self.path_rows[p] {
                out[row as usize] += ns as f32 / 1e6;
            }
        }
        out
    }

    /// Each path's median time a frame, worked out on first use.
    pub fn baseline(&mut self) -> &Baseline {
        if self.baseline.is_none() {
            let n = self.rows.len();
            let median_ns: Vec<u64> = self
                .path_rows
                .iter()
                .map(|list| {
                    // The rows a path is absent from count as zero
                    let zeros = n.saturating_sub(list.len());
                    let mid = n / 2;
                    if n == 0 || mid < zeros {
                        return 0;
                    }
                    let mut v: Vec<u64> = list.iter().map(|&(_, ns)| ns).collect();
                    v.sort_unstable();
                    v[(mid - zeros).min(v.len() - 1)]
                })
                .collect();
            let mut lengths: Vec<u64> = self.rows.iter().map(|r| r.end_ns - r.start_ns).collect();
            lengths.sort_unstable();
            let median_frame_ms = lengths
                .get(lengths.len() / 2)
                .map_or(0.0, |&l| l as f32 / 1e6);
            let excess_ns = self
                .trace
                .threads
                .iter()
                .enumerate()
                .map(|(ti, t)| {
                    let paths = &self.facts[ti].path;
                    t.scopes
                        .iter()
                        .enumerate()
                        .map(|(i, s)| match self.row_of.get(&s.frame) {
                            Some(&row) => {
                                let path = paths[i];
                                self.path_ns(path, row) as i64 - median_ns[path as usize] as i64
                            }
                            None => 0,
                        })
                        .collect()
                })
                .collect::<Vec<Vec<i64>>>();
            // How hot each bar is: its excess against a scale of how far the
            // whole frame ran over the median (so the cause of a 100 ms
            // hitch is as red as that of a 10 ms one), never under half the
            // median frame or half a millisecond (so a calm frame's noise
            // stays grey)
            let heat = self
                .trace
                .threads
                .iter()
                .enumerate()
                .map(|(ti, t)| {
                    t.scopes
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            let row_excess = self
                                .row_of
                                .get(&s.frame)
                                .map_or(0.0, |&r| self.rows[r].ms() - median_frame_ms);
                            let scale = (row_excess * 0.6).max(median_frame_ms * 0.5).max(0.5);
                            let excess = excess_ns[ti][i] as f32 / 1e6;
                            (excess / scale).clamp(0.0, 1.0).powf(0.7)
                        })
                        .collect()
                })
                .collect();
            self.baseline = Some(Baseline {
                median_ns,
                median_frame_ms,
                excess_ns,
                heat,
            });
        }
        self.baseline.as_ref().unwrap()
    }

    /// The baseline, if it has been worked out.
    pub fn baseline_if_made(&self) -> Option<&Baseline> {
        self.baseline.as_ref()
    }

    /// A path's time in `row`, ns.
    pub fn path_ns(&self, path: u32, row: usize) -> u64 {
        let list = &self.path_rows[path as usize];
        match list.binary_search_by_key(&(row as u32), |&(r, _)| r) {
            Ok(i) => list[i].1,
            Err(_) => 0,
        }
    }

    /// How much longer than the median frame's a scope's path took in its
    /// frame, ns (negative when it took less). Needs [`baseline`](Self::baseline).
    pub fn excess_ns(&self, thread: usize, i: usize) -> Option<i64> {
        let b = self.baseline.as_ref()?;
        self.row_of(self.scope(thread, i).frame)?;
        Some(b.excess_ns[thread][i])
    }

    /// The span of time `rows` covers, ns.
    pub fn span_of(&self, rows: Range<usize>) -> Option<(u64, u64)> {
        let a = self.rows.get(rows.start)?;
        let b = self.rows.get(rows.end.saturating_sub(1))?;
        Some((a.start_ns, b.end_ns.max(a.start_ns + 1)))
    }

    /// The rows that touch `[t0, t1)`.
    pub fn rows_touching(&self, t0: u64, t1: u64) -> Range<usize> {
        let a = self.rows.partition_point(|r| r.end_ns <= t0);
        let b = self.rows.partition_point(|r| r.start_ns < t1);
        a..b.max(a)
    }

    /// Notes and hitch marks fall on the scopes' clock; the time the trace
    /// spans, ns.
    pub fn whole(&self) -> Option<(u64, u64)> {
        self.span_of(0..self.rows.len())
    }
}

/// The frames as rows: the `frames` track, else every scope thread's
/// `frame` numbers, each from its first start to its last end.
fn rows_of(trace: &Trace) -> Vec<FrameRow> {
    let mut spans: Vec<(u32, u64, u64)> = trace
        .frames
        .iter()
        .map(|f| (f.index, f.start_ns, f.end_ns))
        .collect();
    if spans.is_empty() {
        let mut by: HashMap<u32, (u64, u64)> = HashMap::new();
        for t in &trace.threads {
            for s in &t.scopes {
                let e = by.entry(s.frame).or_insert((s.start_ns, s.end_ns()));
                e.0 = e.0.min(s.start_ns);
                e.1 = e.1.max(s.end_ns());
            }
        }
        spans = by.into_iter().map(|(f, (a, b))| (f, a, b)).collect();
        spans.sort_by_key(|s| s.1);
    }
    // One row a frame index (a frames track never repeats one, but be sure)
    let mut seen = std::collections::HashSet::new();
    spans.retain(|s| seen.insert(s.0));
    let range_of = |list: &[super::Span], frame: u32| {
        let a = list.partition_point(|s| s.frame < frame);
        let b = list.partition_point(|s| s.frame <= frame);
        a..b
    };
    let mut rows: Vec<FrameRow> = spans
        .into_iter()
        .map(|(index, start_ns, end_ns)| {
            let phases = range_of(&trace.phases, index);
            let mut bands = [0.0f32; BANDS];
            let mut sum = 0.0;
            for p in &trace.phases[phases.clone()] {
                let ms = p.dur_ns as f32 / 1e6;
                bands[band_of(trace.name(p.name)) as usize] += ms;
                sum += ms;
            }
            let ms = (end_ns.saturating_sub(start_ns)) as f32 / 1e6;
            bands[Band::Elsewhere as usize] = (ms - sum).max(0.0);
            FrameRow {
                index,
                start_ns,
                end_ns: end_ns.max(start_ns),
                bands,
                hitch: None,
                phases,
                gpu: range_of(&trace.gpu, index),
            }
        })
        .collect();
    // A hitch's mark spans its frame: the row its middle falls in
    for (h, hitch) in trace.hitches.iter().enumerate() {
        let mid = hitch.start_ns + hitch.dur_ns / 2;
        let i = rows.partition_point(|r| r.start_ns <= mid);
        if i > 0 && mid < rows[i - 1].end_ns {
            rows[i - 1].hitch = Some(h);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::super::tests::sample;
    use super::*;

    fn profile() -> Profile {
        Profile::new(Trace::read(&sample()).unwrap())
    }

    fn find(p: &Profile, thread: usize, name: &str, frame: u32) -> usize {
        p.trace.threads[thread]
            .scopes
            .iter()
            .position(|s| p.name(s.name) == name && s.frame == frame)
            .unwrap()
    }

    #[test]
    fn frames_are_rows_with_their_bands_and_hitch() {
        let p = profile();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.frame_thread, Some(0));
        let r = &p.rows[2];
        assert_eq!(r.ms(), 40.0);
        assert_eq!(r.hitch, Some(0));
        assert!(p.rows[0].hitch.is_none());
        assert!((r.bands[Band::Scripts as usize] - 31.4).abs() < 1e-3);
        assert!((r.bands[Band::Render as usize] - 6.2).abs() < 1e-3);
        let elsewhere = 40.0 - 31.4 - 6.2;
        assert!((r.bands[Band::Elsewhere as usize] - elsewhere).abs() < 1e-3);
        assert_eq!(p.hitch_rows(), vec![2]);
        assert_eq!(p.worst_row(), Some(2));
        assert_eq!(p.hitch_row(0), Some(2));
    }

    #[test]
    fn a_scopes_parent_path_self_time_and_system_are_worked_out() {
        let p = profile();
        let stall = find(&p, 0, "stall.rhai:on_update", 2);
        let update = find(&p, 0, "script::update", 2);
        let tick = find(&p, 0, "tick", 2);
        let f = &p.facts[0];
        assert_eq!(f.parent[stall], update as u32);
        assert_eq!(f.parent[update], tick as u32);
        assert_eq!(f.parent[tick], ROOT);
        assert_eq!(
            p.path_text(f.path[stall]),
            "tick / script::update / stall.rhai:on_update"
        );
        // The same path in every frame
        assert_eq!(
            f.path[find(&p, 0, "stall.rhai:on_update", 0)],
            f.path[stall]
        );
        assert_eq!(f.self_ns[update], 200_000);
        // tick less its two children
        assert_eq!(f.self_ns[tick], 39_700_000 - 31_200_000 - 6_000_000);
        // The phase it ran in colours it; tick spans phases
        assert_eq!(f.band[stall], Some(Band::Scripts));
        assert_eq!(f.band[find(&p, 0, "render", 2)], Some(Band::Render));
        assert_eq!(f.band[tick], None);
        // A worker's scope by its name
        assert_eq!(p.facts[1].band[0], Some(Band::Other));
        assert_eq!(f.rows[2].len(), 4);
    }

    #[test]
    fn statistics_over_a_selection_count_total_mean_p90_and_worst() {
        let p = profile();
        let stats = p.stats(0..3);
        let stall = stats
            .iter()
            .find(|s| p.name(s.name) == "stall.rhai:on_update")
            .unwrap();
        assert_eq!(stall.count, 3);
        assert_eq!(stall.total_ns, 33_000_000);
        assert_eq!(stall.mean_ns, 11_000_000);
        assert_eq!(stall.p90_ns, 31_000_000);
        assert_eq!(stall.worst_ns, 31_000_000);
        assert_eq!(p.scope(stall.worst_at.0, stall.worst_at.1).frame, 2);
        // Largest total first
        assert_eq!(p.name(stats[0].name), "tick");
        // One frame only
        let one = p.stats(0..1);
        assert_eq!(
            one.iter().find(|s| p.name(s.name) == "tick").unwrap().count,
            1
        );
        assert!(one.iter().any(|s| p.name(s.name) == "decode_image"));
        let mut by_count = p.stats(0..3);
        p.sort_stats(&mut by_count, StatSort::Name, false);
        assert_eq!(p.name(by_count[0].name), "decode_image");
    }

    #[test]
    fn a_scopes_time_across_frames_and_its_excess_over_the_median() {
        let mut p = profile();
        let stall = p
            .trace
            .names
            .iter()
            .position(|n| n == "stall.rhai:on_update")
            .unwrap() as u32;
        assert_eq!(p.series(stall), vec![1.0, 1.0, 31.0]);
        let b = p.baseline();
        assert_eq!(b.median_frame_ms, 10.0);
        let i = find(&p, 0, "stall.rhai:on_update", 2);
        assert_eq!(p.excess_ns(0, i), Some(30_000_000));
        let normal = find(&p, 0, "stall.rhai:on_update", 0);
        assert_eq!(p.excess_ns(0, normal), Some(0));
        let render = find(&p, 0, "render", 2);
        assert_eq!(p.excess_ns(0, render), Some(0));
    }

    #[test]
    fn a_trace_with_no_frames_track_makes_its_rows_from_the_scopes() {
        let text = r#"[
            {"ph":"X","name":"a","pid":1,"tid":1,"ts":0,"dur":5,"args":{"frame":1,"depth":0}},
            {"ph":"X","name":"b","pid":1,"tid":1,"ts":10,"dur":5,"args":{"frame":2,"depth":0}},
            {"ph":"X","name":"c","pid":1,"tid":1,"ts":11,"dur":2,"args":{"frame":2,"depth":1}}
        ]"#;
        let p = Profile::new(Trace::read(text).unwrap());
        assert_eq!(p.rows.len(), 2);
        assert_eq!((p.rows[1].start_ns, p.rows[1].end_ns), (10_000, 15_000));
        assert_eq!(p.facts[0].parent[2], 1);
        assert_eq!(p.rows_touching(0, 10_500), 0..2);
        assert_eq!(p.row_at(12_000), Some(1));
    }

    /// A summed record (#1288) overlapping a sibling is a leaf: the
    /// sibling's child keeps the sibling, and the statistics count calls.
    #[test]
    fn a_summed_record_is_nobodys_parent_and_counts_as_its_calls() {
        let x = |name: &str, ts: f64, dur: f64, args: &str| {
            format!("{{\"ph\":\"X\",\"name\":\"{name}\",\"pid\":1,\"tid\":1,\"ts\":{ts},\"dur\":{dur},\"args\":{{\"frame\":0,{args}}}}}")
        };
        let events = [
            "{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":1,\"args\":{\"name\":\"main\"}}".to_string(),
            x("get_field()", 5.0, 30.0, "\"depth\":1,\"calls\":180,\"worst_us\":0.5"),
            x("clamp()", 20.0, 2.0, "\"depth\":2,\"calls\":4,\"worst_us\":0.75"),
            x("helper()", 10.0, 50.0, "\"depth\":1"),
            x("on_update", 0.0, 100.0, "\"depth\":0"),
        ];
        let text = format!("{{\"traceEvents\":[{}]}}", events.join(","));
        let p = Profile::new(Trace::read(&text).unwrap());
        let at = |name: &str| find(&p, 0, name, 0);
        let f = &p.facts[0];
        assert_eq!(f.parent[at("get_field()")], at("on_update") as u32);
        assert_eq!(f.parent[at("clamp()")], at("helper()") as u32);
        assert_eq!(f.self_ns[at("on_update")], 20_000);
        let stats = p.stats(0..p.rows.len());
        let gf = stats
            .iter()
            .find(|s| p.name(s.name) == "get_field()")
            .unwrap();
        assert_eq!(gf.count, 180);
        assert_eq!(gf.worst_ns, 500);
        assert_eq!(gf.mean_ns, 30_000 / 180);
        assert!(p.coverage.is_none());
    }
}
