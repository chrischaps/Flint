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
    use super::super::hitch::{Hitch, HitchDetector};
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
            // The wait for a swapchain image isn't the player's (#1123)
            Phase::Acquire => Band::Elsewhere,
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
        // What no phase accounts for is elsewhere too, beside the acquire
        bands[Band::Elsewhere as usize] += (ms - phases).max(0.0);
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
            // The tools' own hitches (#1129) and a covered load's (#1125)
            // stand as tall bars with no mark: the counters leave them out,
            // so the marks do too
            let mut hitches = detector
                .hitches()
                .iter()
                .filter(|h| h.is_game())
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
                loading: s.loading,
            }
        }
    }

    /// The last hitch in one line, its time first, then what it was in
    /// the words of [`Hitch::cause_line`], as the hitch log, the exit
    /// report and `--perf` say it (#1128): `last 165 ms: style build
    /// ascii, 154.2 ms`, `last 117 ms: script stall.rhai:on_update 100.0
    /// ms`, `last 30 ms: render +14.0 ms` or `last 13 ms: elsewhere,
    /// present 1.0 ms` (or `acquire`, the longer wait, #1123). Written once a hitch, so its allocations are not
    /// a frame's.
    pub(crate) fn write_cause(out: &mut String, h: &Hitch) {
        let _ = write!(out, "last {:.0} ms: {}", h.judged_ms, h.cause_line());
        // The tools' own cost (#1129): named, and said not to count
        if h.cause.is_tooling() {
            out.push_str(", not counted");
        } else if h.loading {
            // Under a script's fade (#1125): counted, as `loading`
            out.push_str(", behind a cover");
        }
    }
}

/// The most style textures the overlay lists one a row; the rest share one.
pub(crate) const STYLE_TARGET_ROWS: usize = 16;
/// The widest name the overlay prints whole.
const STYLE_TARGET_NAME_COLS: usize = 22;

/// The F2 overlay's rows under `Style targets` (#561): each of the style
/// chain's textures with its size and MB, as `flint render --gpu-time`
/// lists them (`StyleTargets::entries`, in name order, so a user style's
/// `<style>.<target>` and its history pair's `.a`/`.b` sit together).
/// Past [`STYLE_TARGET_ROWS`], the rest are one row with their count and
/// MB. Called only while the full overlay shows.
pub(crate) fn style_target_rows(targets: &[flint_render::stylize::TargetEntry]) -> Vec<String> {
    const MB: f64 = 1024.0 * 1024.0;
    let w = targets
        .iter()
        .map(|t| t.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(STYLE_TARGET_NAME_COLS);
    let listed = if targets.len() > STYLE_TARGET_ROWS {
        STYLE_TARGET_ROWS - 1
    } else {
        targets.len()
    };
    let mut rows: Vec<String> = targets[..listed]
        .iter()
        .map(|t| {
            let name = if t.name.chars().count() > w {
                let kept: String = t.name.chars().take(w - 1).collect();
                format!("{kept}~")
            } else {
                t.name.to_string()
            };
            let size = format!("{}x{}", t.width, t.height);
            format!("  {name:<w$} {size:>9} {:>6.2} MB", t.bytes as f64 / MB)
        })
        .collect();
    let rest = &targets[listed..];
    if !rest.is_empty() {
        let bytes: u64 = rest.iter().map(|t| t.bytes).sum();
        let more = format!("+{} more", rest.len());
        rows.push(format!(
            "  {more:<w$} {:>9} {:>6.2} MB",
            "",
            bytes as f64 / MB
        ));
    }
    rows
}

/// How an overlay row reads: plain, dim, or the alert colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Calm,
    Quiet,
    Alert,
}

/// One row of the overlay's GPU section.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) text: String,
    pub(crate) tone: Tone,
}

/// How long the last frame that created GPU objects stays named under
/// `Alloc/frame`, seconds of game time.
pub(crate) const LAST_ALLOC_SECS: f64 = 10.0;

/// The F2 overlay's GPU section (#1091): `Alloc/frame` (the objects this
/// frame created, `Alert` when any, so a renderer that allocates every
/// frame is never quiet), the last frame that made any while it is recent,
/// the frame's writes, then the memory the renderer knows it holds by
/// class and the device's own totals. Called only while the full overlay
/// shows.
pub(crate) fn gpu_alloc_rows(
    stats: &flint_render::RenderStats,
    last: Option<(f64, flint_render::gpu_counts::GpuCalls)>,
) -> Vec<Row> {
    use flint_render::gpu_counts::format_bytes as b;
    let row = |text: String, tone| Row { text, tone };
    let g = &stats.gpu_calls;
    let created = g.created();
    let mut rows = vec![row(
        if created == 0 {
            "Alloc/frame: 0".to_string()
        } else {
            format!("Alloc/frame: {created} ({})", g.created_line())
        },
        if created == 0 {
            Tone::Calm
        } else {
            Tone::Alert
        },
    )];
    if let Some((ago, calls)) = last.filter(|(ago, _)| created == 0 && *ago < LAST_ALLOC_SECS) {
        // Two rows: a startup frame's list is wider than the overlay
        rows.push(row(format!("  last {ago:.1}s ago:"), Tone::Quiet));
        rows.push(row(format!("    {}", calls.created_line()), Tone::Quiet));
    }
    rows.push(row(
        format!(
            "Writes: {} ({})",
            g.writes + g.texture_writes,
            b(g.write_bytes + g.texture_write_bytes)
        ),
        Tone::Calm,
    ));
    let m = &stats.gpu_memory;
    rows.push(row(
        format!("GPU memory: {} textures", b(m.textures())),
        Tone::Calm,
    ));
    rows.push(row(
        format!(
            "  scene {}  style {}",
            b(m.scene_targets),
            b(m.style_targets)
        ),
        Tone::Quiet,
    ));
    rows.push(row(
        format!(
            "  shadow {}  model {}",
            b(m.shadow_maps),
            b(m.model_textures)
        ),
        Tone::Quiet,
    ));
    if m.glyph_atlases > 0 {
        rows.push(row(format!("  glyphs {}", b(m.glyph_atlases)), Tone::Quiet));
    }
    rows.push(row(format!("  meshes {}", b(m.mesh_buffers)), Tone::Quiet));
    if let Some((buffers, textures)) = stats.device_memory {
        rows.push(row(
            format!("Device: {} buffers, {} textures", b(buffers), b(textures)),
            Tone::Quiet,
        ));
    }
    rows
}

#[cfg(test)]
mod gpu_alloc_tests {
    use super::*;
    use flint_render::gpu_counts::{GpuCalls, GpuMemory};

    fn stats(calls: GpuCalls) -> flint_render::RenderStats {
        flint_render::RenderStats {
            gpu_calls: calls,
            gpu_memory: GpuMemory {
                scene_targets: 40 << 20,
                style_targets: 8 << 20,
                shadow_maps: 16 << 20,
                model_textures: 20 << 20,
                glyph_atlases: 0,
                mesh_buffers: 3 << 20,
            },
            device_memory: Some((12 << 20, 96 << 20)),
            ..Default::default()
        }
    }

    #[test]
    fn a_steady_frame_reads_zero_and_a_creating_one_is_the_alert() {
        let steady = gpu_alloc_rows(
            &stats(GpuCalls {
                writes: 15,
                ..Default::default()
            }),
            None,
        );
        assert_eq!(
            steady[0],
            Row {
                text: "Alloc/frame: 0".into(),
                tone: Tone::Calm
            }
        );
        assert_eq!(steady[1].text, "Writes: 15 (0 KB)");
        assert_eq!(steady[2].text, "GPU memory: 84.0 MB textures");
        assert!(
            steady.iter().all(|r| !r.text.contains("glyphs")),
            "no atlas, no row"
        );
        assert_eq!(
            steady.last().unwrap().text,
            "Device: 12.0 MB buffers, 96.0 MB textures"
        );

        let spawned = GpuCalls {
            buffers: 2,
            bind_groups: 1,
            ..Default::default()
        };
        let rows = gpu_alloc_rows(&stats(spawned), None);
        assert_eq!(rows[0].text, "Alloc/frame: 3 (2 buffers, 1 bind group)");
        assert_eq!(rows[0].tone, Tone::Alert);

        // The frame after: zero, and the creation named while it is recent
        let after = gpu_alloc_rows(&stats(GpuCalls::default()), Some((1.25, spawned)));
        assert_eq!(after[0].tone, Tone::Calm);
        assert_eq!(after[1].text, "  last 1.2s ago:");
        assert_eq!(after[2].text, "    2 buffers, 1 bind group");
        let later = gpu_alloc_rows(
            &stats(GpuCalls::default()),
            Some((LAST_ALLOC_SECS, spawned)),
        );
        assert!(
            !later[1].text.contains("last"),
            "forgotten after {LAST_ALLOC_SECS} s"
        );
    }
}

#[cfg(test)]
mod style_target_tests {
    use super::{style_target_rows, STYLE_TARGET_ROWS};
    use flint_render::stylize::TargetEntry;

    fn entry(name: &'static str, width: u32, height: u32, texel: u64) -> TargetEntry {
        TargetEntry {
            name,
            format: wgpu::TextureFormat::Rgba16Float,
            width,
            height,
            bytes: u64::from(width) * u64::from(height) * texel,
        }
    }

    #[test]
    fn each_target_is_a_row_with_its_name_size_and_mb() {
        let rows = style_target_rows(&[
            entry("orton.exposed", 640, 360, 8),
            entry("orton.trail.a", 640, 360, 8),
            entry("style display in", 1280, 720, 4),
        ]);
        assert_eq!(
            rows,
            [
                "  orton.exposed      640x360   1.76 MB",
                "  orton.trail.a      640x360   1.76 MB",
                "  style display in  1280x720   3.52 MB",
            ]
        );
    }

    #[test]
    fn a_long_chain_ends_in_one_row_for_the_rest() {
        let names: Vec<&'static str> = (0..20)
            .map(|i| &*Box::leak(format!("display_bloom.mip{i:02}").into_boxed_str()))
            .collect();
        let targets: Vec<_> = names.iter().map(|n| entry(n, 64, 64, 4)).collect();
        let rows = style_target_rows(&targets);
        assert_eq!(rows.len(), STYLE_TARGET_ROWS);
        assert_eq!(rows[0], "  display_bloom.mip00     64x64   0.02 MB");
        // 20 - 15 listed = 5 left, 5 x 16 KB
        assert_eq!(
            rows.last().unwrap(),
            "  +5 more                         0.08 MB"
        );
        // Its MB in the column the others' are in
        assert!(rows.iter().all(|r| r.len() == rows[0].len()));
    }

    #[test]
    fn a_name_past_the_column_is_cut_with_a_mark() {
        let rows = style_target_rows(&[entry("a_very_long_user_style.its_target", 8, 8, 4)]);
        // 22 columns: 21 of the name and the mark
        assert_eq!(rows, ["  a_very_long_user_styl~       8x8   0.00 MB"]);
    }

    #[test]
    fn no_targets_no_rows() {
        assert!(style_target_rows(&[]).is_empty());
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
        use super::super::super::hitch::{Cause, Hitch, HitchDetector, HitchNote};
        use super::super::super::perf::Phase;
        use super::super::graph::*;
        use flint_core::frame_notes::{FrameNote, NoteKind};
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
                last == "last 117 ms: script stall.rhai:on_update 100.0 ms",
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

        fn hitch(judged_ms: f32, cause: Cause, notes: Vec<HitchNote>) -> Hitch {
            Hitch {
                index: 3,
                at: 1.0,
                game_time: 1.0,
                ms: judged_ms,
                judged_ms,
                median_ms: 16.7,
                player_ms: 4.0,
                gpu_ms: None,
                refreshes: Some(3),
                cause,
                loading: false,
                notes,
            }
        }

        fn player(phase: Phase, phase_ms: f32, script: Option<(&str, f32)>) -> Cause {
            Cause::Player {
                phase,
                phase_ms,
                script: script.map(|(name, ms)| (name.to_string(), ms)),
            }
        }

        /// The overlay says what the hitch log, the exit report and
        /// `--perf` say (#1128): the line is the hitch's time, then
        /// `Hitch::cause_line` word for word.
        #[test]
        fn the_overlay_names_the_cause_the_hitch_log_names() {
            let cases = [
                // ASCII's first use on the looks tour: its pipelines build
                // inside `on_draw_ui`, so the script-UI phase grew. The
                // overlay used to say `script ui +156`.
                (
                    hitch(
                        165.6,
                        player(
                            Phase::ScriptUi,
                            156.0,
                            Some(("looks_tour.rhai:on_draw_ui", 1.2)),
                        ),
                        vec![HitchNote {
                            note: FrameNote::new(NoteKind::StyleBuild, "ascii", Some(154.2)),
                            frame_before: false,
                        }],
                    ),
                    "last 166 ms: style build ascii, 154.2 ms",
                ),
                // A script's own stall names the callback.
                (
                    hitch(
                        117.0,
                        player(Phase::Scripts, 98.0, Some(("stall.rhai:on_update", 100.0))),
                        vec![],
                    ),
                    "last 117 ms: script stall.rhai:on_update 100.0 ms",
                ),
                // A slow `on_draw_ui` is blamed too, as the log blames it:
                // the overlay used to name only a `scripts`-phase script.
                (
                    hitch(
                        60.0,
                        player(Phase::ScriptUi, 42.0, Some(("hud.rhai:on_draw_ui", 40.0))),
                        vec![],
                    ),
                    "last 60 ms: script hud.rhai:on_draw_ui 40.0 ms",
                ),
                // A render stall in a frame whose longest callback was
                // 0.3 ms is the render's.
                (
                    hitch(
                        55.0,
                        player(Phase::Render, 38.0, Some(("idle.rhai:on_update", 0.3))),
                        vec![],
                    ),
                    "last 55 ms: render +38.0 ms",
                ),
                (
                    hitch(
                        50.0,
                        Cause::Elsewhere {
                            acquire_ms: 0.5,
                            present_ms: 44.0,
                        },
                        vec![],
                    ),
                    "last 50 ms: elsewhere, present 44.0 ms",
                ),
                // The longer of the swapchain's two waits is named (#1123)
                (
                    hitch(
                        50.0,
                        Cause::Elsewhere {
                            acquire_ms: 41.0,
                            present_ms: 0.3,
                        },
                        vec![],
                    ),
                    "last 50 ms: elsewhere, acquire 41.0 ms",
                ),
            ];
            for (h, said) in cases {
                let mut out = String::new();
                write_cause(&mut out, &h);
                assert_eq!(out, said);
                assert_eq!(
                    out,
                    format!("last {:.0} ms: {}", h.judged_ms, h.cause_line())
                );
            }
        }

        /// #1123: the acquire's wait is drawn as `Elsewhere`, with what no
        /// phase accounts for, not as the renderer's.
        #[test]
        fn the_acquire_is_drawn_as_elsewhere() {
            let mut f = Frame {
                ms: 50.0,
                ..Frame::default()
            };
            f.phases[Phase::Render as usize] = 3.0;
            f.phases[Phase::Acquire as usize] = 40.0;
            f.phases[Phase::Present as usize] = 1.0;
            let g = graph_frame(&f, true);
            assert_eq!(g.bands[Band::Render as usize], 3.0);
            assert_eq!(g.bands[Band::HudPresent as usize], 1.0);
            // 40 in the acquire and the 6 no phase holds
            assert_eq!(g.bands[Band::Elsewhere as usize], 46.0);
        }

        #[test]
        fn the_overlays_own_hitch_is_named_as_not_counted() {
            use super::super::super::hitch::HitchNote;
            use flint_core::frame_notes::{FrameNote, NoteKind};
            let tooling = Hitch {
                index: 3,
                at: 1.0,
                game_time: 1.0,
                ms: 110.0,
                judged_ms: 110.0,
                median_ms: 16.7,
                player_ms: 104.0,
                gpu_ms: None,
                refreshes: None,
                cause: Cause::Tooling { tool_ms: 104.0 },
                loading: false,
                notes: vec![HitchNote {
                    note: FrameNote::new(NoteKind::StatsOverlay, "opened", Some(104.0)),
                    frame_before: false,
                }],
            };
            let mut out = String::new();
            write_cause(&mut out, &tooling);
            assert_eq!(
                out,
                "last 110 ms: stats overlay opened, 104.0 ms, not counted"
            );
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
