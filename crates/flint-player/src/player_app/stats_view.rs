//! What F2 shows (#1078), and the frame-time graph's share of it: the
//! frame history (`frame_history.rs`) and the hitch detector (`hitch.rs`)
//! turned into what `flint_debug_ui::FrameGraph` draws.
//!
//! F2 steps through [`StatsView`]: off, the full overlay (the graph, its
//! counters and the rendering stats), the graph alone in a corner, small
//! enough to leave on while playing, and off again. `--stats` opens the
//! full overlay from the first frame and `--stats=graph` the graph alone.
//!
//! While F2 shows nothing the graph does nothing: no record is read and
//! nothing is allocated. While it shows, the frames go into one buffer
//! that keeps its room, and the last hitch's line is written again only
//! when there is a new hitch, so nothing grows with the history either.

/// What the F2 overlay shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum StatsView {
    /// Nothing
    #[default]
    #[value(skip)]
    Off,
    /// The frame-time graph, its hitch counters and the rendering stats
    Full,
    /// The graph and its counters alone, in a corner
    Graph,
}

impl StatsView {
    /// Whether anything shows.
    pub fn is_open(self) -> bool {
        self != StatsView::Off
    }

    /// The next view, for an F2 press. A build without the debug HUD has
    /// no graph, so F2 there opens and closes the full overlay.
    pub fn next(self) -> StatsView {
        match self {
            StatsView::Off => StatsView::Full,
            StatsView::Full if cfg!(feature = "debug-hud") => StatsView::Graph,
            StatsView::Full | StatsView::Graph => StatsView::Off,
        }
    }
}

#[cfg(feature = "debug-hud")]
pub(crate) use graph::{StatsGraph, COMPACT_SECS, GRAPH_SECS};

/// The frame-time graph's part of the F2 overlay, lent to the HUD pass
/// for one frame. Without the debug HUD there is none.
pub(crate) struct OverlayGraph<'a> {
    #[cfg(feature = "debug-hud")]
    pub(crate) graph: &'a mut StatsGraph,
    #[cfg(feature = "debug-hud")]
    pub(crate) hitches: &'a super::hitch::HitchDetector,
    /// The display's refresh period, ms: the graph's budget line.
    #[cfg(feature = "debug-hud")]
    pub(crate) budget_ms: f32,
    #[cfg(not(feature = "debug-hud"))]
    _none: std::marker::PhantomData<&'a ()>,
}

#[cfg(feature = "debug-hud")]
impl super::state::Stats {
    /// This frame's graph, while F2 shows anything: its frames filled in
    /// from the history, and the budget from `refresh` (the display's
    /// refresh period, seconds; 60 Hz when it can't say). `None`, having
    /// read and allocated nothing, while F2 shows nothing.
    pub(crate) fn overlay_graph(
        &mut self,
        refresh: impl FnOnce() -> Option<f64>,
    ) -> Option<OverlayGraph<'_>> {
        if !self.view.is_open() {
            return None;
        }
        self.graph.prepare(&self.perf, &self.hitches);
        Some(OverlayGraph {
            graph: &mut self.graph,
            hitches: &self.hitches,
            budget_ms: refresh().map_or(1000.0 / 60.0, |s| s * 1000.0) as f32,
        })
    }
}

impl super::PlayerApp {
    /// Show `view` in the F2 overlay. While it shows, scripts are timed,
    /// so a hitch in them names the callback (as under `--perf`).
    pub(super) fn set_stats_view(&mut self, view: StatsView) {
        self.stats.view = view;
        let perf = &mut self.stats.perf;
        self.core.script.set_timing(view.is_open() || perf.readout);
        // The graph spans seconds, not frames: at a high refresh rate the
        // history's 600 frames are less, so it keeps the seconds too (it
        // grows once; the perf readout's ten seconds are more already)
        #[cfg(feature = "debug-hud")]
        if view.is_open() && !perf.readout {
            perf.keep_secs(Some(graph::KEEP_SECS));
        }
    }
}

#[cfg(feature = "debug-hud")]
mod graph {
    use super::super::frame_history::{Frame, FrameHistory};
    use super::super::hitch::{Cause, Hitch, HitchDetector};
    use super::super::perf::Phase;
    use flint_debug_ui::widgets::{Band, FrameGraph, GraphFrame, HitchCounters, BANDS};
    use std::fmt::Write as _;

    /// The seconds the full graph spans, and the compact one.
    pub(crate) const GRAPH_SECS: f32 = 4.0;
    pub(crate) const COMPACT_SECS: f32 = 3.0;
    /// What the history keeps at least while F2 shows the graph: its
    /// 600 frames are two and a half seconds at 240 Hz.
    pub(crate) const KEEP_SECS: f64 = GRAPH_SECS as f64 + 0.5;

    /// Which band each phase's time goes to.
    pub(crate) fn band_of(phase: Phase) -> Band {
        match phase {
            Phase::Scripts | Phase::ScriptUi => Band::Scripts,
            Phase::Physics => Band::Physics,
            Phase::Input | Phase::AnimFx => Band::Other,
            Phase::Sync | Phase::Render => Band::Render,
            Phase::Hud | Phase::Present => Band::HudPresent,
        }
    }

    /// A frame of the history as the graph draws it: as wide as its wall
    /// time, as tall as it stood on screen (the hitch detector's judged
    /// time), split into the bands, with what no phase accounts for as
    /// `Elsewhere`.
    pub(crate) fn graph_frame(frame: &Frame, hitch: bool) -> GraphFrame {
        let mut bands = [0.0; BANDS];
        let mut phases = 0.0;
        for phase in Phase::ALL {
            let ms = frame.phases[phase as usize];
            bands[band_of(phase) as usize] += ms;
            phases += ms;
        }
        let ms = frame.judged_ms();
        bands[Band::Elsewhere as usize] = (ms - phases).max(0.0);
        GraphFrame {
            at: frame.at,
            wall_ms: frame.ms,
            ms,
            bands,
            hitch,
        }
    }

    /// The graph's state, kept across frames; see the module docs.
    #[derive(Default)]
    pub(crate) struct StatsGraph {
        pub(crate) widget: FrameGraph,
        /// The frames handed to the widget, oldest first: the last
        /// [`GRAPH_SECS`]. The buffer grows to hold them once, then keeps
        /// its room.
        pub(crate) frames: Vec<GraphFrame>,
        /// The last hitch in one line, and which hitch it says.
        last: String,
        last_of: Option<u64>,
    }

    impl StatsGraph {
        /// Fill the frames in from the history and the detector's hitches.
        pub(crate) fn prepare(&mut self, history: &FrameHistory, detector: &HitchDetector) {
            self.frames.clear();
            let ring = history.frames();
            let horizon = ring
                .back()
                .map_or(0.0, |f| f.end() - GRAPH_SECS as f64 - 0.05);
            let first = ring.partition_point(|f| f.end() < horizon);
            let oldest = ring.get(first).map_or(0, |f| f.index);
            // Both run oldest first, so one walk marks every hitch.
            let mut hitches = detector
                .hitches()
                .iter()
                .map(|h| h.index)
                .skip_while(|&i| i < oldest)
                .peekable();
            for frame in ring.range(first..) {
                while hitches.next_if(|&i| i < frame.index).is_some() {}
                let hitch = hitches.next_if_eq(&frame.index).is_some();
                self.frames.push(graph_frame(frame, hitch));
            }

            let newest = detector.last();
            if newest.map(|h| h.index) != self.last_of {
                self.last.clear();
                if let Some(h) = newest {
                    write_cause(&mut self.last, h);
                }
                self.last_of = newest.map(|h| h.index);
            }
        }

        /// The counters under the graph, over the session.
        pub(crate) fn counters<'a>(&'a self, detector: &HitchDetector) -> HitchCounters<'a> {
            let s = detector.session();
            HitchCounters {
                hitches: s.hitches,
                per_minute: s.per_minute(),
                worst_ms: s.worst_ms,
                since_last_secs: s.since_last_secs,
                last: self.last_of.map(|_| self.last.as_str()),
            }
        }
    }

    /// The last hitch in one line, its time first, then whose it was:
    /// `last 117 ms: stall.rhai:on_update took 100`, `last 30 ms: render
    /// +14` or `last 13 ms: elsewhere, present 1`.
    pub(crate) fn write_cause(out: &mut String, h: &Hitch) {
        let _ = write!(out, "last {:.0} ms: ", h.judged_ms);
        let _ = match &h.cause {
            // A script's hitch names the script; any other, the phase
            Cause::Player {
                phase,
                phase_ms,
                script,
            } => match script.as_ref().filter(|_| *phase == Phase::Scripts) {
                Some((name, ms)) => write!(out, "{name} took {ms:.0}"),
                None => write!(out, "{} +{:.0}", phase.label(), phase_ms),
            },
            Cause::Elsewhere { present_ms } => {
                write!(out, "elsewhere, present {present_ms:.0}")
            }
        };
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f2_steps_through_full_graph_and_off() {
        let mut v = StatsView::Off;
        let mut seen = vec![];
        for _ in 0..4 {
            v = v.next();
            seen.push(v);
        }
        if cfg!(feature = "debug-hud") {
            assert_eq!(
                seen,
                [
                    StatsView::Full,
                    StatsView::Graph,
                    StatsView::Off,
                    StatsView::Full
                ]
            );
        } else {
            assert_eq!(
                seen,
                [
                    StatsView::Full,
                    StatsView::Off,
                    StatsView::Full,
                    StatsView::Off
                ]
            );
        }
    }

    #[cfg(feature = "debug-hud")]
    mod graph {
        use super::super::super::frame_history::{ClockSample, Frame, FrameHistory};
        use super::super::super::hitch::{Cause, Hitch, HitchDetector};
        use super::super::super::perf::Phase;
        use super::super::graph::*;
        use flint_debug_ui::widgets::Band;
        use std::time::Duration;

        fn ms(n: f64) -> Duration {
            Duration::from_secs_f64(n / 1000.0)
        }

        /// 400 frames at 60 Hz with a 100 ms script stall every 90, as the
        /// player plays them into the history and the detector.
        fn stalling_run() -> (FrameHistory, HitchDetector) {
            let mut h = FrameHistory::new(600);
            let mut d = HitchDetector::default();
            play(&mut h, &mut d);
            (h, d)
        }

        fn play(h: &mut FrameHistory, d: &mut HitchDetector) {
            let mut t = h.start;
            for i in 0..400 {
                h.begin_frame_at(t);
                h.clock_ticked(ClockSample {
                    raw_ms: 16.7,
                    dt_ms: 16.7,
                    paced_ms: None,
                    vsync_ms: Some(16.7),
                });
                d.observe(h);
                let stall = i % 90 == 89;
                let scripts = if stall { 100.0 } else { 2.0 };
                h.mark_at(Phase::Input, t + ms(0.5));
                h.mark_at(Phase::Scripts, t + ms(0.5 + scripts));
                h.mark_at(Phase::Render, t + ms(3.0 + scripts));
                h.mark_at(Phase::Present, t + ms(4.0 + scripts));
                if stall {
                    h.record_scripts(&[("scripts/stall.rhai:on_update".into(), 100.0)]);
                }
                t += ms(if stall { 116.7 } else { 16.7 });
            }
            h.begin_frame_at(t);
            d.observe(h);
        }

        #[test]
        fn a_frame_stacks_its_phases_into_bands_and_the_rest_is_elsewhere() {
            let mut f = Frame {
                ms: 20.0,
                ..Frame::default()
            };
            f.phases[Phase::Scripts as usize] = 3.0;
            f.phases[Phase::ScriptUi as usize] = 1.0;
            f.phases[Phase::Render as usize] = 5.0;
            f.phases[Phase::Present as usize] = 4.0;
            let g = graph_frame(&f, false);
            assert_eq!(g.ms, 20.0);
            assert_eq!(g.bands[Band::Scripts as usize], 4.0);
            assert_eq!(g.bands[Band::Render as usize], 5.0);
            assert_eq!(g.bands[Band::HudPresent as usize], 4.0);
            assert_eq!(g.bands[Band::Elsewhere as usize], 7.0);
            assert_eq!(g.bands.iter().sum::<f32>(), 20.0);
        }

        #[test]
        fn the_graph_marks_each_stall_and_names_the_script() {
            let (h, d) = stalling_run();
            let mut g = StatsGraph::default();
            g.prepare(&h, &d);
            let span = g.frames.last().unwrap().end() - g.frames[0].at;
            assert!((4.0..4.1).contains(&span), "the last four seconds: {span}");
            let marked: Vec<&_> = g.frames.iter().filter(|f| f.hitch).collect();
            // Of the four stalls (frames 89, 179, 269, 359), the newest
            // four seconds hold the last three.
            assert_eq!(marked.len(), 3);
            for f in &marked {
                let tallest = f
                    .bands
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0;
                assert_eq!(
                    tallest,
                    Band::Scripts as usize,
                    "a spike in the scripts' colour"
                );
            }
            let c = g.counters(&d);
            assert_eq!(c.hitches, 4);
            let last = c.last.unwrap();
            assert!(
                last == "last 117 ms: stall.rhai:on_update took 100",
                "{last}"
            );
        }

        #[test]
        fn preparing_again_allocates_nothing_new() {
            let (h, d) = stalling_run();
            let mut g = StatsGraph::default();
            g.prepare(&h, &d);
            let buffer = (g.frames.as_ptr(), g.frames.capacity());
            let line = g.counters(&d).last.unwrap().as_ptr();
            for _ in 0..3 {
                g.prepare(&h, &d);
            }
            assert_eq!((g.frames.as_ptr(), g.frames.capacity()), buffer);
            assert_eq!(g.counters(&d).last.unwrap().as_ptr(), line);
        }

        #[test]
        fn a_hitch_elsewhere_says_so() {
            let mut out = String::new();
            write_cause(
                &mut out,
                &Hitch {
                    index: 3,
                    at: 1.0,
                    ms: 50.0,
                    judged_ms: 50.0,
                    median_ms: 16.7,
                    player_ms: 4.0,
                    refreshes: Some(3),
                    cause: Cause::Elsewhere { present_ms: 44.0 },
                    notes: Vec::new(),
                },
            );
            assert_eq!(out, "last 50 ms: elsewhere, present 44");
        }

        /// The ticket's "no allocation for it while closed, none that grows
        /// with the history while open", counted: the whole of what a
        /// frame does for the graph before egui draws it.
        #[test]
        fn a_frame_allocates_nothing_for_the_graph_closed_or_open() {
            use super::super::super::alloc_count::allocations_in;
            use super::super::super::state::Stats;
            use super::super::StatsView;
            let mut stats = Stats::default();
            play(&mut stats.perf, &mut stats.hitches);
            let sixty = || Some(1.0 / 60.0);

            let (closed, n) = allocations_in(|| stats.overlay_graph(sixty).is_none());
            assert!(closed);
            assert_eq!(n, 0, "closed: nothing at all");
            assert_eq!(stats.graph.frames.capacity(), 0, "never touched");

            stats.view = StatsView::Graph;
            let budget = stats.overlay_graph(sixty).map(|g| g.budget_ms);
            assert!((budget.unwrap() - 16.67).abs() < 0.01);
            let (_, n) = allocations_in(|| stats.overlay_graph(sixty).is_some());
            assert_eq!(n, 0, "open and warm: the buffers are reused");
        }
    }
}
