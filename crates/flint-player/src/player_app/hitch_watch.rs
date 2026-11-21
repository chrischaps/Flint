//! What a run keeps of its hitches (#1079): a JSON line for each in the
//! hitch log (`--hitch-log`), the counts `--exit-after`'s report prints
//! (`[perf] hitches: ...`), the gate that fails a run with too many
//! (`--max-hitches`), and the numbers `--perf`'s line and `?perf=1` show.
//!
//! The detector (`hitch.rs`) judges each frame as its clock comes in; this
//! hears what it found, once a frame, right after. A hitch in the first
//! [`STARTUP_SECS`] of game time (loading's last uploads, the first
//! pipeline compiles) is counted apart as `startup` and is held against
//! `--max-hitches-startup` only. A hitch of a scene loaded behind a
//! script's cover (`Hitch::loading`, #1125) is tallied as `loading` alone,
//! said on its own `[perf] loading hitches` line, and held against a limit
//! only with `--max-hitches-of loading`.
//!
//! A log line waits for the [`AROUND`] frames after its hitch, so it shows
//! the frames either side; it is written once they have closed, about
//! 130 ms later at 60 fps, or at the end of the run with what there is.
//! Each line is flushed as it is written, so a run that crashes keeps every
//! line before it.

use super::frame_history::FrameHistory;
use super::hitch::{Cause, Hitch, HitchDetector};
use crate::hitch_args::{HitchClass, HitchGate, STARTUP_SECS};
use serde::Serialize;
use std::io::Write;

/// Frames shown on each side of a hitch in its log line.
pub(crate) const AROUND: usize = 8;
/// Causes the report names before "N more".
const CAUSES_NAMED: usize = 4;

/// The hitches of one part of the run (the start, or after it), all of
/// them and each class's apart, so a gate narrowed to one class names
/// that class's worst and causes (#1130). A scene loaded behind a
/// script's cover (#1125) is `loading` alone: in neither `all` nor its
/// player/elsewhere class, since the player saw none of it.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Tally {
    pub all: Seen,
    pub player: Seen,
    pub elsewhere: Seen,
    pub loading: Seen,
}

impl Tally {
    fn add(&mut self, h: &Hitch) {
        if h.loading {
            self.loading.add(h);
            return;
        }
        self.all.add(h);
        match h.cause {
            Cause::Player { .. } => self.player.add(h),
            Cause::Elsewhere { .. } => self.elsewhere.add(h),
            // Only the tools' own tally holds these (#1129), in `all`
            Cause::Tooling { .. } => {}
        }
    }

    /// The hitches a gate of `class` counts.
    pub(crate) fn of(&self, class: HitchClass) -> &Seen {
        match class {
            HitchClass::All => &self.all,
            HitchClass::Player => &self.player,
            HitchClass::Elsewhere => &self.elsewhere,
            HitchClass::Loading => &self.loading,
        }
    }
}

/// Some hitches: how many, the longest, and what caused them.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Seen {
    pub hitches: u64,
    /// The longest: its wall ms, game time and cause.
    pub worst: Option<(f32, f64, String)>,
    /// How many hitches each cause had ([`Hitch::cause_key`]), first seen
    /// first.
    pub causes: Vec<(String, u64)>,
}

impl Seen {
    fn add(&mut self, h: &Hitch) {
        self.hitches += 1;
        if self.worst.as_ref().is_none_or(|w| h.ms > w.0) {
            self.worst = Some((h.ms, h.game_time, h.cause_line()));
        }
        let key = h.cause_key();
        match self.causes.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => self.causes.push((key, 1)),
        }
    }

    /// `3 script stall.rhai:on_update, 1 model load`: most first.
    fn causes_line(&self) -> String {
        let mut causes: Vec<&(String, u64)> = self.causes.iter().collect();
        causes.sort_by_key(|c| std::cmp::Reverse(c.1));
        let mut line = causes
            .iter()
            .take(CAUSES_NAMED)
            .map(|(k, n)| format!("{n} {k}"))
            .collect::<Vec<_>>()
            .join(", ");
        if causes.len() > CAUSES_NAMED {
            line.push_str(&format!(", {} more", causes.len() - CAUSES_NAMED));
        }
        line
    }

    /// `worst 201.3 ms at t=4.02 s (script ...); causes: ...`
    fn worst_and_causes(&self) -> String {
        match &self.worst {
            Some((ms, t, cause)) => format!(
                "worst {ms:.1} ms at t={t:.2} s ({cause}); causes: {}",
                self.causes_line()
            ),
            None => String::new(),
        }
    }
}

/// One line of the hitch log. Times are ms unless named; fields in the
/// order a reader wants them.
#[derive(Debug, Clone, Serialize)]
struct LogLine {
    /// The frame's number since recording began.
    frame: u64,
    /// The game time it ran at, seconds (what scripts read as
    /// `total_time()`).
    game_time: f64,
    /// When it began, seconds since recording began.
    wall_time: f64,
    ms: f64,
    /// The time it was judged by: paced under vsync, else `ms`.
    judged_ms: f64,
    /// The median of the frames before it.
    median_ms: f64,
    /// Under vsync, the refreshes it ran past its one; `null` without.
    missed_refreshes: Option<u32>,
    /// `player`, `elsewhere`, `tooling` (the player's own screenshot or
    /// F2 overlay, #1129) or `loading` (a scene loaded behind a script's
    /// cover, #1125; `phase` and `script` still say whose work it was).
    class: &'static str,
    /// In the first [`STARTUP_SECS`] of game time.
    startup: bool,
    /// The player's work in it (every phase but acquire and present).
    player_ms: f64,
    /// The player's phase that grew most, and by how much; `null` for
    /// `elsewhere`.
    phase: Option<&'static str>,
    phase_ms: Option<f64>,
    /// The script callback that took longest in it, when scripts were timed.
    script: Option<String>,
    script_ms: Option<f64>,
    /// For `elsewhere`: the swapchain's two waits, for an image (#1123)
    /// and handing it back.
    acquire_ms: Option<f64>,
    present_ms: Option<f64>,
    /// The GPU's busy time on its own work (#1104), beside the waits: a
    /// long acquire with a long GPU frame waited on the GPU, one without on
    /// the compositor. `null` while the GPU isn't timed.
    gpu_ms: Option<f64>,
    /// [`Hitch::cause_line`].
    cause: String,
    /// What the engine did in it and the frame before (#1077).
    notes: Vec<NoteLine>,
    /// The frames before it and after it, oldest first.
    before_ms: Vec<f64>,
    after_ms: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct NoteLine {
    kind: &'static str,
    detail: String,
    ms: Option<f64>,
    depth: u8,
    frame_before: bool,
}

/// To two places: what a log keeps.
fn r2(x: f32) -> f64 {
    (x as f64 * 100.0).round() / 100.0
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

impl LogLine {
    fn of(h: &Hitch, startup: bool, history: &FrameHistory) -> Self {
        let (phase, phase_ms, script, script_ms, waits) = match &h.cause {
            Cause::Player {
                phase,
                phase_ms,
                script,
            } => (
                Some(phase.label()),
                Some(r2(*phase_ms)),
                script.as_ref().map(|(n, _)| n.clone()),
                script.as_ref().map(|(_, ms)| r2(*ms)),
                None,
            ),
            Cause::Elsewhere {
                acquire_ms,
                present_ms,
            } => (
                None,
                None,
                None,
                None,
                Some((r2(*acquire_ms), r2(*present_ms))),
            ),
            Cause::Tooling { .. } => (None, None, None, None, None),
        };
        let frames = history.frames();
        let mut before: Vec<f64> = frames
            .iter()
            .rev()
            .filter(|f| f.index < h.index)
            .take(AROUND)
            .map(|f| r2(f.ms))
            .collect();
        before.reverse();
        let after = frames
            .iter()
            .filter(|f| f.index > h.index)
            .take(AROUND)
            .map(|f| r2(f.ms))
            .collect();
        Self {
            frame: h.index,
            game_time: r3(h.game_time),
            wall_time: r3(h.at),
            ms: r2(h.ms),
            judged_ms: r2(h.judged_ms),
            median_ms: r2(h.median_ms),
            missed_refreshes: h.refreshes.map(|r| r.saturating_sub(1)),
            class: h.class(),
            startup,
            player_ms: r2(h.player_ms),
            phase,
            phase_ms,
            script,
            script_ms,
            acquire_ms: waits.map(|w| w.0),
            present_ms: waits.map(|w| w.1),
            gpu_ms: h.gpu_ms.map(r2),
            cause: h.cause_line(),
            notes: h
                .notes
                .iter()
                .map(|n| NoteLine {
                    kind: n.note.kind.label(),
                    detail: n.note.detail.clone(),
                    ms: n.note.ms.map(r2),
                    depth: n.note.depth,
                    frame_before: n.frame_before,
                })
                .collect(),
            before_ms: before,
            after_ms: after,
        }
    }
}

/// The hitch log: where it goes and what it has written.
struct Log {
    /// What it is called in the report.
    name: String,
    out: Box<dyn Write>,
    written: u64,
    /// The first write that failed: the log stops there, said once.
    failed: Option<String>,
}

impl Log {
    fn write(&mut self, line: &LogLine) {
        if self.failed.is_some() {
            return;
        }
        let result = serde_json::to_string(line)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                writeln!(self.out, "{json}")?;
                self.out.flush()
            });
        match result {
            Ok(()) => self.written += 1,
            Err(e) => {
                tracing::warn!("--hitch-log {}: {e}; no more lines", self.name);
                self.failed = Some(e.to_string());
            }
        }
    }
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct HitchWatch {
    /// `--max-hitches` and its kin; `None`, no gate.
    pub gate: Option<HitchGate>,
    /// After the start, and in it.
    pub run: Tally,
    pub startup: Tally,
    /// The player's own tools' hitches (`Cause::Tooling`, #1129), start or
    /// not: in neither tally above, so no gate counts them.
    pub tooling: Seen,
    log: Option<Log>,
    /// Lines waiting for the frames after their hitch.
    pending: Vec<LogLine>,
    /// The newest frame already looked at.
    seen: Option<u64>,
    /// The wall time of the frames after the start, seconds: what the
    /// report's hitches a minute are over.
    after_start_secs: f64,
}

impl HitchWatch {
    /// Log to `out`, called `name` in the report.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the web opens no file
    pub(crate) fn log_to(&mut self, name: String, out: Box<dyn Write>) {
        self.log = Some(Log {
            name,
            out,
            written: 0,
            failed: None,
        });
    }

    /// The detector has judged this frame's newest record and found
    /// `found` hitches in it (the newest `found` it keeps).
    pub(crate) fn observe(
        &mut self,
        found: usize,
        detector: &HitchDetector,
        history: &FrameHistory,
    ) {
        self.add_closed_frames(history);
        if found > 0 {
            let hitches = detector.hitches();
            let new = hitches.iter().skip(hitches.len().saturating_sub(found));
            for h in new {
                let startup = h.game_time < STARTUP_SECS;
                if h.cause.is_tooling() {
                    self.tooling.add(h);
                } else if startup {
                    self.startup.add(h);
                } else {
                    self.run.add(h);
                }
                if self.log.is_some() {
                    self.pending.push(LogLine::of(h, startup, history));
                }
            }
        }
        if self.log.is_some() {
            self.write_due(false);
        }
    }

    /// The frames closed since the last look (one, in the player): their
    /// time after the start, and onto the lines waiting for them.
    fn add_closed_frames(&mut self, history: &FrameHistory) {
        let frames = history.frames();
        let Some(newest) = frames.back() else {
            return;
        };
        let seen = self.seen.replace(newest.index);
        let new = frames
            .iter()
            .rev()
            .take_while(|f| seen.is_none_or(|s| f.index > s))
            .count();
        for f in frames.iter().skip(frames.len() - new) {
            if f.game_time >= STARTUP_SECS {
                self.after_start_secs += f.ms as f64 / 1000.0;
            }
            for line in &mut self.pending {
                if f.index > line.frame && line.after_ms.len() < AROUND {
                    line.after_ms.push(r2(f.ms));
                }
            }
        }
        // The hitch's own GPU time, back a frame or two after it (#1104):
        // the line waits AROUND frames, so it is on the record by then
        for line in &mut self.pending {
            if line.gpu_ms.is_none() {
                line.gpu_ms = history.frame(line.frame).and_then(|f| f.gpu_ms).map(r2);
            }
        }
    }

    /// Write the lines whose frames after have all closed; with `all`,
    /// every line, with what it has.
    fn write_due(&mut self, all: bool) {
        let Some(log) = &mut self.log else {
            return;
        };
        let mut i = 0;
        while i < self.pending.len() {
            if all || self.pending[i].after_ms.len() >= AROUND {
                let line = self.pending.remove(i);
                log.write(&line);
            } else {
                i += 1;
            }
        }
    }

    /// Write every waiting line now (the run is ending).
    pub(crate) fn finish(&mut self) {
        self.write_due(true);
    }

    /// Why the run fails for its hitches, if it does.
    pub(crate) fn failure(&self) -> Option<String> {
        let gate = self.gate?;
        let of = gate.of;
        if let Some(max) = gate.max {
            let seen = self.run.of(of);
            let n = seen.hitches;
            if n > max {
                return Some(format!(
                    "{n} {}{} after the first {STARTUP_SECS} s, over --max-hitches {max}: {}",
                    of.words(),
                    plural(n, "hitch", "hitches"),
                    seen.worst_and_causes()
                ));
            }
        }
        if let Some(max) = gate.max_startup {
            let seen = self.startup.of(of);
            let n = seen.hitches;
            if n > max {
                return Some(format!(
                    "{n} {}{} in the first {STARTUP_SECS} s, over --max-hitches-startup {max}: {}",
                    of.words(),
                    plural(n, "hitch", "hitches"),
                    seen.worst_and_causes()
                ));
            }
        }
        None
    }

    /// The wall seconds of the frames after the start: what the report's
    /// hitches a minute are over.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the report's, desktop only
    pub(crate) fn after_start_secs(&self) -> f64 {
        self.after_start_secs
    }

    /// `--exit-after`'s lines (`[perf] hitches: ...`).
    pub(crate) fn report(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let r = &self.run.all;
        let secs = self.after_start_secs;
        if r.hitches == 0 {
            lines.push(format!(
                "[perf] hitches: 0 in {secs:.1} s after t={STARTUP_SECS:.0} s"
            ));
        } else {
            let per_min = if secs > 0.0 {
                r.hitches as f64 * 60.0 / secs
            } else {
                0.0
            };
            lines.push(format!(
                "[perf] hitches: {} ({per_min:.1}/min over {secs:.1} s after t={STARTUP_SECS:.0} s; {} player, {} elsewhere), {}",
                r.hitches,
                self.run.player.hitches,
                self.run.elsewhere.hitches,
                r.worst_and_causes()
            ));
        }
        let s = &self.startup.all;
        let gated = match self.gate.and_then(|g| g.max_startup) {
            Some(max) => format!("--max-hitches-startup {max}"),
            None => "not gated".to_string(),
        };
        if s.hitches == 0 {
            lines.push(format!(
                "[perf] startup hitches (before t={STARTUP_SECS:.0} s, {gated}): 0"
            ));
        } else {
            lines.push(format!(
                "[perf] startup hitches (before t={STARTUP_SECS:.0} s, {gated}): {}, {}",
                s.hitches,
                s.worst_and_causes()
            ));
        }
        // Said only when there were any: most runs load no scene behind a
        // fade (#1125). The start's are the start's, said beside them
        let (l, ls) = (&self.run.loading, &self.startup.loading);
        if l.hitches + ls.hitches > 0 {
            let gated = match self.gate {
                Some(g) if g.of == HitchClass::Loading => "--max-hitches-of loading",
                _ => "not gated",
            };
            let start = if ls.hitches > 0 {
                format!(" (and {} in the start)", ls.hitches)
            } else {
                String::new()
            };
            let worst = if l.hitches > 0 {
                format!(", {}", l.worst_and_causes())
            } else {
                String::new()
            };
            lines.push(format!(
                "[perf] loading hitches (scene loads behind a script's cover, {gated}): {}{start}{worst}",
                l.hitches
            ));
        }
        // Said only when there were any: most runs take no screenshot
        let t = &self.tooling;
        if t.hitches > 0 {
            lines.push(format!(
                "[perf] tooling hitches (screenshots, the F2 overlay, the trace ring; never gated): {}, {}",
                t.hitches,
                t.worst_and_causes()
            ));
        }
        if let Some(log) = &self.log {
            let failed = log
                .failed
                .as_ref()
                .map(|e| format!(", stopped: {e}"))
                .unwrap_or_default();
            lines.push(format!(
                "[perf] hitch log: {} ({} {}{failed})",
                log.name,
                log.written,
                plural(log.written, "line", "lines")
            ));
        }
        lines
    }
}

fn plural(n: u64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

impl Drop for HitchWatch {
    /// A run that ends without its report still writes its last lines.
    fn drop(&mut self) {
        self.finish();
    }
}

/// The readout's hitch numbers (`--perf`'s line, `?perf=1`): hitches in
/// its last `secs`, in the whole run, and the worst of the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct HitchReadout {
    pub secs: f64,
    pub window: u64,
    pub run: u64,
    /// The window's longest: wall ms and its cause.
    pub worst: Option<(f32, String)>,
}

impl HitchReadout {
    pub(crate) fn of(detector: &HitchDetector, history: &FrameHistory, secs: f64) -> Self {
        let horizon = history.frames().back().map_or(0.0, |f| f.end() - secs);
        let mut window = 0;
        let mut worst: Option<&Hitch> = None;
        // The game's, not its tools' (#1129) nor a covered load's (#1125):
        // the run's count leaves them out too
        let game = detector.hitches().iter().filter(|h| h.is_game());
        for h in game.filter(|h| h.at >= horizon) {
            window += 1;
            if worst.is_none_or(|w| h.ms > w.ms) {
                worst = Some(h);
            }
        }
        Self {
            secs,
            window,
            run: detector.session().hitches,
            worst: worst.map(|h| (h.ms, h.cause_line())),
        }
    }

    /// The copied line's part: `hitches 1 in 10s (3 in run), worst 201.3
    /// ms: script stall.rhai:on_update 200.0 ms`
    pub(crate) fn part(&self) -> String {
        let mut part = format!(
            "hitches {} in {:.0}s ({} in run)",
            self.window, self.secs, self.run
        );
        if let Some((ms, cause)) = &self.worst {
            part.push_str(&format!(", worst {ms:.1} ms: {cause}"));
        }
        part
    }

    /// The readout's rows.
    pub(crate) fn rows(&self) -> String {
        let mut rows = format!(
            "hitch  {} in {:.0} s, {} in the run",
            self.window, self.secs, self.run
        );
        if let Some((ms, cause)) = &self.worst {
            rows.push_str(&format!("\n       worst {ms:.1} ms: {cause}"));
        }
        rows
    }
}

impl super::PlayerApp {
    /// Record every frame and time every script callback, whatever the
    /// build's `frame-history` and `--perf` say: a hitch log or a gate
    /// needs both.
    fn watch_hitches(&mut self) {
        self.stats.perf.always_on = true;
        self.core.script.set_timing(true);
    }

    /// `--hitch-log FILE`: a JSON line for every hitch, from the first
    /// frame (#1079). The file is created (or emptied) now.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn log_hitches_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            #[allow(clippy::disallowed_methods)] // a write: the log is the run's output
            std::fs::create_dir_all(parent)?;
        }
        #[allow(clippy::disallowed_methods)] // a write: the log is the run's output
        let file = std::fs::File::create(path)?;
        self.stats
            .watch
            .log_to(path.display().to_string(), Box::new(file));
        self.watch_hitches();
        Ok(())
    }

    /// `--max-hitches` and its kin: `run_failure` fails a run past them.
    pub fn gate_hitches(&mut self, gate: HitchGate) {
        self.stats.watch.gate = Some(gate);
        self.watch_hitches();
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame_history::ClockSample;
    use super::super::perf::Phase;
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use web_time::Instant;

    const HZ60: f64 = 1000.0 / 60.0;

    /// A log into memory, readable while the watch holds it.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Shared {
        fn lines(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    /// Frames played into a history, a detector and the watch, as the
    /// player's tick does: the frame, then the next frame's start and
    /// tick close and measure it, then the looks.
    struct Run {
        history: FrameHistory,
        detector: HitchDetector,
        watch: HitchWatch,
        t: Instant,
        game: f64,
    }

    impl Run {
        fn new() -> Self {
            let mut history = FrameHistory::default();
            let t = history.start;
            history.begin_frame_at(t);
            Self {
                history,
                detector: HitchDetector::default(),
                watch: HitchWatch::default(),
                t,
                game: 0.0,
            }
        }

        /// A frame of `wall` ms, `script_ms` of it in `stall.rhai`.
        fn frame(&mut self, wall: f64, script_ms: f64) {
            let h = &mut self.history;
            h.mark_at(Phase::Input, self.t + Duration::from_secs_f64(0.0003));
            h.mark_at(
                Phase::Scripts,
                self.t + Duration::from_secs_f64((0.3 + script_ms) / 1000.0),
            );
            h.record_scripts(&[("scripts/stall.rhai:on_update".into(), script_ms as f32)]);
            self.t += Duration::from_secs_f64(wall / 1000.0);
            h.begin_frame_at(self.t);
            h.clock_ticked(ClockSample {
                raw_ms: wall as f32,
                dt_ms: HZ60 as f32,
                paced_ms: None,
                vsync_ms: None,
            });
            self.game += HZ60 / 1000.0;
            h.game_time(self.game);
            let found = self.detector.observe(&self.history);
            self.watch.observe(found, &self.detector, &self.history);
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(HZ60, 0.5);
            }
        }
    }

    #[test]
    fn a_line_waits_for_the_frames_after_it_and_names_the_script() {
        let log = Shared::default();
        let mut run = Run::new();
        run.watch.log_to("mem".into(), Box::new(log.clone()));
        run.frames(150); // past the start: 2.5 s
        run.frame(120.0, 100.0);
        run.frames(AROUND - 1);
        assert!(log.lines().is_empty(), "waits for its eighth frame after");
        run.frame(HZ60, 0.5);
        let lines = log.lines();
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l["class"], "player");
        assert_eq!(l["phase"], "scripts");
        assert_eq!(l["script"], "stall.rhai:on_update");
        assert_eq!(l["startup"], false);
        assert_eq!(l["ms"], 120.0);
        assert!((l["game_time"].as_f64().unwrap() - 150.0 / 60.0).abs() < 0.01);
        assert_eq!(l["before_ms"].as_array().unwrap().len(), AROUND);
        assert_eq!(l["after_ms"].as_array().unwrap().len(), AROUND);
        assert_eq!(l["missed_refreshes"], serde_json::Value::Null);
        assert!(l["cause"]
            .as_str()
            .unwrap()
            .starts_with("script stall.rhai:on_update 100.0 ms"));
        // The keys in the order the struct gives them
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(text.starts_with("{\"frame\":150,\"game_time\":"), "{text}");
    }

    /// #1123: a frame that waits on the swapchain for its image is the
    /// compositor's, not the renderer's. Frames go through the player's own
    /// calls on the wall clock, the acquire through `PerfRecorder::acquire`
    /// on a surface that stalls once (#1080's machine waited 515 ms in
    /// `get_current_texture`; here 120).
    #[test]
    fn a_frame_whose_acquire_stalls_is_elsewhere_with_its_wait_on_the_line() {
        use super::super::perf::PerfRecorder;
        use flint_render::{AcquireFrame, SurfaceError};
        use std::cell::Cell;

        /// A swapchain with no images: each acquire waits as told, then
        /// times out (a real surface can't be faked into giving one).
        struct Stalls(Cell<Duration>);
        impl AcquireFrame for Stalls {
            fn acquire(&self) -> Result<wgpu::SurfaceTexture, SurfaceError> {
                std::thread::sleep(self.0.take());
                Err(SurfaceError::Timeout)
            }
        }

        const STALL_MS: f64 = 120.0;
        let log = Shared::default();
        let mut perf = PerfRecorder::recording_always(true);
        // A floor well over a busy test machine's own hiccups
        let mut detector = HitchDetector::new(super::super::hitch::HitchThresholds {
            factor: 2.0,
            floor_ms: 40.0,
        });
        let mut watch = HitchWatch::default();
        watch.log_to("mem".into(), Box::new(log.clone()));
        let surface = Stalls(Cell::new(Duration::ZERO));
        let mut game = 3.0;
        // Each frame's GPU time comes back two frames after it (#1104)
        let mut in_flight = std::collections::VecDeque::new();
        let mut frame = |stall: f64, gpu: f32| {
            // `tick`: the frame begins, and the clock measures the last
            perf.begin_frame();
            let last_ms = perf.frames().back().map_or(0.0, |f| f.ms);
            perf.clock_ticked(ClockSample {
                raw_ms: last_ms,
                dt_ms: last_ms,
                paced_ms: None,
                vsync_ms: None,
            });
            game += HZ60 / 1000.0;
            perf.game_time(game);
            let found = detector.observe(&perf);
            watch.observe(found, &detector, &perf);
            for phase in [Phase::Input, Phase::Scripts, Phase::Sync] {
                perf.mark(phase);
            }
            // `render`
            surface.0.set(Duration::from_secs_f64(stall / 1000.0));
            assert_eq!(perf.acquire(&surface).err(), Some(SurfaceError::Timeout));
            perf.mark(Phase::Render);
            in_flight.push_back((perf.gpu_frame_index(), gpu));
            if in_flight.len() > 2 {
                let (index, ms) = in_flight.pop_front().unwrap();
                perf.gpu_time(index, ms);
            }
            perf.mark(Phase::Hud);
            perf.presented_at(Instant::now());
            perf.mark(Phase::Present);
        };
        for _ in 0..60 {
            frame(0.0, 1.5);
        }
        // The stalled frame's own GPU work was long: its time, not the
        // 1.5 ms of the frame its read-back arrives in, is the line's
        frame(STALL_MS, 9.0);
        // Its line waits for the frames after it; the next frame's start
        // closes the stalled one
        for _ in 0..=AROUND + 1 {
            frame(0.0, 1.5);
        }
        let lines = log.lines();
        let l = lines
            .iter()
            .find(|l| l["acquire_ms"].as_f64().is_some_and(|ms| ms >= STALL_MS))
            .unwrap_or_else(|| panic!("no line for the stalled acquire: {lines:?}"));
        assert_eq!(l["class"], "elsewhere", "{l}");
        assert_eq!(l["phase"], serde_json::Value::Null);
        assert!(l["player_ms"].as_f64().unwrap() < 40.0, "{l}");
        assert!(l["present_ms"].as_f64().is_some(), "{l}");
        assert_eq!(l["gpu_ms"], 9.0, "the hitch's own GPU time: {l}");
        let hitch = detector.last().unwrap();
        assert_eq!(hitch.index, l["frame"].as_u64().unwrap());
        assert_eq!(hitch.gpu_ms, Some(9.0), "filled in once it came back");
        let record = perf.frame(hitch.index).unwrap();
        assert_eq!(record.gpu_ms, Some(9.0));
        assert_eq!(perf.frame(hitch.index + 1).unwrap().gpu_ms, Some(1.5));
        assert!(
            l["cause"]
                .as_str()
                .unwrap()
                .starts_with("elsewhere, acquire 1"),
            "{l}"
        );
        // Beside each other on the line, in that order
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        let at = |key: &str| text.find(key).unwrap();
        assert!(at("\"acquire_ms\"") < at("\"present_ms\""));
        assert!(at("\"present_ms\"") < at("\"gpu_ms\""));
        assert_eq!(watch.run.elsewhere.hitches, 1, "{lines:?}");
        assert_eq!(watch.run.player.hitches, 0, "{lines:?}");
    }

    #[test]
    fn the_start_is_counted_apart_and_gated_only_when_asked() {
        let mut run = Run::new();
        run.frames(60);
        run.frame(150.0, 1.0); // t = 1 s: the start, and not the player's
        run.frames(120);
        run.frame(120.0, 100.0);
        run.frames(30);
        assert_eq!(run.watch.startup.all.hitches, 1);
        assert_eq!(run.watch.startup.elsewhere.hitches, 1);
        assert_eq!(run.watch.run.all.hitches, 1);
        assert_eq!(run.watch.run.player.hitches, 1);
        assert_eq!(run.watch.failure(), None, "no gate");

        run.watch.gate = Some(HitchGate {
            max: Some(1),
            ..Default::default()
        });
        assert_eq!(run.watch.failure(), None, "one is not over one");
        run.watch.gate = Some(HitchGate {
            max: Some(0),
            ..Default::default()
        });
        let why = run.watch.failure().unwrap();
        assert!(
            why.starts_with("1 hitch after the first 2 s, over --max-hitches 0"),
            "{why}"
        );
        assert!(why.contains("script stall.rhai:on_update"), "{why}");
        // Only the elsewhere ones: the player's stall passes
        run.watch.gate = Some(HitchGate {
            max: Some(0),
            max_startup: None,
            of: HitchClass::Elsewhere,
        });
        assert_eq!(run.watch.failure(), None);
        // The start's one fails a startup gate of 0
        run.watch.gate = Some(HitchGate {
            max: None,
            max_startup: Some(0),
            of: HitchClass::All,
        });
        let why = run.watch.failure().unwrap();
        assert!(why.contains("--max-hitches-startup 0"), "{why}");
        assert!(why.contains("elsewhere"), "{why}");

        let report = run.watch.report();
        // 92 frames after t = 2 s: 91 of 16.7 ms and the 120 ms one
        assert!(
            report[0].starts_with("[perf] hitches: 1 (37.0/min over 1.6 s after t=2 s; 1 player, 0 elsewhere), worst 120.0 ms at t="),
            "{report:?}"
        );
        assert!(
            report[0].ends_with("; causes: 1 script stall.rhai:on_update"),
            "{report:?}"
        );
        assert!(
            report[1].starts_with(
                "[perf] startup hitches (before t=2 s, --max-hitches-startup 0): 1, worst 150.0 ms"
            ),
            "{report:?}"
        );
    }

    /// #1130: a gate narrowed to one class names that class's worst and
    /// causes, not a longer hitch of the class it was told to leave out.
    #[test]
    fn a_gate_of_one_class_names_that_class_s_worst_and_causes() {
        let mut run = Run::new();
        run.frames(150); // past the start
        run.frame(60.0, 40.0); // the player's: the script
        run.frames(60);
        run.frame(150.0, 0.5); // longer, and not the player's
        run.frames(30);
        let r = &run.watch.run;
        assert_eq!(
            (r.all.hitches, r.player.hitches, r.elsewhere.hitches),
            (2, 1, 1)
        );

        let gate = |of| HitchGate {
            max: Some(0),
            max_startup: None,
            of,
        };
        run.watch.gate = Some(gate(HitchClass::Player));
        let why = run.watch.failure().unwrap();
        assert!(
            why.starts_with("1 player hitch after the first 2 s"),
            "{why}"
        );
        assert!(why.contains(": worst 60.0 ms at t="), "{why}");
        assert!(
            why.ends_with("; causes: 1 script stall.rhai:on_update"),
            "{why}"
        );

        run.watch.gate = Some(gate(HitchClass::Elsewhere));
        let why = run.watch.failure().unwrap();
        assert!(
            why.starts_with("1 elsewhere hitch after the first 2 s"),
            "{why}"
        );
        assert!(why.contains(": worst 150.0 ms at t="), "{why}");
        assert!(why.ends_with("; causes: 1 elsewhere"), "{why}");

        // Every class: the worst of all, and both causes
        run.watch.gate = Some(gate(HitchClass::All));
        let why = run.watch.failure().unwrap();
        assert!(why.starts_with("2 hitches after the first 2 s"), "{why}");
        assert!(why.contains(": worst 150.0 ms at t="), "{why}");
        assert!(why.contains("1 script stall.rhai:on_update"), "{why}");
        assert!(why.contains("1 elsewhere"), "{why}");
    }

    #[test]
    fn a_screenshot_is_logged_and_reported_as_tooling_and_never_gated() {
        use flint_core::frame_notes::{self, NoteKind};
        frame_notes::listen(true);
        let log = Shared::default();
        let mut run = Run::new();
        run.watch.log_to("mem".into(), Box::new(log.clone()));
        run.watch.gate = Some(HitchGate {
            max: Some(0),
            max_startup: Some(0),
            of: HitchClass::All,
        });
        run.frames(150);
        // The read-back and the PNG write, in no phase of the game's
        frame_notes::note(NoteKind::Screenshot, || "shot_0003.png".into(), Some(92.5));
        run.frame(110.0, 0.5);
        run.frames(AROUND + 2);
        frame_notes::listen(false);

        let lines = log.lines();
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l["class"], "tooling");
        assert_eq!(l["cause"], "screenshot shot_0003.png, 92.5 ms");
        assert_eq!(l["present_ms"], serde_json::Value::Null);
        assert_eq!(l["notes"][0]["kind"], "screenshot");

        assert_eq!(
            (run.watch.run.all.hitches, run.watch.startup.all.hitches),
            (0, 0)
        );
        assert_eq!(run.watch.tooling.hitches, 1);
        assert_eq!(
            run.watch.failure(),
            None,
            "its own screenshots never fail a run"
        );
        let report = run.watch.report();
        assert!(report[0].starts_with("[perf] hitches: 0 in "), "{report:?}");
        assert!(
            report[2].starts_with(
                "[perf] tooling hitches (screenshots, the F2 overlay, the trace ring; never gated): 1, worst 110.0 ms at t="
            ),
            "{report:?}"
        );
        assert!(report[2].ends_with("; causes: 1 screenshot"), "{report:?}");
        // The readout counts the game's alone
        let readout = HitchReadout::of(&run.detector, &run.history, 10.0);
        assert_eq!((readout.window, readout.run), (0, 0));
        assert_eq!(readout.worst, None);
    }

    #[test]
    fn a_clean_run_reports_none_and_passes_its_gate() {
        let mut run = Run::new();
        run.watch.gate = Some(HitchGate {
            max: Some(0),
            max_startup: Some(0),
            of: HitchClass::All,
        });
        run.frames(300);
        assert_eq!(run.watch.failure(), None);
        let report = run.watch.report();
        assert_eq!(report[0], "[perf] hitches: 0 in 3.0 s after t=2 s");
        assert_eq!(
            report[1],
            "[perf] startup hitches (before t=2 s, --max-hitches-startup 0): 0"
        );
    }

    #[test]
    fn the_end_of_the_run_writes_what_waits() {
        let log = Shared::default();
        let mut run = Run::new();
        run.watch.log_to("mem".into(), Box::new(log.clone()));
        run.frames(150);
        run.frame(120.0, 100.0);
        run.frames(3);
        drop(run);
        let lines = log.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["after_ms"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn the_readout_counts_its_window_and_the_run() {
        let mut run = Run::new();
        run.frames(150);
        run.frame(120.0, 100.0);
        run.frames(700); // past the ten seconds
        run.frame(110.0, 90.0);
        run.frames(10);
        let r = HitchReadout::of(&run.detector, &run.history, 10.0);
        assert_eq!((r.window, r.run), (1, 2));
        assert_eq!(
            r.part(),
            "hitches 1 in 10s (2 in run), worst 110.0 ms: script stall.rhai:on_update 90.0 ms"
        );
        assert!(r
            .rows()
            .starts_with("hitch  1 in 10 s, 2 in the run\n       worst 110.0 ms"));
    }
}
