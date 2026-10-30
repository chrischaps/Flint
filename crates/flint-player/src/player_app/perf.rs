//! The perf readout's numbers (#120): what each frame cost, kept for the
//! last ten seconds. The web page shows them in a corner with `?perf=1`
//! (`web_perf.rs`); this half has no browser in it, so it is tested here.
//!
//! A frame is the time from one `RedrawRequested` to the next, which on the
//! web is one `requestAnimationFrame` to the next. Inside it the player
//! marks where each phase of its own work ends ([`Phase`]); whatever the
//! frame took beyond the phases is time the main thread spent elsewhere
//! (the browser, the compositor, a wait for the next vblank).

use super::pacing::Spread;
use std::collections::VecDeque;
use web_time::Instant;

/// How far back the readout looks.
pub(crate) const WINDOW_SECS: f64 = 10.0;

/// The player's work in a frame, in the order it runs. A mark ends the
/// phase it names and starts the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Pointer lock, pads, the clock, transitions, extensions, events
    Input,
    /// Scripts' `on_update`
    Scripts,
    /// The fixed-step physics and the camera that follows it
    Physics,
    /// Camera overrides, `on_draw_ui` and script commands
    ScriptUi,
    /// Audio triggers, animation, particles
    AnimFx,
    /// Procgen, the renderer's sync with the world, late audio
    Sync,
    /// The renderer: draw lists, encoding and submission
    Render,
    /// The egui pass over the frame
    Hud,
    /// Handing the frame to the swapchain
    Present,
}

impl Phase {
    pub(crate) const ALL: [Phase; 9] = [
        Phase::Input,
        Phase::Scripts,
        Phase::Physics,
        Phase::ScriptUi,
        Phase::AnimFx,
        Phase::Sync,
        Phase::Render,
        Phase::Hud,
        Phase::Present,
    ];
    const COUNT: usize = Self::ALL.len();

    pub(crate) fn label(self) -> &'static str {
        match self {
            Phase::Input => "input",
            Phase::Scripts => "scripts",
            Phase::Physics => "physics",
            Phase::ScriptUi => "script ui",
            Phase::AnimFx => "anim/fx",
            Phase::Sync => "sync",
            Phase::Render => "render",
            Phase::Hud => "hud",
            Phase::Present => "present",
        }
    }
}

/// One frame: when it began (seconds since the recorder started), how long
/// it lasted, and its phases' times, all ms.
#[derive(Debug, Clone)]
struct Frame {
    at: f64,
    ms: f32,
    phases: [f32; Phase::COUNT],
    /// Each script's `on_update`, indexed as `PerfRecorder::script_names`.
    scripts: Vec<f32>,
}

/// A phase's time over the window: the mean frame's, and the worst.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhaseTime {
    pub phase: Phase,
    pub mean: f32,
    pub max: f32,
}

/// A script file's `on_update` over the window: the mean frame's, and the
/// worst.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScriptTime {
    pub name: String,
    pub mean: f32,
    pub max: f32,
}

/// How many scripts the readout names, worst first.
const SCRIPTS_SHOWN: usize = 4;

/// What the window holds, summed up for the readout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerfSummary {
    /// Frame to frame, ms.
    pub frames: Spread,
    /// Seconds the frames cover.
    pub secs: f64,
    pub phases: Vec<PhaseTime>,
    /// The mean frame's work in every phase together, ms.
    pub cpu_mean: f32,
    /// What the worst frame's phases added up to: whether the longest
    /// frames were the player's own work or time spent elsewhere.
    pub worst_frame: f32,
    pub worst_frame_cpu: f32,
    /// The scripts with the worst single `on_update`, worst first.
    pub scripts: Vec<ScriptTime>,
}

/// Records frames and their phases; see the module docs.
pub(crate) struct PerfRecorder {
    start: Instant,
    frames: VecDeque<Frame>,
    /// The frame under way: when it began and when its last mark fell.
    frame_start: Option<Instant>,
    last_mark: Option<Instant>,
    phases: [f32; Phase::COUNT],
    /// Script files seen so far, and the frame under way's time in each.
    script_names: Vec<String>,
    scripts: Vec<f32>,
    /// The renderer's GPU timing was asked for (the readout's GPU lines).
    pub gpu_wanted: bool,
    /// When the readout was last drawn, so it is drawn twice a second.
    pub shown_at: Option<Instant>,
}

impl PerfRecorder {
    pub(crate) fn new(gpu_wanted: bool) -> Self {
        Self {
            start: Instant::now(),
            frames: VecDeque::new(),
            frame_start: None,
            last_mark: None,
            phases: [0.0; Phase::COUNT],
            script_names: Vec::new(),
            scripts: Vec::new(),
            gpu_wanted,
            shown_at: None,
        }
    }

    /// A new frame begins now: the one before it ends.
    pub(crate) fn begin_frame(&mut self) {
        self.begin_frame_at(Instant::now());
    }

    /// This frame's time in each script file's `on_update`
    /// (`ScriptSystem::update_times`).
    pub(crate) fn record_scripts(&mut self, times: &[(String, f32)]) {
        for (path, ms) in times {
            let i = match self.script_names.iter().position(|n| n == path) {
                Some(i) => i,
                None => {
                    self.script_names.push(path.clone());
                    self.script_names.len() - 1
                }
            };
            if self.scripts.len() <= i {
                self.scripts.resize(i + 1, 0.0);
            }
            self.scripts[i] += ms;
        }
    }

    /// The end of `phase`, now.
    pub(crate) fn mark(&mut self, phase: Phase) {
        self.mark_at(phase, Instant::now());
    }

    fn begin_frame_at(&mut self, now: Instant) {
        if let Some(began) = self.frame_start {
            let ms = now.duration_since(began).as_secs_f32() * 1000.0;
            let frame = Frame {
                at: began.duration_since(self.start).as_secs_f64(),
                ms,
                phases: self.phases,
                scripts: std::mem::take(&mut self.scripts),
            };
            self.push(frame);
        }
        self.frame_start = Some(now);
        self.last_mark = Some(now);
        self.phases = [0.0; Phase::COUNT];
        self.scripts.clear();
    }

    fn mark_at(&mut self, phase: Phase, now: Instant) {
        let Some(last) = self.last_mark else {
            return;
        };
        // `+=`: a phase marked twice in a frame (a second render path)
        // counts both.
        self.phases[phase as usize] += now.duration_since(last).as_secs_f32() * 1000.0;
        self.last_mark = Some(now);
    }

    fn push(&mut self, frame: Frame) {
        let horizon = frame.at - WINDOW_SECS;
        self.frames.push_back(frame);
        while self.frames.front().is_some_and(|f| f.at < horizon) {
            self.frames.pop_front();
        }
    }

    /// The window so far, or `None` before the first whole frame.
    pub(crate) fn summary(&self) -> Option<PerfSummary> {
        let ms: Vec<f32> = self.frames.iter().map(|f| f.ms).collect();
        let frames = Spread::of(&ms)?;
        let n = self.frames.len() as f32;
        let phases: Vec<PhaseTime> = Phase::ALL
            .iter()
            .map(|&phase| {
                let i = phase as usize;
                let (sum, max) = self.frames.iter().fold((0.0f32, 0.0f32), |(s, m), f| {
                    (s + f.phases[i], m.max(f.phases[i]))
                });
                PhaseTime {
                    phase,
                    mean: sum / n,
                    max,
                }
            })
            .collect();
        let worst = self.frames.iter().max_by(|a, b| a.ms.total_cmp(&b.ms))?;
        let mut scripts: Vec<ScriptTime> = self
            .script_names
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let (sum, max) = self.frames.iter().fold((0.0f32, 0.0f32), |(s, m), f| {
                    let ms = f.scripts.get(i).copied().unwrap_or(0.0);
                    (s + ms, m.max(ms))
                });
                ScriptTime {
                    name: script_name(path),
                    mean: sum / n,
                    max,
                }
            })
            .filter(|s| s.max > 0.0)
            .collect();
        scripts.sort_by(|a, b| b.max.total_cmp(&a.max));
        scripts.truncate(SCRIPTS_SHOWN);
        let secs = self.frames.iter().map(|f| f.ms as f64).sum::<f64>() / 1000.0;
        Some(PerfSummary {
            frames,
            secs,
            cpu_mean: phases.iter().map(|p| p.mean).sum(),
            phases,
            worst_frame: worst.ms,
            worst_frame_cpu: worst.phases.iter().sum(),
            scripts,
        })
    }
}

/// Everything the readout shows besides the frames: gathered by the page
/// each time it is drawn.
#[derive(Debug, Clone, Default)]
pub(crate) struct PerfContext {
    /// The game's name, for the copied line.
    pub game: String,
    /// The browser and its version, e.g. "Chrome 156".
    pub browser: String,
    /// The adapter as the browser describes it.
    pub adapter: String,
    /// The surface in device pixels, and the page's pixel ratio.
    pub surface: [u32; 2],
    pub dpr: f64,
    /// The size the 3D scene is drawn at, and its render scale (#122).
    pub scene: [u32; 2],
    pub scale: f32,
    pub draw_calls: u32,
    pub triangles: u32,
    /// Game time, seconds.
    pub game_time: f64,
    /// Late audio buffers of all buffers so far, one buffer's length and
    /// the mean time to mix one, ms; `None` before the sound starts, and on
    /// the desktop, whose device stream isn't metered.
    pub audio: Option<(u64, u64, f64, f64)>,
    /// The voices and spatial tracks being mixed, and how many of each are
    /// paused because nobody can hear them (#138); `None` without sound.
    pub mix: Option<flint_audio::MixCounts>,
    /// What the GPU lines are.
    pub gpu: GpuSource,
    /// The frame-rate pacing (#140), when the web player paces.
    pub pacing: Option<String>,
}

/// Where the GPU's own times come from, said on the readout.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum GpuSource {
    /// `?perf=cpu`: GPU timing left off, so it costs nothing.
    #[default]
    Off,
    /// The adapter has no `timestamp-query`: the CPU phases are all there is.
    Unsupported,
    /// Timestamp queries: each scope's time, smoothed, and the frame's.
    Timed {
        passes: Vec<(&'static str, f32)>,
        frame_ms: f32,
    },
}

/// The scene's pixels as a share of the canvas's.
fn pixel_share(c: &PerfContext) -> f64 {
    let canvas = c.surface[0] as f64 * c.surface[1] as f64;
    if canvas == 0.0 {
        return 1.0;
    }
    c.scene[0] as f64 * c.scene[1] as f64 / canvas
}

fn count(n: u32) -> String {
    flint_render::format_count(n)
}

/// The readout, a line per row; the page sets it as the panel's text.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn readout(s: &PerfSummary, c: &PerfContext) -> String {
    let mut out = String::new();
    let mut line = |l: String| {
        out.push_str(&l);
        out.push('\n');
    };
    let f = &s.frames;
    line(format!(
        "frame  {:5.1} p50  {:5.1} p90  {:5.1} p99  {:5.1} max ms",
        f.p50, f.p90, f.p99, f.max
    ));
    line(format!(
        "       {} frames in {:.0} s, {} over 1.5x p50",
        f.frames, s.secs, f.lumps
    ));
    if let Some(pacing) = &c.pacing {
        line(format!("pace   {pacing}"));
    }
    line(format!(
        "worst  {:.1} ms, {:.1} of it the player's",
        s.worst_frame, s.worst_frame_cpu
    ));
    line(format!(
        "canvas {}x{} px at DPR {}",
        c.surface[0],
        c.surface[1],
        trim(c.dpr)
    ));
    line(format!(
        "scene  {}x{} px, scale {} ({:.0}% of the pixels)",
        c.scene[0],
        c.scene[1],
        trim(c.scale as f64),
        pixel_share(c) * 100.0
    ));
    line(format!("gpu    {}", c.adapter));
    line(format!(
        "draws  {} calls, {} tris",
        c.draw_calls,
        count(c.triangles)
    ));
    line(match c.audio {
        Some((late, buffers, buffer_ms, mix_ms)) => format!(
            "audio  {late} late of {buffers} ({buffer_ms:.0} ms buffers), mix {mix_ms:.2} ms"
        ),
        None => "audio  not started".to_string(),
    });
    if let Some(m) = c.mix {
        line(format!(
            "voices {} of {} mixed, {} of {} spatial tracks",
            m.live_voices(),
            m.voices,
            m.live_tracks(),
            m.tracks
        ));
        if m.skipped_oneshots + m.stolen_oneshots > 0 {
            line(format!(
                "       {} one-shots out of earshot, {} stolen",
                m.skipped_oneshots, m.stolen_oneshots
            ));
        }
    }
    line(String::new());
    line(format!("cpu ms   mean   max   ({:.2} a frame)", s.cpu_mean));
    for p in &s.phases {
        line(format!(
            "  {:<9}{:5.2} {:5.1}",
            p.phase.label(),
            p.mean,
            p.max
        ));
    }
    if !s.scripts.is_empty() {
        line(String::new());
        line("scripts         mean   max   (worst first)".to_string());
        for sc in &s.scripts {
            line(format!("  {:<13}{:5.2} {:5.1}", sc.name, sc.mean, sc.max));
        }
    }
    line(String::new());
    match &c.gpu {
        GpuSource::Off => line("gpu ms   off (?perf=cpu)".to_string()),
        GpuSource::Unsupported => {
            line("gpu ms   no timestamp-query here: cpu phases only".to_string())
        }
        GpuSource::Timed { passes, frame_ms } => {
            line(format!("gpu ms   timestamp-query ({frame_ms:.2} a frame)"));
            for (label, ms) in passes {
                line(format!("  {label:<12}{ms:5.2}"));
            }
        }
    }
    out.pop();
    out
}

/// The copyable line: one row of plain text a visitor can paste into an
/// issue, every number the readout has.
pub(crate) fn summary_line(s: &PerfSummary, c: &PerfContext) -> String {
    let f = &s.frames;
    let mut parts = vec![
        format!("flint perf · {} · t={:.0}s", c.game, c.game_time),
        format!("{} · {}", c.browser, c.adapter),
        format!("{}x{} DPR {}", c.surface[0], c.surface[1], trim(c.dpr)),
        format!(
            "scene {}x{} scale {}",
            c.scene[0],
            c.scene[1],
            trim(c.scale as f64)
        ),
        format!(
            "frame p50 {:.1} p90 {:.1} p99 {:.1} max {:.1} ms ({} frames/{:.0}s)",
            f.p50, f.p90, f.p99, f.max, f.frames, s.secs
        ),
        format!(
            "cpu {:.2} ms: {}",
            s.cpu_mean,
            s.phases
                .iter()
                .filter(|p| p.max > 0.0)
                .map(|p| format!("{} {:.2}/{:.1}", p.phase.label(), p.mean, p.max))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];
    if let Some(pacing) = &c.pacing {
        parts.insert(5, format!("pace {pacing}"));
    }
    if !s.scripts.is_empty() {
        parts.push(format!(
            "worst scripts: {}",
            s.scripts
                .iter()
                .map(|sc| format!("{} {:.2}/{:.1}", sc.name, sc.mean, sc.max))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match &c.gpu {
        GpuSource::Off => parts.push("gpu off".into()),
        GpuSource::Unsupported => parts.push("gpu untimed".into()),
        GpuSource::Timed { passes, frame_ms } => parts.push(format!(
            "gpu {frame_ms:.2} ms: {}",
            passes
                .iter()
                .map(|(l, ms)| format!("{l} {ms:.2}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
    parts.push(format!(
        "{} draws {} tris",
        c.draw_calls,
        count(c.triangles)
    ));
    if let Some((late, buffers, _, mix_ms)) = c.audio {
        parts.push(format!("audio {late}/{buffers} late mix {mix_ms:.2} ms"));
    }
    if let Some(m) = c.mix {
        parts.push(format!(
            "voices {}/{} tracks {}/{} one-shots {} unheard {} stolen",
            m.live_voices(),
            m.voices,
            m.live_tracks(),
            m.tracks,
            m.skipped_oneshots,
            m.stolen_oneshots
        ));
    }
    parts.join(" | ")
}

impl super::PlayerApp {
    /// Record frame and phase times for a perf readout (the web page's
    /// `?perf=1`, #120). `gpu` also turns on the renderer's GPU timing
    /// (ADR 0108) where the adapter has timestamps; it adds a few small
    /// submissions a frame, so `?perf=cpu` leaves it off.
    pub fn enable_perf(&mut self, gpu: bool) {
        self.perf = Some(PerfRecorder::new(gpu));
        self.script.set_update_timing(true);
    }

    /// The end of `phase` in this frame, when recording.
    pub(super) fn perf_mark(&mut self, phase: Phase) {
        if let Some(perf) = &mut self.perf {
            perf.mark(phase);
        }
    }

    /// Whether the readout is due again: at most once every `every`
    /// seconds. Never while not recording.
    pub(super) fn perf_due(&mut self, every: f64) -> bool {
        let Some(perf) = &mut self.perf else {
            return false;
        };
        let now = Instant::now();
        if perf
            .shown_at
            .is_some_and(|t| now.duration_since(t).as_secs_f64() < every)
        {
            return false;
        }
        perf.shown_at = Some(now);
        true
    }

    /// The window's frames and everything else the readout shows, with the
    /// browser and adapter as the platform names them.
    pub(super) fn perf_snapshot(
        &self,
        browser: String,
        adapter: String,
    ) -> Option<(PerfSummary, PerfContext)> {
        let perf = self.perf.as_ref()?;
        let summary = perf.summary()?;
        let renderer = self.scene_renderer.as_ref()?;
        let context = self.render_context.as_ref()?;
        let stats = renderer.collect_stats();
        let gpu = if !perf.gpu_wanted {
            GpuSource::Off
        } else if !renderer.gpu_timing() {
            GpuSource::Unsupported
        } else {
            GpuSource::Timed {
                passes: stats.gpu_passes.iter().map(|p| (p.label, p.ms)).collect(),
                frame_ms: stats.gpu_frame_ms,
            }
        };
        let game = std::path::Path::new(&self.scene_path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split('.').next().unwrap_or(n).to_string())
            .unwrap_or_default();
        Some((
            summary,
            PerfContext {
                game,
                browser,
                adapter,
                surface: [context.config.width, context.config.height],
                dpr: self.window.as_ref().map_or(1.0, |w| w.scale_factor()),
                scene: renderer.scene_size().into(),
                scale: renderer.render_scale(),
                draw_calls: stats.draw_calls,
                triangles: stats.triangles,
                game_time: self.clock.total_time,
                audio: self
                    .audio
                    .engine
                    .buffer_stats()
                    .map(|b| (b.late, b.buffers, b.buffer_ms, b.mix_mean_ms)),
                mix: self.audio.mix_counts(),
                gpu,
                pacing: self.frame_rate_line(),
            },
        ))
    }

    /// `flint-player --perf`: the line a web visitor would copy, printed
    /// every ten seconds, so the desktop can be set beside the browser.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn print_perf(&mut self) {
        if !self.perf_due(WINDOW_SECS) {
            return;
        }
        let adapter = self
            .render_context
            .as_ref()
            .map(|c| {
                let info = c.adapter.get_info();
                format!("{} ({:?})", info.name, info.backend)
            })
            .unwrap_or_default();
        if let Some((summary, context)) = self.perf_snapshot("native".into(), adapter) {
            // A window that began before the first readout is short; skip it.
            if summary.secs >= WINDOW_SECS * 0.9 {
                eprintln!("{}", summary_line(&summary, &context));
            }
        }
    }
}

/// `scripts/biolum.rhai` -> `biolum`.
fn script_name(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.strip_suffix(".rhai").unwrap_or(file).to_string()
}

/// "Chrome 156", "Firefox 159": the browser and its major version from a
/// user agent. Edge and Opera say Chrome too, so they're looked for first;
/// Safari puts its version in `Version/`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn browser_name(user_agent: &str) -> String {
    let version = |token: &str| {
        let rest = &user_agent[user_agent.find(token)? + token.len()..];
        let major: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!major.is_empty()).then_some(major)
    };
    for (token, name) in [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
    ] {
        if let Some(v) = version(token) {
            return format!("{name} {v}");
        }
    }
    match (user_agent.contains("Safari/"), version("Version/")) {
        (true, Some(v)) => format!("Safari {v}"),
        _ => "a browser".to_string(),
    }
}

/// The fingers the page has seen (#132), for the readout: where the ones
/// down now are, in `InputState`'s 0-1 screen space, how many were ever
/// down at once and how many have touched in all. Fed once a frame.
#[derive(Default)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) struct TouchSeen {
    /// The fingers down at the last look, by id.
    down: Vec<(u64, (f64, f64))>,
    /// The last fingers that were down, kept after they lift.
    last: Vec<(f64, f64)>,
    peak: usize,
    started: usize,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
impl TouchSeen {
    /// Take the fingers down this frame.
    pub(crate) fn observe(&mut self, touches: impl IntoIterator<Item = (u64, (f64, f64))>) {
        let mut now: Vec<(u64, (f64, f64))> = touches.into_iter().collect();
        now.sort_by_key(|(id, _)| *id);
        self.started += now
            .iter()
            .filter(|(id, _)| !self.down.iter().any(|(d, _)| d == id))
            .count();
        self.peak = self.peak.max(now.len());
        if !now.is_empty() {
            self.last = now.iter().map(|(_, p)| *p).collect();
        }
        self.down = now;
    }

    /// The readout's row, once a finger has touched.
    pub(crate) fn line(&self) -> Option<String> {
        if self.started == 0 {
            return None;
        }
        let at = |ps: &[(f64, f64)]| {
            ps.iter()
                .map(|(x, y)| format!("({x:.2} {y:.2})"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let now = if self.down.is_empty() {
            format!("none down, last {}", at(&self.last))
        } else {
            let ps: Vec<_> = self.down.iter().map(|(_, p)| *p).collect();
            format!("{} down {}", self.down.len(), at(&ps))
        };
        Some(format!(
            "touch  {now}\n       most {} at once, {} in all",
            self.peak, self.started
        ))
    }

    /// The copyable line's part.
    pub(crate) fn part(&self) -> Option<String> {
        (self.started > 0).then(|| format!("touch most {} of {}", self.peak, self.started))
    }
}

/// 1, 1.5, 2.25: a pixel ratio without trailing zeros.
fn trim(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: f64) -> Duration {
        Duration::from_secs_f64(n / 1000.0)
    }

    /// Frames of `frame_ms` with `scripts_ms` of script work each.
    fn run(rec: &mut PerfRecorder, t: &mut Instant, frames: usize, frame_ms: f64, scripts_ms: f64) {
        for _ in 0..frames {
            rec.begin_frame_at(*t);
            rec.mark_at(Phase::Input, *t + ms(0.5));
            rec.mark_at(Phase::Scripts, *t + ms(0.5 + scripts_ms));
            *t += ms(frame_ms);
        }
    }

    #[test]
    fn no_summary_before_a_whole_frame() {
        let mut rec = PerfRecorder::new(false);
        assert!(rec.summary().is_none());
        rec.begin_frame();
        assert!(rec.summary().is_none());
    }

    #[test]
    fn phases_are_the_time_between_marks() {
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        run(&mut rec, &mut t, 10, 16.0, 2.0);
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert_eq!(s.frames.frames, 10);
        assert!((s.frames.p50 - 16.0).abs() < 0.01);
        let scripts = s.phases.iter().find(|p| p.phase == Phase::Scripts).unwrap();
        assert!((scripts.mean - 2.0).abs() < 0.01);
        assert!((s.cpu_mean - 2.5).abs() < 0.01);
        let physics = s.phases.iter().find(|p| p.phase == Phase::Physics).unwrap();
        assert_eq!(physics.mean, 0.0);
    }

    #[test]
    fn the_window_forgets_frames_older_than_ten_seconds() {
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        // 20 s of slow frames, then 10 s of quick ones: only the quick remain.
        run(&mut rec, &mut t, 400, 50.0, 30.0);
        run(&mut rec, &mut t, 600, 16.666, 1.0);
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert!(s.frames.max < 17.0, "{:?}", s.frames);
        assert!(s.secs <= WINDOW_SECS + 0.05);
    }

    #[test]
    fn the_worst_frame_says_how_much_was_the_players_work() {
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        run(&mut rec, &mut t, 5, 16.0, 1.0);
        // A 100 ms frame with only 1 ms of script work: time spent elsewhere.
        run(&mut rec, &mut t, 1, 100.0, 1.0);
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert!((s.worst_frame - 100.0).abs() < 0.01);
        assert!((s.worst_frame_cpu - 1.5).abs() < 0.01);
    }

    fn sample() -> (PerfSummary, PerfContext) {
        let mut rec = PerfRecorder::new(true);
        let mut t = rec.start;
        run(&mut rec, &mut t, 60, 16.7, 2.0);
        rec.begin_frame_at(t);
        let c = PerfContext {
            game: "float".into(),
            browser: "Chrome 156".into(),
            adapter: "nvidia ampere".into(),
            surface: [2560, 1440],
            dpr: 2.0,
            scene: [1920, 1080],
            scale: 0.75,
            draw_calls: 212,
            triangles: 1_234_567,
            game_time: 812.4,
            audio: Some((3, 1200, 46.4, 2.5)),
            mix: Some(flint_audio::MixCounts {
                voices: 18,
                paused_voices: 11,
                tracks: 9,
                paused_tracks: 5,
                skipped_oneshots: 2,
                stolen_oneshots: 1,
            }),
            gpu: GpuSource::Timed {
                passes: vec![("Scene", 3.1), ("SSAO", 0.8)],
                frame_ms: 3.9,
            },
            pacing: Some("30 fps, 1 in 4 of 120 Hz (auto)".into()),
        };
        (rec.summary().unwrap(), c)
    }

    #[test]
    fn the_copied_line_is_one_line_with_every_part() {
        let (s, c) = sample();
        let line = summary_line(&s, &c);
        assert!(!line.contains('\n'));
        for part in [
            "float",
            "t=812s",
            "Chrome 156",
            "2560x1440 DPR 2",
            "scene 1920x1080 scale 0.75",
            "p50 16.7",
            "p90 16.7",
            "pace 30 fps, 1 in 4 of 120 Hz (auto)",
            "scripts 2.00/2.0",
            "Scene 3.10",
            "212 draws 1.2M tris",
            "audio 3/1200 late mix 2.50 ms",
            "voices 7/18 tracks 4/9 one-shots 2 unheard 1 stolen",
        ] {
            assert!(line.contains(part), "{part:?} missing from {line}");
        }
        // A phase that never ran isn't listed.
        assert!(!line.contains("physics"), "{line}");
    }

    #[test]
    fn the_readout_says_what_share_of_the_pixels_the_scene_draws() {
        let (s, c) = sample();
        let text = readout(&s, &c);
        assert!(
            text.contains("scene  1920x1080 px, scale 0.75 (56% of the pixels)"),
            "{text}"
        );
    }

    #[test]
    fn the_slowest_scripts_are_named_worst_first() {
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        for i in 0..10 {
            rec.begin_frame_at(t);
            let spike = if i == 4 { 40.0 } else { 0.1 };
            rec.record_scripts(&[
                ("scripts/weather.rhai".to_string(), 0.5),
                ("scripts/whale.rhai".to_string(), spike),
            ]);
            t += ms(16.0);
        }
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert_eq!(s.scripts[0].name, "whale");
        assert!((s.scripts[0].max - 40.0).abs() < 0.01);
        assert_eq!(s.scripts[1].name, "weather");
        assert!((s.scripts[1].mean - 0.5).abs() < 0.01);
    }

    #[test]
    fn browsers_by_name_and_major_version() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.8078.74 Safari/537.36";
        assert_eq!(browser_name(chrome), "Chrome 156");
        assert_eq!(browser_name(&format!("{chrome} Edg/156.0.1")), "Edge 156");
        assert_eq!(
            browser_name(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:159.0) Gecko/20100101 Firefox/159.0"
            ),
            "Firefox 159"
        );
        assert_eq!(
            browser_name("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.1 Safari/605.1.15"),
            "Safari 26"
        );
        assert_eq!(browser_name(""), "a browser");
    }

    #[test]
    fn the_readout_says_where_gpu_times_come_from() {
        let (s, mut c) = sample();
        assert!(readout(&s, &c).contains("timestamp-query"));
        c.gpu = GpuSource::Unsupported;
        assert!(readout(&s, &c).contains("cpu phases only"));
        c.gpu = GpuSource::Off;
        assert!(readout(&s, &c).contains("?perf=cpu"));
        c.dpr = 1.5;
        assert!(readout(&s, &c).contains("DPR 1.5"));
    }

    #[test]
    fn touches_count_fingers_once_and_keep_the_last_after_they_lift() {
        let mut t = TouchSeen::default();
        assert_eq!(t.line(), None);
        t.observe([(7, (0.25, 0.5))]);
        t.observe([(7, (0.3, 0.5)), (9, (0.75, 0.4))]);
        t.observe([(9, (0.75, 0.4)), (7, (0.3, 0.5)), (11, (0.5, 0.9))]);
        assert_eq!(
            t.line().unwrap(),
            "touch  3 down (0.30 0.50) (0.75 0.40) (0.50 0.90)
       most 3 at once, 3 in all"
        );
        t.observe([]);
        t.observe([(12, (0.1, 0.2))]);
        t.observe([]);
        assert_eq!(
            t.line().unwrap(),
            "touch  none down, last (0.10 0.20)
       most 3 at once, 4 in all"
        );
        assert_eq!(t.part().unwrap(), "touch most 3 of 4");
    }
}
