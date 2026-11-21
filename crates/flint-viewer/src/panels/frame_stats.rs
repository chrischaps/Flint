//! The F2 overlay's frame history (#1110): the last few seconds of frames
//! the host timed, judged for hitches, and drawn as the player's
//! frame-time graph (`flint_debug_ui::widgets::FrameGraph`, #1078) above
//! the rendering stats.
//!
//! The host times a frame only while the viewer asks
//! ([`crate::host::Editor::times_frames`], F2 open), so with F2 closed none
//! of this runs: no clock is read for it and nothing is kept. Opening F2
//! starts the history afresh.
//!
//! The viewer times fewer phases than the player, so its bars use four of
//! the graph's six bands: `update` (the viewer's own frame: reload poll,
//! camera, particles, boil, the renderer's inputs), `render`, `egui` (the
//! panels built, tessellated, painted and submitted) and `elsewhere` (the
//! waits for a swapchain image and for present, and what no phase holds).
//! A hitch is the player's rule without its detector: over twice the median
//! of the frames before it and the median plus 24 ms, judged on how long
//! the frame stood on screen (vsync pacing, `GameClock::advance_wall`).

use std::collections::VecDeque;
use std::fmt::Write as _;

use flint_debug_ui::widgets::{
    band_legend_of, hitch_counters, Band, FrameGraph, GraphFrame, GraphLayout, HitchCounters, BANDS,
};
use flint_runtime::GameClock;

use crate::host::FrameTimes;

/// The seconds the graph spans.
pub const GRAPH_SECS: f32 = 4.0;
/// What the history keeps: the graph's span and a little more.
const KEEP_SECS: f64 = GRAPH_SECS as f64 + 0.5;
/// The graph's size in the overlay, points: as wide as the overlay was
/// before it (#1110), so it stays over the inspector and clear of the view
/// cube. A column is then 22 ms, about a frame at 60 Hz.
pub const GRAPH_SIZE: egui::Vec2 = egui::vec2(180.0, 56.0);
/// A hitch is judged against the median of this many frames before it.
const MEDIAN_OF: usize = 120;
/// The first frames after F2 opens aren't judged (its own first frame
/// lays out the overlay's text, and the median needs frames).
const UNJUDGED: usize = 30;
/// Over `HITCH_FACTOR` x the median and the median + `HITCH_FLOOR_MS`:
/// the player's defaults (#1076, #1109).
const HITCH_FACTOR: f32 = 2.0;
const HITCH_FLOOR_MS: f32 = 24.0;
/// The budget line when the monitor doesn't say its refresh.
const DEFAULT_REFRESH_MS: f32 = 1000.0 / 60.0;
/// How often the FPS row changes, seconds.
const FPS_EVERY: f64 = 0.25;

/// The bands the viewer fills, bottom up, in its own words.
pub const BANDS_SHOWN: [(Band, &str); 4] = [
    (Band::Other, "update"),
    (Band::Render, "render"),
    (Band::HudPresent, "egui"),
    (Band::Elsewhere, "elsewhere"),
];

/// The viewer's own phases, as the cause line names them.
const OWN_PHASES: [&str; 3] = ["update", "render", "egui"];

/// The F2 overlay's history and graph; see the module docs.
pub struct FrameStats {
    /// The frames, oldest first: the last [`KEEP_SECS`]. It grows to hold
    /// them once, then keeps its room.
    frames: VecDeque<GraphFrame>,
    graph: FrameGraph,
    /// Paces each frame's wall time to the refresh, as the player's clock
    /// does: what a hitch is judged by
    clock: GameClock,
    refresh_ms: Option<f32>,
    /// Frames since F2 opened
    seen: usize,
    /// The last frame's own phases (update, render, egui), for what grew
    last_phases: [f32; 3],
    /// Room for the median, kept
    scratch: Vec<f32>,
    hitches: u64,
    worst_ms: f32,
    /// When the last hitch ended, seconds on the frames' epoch
    last_hitch_end: Option<f64>,
    /// The last hitch in one line, written once a hitch
    last: String,
    /// The FPS row's value and frame time, and when they were taken
    fps: (f32, f32),
    fps_at: f64,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            frames: VecDeque::new(),
            graph: FrameGraph::new(),
            clock: GameClock::new(),
            refresh_ms: None,
            seen: 0,
            last_phases: [0.0; 3],
            scratch: Vec::with_capacity(MEDIAN_OF),
            hitches: 0,
            worst_ms: 0.0,
            last_hitch_end: None,
            last: String::new(),
            fps: (0.0, 0.0),
            fps_at: f64::NEG_INFINITY,
        }
    }
}

impl FrameStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start afresh: F2 opened. The room the history grew is kept.
    pub fn reset(&mut self) {
        let mut frames = std::mem::take(&mut self.frames);
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut last = std::mem::take(&mut self.last);
        frames.clear();
        scratch.clear();
        last.clear();
        *self = Self {
            frames,
            scratch,
            last,
            graph: std::mem::take(&mut self.graph),
            ..Self::default()
        };
    }

    /// The frames held, oldest first.
    pub fn frames(&self) -> impl Iterator<Item = &GraphFrame> {
        self.frames.iter()
    }

    /// Hitches since F2 opened.
    pub fn hitches(&self) -> u64 {
        self.hitches
    }

    /// The last hitch in one line, if there was one.
    pub fn last_hitch(&self) -> Option<&str> {
        self.last_hitch_end.map(|_| self.last.as_str())
    }

    /// The display's refresh period the graph's budget line stands for.
    pub fn budget_ms(&self) -> f32 {
        self.refresh_ms.unwrap_or(DEFAULT_REFRESH_MS)
    }

    /// One frame the host timed.
    pub fn record(&mut self, t: &FrameTimes) {
        if t.refresh_ms != self.refresh_ms {
            self.refresh_ms = t.refresh_ms;
            self.clock
                .set_vsync_period(t.refresh_ms.map(|ms| ms as f64 / 1000.0));
        }
        self.clock.advance_wall(t.wall_ms as f64 / 1000.0);
        let ms = self
            .clock
            .paced_delta_time()
            .map_or(t.wall_ms, |s| (s * 1000.0) as f32);

        let mut bands = [0.0; BANDS];
        bands[Band::Other as usize] = t.update_ms;
        bands[Band::Render as usize] = t.draw_ms;
        bands[Band::HudPresent as usize] = t.ui_ms;
        bands[Band::Elsewhere as usize] = t.acquire_ms + t.present_ms + t.unaccounted_ms();

        let phases = [t.update_ms, t.draw_ms, t.ui_ms];
        let mut hitch = false;
        if self.seen >= UNJUDGED {
            let median = self.median_ms();
            hitch = ms > median * HITCH_FACTOR && ms > median + HITCH_FLOOR_MS;
            if hitch {
                self.hitches += 1;
                self.last_hitch_end = Some(t.at + t.wall_ms as f64 / 1000.0);
                self.write_cause(ms, median, &phases, t);
            }
            self.worst_ms = self.worst_ms.max(ms);
        }
        self.last_phases = phases;
        self.seen += 1;

        self.frames.push_back(GraphFrame {
            at: t.at,
            wall_ms: t.wall_ms,
            ms,
            bands,
            hitch,
        });
        let horizon = t.at + t.wall_ms as f64 / 1000.0 - KEEP_SECS;
        while self.frames.front().is_some_and(|f| f.end() < horizon) {
            self.frames.pop_front();
        }
    }

    /// The median judged time of the last [`MEDIAN_OF`] frames.
    fn median_ms(&mut self) -> f32 {
        self.scratch.clear();
        let skip = self.frames.len().saturating_sub(MEDIAN_OF);
        self.scratch
            .extend(self.frames.iter().skip(skip).map(|f| f.ms));
        if self.scratch.is_empty() {
            return 0.0;
        }
        let mid = self.scratch.len() / 2;
        let (_, m, _) = self.scratch.select_nth_unstable_by(mid, f32::total_cmp);
        *m
    }

    /// The hitch's line: `last 120 ms: render +98.0 ms` when one of the
    /// viewer's phases grew by half the excess over the frame before, else
    /// `last 48 ms: elsewhere, acquire 30.1 ms` (the longer wait), as the
    /// player says it.
    fn write_cause(&mut self, ms: f32, median: f32, phases: &[f32; 3], t: &FrameTimes) {
        self.last.clear();
        let _ = write!(self.last, "last {ms:.0} ms: ");
        let grew = phases
            .iter()
            .zip(&self.last_phases)
            .map(|(now, before)| now - before)
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match grew {
            Some((i, by)) if by >= (ms - median) * 0.5 => {
                let _ = write!(self.last, "{} +{by:.1} ms", OWN_PHASES[i]);
            }
            _ => {
                let (wait, wait_ms) = if t.acquire_ms >= t.present_ms {
                    ("acquire", t.acquire_ms)
                } else {
                    ("present", t.present_ms)
                };
                let _ = write!(self.last, "elsewhere, {wait} {wait_ms:.1} ms");
            }
        }
    }

    /// The FPS row: frames over the last second and their mean wall time,
    /// changed four times a second so it can be read.
    pub fn fps(&mut self) -> (f32, f32) {
        let Some(newest) = self.frames.back() else {
            return (0.0, 0.0);
        };
        let now = newest.end();
        if now - self.fps_at >= FPS_EVERY {
            let (mut n, mut wall) = (0usize, 0.0f32);
            for f in self.frames.iter().rev().take_while(|f| f.end() > now - 1.0) {
                n += 1;
                wall += f.wall_ms;
            }
            let ms = if n > 0 { wall / n as f32 } else { 0.0 };
            self.fps = (if ms > 0.0 { 1000.0 / ms } else { 0.0 }, ms);
            self.fps_at = now;
        }
        self.fps
    }

    /// The graph, its counters (no rate: the overlay is narrow) and the
    /// bands' legend.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let layout = GraphLayout {
            size: GRAPH_SIZE,
            window_secs: GRAPH_SECS,
            budget_ms: self.budget_ms(),
            fps_labels: true,
        };
        let frames = self.frames.make_contiguous();
        self.graph.show(ui, frames, layout);
        ui.add_space(3.0);
        let newest = frames.last().map_or(0.0, GraphFrame::end);
        let counters = HitchCounters {
            hitches: self.hitches,
            // Not shown at this width
            per_minute: 0.0,
            worst_ms: self.worst_ms,
            since_last_secs: self.last_hitch_end.map(|end| (newest - end).max(0.0)),
            last: self.last_hitch_end.map(|_| self.last.as_str()),
            // The viewer draws no scene load behind a fade (#1125)
            loading: 0,
        };
        ui.scope(|ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(9.5));
            ui.set_max_width(GRAPH_SIZE.x);
            // The rate is left out at this width, as the player's compact form does
            hitch_counters(ui, &counters, true);
            ui.style_mut().override_font_id = Some(egui::FontId::monospace(8.5));
            band_legend_of(ui, BANDS_SHOWN);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ60: f32 = 1000.0 / 60.0;

    /// A frame of `wall` ms with the viewer's usual split, after `stats`'s last.
    fn frame(stats: &FrameStats, wall: f32, refresh: Option<f32>) -> FrameTimes {
        let at = stats.frames.back().map_or(0.0, GraphFrame::end);
        FrameTimes {
            at,
            wall_ms: wall,
            update_ms: 1.0,
            acquire_ms: 0.5,
            draw_ms: 3.0,
            ui_ms: 2.0,
            present_ms: 0.2,
            refresh_ms: refresh,
        }
    }

    fn steady(stats: &mut FrameStats, n: usize, wall: f32, refresh: Option<f32>) {
        for _ in 0..n {
            let t = frame(stats, wall, refresh);
            stats.record(&t);
        }
    }

    #[test]
    fn a_frame_is_split_into_the_viewers_bands() {
        let mut stats = FrameStats::new();
        steady(&mut stats, 1, 10.0, None);
        let f = stats.frames().next().unwrap();
        assert_eq!(f.ms, 10.0, "unpaced: its wall time");
        assert_eq!(f.bands[Band::Other as usize], 1.0);
        assert_eq!(f.bands[Band::Render as usize], 3.0);
        assert_eq!(f.bands[Band::HudPresent as usize], 2.0);
        // The waits and what no phase holds: 0.5 + 0.2 + 3.3
        assert!((f.bands[Band::Elsewhere as usize] - 4.0).abs() < 1e-4);
        assert_eq!(f.bands[Band::Scripts as usize], 0.0);
        assert_eq!(f.bands[Band::Physics as usize], 0.0);
        let sum: f32 = f.bands.iter().sum();
        assert!((sum - f.wall_ms).abs() < 1e-4);
    }

    #[test]
    fn a_render_stall_is_a_hitch_named_for_the_render() {
        let mut stats = FrameStats::new();
        steady(&mut stats, 200, HZ60, None);
        assert_eq!(stats.hitches(), 0);
        let mut t = frame(&stats, 120.0, None);
        t.draw_ms = 101.0;
        stats.record(&t);
        assert_eq!(stats.hitches(), 1);
        assert!(stats.frames().last().unwrap().hitch);
        assert_eq!(stats.last_hitch(), Some("last 120 ms: render +98.0 ms"));
        // A long frame none of the viewer's phases took is elsewhere's
        steady(&mut stats, 60, HZ60, None);
        let mut t = frame(&stats, 80.0, None);
        t.acquire_ms = 60.0;
        stats.record(&t);
        assert_eq!(stats.hitches(), 2);
        assert_eq!(
            stats.last_hitch(),
            Some("last 80 ms: elsewhere, acquire 60.0 ms")
        );
    }

    #[test]
    fn the_first_frames_and_a_frame_under_the_floor_are_not_hitches() {
        let mut stats = FrameStats::new();
        // F2's own first frame, slow, is not judged
        steady(&mut stats, 1, 90.0, None);
        steady(&mut stats, 100, 4.0, None);
        // Five times the median, but under the median + 24 ms
        steady(&mut stats, 1, 20.0, None);
        assert_eq!(stats.hitches(), 0);
    }

    #[test]
    fn vsync_bursts_stand_one_refresh_each_and_are_not_hitches() {
        let mut stats = FrameStats::new();
        for i in 0..400 {
            let wall = if i % 4 == 3 { 4.0 * HZ60 - 18.0 } else { 6.0 };
            let t = frame(&stats, wall, Some(HZ60));
            stats.record(&t);
        }
        assert_eq!(stats.hitches(), 0);
        let f = stats.frames().last().unwrap();
        assert!((f.ms - HZ60).abs() < 1e-3, "{}", f.ms);
        assert_eq!(stats.budget_ms(), HZ60);
    }

    #[test]
    fn the_history_keeps_the_graphs_span_and_no_more() {
        let mut stats = FrameStats::new();
        steady(&mut stats, 600, HZ60, None);
        let frames: Vec<_> = stats.frames().copied().collect();
        let span = frames.last().unwrap().end() - frames[0].at;
        assert!(
            span <= KEEP_SECS + 0.02 && span > GRAPH_SECS as f64,
            "{span}"
        );
        let (fps, ms) = stats.fps();
        assert!(
            (fps - 60.0).abs() < 0.5 && (ms - HZ60).abs() < 0.01,
            "{fps} {ms}"
        );
    }

    #[test]
    fn reopening_starts_afresh_and_keeps_the_room() {
        let mut stats = FrameStats::new();
        steady(&mut stats, 300, HZ60, None);
        let mut t = frame(&stats, 150.0, None);
        t.draw_ms = 140.0;
        stats.record(&t);
        assert_eq!(stats.hitches(), 1);
        let room = stats.frames.capacity();
        stats.reset();
        assert_eq!(stats.frames().count(), 0);
        assert_eq!(stats.hitches(), 0);
        assert_eq!(stats.last_hitch(), None);
        assert_eq!(stats.frames.capacity(), room);
    }

    #[test]
    fn the_overlay_draws_the_graph_and_its_legend() {
        let mut stats = FrameStats::new();
        steady(&mut stats, 300, HZ60, Some(HZ60));
        let ctx = egui::Context::default();
        let mut drawn = None;
        let out = flint_debug_ui::run_headless_frame(&ctx, Default::default(), |ui| {
            stats.ui(ui);
            drawn = stats.graph.placed();
        });
        let placed = drawn.expect("the graph drew");
        assert_eq!(placed.rect.size(), GRAPH_SIZE);
        assert!(!out.shapes.is_empty());
    }
}
