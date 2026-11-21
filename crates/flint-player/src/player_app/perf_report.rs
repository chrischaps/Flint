//! `--perf-report FILE` (#1093): the `--exit-after` report as data.
//!
//! The `[perf]` lines are prose: an agent comparing two runs copied numbers
//! by eye. [`ExitNumbers`] is what those lines say, gathered once; the lines
//! are printed from it ([`ExitNumbers::lines`]) and the report is written
//! from it, so the two can't disagree. The report adds what the lines
//! leave out: each phase's and script's mean and max over the whole run
//! ([`RunTotals`], the frame history's frames after the start), the GPU's
//! passes as spreads (`GpuTimer::tally`, in the shape `flint render
//! --gpu-time-json` writes, `GpuSpread::to_json`), the hitch counts, and the
//! machine (adapter, backend, present mode, size, build profile, commit), so
//! `scripts/perf-compare.py` can refuse to compare unlike runs.
//!
//! The field names are held by `the_report_keeps_its_field_names`: a reader
//! keys on them, so a change is a `version` bump.

// The web writes no file: the report's types go unused there
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use super::frame_history::FrameHistory;
use super::pacing::Spread;
use super::perf::Phase;
use serde::Serialize;
use std::path::PathBuf;

/// The report's `format`, and its `version`: bumped when a field is
/// removed or changes meaning, never for one added.
pub(crate) const FORMAT: &str = "flint-perf-report";
pub(crate) const VERSION: u32 = 1;

/// `--perf-report`'s file and what the run keeps for it.
#[derive(Debug, Default)]
pub(crate) struct ReportTo {
    pub path: PathBuf,
    pub totals: RunTotals,
    /// Why the file wasn't written, for `run_failure`.
    pub failure: Option<String>,
}

/// Each phase's and script callback's time summed over the frames the
/// history closed after the start (`PERF_SKIP_SECS` of game time), and
/// the worst: the run's, where the `--perf` line has only ten seconds.
#[derive(Debug, Default)]
pub(crate) struct RunTotals {
    frames: u64,
    phase_sum: [f64; Phase::COUNT],
    phase_max: [f32; Phase::COUNT],
    /// By the history's script index (`FrameHistory::script_names`)
    script_sum: Vec<f64>,
    script_max: Vec<f32>,
    /// The newest frame already added
    seen: Option<u64>,
}

impl RunTotals {
    /// Add the frames closed since the last call (one, in the player)
    /// whose game time is at least `from`.
    pub(crate) fn add_closed(&mut self, history: &FrameHistory, from: f64) {
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
            if f.game_time < from {
                continue;
            }
            self.frames += 1;
            for (i, &ms) in f.phases.iter().enumerate() {
                self.phase_sum[i] += ms as f64;
                self.phase_max[i] = self.phase_max[i].max(ms);
            }
            if self.script_sum.len() < f.scripts.len() {
                self.script_sum.resize(f.scripts.len(), 0.0);
                self.script_max.resize(f.scripts.len(), 0.0);
            }
            for (i, &ms) in f.scripts.iter().enumerate() {
                self.script_sum[i] += ms as f64;
                self.script_max[i] = self.script_max[i].max(ms);
            }
        }
    }

    /// The run's CPU side: each phase, in frame order, and each script
    /// that ran, slowest mean first.
    pub(crate) fn cpu(&self, names: &[String]) -> Cpu {
        let n = self.frames.max(1) as f64;
        let phases: Vec<Timed> = Phase::ALL
            .iter()
            .map(|&p| Timed {
                name: p.label().to_string(),
                mean_ms: self.phase_sum[p as usize] / n,
                max_ms: self.phase_max[p as usize],
            })
            .collect();
        let mut scripts: Vec<Timed> = self
            .script_sum
            .iter()
            .zip(&self.script_max)
            .enumerate()
            .filter(|(_, (_, &max))| max > 0.0)
            .map(|(i, (&sum, &max))| Timed {
                name: names.get(i).cloned().unwrap_or_default(),
                mean_ms: sum / n,
                max_ms: max,
            })
            .collect();
        scripts.sort_by(|a, b| b.mean_ms.total_cmp(&a.mean_ms));
        Cpu {
            frames: self.frames,
            // The player's own work: every phase but the swapchain's waits
            work_ms: phases
                .iter()
                .zip(Phase::ALL)
                .filter(|(_, p)| !p.is_wait())
                .map(|(t, _)| t.mean_ms)
                .sum(),
            phases,
            scripts,
        }
    }
}

/// What the `[perf]` lines of `--exit-after` say, gathered once.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExitNumbers {
    /// Frames presented after the start, and the game time they covered
    pub frames: u32,
    pub secs: f64,
    /// The clock's dt and the wall time between presents (#406)
    pub dt: Option<Spread>,
    pub present: Option<Spread>,
    pub draws: Draws,
    /// The GPU's frame time as F2 shows it, over the frames that had one
    pub gpu: Option<GpuMean>,
    /// What the renderer created and wrote on the GPU, a frame's mean (#1091)
    pub gpu_objects: GpuObjects,
}

/// The GPU objects the renderer created and what it wrote, a frame's mean
/// over the run (#1091, `flint_render::gpu_counts`): zero objects on a
/// steady run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct GpuObjects {
    /// Every object: buffers, bind groups, textures, pipelines and shader
    /// modules
    pub created: f64,
    pub buffers: f64,
    pub bind_groups: f64,
    pub textures: f64,
    /// Render and compute pipelines and shader modules
    pub pipelines: f64,
    /// Frames of the run that created any
    pub frames_creating: u64,
    /// Buffer and texture writes, and their KB
    pub writes: f64,
    pub write_kb: f64,
}

/// The F2 overlay's draw figures, a frame's mean.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub(crate) struct Draws {
    pub draw_calls: f64,
    pub entity_draws: f64,
    pub culled: f64,
    pub lod_culled: f64,
    pub triangles: f64,
    pub shadow_triangles: f64,
}

/// `[perf] avg GPU`'s figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GpuMean {
    pub mean_ms: f64,
    pub frames: u64,
}

impl ExitNumbers {
    /// The mean frame, ms.
    pub(crate) fn mean_ms(&self) -> f64 {
        self.secs * 1000.0 / self.frames.max(1) as f64
    }

    /// The `[perf]` lines after the first (which carries fields of its own
    /// and is said by `print_exit_report`): dt, present, draws, GPU. `note`
    /// follows the GPU time (`gpu_vsync_note`).
    pub(crate) fn lines(&self, note: &str) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(dt) = &self.dt {
            lines.push(format!("[perf] dt: {dt}"));
        }
        if let Some(present) = &self.present {
            lines.push(format!("[perf] present to present: {present}"));
        }
        let d = &self.draws;
        lines.push(format!(
            "[perf] avg per frame: {:.0} draw calls ({:.0} entity draws, {:.0} culled, {:.0} of them by LOD), {:.0} triangles, {:.0} in shadows",
            d.draw_calls, d.entity_draws, d.culled, d.lod_culled, d.triangles, d.shadow_triangles
        ));
        let o = &self.gpu_objects;
        lines.push(format!(
            "[perf] GPU objects a frame: {:.2} ({:.2} buffers, {:.2} bind groups, {:.2} textures, {:.2} pipelines), in {} of {} frames; {:.1} writes ({:.1} KB)",
            o.created, o.buffers, o.bind_groups, o.textures, o.pipelines, o.frames_creating, self.frames, o.writes, o.write_kb
        ));
        if let Some(g) = &self.gpu {
            lines.push(format!(
                "[perf] avg GPU {:.2} ms/frame over {} timed frames{note}",
                g.mean_ms, g.frames
            ));
        }
        lines
    }
}

/// The report, as written.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PerfReport {
    pub format: &'static str,
    pub version: u32,
    pub written_by: &'static str,
    pub scene: String,
    pub machine: Machine,
    /// The first `[perf]` line: frames after the start, their mean
    pub run: Run,
    /// Present to present: the wall time between frames shown
    pub frame: Option<SpreadJson>,
    /// The clock's dt (paced to whole refreshes under vsync)
    pub dt: Option<SpreadJson>,
    pub draws: Draws,
    /// The GPU objects created and writes, a frame's mean (#1091)
    pub gpu_objects: GpuObjects,
    pub cpu: Cpu,
    pub gpu: Option<Gpu>,
    pub hitches: Hitches,
}

/// Where the run ran: what makes two reports comparable or not.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Machine {
    pub adapter: Option<String>,
    pub backend: Option<String>,
    pub driver: Option<String>,
    pub present_mode: Option<&'static str>,
    pub vsync: Option<bool>,
    pub width: u32,
    pub height: u32,
    pub scene_width: u32,
    pub scene_height: u32,
    pub render_scale: f32,
    pub build_profile: &'static str,
    pub opt_level: &'static str,
    pub engine_commit: Option<String>,
    pub engine_dirty: Option<bool>,
    pub os: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Run {
    pub exit_after_s: Option<f64>,
    pub skip_s: f64,
    pub frame_step_s: Option<f64>,
    pub frames: u32,
    pub secs: f64,
    pub mean_ms: f64,
    pub fps: f64,
}

/// A [`Spread`] with its unit in each name, as `--gpu-time-json`'s passes
/// name theirs (`p50_ms`, `p90_ms`, `max_ms`, `mean_ms`, `frames`).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpreadJson {
    pub p50_ms: f32,
    pub p90_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
    pub mean_ms: f32,
    pub frames: usize,
    /// Frames over 1.5x the median
    pub lumps: usize,
}

impl From<&Spread> for SpreadJson {
    fn from(s: &Spread) -> Self {
        Self {
            p50_ms: s.p50,
            p90_ms: s.p90,
            p99_ms: s.p99,
            max_ms: s.max,
            mean_ms: s.mean,
            frames: s.frames,
            lumps: s.lumps,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Cpu {
    /// The frames the totals are over (the history's, after the start)
    pub frames: u64,
    /// The mean frame's own work, every phase but acquire and present
    pub work_ms: f64,
    pub phases: Vec<Timed>,
    pub scripts: Vec<Timed>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Timed {
    pub name: String,
    pub mean_ms: f64,
    pub max_ms: f32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Gpu {
    /// `[perf] avg GPU`'s figure and its frames
    pub mean_ms: f64,
    pub timed_frames: u64,
    /// Why the pass times can't be trusted (#1316: scopes that read zero
    /// ticks, which the total then leaves out), or `None` when they can
    pub timestamps_unavailable: Option<String>,
    /// `frames_kept`, `frames_dropped`, `passes`, `frame`: each pass's
    /// spread over the run, raw (`GpuSpread::to_json`)
    #[serde(flatten)]
    pub spread: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Hitches {
    /// After the start, all of them and each class
    pub after_start: u64,
    pub player: u64,
    pub elsewhere: u64,
    pub per_minute: f64,
    /// The wall seconds after the start they are over
    pub secs: f64,
    pub startup: u64,
    pub tooling: u64,
    /// Scene loads behind a script's cover, start and after (#1125): in
    /// none of the counts above
    pub loading: u64,
    pub worst_ms: Option<f32>,
    pub worst_at_s: Option<f64>,
    pub worst_cause: Option<String>,
}

impl Hitches {
    pub(crate) fn of(watch: &super::hitch_watch::HitchWatch) -> Self {
        let run = &watch.run;
        let secs = watch.after_start_secs();
        let (worst_ms, worst_at_s, worst_cause) = match &run.all.worst {
            Some((ms, t, cause)) => (Some(*ms), Some(*t), Some(cause.clone())),
            None => (None, None, None),
        };
        Self {
            after_start: run.all.hitches,
            player: run.player.hitches,
            elsewhere: run.elsewhere.hitches,
            per_minute: if secs > 0.0 {
                run.all.hitches as f64 * 60.0 / secs
            } else {
                0.0
            },
            secs,
            startup: watch.startup.all.hitches,
            tooling: watch.tooling.hitches,
            loading: watch.run.loading.hitches + watch.startup.loading.hitches,
            worst_ms,
            worst_at_s,
            worst_cause,
        }
    }
}

impl PerfReport {
    /// The report of a run whose `[perf]` lines are `numbers`.
    pub(crate) fn new(numbers: &ExitNumbers, scene: String, run: Run, machine: Machine) -> Self {
        Self {
            format: FORMAT,
            version: VERSION,
            written_by: "flint-player",
            scene,
            machine,
            run,
            frame: numbers.present.as_ref().map(SpreadJson::from),
            dt: numbers.dt.as_ref().map(SpreadJson::from),
            draws: numbers.draws,
            gpu_objects: numbers.gpu_objects,
            cpu: RunTotals::default().cpu(&[]),
            gpu: None,
            hitches: Hitches::default(),
        }
    }
}

impl super::PlayerApp {
    /// `--perf-report FILE` (#1093): write the exit report there as JSON.
    /// Records every frame and times every script callback, as a hitch log
    /// does, and turns the GPU timing on where the adapter has it.
    pub fn report_perf_to(&mut self, path: PathBuf) {
        self.stats.report = Some(ReportTo {
            path,
            ..Default::default()
        });
        self.stats.perf.always_on = true;
        self.core.script.set_timing(true);
    }

    /// What the `[perf]` lines say, or `None` before a frame was counted.
    pub(super) fn exit_numbers(&self) -> Option<ExitNumbers> {
        let c = &self.capture;
        let frames = c.perf_frames;
        if frames == 0 {
            return None;
        }
        let avg = |sum: u64| sum as f64 / frames as f64;
        Some(ExitNumbers {
            frames,
            secs: c.perf_time,
            dt: c.perf_pacing.dt(),
            present: c.perf_pacing.present(),
            draws: Draws {
                draw_calls: avg(c.perf_draws),
                entity_draws: avg(c.perf_entity_draws),
                culled: avg(c.perf_culled),
                lod_culled: avg(c.perf_lod_culled),
                triangles: avg(c.perf_triangles),
                shadow_triangles: avg(c.perf_shadow_triangles),
            },
            gpu: (c.perf_gpu_frames > 0).then(|| GpuMean {
                mean_ms: c.perf_gpu_ms / c.perf_gpu_frames as f64,
                frames: c.perf_gpu_frames,
            }),
            gpu_objects: {
                let g = &c.perf_gpu_calls;
                GpuObjects {
                    created: avg(g.created()),
                    buffers: avg(g.buffers),
                    bind_groups: avg(g.bind_groups),
                    textures: avg(g.textures),
                    pipelines: avg(g.pipelines + g.shader_modules),
                    frames_creating: c.perf_gpu_creating,
                    writes: avg(g.writes + g.texture_writes),
                    write_kb: avg(g.write_bytes + g.texture_write_bytes) / 1024.0,
                }
            },
        })
    }

    /// The machine the run is on, as far as the player can say.
    fn report_machine(&self) -> Machine {
        let engine = crate::provenance::engine();
        let mut m = Machine {
            build_profile: crate::provenance::BUILD_PROFILE,
            opt_level: crate::provenance::BUILD_OPT_LEVEL,
            engine_commit: engine.commit,
            engine_dirty: engine.dirty,
            os: std::env::consts::OS,
            render_scale: 1.0,
            ..Default::default()
        };
        if let Some(c) = &self.render_context {
            let info = c.adapter.get_info();
            let mode = c.present_mode_in_use();
            m.adapter = Some(info.name);
            m.backend = Some(format!("{:?}", info.backend));
            m.driver = Some(info.driver).filter(|d| !d.is_empty());
            m.present_mode = Some(super::perf::present_name(mode));
            m.vsync = Some(super::perf::under_vsync(mode));
            (m.width, m.height) = (c.size.width, c.size.height);
        }
        if let Some(r) = &self.scene_renderer {
            (m.scene_width, m.scene_height) = r.scene_size();
            m.render_scale = r.render_scale();
        }
        m
    }

    /// Why the GPU's pass times read empty on this adapter (#1316), the
    /// warning's clause; `None` while they can be shown.
    pub(super) fn gpu_timestamps_unusable(&self) -> Option<String> {
        self.scene_renderer.as_ref()?.gpu_timestamps().why()
    }

    /// The GPU's passes over the run, from the timer's tally, after waiting
    /// for the read-backs still out. `None` with no timer.
    fn report_gpu(&mut self, numbers: &ExitNumbers) -> Option<Gpu> {
        let device = &self.render_context.as_ref()?.device;
        let timer = self.scene_renderer.as_mut()?.gpu_timer_mut()?;
        timer.flush(device);
        let spread = flint_render::gpu_timer::spread(timer.tallied(), 0);
        timer.tally(false);
        let serde_json::Value::Object(spread) = spread.to_json(false) else {
            return None;
        };
        let mean = numbers.gpu.unwrap_or(GpuMean {
            mean_ms: 0.0,
            frames: 0,
        });
        Some(Gpu {
            mean_ms: mean.mean_ms,
            timed_frames: mean.frames,
            timestamps_unavailable: self.gpu_timestamps_unusable(),
            spread,
        })
    }

    /// The whole report, from the same numbers the lines were printed from.
    pub(super) fn perf_report(&mut self, numbers: &ExitNumbers) -> PerfReport {
        let clock = &self.core.clock;
        let run = Run {
            exit_after_s: self.capture.exit_after,
            skip_s: super::PERF_SKIP_SECS,
            frame_step_s: clock.frame_step,
            frames: numbers.frames,
            secs: numbers.secs,
            mean_ms: numbers.mean_ms(),
            fps: 1000.0 / numbers.mean_ms(),
        };
        let scene = self.core.scene_path.clone();
        let mut report = PerfReport::new(numbers, scene, run, self.report_machine());
        if let Some(to) = &self.stats.report {
            report.cpu = to.totals.cpu(self.stats.perf.script_names());
        }
        report.gpu = self.report_gpu(numbers);
        report.hitches = Hitches::of(&self.stats.watch);
        report
    }

    /// `--perf-report`'s file, written at the exit report. A run with no
    /// frame counted writes nothing, which `run_failure` says.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn write_perf_report(&mut self, numbers: Option<&ExitNumbers>) {
        if self.stats.report.is_none() {
            return;
        }
        let result = match numbers {
            Some(numbers) => {
                let report = self.perf_report(numbers);
                let to = self.stats.report.as_ref().expect("checked above");
                serde_json::to_string_pretty(&report)
                    .map_err(|e| e.to_string())
                    .and_then(|text| {
                        crate::user_files::write(&to.path, &text).map_err(|e| e.to_string())
                    })
                    .map(|()| to.path.clone())
            }
            None => Err(format!(
                "no frame was counted after t={:.0} s",
                super::PERF_SKIP_SECS
            )),
        };
        let to = self.stats.report.as_mut().expect("checked above");
        match result {
            Ok(path) => tracing::info!(
                target: "report",
                "[perf] report: {}",
                flint_core::project::shown(&path)
            ),
            Err(e) => {
                let why = format!("--perf-report {}: {e}", to.path.display());
                tracing::error!(target: "report", "{why}");
                to.failure = Some(why);
            }
        }
    }

    /// Why `--perf-report` wrote nothing, if it didn't.
    pub(super) fn perf_report_failure(&self) -> Option<String> {
        self.stats.report.as_ref()?.failure.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers() -> ExitNumbers {
        let present: Vec<f32> = (0..120)
            .map(|i| if i % 10 == 9 { 25.0 } else { 16.5 })
            .collect();
        ExitNumbers {
            frames: 120,
            secs: 2.0,
            dt: Spread::of(&[16.6667; 120]),
            present: Spread::of(&present),
            draws: Draws {
                draw_calls: 42.0,
                entity_draws: 30.0,
                culled: 5.0,
                lod_culled: 1.0,
                triangles: 120_000.0,
                shadow_triangles: 80_000.0,
            },
            gpu: Some(GpuMean {
                mean_ms: 2.345,
                frames: 118,
            }),
            gpu_objects: GpuObjects {
                created: 0.025,
                bind_groups: 0.025,
                frames_creating: 1,
                writes: 17.0,
                write_kb: 27.4,
                ..Default::default()
            },
        }
    }

    fn report() -> serde_json::Value {
        let n = numbers();
        let run = Run {
            exit_after_s: Some(4.0),
            skip_s: 2.0,
            frame_step_s: None,
            frames: n.frames,
            secs: n.secs,
            mean_ms: n.mean_ms(),
            fps: 1000.0 / n.mean_ms(),
        };
        let mut r = PerfReport::new(&n, "a.scene.toml".into(), run, Machine::default());
        r.gpu = Some(Gpu {
            mean_ms: 2.345,
            timed_frames: 118,
            timestamps_unavailable: None,
            spread: match flint_render::GpuSpread::default().to_json(false) {
                serde_json::Value::Object(m) => m,
                _ => unreachable!(),
            },
        });
        serde_json::to_value(&r).unwrap()
    }

    /// Every key of `v` as a dotted path, arrays read through their first
    fn keys(v: &serde_json::Value, at: &str, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    let path = if at.is_empty() {
                        k.clone()
                    } else {
                        format!("{at}.{k}")
                    };
                    out.push(path.clone());
                    keys(v, &path, out);
                }
            }
            serde_json::Value::Array(a) => {
                if let Some(first) = a.first() {
                    keys(first, &format!("{at}[]"), out);
                }
            }
            _ => {}
        }
    }

    /// `scripts/perf-compare.py` and any agent key on these names: a change
    /// here is a `VERSION` bump and an edit to the compare script.
    #[test]
    fn the_report_keeps_its_field_names() {
        let mut got = Vec::new();
        keys(&report(), "", &mut got);
        got.sort();
        let mut want: Vec<&str> = vec![
            "format",
            "version",
            "written_by",
            "scene",
            "machine",
            "machine.adapter",
            "machine.backend",
            "machine.driver",
            "machine.present_mode",
            "machine.vsync",
            "machine.width",
            "machine.height",
            "machine.scene_width",
            "machine.scene_height",
            "machine.render_scale",
            "machine.build_profile",
            "machine.opt_level",
            "machine.engine_commit",
            "machine.engine_dirty",
            "machine.os",
            "run",
            "run.exit_after_s",
            "run.skip_s",
            "run.frame_step_s",
            "run.frames",
            "run.secs",
            "run.mean_ms",
            "run.fps",
            "frame",
            "frame.p50_ms",
            "frame.p90_ms",
            "frame.p99_ms",
            "frame.max_ms",
            "frame.mean_ms",
            "frame.frames",
            "frame.lumps",
            "dt",
            "dt.p50_ms",
            "dt.p90_ms",
            "dt.p99_ms",
            "dt.max_ms",
            "dt.mean_ms",
            "dt.frames",
            "dt.lumps",
            "draws",
            "draws.draw_calls",
            "draws.entity_draws",
            "draws.culled",
            "draws.lod_culled",
            "draws.triangles",
            "draws.shadow_triangles",
            "gpu_objects",
            "gpu_objects.created",
            "gpu_objects.buffers",
            "gpu_objects.bind_groups",
            "gpu_objects.textures",
            "gpu_objects.pipelines",
            "gpu_objects.frames_creating",
            "gpu_objects.writes",
            "gpu_objects.write_kb",
            "cpu",
            "cpu.frames",
            "cpu.work_ms",
            "cpu.phases",
            "cpu.phases[].name",
            "cpu.phases[].mean_ms",
            "cpu.phases[].max_ms",
            "cpu.scripts",
            "gpu",
            "gpu.mean_ms",
            "gpu.timed_frames",
            "gpu.timestamps_unavailable",
            "gpu.frames_kept",
            "gpu.frames_dropped",
            "gpu.passes",
            "gpu.frame",
            "hitches",
            "hitches.after_start",
            "hitches.player",
            "hitches.elsewhere",
            "hitches.per_minute",
            "hitches.secs",
            "hitches.startup",
            "hitches.tooling",
            "hitches.loading",
            "hitches.worst_ms",
            "hitches.worst_at_s",
            "hitches.worst_cause",
        ];
        want.sort();
        assert_eq!(got, want);
        assert_eq!(report()["format"], FORMAT);
    }

    /// A pass spread in the report is `--gpu-time-json`'s, key for key
    #[test]
    fn a_gpu_pass_is_named_as_render_names_it() {
        let spread = flint_render::GpuSpread {
            frames: 3,
            dropped: 0,
            passes: vec![flint_render::PassSpread::of("Scene", 0, vec![1.0, 2.0, 3.0]).unwrap()],
            frame: flint_render::PassSpread::of("frame", 0, vec![1.0, 2.0, 3.0]),
        };
        let v = spread.to_json(false);
        let pass = &v["passes"][0];
        let mut names: Vec<&str> = pass
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["frames", "label", "max_ms", "mean_ms", "min_ms", "p50_ms", "p90_ms"]
        );
        assert_eq!(pass["p50_ms"], 2.0);
        assert!(spread.to_json(true)["passes"][0]["samples_ms"].is_array());
    }

    /// The report's numbers are the printed lines' numbers: both come from
    /// one `ExitNumbers`
    #[test]
    fn the_report_says_what_the_lines_say() {
        let n = numbers();
        let lines = n.lines("");
        let r = report();
        let present = n.present.unwrap();
        assert_eq!(
            lines[1],
            format!("[perf] present to present: {present}"),
            "{lines:?}"
        );
        let shown = |x: f64| format!("{x:.1}");
        for key in ["p50_ms", "p90_ms", "p99_ms", "max_ms"] {
            let v = r["frame"][key].as_f64().unwrap();
            let word = &key[..3];
            assert!(
                lines[1].contains(&format!("{word} {}", shown(v))),
                "{key} {v} not in {}",
                lines[1]
            );
        }
        assert!(lines[1].contains(&format!(
            "{} of {} over",
            r["frame"]["lumps"], r["frame"]["frames"]
        )));
        assert!(lines[2].contains(&format!(
            "{:.0} draw calls",
            r["draws"]["draw_calls"].as_f64().unwrap()
        )));
        // The renderer's GPU objects (#1091)
        assert_eq!(
            lines[3],
            format!(
                "[perf] GPU objects a frame: {:.2} (0.00 buffers, {:.2} bind groups, 0.00 textures, 0.00 pipelines), in {} of 120 frames; {:.1} writes ({:.1} KB)",
                r["gpu_objects"]["created"].as_f64().unwrap(),
                r["gpu_objects"]["bind_groups"].as_f64().unwrap(),
                r["gpu_objects"]["frames_creating"],
                r["gpu_objects"]["writes"].as_f64().unwrap(),
                r["gpu_objects"]["write_kb"].as_f64().unwrap()
            )
        );
        assert!(lines[4].starts_with(&format!(
            "[perf] avg GPU {:.2} ms/frame over {} timed frames",
            r["gpu"]["mean_ms"].as_f64().unwrap(),
            r["gpu"]["timed_frames"]
        )));
        assert!((r["run"]["mean_ms"].as_f64().unwrap() - 2000.0 / 120.0).abs() < 1e-9);
    }

    #[test]
    fn the_totals_count_only_frames_after_the_start() {
        let mut history = FrameHistory::new(16);
        let t0 = web_time::Instant::now();
        for i in 0..6u64 {
            let at = t0 + std::time::Duration::from_millis(10 * i);
            history.begin_frame_at(at);
            history.mark_at(Phase::Render, at + std::time::Duration::from_millis(4));
            history.game_time(i as f64);
        }
        history.begin_frame_at(t0 + std::time::Duration::from_millis(60));
        let mut totals = RunTotals::default();
        totals.add_closed(&history, 2.0);
        // Adding again adds nothing new
        totals.add_closed(&history, 2.0);
        let cpu = totals.cpu(&[]);
        assert_eq!(cpu.frames, 4, "frames at game time 2, 3, 4 and 5");
        let render = cpu.phases.iter().find(|p| p.name == "render").unwrap();
        assert!(render.mean_ms > 3.0 && render.max_ms > 3.0, "{render:?}");
    }
}
