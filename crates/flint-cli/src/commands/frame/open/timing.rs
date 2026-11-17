//! The frame's times as the bottom panel draws them: the CPU's phases and
//! scripts (a player's capture has them, `flint render`'s doesn't), the GPU's
//! passes, and the frame history around the captured frame as the F2
//! overlay's graph.

use flint_debug_ui::widgets::{Band, GraphFrame, BANDS};
use flint_render::frame_capture::{CpuFrame, FrameDescription};

/// The player's phases in the order a frame runs them
/// (`flint_player::perf::Phase`'s labels); a phase not listed goes after.
const PHASE_ORDER: [&str; 10] = [
    "input",
    "scripts",
    "physics",
    "script ui",
    "anim/fx",
    "sync",
    "acquire",
    "render",
    "hud",
    "present",
];

/// Which of the graph's bands a phase's time goes to (the player's
/// `stats_view::band_of`, by label).
pub fn band_of(phase: &str) -> Band {
    match phase {
        "scripts" | "script ui" => Band::Scripts,
        "physics" => Band::Physics,
        "sync" | "render" => Band::Render,
        "hud" | "present" => Band::HudPresent,
        // The wait for a swapchain image isn't the player's (#1123)
        "acquire" => Band::Elsewhere,
        _ => Band::Other,
    }
}

/// One segment of a bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub label: String,
    pub ms: f32,
    /// The row of the pass tree it is (GPU segments only).
    pub pass: Option<usize>,
}

/// What the bottom panel draws.
#[derive(Debug, Clone, Default)]
pub struct Timing {
    /// The captured frame's CPU phases, in run order.
    pub cpu: Vec<Segment>,
    /// Its scripts' callbacks, slowest first.
    pub scripts: Vec<Segment>,
    /// The frame's wall time, ms.
    pub cpu_ms: Option<f32>,
    /// Why there are no CPU phases, when there are none.
    pub cpu_missing: Option<String>,
    /// Why there is no frame history, when there is none.
    pub history_missing: String,
    /// The CPU side (phases, scripts, history, hitch) is the captured
    /// frame's, shown beside a replay's GPU times (#1208): a replay draws
    /// the frame again and runs no game loop, so these describe the frame
    /// it draws.
    pub cpu_of_captured: bool,
    /// The timed top-level passes, in order.
    pub gpu: Vec<Segment>,
    /// The GPU frame's time (the timer's whole frame).
    pub gpu_ms: Option<f32>,
    /// The frame history, oldest first, for the graph; `at` counts from
    /// the first frame.
    pub history: Vec<GraphFrame>,
    /// The captured frame's place in `history`.
    pub captured: Option<usize>,
    /// The hitch that asked for the capture, in one line.
    pub hitch: Option<String>,
}

impl Timing {
    pub fn new(d: &FrameDescription, rows: &[super::model::PassRow]) -> Self {
        let mut t = Timing {
            gpu_ms: d.frame.gpu_frame_ms,
            ..Default::default()
        };
        for (i, row) in rows.iter().enumerate() {
            if row.depth == 0 {
                if let Some(ms) = row.gpu_ms {
                    t.gpu.push(Segment {
                        label: row.key.clone(),
                        ms,
                        pass: Some(i),
                    });
                }
            }
        }
        t.lay_cpu(d);
        t
    }

    /// A replay's timing (#1208): its own GPU passes, and the CPU side of
    /// the capture it replays, which describes the frame being drawn.
    pub fn replaying(mut self, captured: &FrameDescription) -> Self {
        self.cpu.clear();
        self.scripts.clear();
        self.cpu_ms = None;
        self.cpu_missing = None;
        self.history.clear();
        self.captured = None;
        self.hitch = None;
        self.lay_cpu(captured);
        self.cpu_of_captured = true;
        self
    }

    /// The CPU side of `d`: its phases, scripts, history and hitch, or why
    /// it has none.
    fn lay_cpu(&mut self, d: &FrameDescription) {
        let t = self;
        let replay = d.written_by.as_deref() == Some("flint frame replay");
        t.history_missing = if replay {
            "no frame history: a replay draws one frame; the capture it replays keeps the frames around it"
        } else {
            "no frame history: the player's captures keep the frames around theirs"
        }
        .into();
        let Some(cpu) = &d.cpu else {
            t.cpu_missing = Some(
                if replay {
                    "no CPU phases: a replay draws the frame again with no game loop;                      the capture it replays has the frame's, if the player wrote it"
                } else {
                    "no CPU phases: a capture from `flint render` has none, the player's do"
                }
                .into(),
            );
            return;
        };
        match &cpu.frame {
            Some(f) => {
                t.cpu = phases(f);
                t.cpu_ms = Some(f.ms);
                let mut scripts: Vec<Segment> = f
                    .scripts_ms
                    .iter()
                    .map(|(name, ms)| Segment {
                        label: name.clone(),
                        ms: *ms,
                        pass: None,
                    })
                    .collect();
                scripts.sort_by(|a, b| b.ms.total_cmp(&a.ms));
                t.scripts = scripts;
            }
            None => {
                t.cpu_missing = Some(cpu.missing.clone().unwrap_or_else(|| {
                    "the captured frame isn't in the history the capture kept".into()
                }))
            }
        }
        let mut at = 0.0f64;
        for f in &cpu.window {
            if Some(f.index) == cpu.frame.as_ref().map(|c| c.index) {
                t.captured = Some(t.history.len());
            }
            t.history.push(graph_frame(f, at));
            at += f.ms as f64 / 1000.0;
        }
        if let Some(h) = &cpu.hitch {
            // The cause names its phase or script already (`script
            // stall.rhai:on_update 100.0 ms`); say them only when it doesn't
            let mut cause = h.cause.clone();
            if cause.is_empty() {
                if let (Some(s), Some(ms)) = (&h.script, h.script_ms) {
                    cause = format!("{s} {ms:.1} ms");
                } else if let (Some(p), Some(ms)) = (&h.phase, h.phase_ms) {
                    cause = format!("{p} +{ms:.1} ms");
                }
            }
            let mut line = format!(
                "hitch at frame {}: {:.1} ms against a median of {:.2} ms",
                h.frame, h.judged_ms, h.median_ms
            );
            if !cause.is_empty() {
                line.push_str(&format!(" · {cause}"));
            }
            t.hitch = Some(line);
        }
    }

    /// The history's span, seconds.
    pub fn history_secs(&self) -> f32 {
        self.history.last().map_or(0.0, |f| f.end() as f32)
    }
}

/// A frame's phases in the order they run, with nothing of 0 ms.
fn phases(f: &CpuFrame) -> Vec<Segment> {
    let rank = |name: &str| PHASE_ORDER.iter().position(|p| *p == name).unwrap_or(99);
    let mut out: Vec<Segment> = f
        .phases_ms
        .iter()
        .filter(|(_, ms)| **ms > 0.0)
        .map(|(name, ms)| Segment {
            label: name.clone(),
            ms: *ms,
            pass: None,
        })
        .collect();
    out.sort_by_key(|s| rank(&s.label));
    out
}

/// A captured frame as the graph draws it: as wide as its wall time, as
/// tall as it stood on screen, the phases in bands and what none accounts
/// for as `Elsewhere` (the player's `stats_view::graph_frame`).
fn graph_frame(f: &CpuFrame, at: f64) -> GraphFrame {
    let mut bands = [0.0; BANDS];
    let mut sum = 0.0;
    for (name, ms) in &f.phases_ms {
        bands[band_of(name) as usize] += ms;
        sum += ms;
    }
    let ms = if f.judged_ms > 0.0 { f.judged_ms } else { f.ms };
    bands[Band::Elsewhere as usize] = (ms - sum).max(0.0);
    GraphFrame {
        at,
        wall_ms: f.ms,
        ms,
        bands,
        hitch: f.hitch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::frame_capture::{CpuDescription, HitchDescription};

    fn cpu_frame(index: u64, ms: f32, hitch: bool) -> CpuFrame {
        CpuFrame {
            index,
            game_time: index as f64 / 60.0,
            ms,
            judged_ms: ms,
            phases_ms: [
                ("render", 2.0),
                ("scripts", 1.0),
                ("present", 0.0),
                ("input", 0.5),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            scripts_ms: [("hud.rhai on_update", 0.2), ("stall.rhai on_update", 0.7)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            hitch,
            ..Default::default()
        }
    }

    #[test]
    fn a_render_capture_says_why_it_has_no_cpu_times() {
        let t = Timing::new(&FrameDescription::default(), &[]);
        assert!(t.cpu.is_empty() && t.history.is_empty() && t.gpu.is_empty());
        assert!(t.cpu_missing.unwrap().contains("flint render"));
    }

    /// A player's capture with a history of two frames, the last captured.
    fn player_capture() -> FrameDescription {
        let mut d = FrameDescription::default();
        let window = vec![cpu_frame(10, 16.0, false), cpu_frame(11, 17.0, false)];
        d.written_by = Some("flint-player".into());
        d.cpu = Some(CpuDescription {
            frame: Some(window[1].clone()),
            window,
            ..Default::default()
        });
        d
    }

    #[test]
    fn a_replay_opened_alone_says_it_runs_no_game_loop() {
        let d = FrameDescription {
            written_by: Some("flint frame replay".into()),
            ..Default::default()
        };
        let t = Timing::new(&d, &[]);
        let why = t.cpu_missing.unwrap();
        assert!(why.contains("a replay"), "{why}");
        assert!(!why.contains("flint render"), "{why}");
        assert!(t.history_missing.contains("a replay"));
        assert!(!t.cpu_of_captured);
    }

    #[test]
    fn a_replay_shown_for_a_player_capture_keeps_the_captured_cpu_side() {
        let captured = player_capture();
        let mut replayed = FrameDescription {
            written_by: Some("flint frame replay".into()),
            ..Default::default()
        };
        replayed.frame.gpu_frame_ms = Some(3.5);
        let t = Timing::new(&replayed, &[]).replaying(&captured);
        assert!(t.cpu_of_captured);
        assert_eq!(t.cpu_missing, None);
        assert_eq!(t.cpu_ms, Some(17.0));
        assert_eq!(t.history.len(), 2);
        assert_eq!(t.captured, Some(1));
        assert_eq!(t.gpu_ms, Some(3.5), "the GPU side is the replay's own");
        // A replay of a render capture has the render's reason, which is
        // about the frame replayed and true
        let render = FrameDescription {
            written_by: Some("flint render".into()),
            ..Default::default()
        };
        let t = Timing::new(&replayed, &[]).replaying(&render);
        assert!(t.cpu_missing.unwrap().contains("flint render"));
    }

    #[test]
    fn a_player_capture_lays_its_phases_in_run_order_and_marks_the_frame() {
        let mut d = FrameDescription::default();
        let window = vec![
            cpu_frame(10, 16.0, false),
            cpu_frame(11, 110.0, true),
            cpu_frame(12, 16.0, false),
        ];
        d.cpu = Some(CpuDescription {
            frame: Some(window[2].clone()),
            window,
            hitch: Some(HitchDescription {
                frame: 11,
                judged_ms: 110.0,
                median_ms: 16.0,
                class: "stall".into(),
                cause: "script".into(),
                script: Some("stall.rhai on_update".into()),
                script_ms: Some(100.0),
                ..Default::default()
            }),
            ..Default::default()
        });
        let t = Timing::new(&d, &[]);
        let names: Vec<&str> = t.cpu.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(names, ["input", "scripts", "render"], "no 0 ms phase");
        assert_eq!(t.scripts[0].label, "stall.rhai on_update");
        assert_eq!(t.captured, Some(2));
        assert_eq!(t.history.len(), 3);
        assert!(t.history[1].hitch);
        assert!((t.history[2].at - 0.126).abs() < 1e-6);
        assert!((t.history_secs() - 0.142).abs() < 1e-4);
        let elsewhere = t.history[0].bands[Band::Elsewhere as usize];
        assert!((elsewhere - 12.5).abs() < 1e-4, "{elsewhere}");
        let line = t.hitch.unwrap();
        assert!(
            line.contains("110.0 ms against a median of 16.00 ms"),
            "{line}"
        );
        assert!(
            line.ends_with("· script"),
            "the cause as the detector said it: {line}"
        );
    }
}
