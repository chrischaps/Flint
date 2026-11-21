//! The perf readout's numbers (#120): what each frame cost, kept for the
//! last ten seconds. The web page shows them in a corner with `?perf=1`
//! (`web_perf.rs`); this half has no browser in it, so it is tested here.
//!
//! A frame is the time from one `RedrawRequested` to the next, which on the
//! web is one `requestAnimationFrame` to the next. Inside it the player
//! marks where each phase of its own work ends ([`Phase`]); whatever the
//! frame took beyond the phases is time the main thread spent elsewhere
//! (the browser, the compositor, a wait for the next vblank).
//!
//! The frames themselves are the always-on frame history's
//! (`frame_history.rs`, #1076): the readout sums up its last ten seconds.

use super::frame_history::{Frame, FrameHistory};
use super::pacing::Spread;
use web_time::Instant;

/// How far back the readout looks.
pub(crate) const WINDOW_SECS: f64 = 10.0;

/// The player's work in a frame, in the order it runs. A mark ends the
/// phase it names and starts the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Pointer lock, pads, the clock, transitions, extensions, events
    Input,
    /// Scripts' `on_update`, and the events and `on_init`s before it
    Scripts,
    /// The fixed-step physics and the camera that follows it
    Physics,
    /// Camera overrides, `on_draw_ui` and script commands
    ScriptUi,
    /// Audio triggers, animation, particles
    AnimFx,
    /// Procgen, the renderer's sync with the world, late audio
    Sync,
    /// The wait for the next swapchain image (`AcquireFrame::acquire`):
    /// the presentation engine's, the compositor's or the GPU's time, not
    /// the player's (#1123). A full swapchain waits here for a free image.
    Acquire,
    /// The renderer: draw lists, encoding and submission
    Render,
    /// The egui pass over the frame
    Hud,
    /// Handing the frame to the swapchain
    Present,
}

impl Phase {
    pub(crate) const ALL: [Phase; 10] = [
        Phase::Input,
        Phase::Scripts,
        Phase::Physics,
        Phase::ScriptUi,
        Phase::AnimFx,
        Phase::Sync,
        Phase::Acquire,
        Phase::Render,
        Phase::Hud,
        Phase::Present,
    ];
    pub(crate) const COUNT: usize = Self::ALL.len();

    pub(crate) fn label(self) -> &'static str {
        match self {
            Phase::Input => "input",
            Phase::Scripts => "scripts",
            Phase::Physics => "physics",
            Phase::ScriptUi => "script ui",
            Phase::AnimFx => "anim/fx",
            Phase::Sync => "sync",
            Phase::Acquire => "acquire",
            Phase::Render => "render",
            Phase::Hud => "hud",
            Phase::Present => "present",
        }
    }

    /// A wait on the swapchain rather than the player's own work: the
    /// acquire and the present. `Frame::player_ms` leaves them out, and a
    /// hitch they hold the excess of is `elsewhere` (#1123).
    pub(crate) fn is_wait(self) -> bool {
        matches!(self, Phase::Acquire | Phase::Present)
    }
}

/// A phase's time over the window: the mean frame's, and the worst.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PhaseTime {
    pub phase: Phase,
    pub mean: f32,
    pub max: f32,
}

/// A script callback over the window (`hud.rhai:on_draw_ui`, #703; or one
/// of the script system's own steps, `[load]`): the mean frame's, and the
/// worst.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScriptTime {
    pub name: String,
    pub mean: f32,
    pub max: f32,
    /// The function that took longest in one of the callback's frames in
    /// the window (#1100): a helper of the script's (`slow_helper()`) or
    /// an engine function (`raycast()`), with its calls that frame.
    pub worst_fn: Option<WorstFn>,
}

/// A callback's slowest function in one frame (#1100).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorstFn {
    /// `slow_helper()`, without the callback's own file; `raycast()`
    pub name: String,
    pub calls: u32,
    pub ms: f32,
}

impl WorstFn {
    /// `slow_helper() 99.8` or `raycast() x400 3.1`: the calls when more
    /// than one.
    fn part(&self) -> String {
        match self.calls {
            1 => format!("{} {:.1}", self.name, self.ms),
            n => format!("{} x{n} {:.1}", self.name, self.ms),
        }
    }
}

/// The slowest function seen in a callback, and when (#1100).
#[derive(Debug, Clone)]
struct SeenFn {
    callback: String,
    function: &'static str,
    calls: u32,
    ms: f32,
    at: Instant,
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
    /// What the worst frame's own work added up to (its phases but the
    /// swapchain's waits, `Frame::player_ms`): whether the longest frames
    /// were the player's own work or time spent elsewhere.
    pub worst_frame: f32,
    pub worst_frame_cpu: f32,
    /// The script callbacks with the worst single frame, worst first.
    pub scripts: Vec<ScriptTime>,
}

/// The player's frame recorder: the frame history (#1076, which it derefs
/// to) and the readout's own state. Every player has one. It records while
/// [`recording`](Self::recording): always, in a build with the
/// `frame-history` feature (the default) unless `FLINT_FRAME_HISTORY=0`;
/// otherwise only while `--perf` / `?perf=1` has the readout on, as before
/// #1076. Off, each of the frame's calls is one branch.
pub(crate) struct PerfRecorder {
    history: FrameHistory,
    /// Record every frame, readout or not (`frame-history`, and
    /// `FLINT_FRAME_HISTORY` over it).
    pub always_on: bool,
    /// The readout is shown (`--perf`, `?perf=1`): it is due twice a
    /// second, scripts are timed, and the history keeps its ten seconds.
    pub readout: bool,
    /// The renderer's GPU timing was asked for (the readout's GPU lines).
    pub gpu_wanted: bool,
    /// When the readout was last drawn, so it is drawn twice a second.
    pub shown_at: Option<Instant>,
    /// The scope recorder's look-back and its saves (#1097), while the
    /// player records: `--trace`, `--trace-on-hitch`, the `save_trace` key.
    pub(crate) trace: Option<Box<super::trace::Tracer>>,
    /// Each callback's slowest function in one frame of the window (#1100),
    /// one a callback, while scripts are timed.
    worst_fns: Vec<SeenFn>,
}

impl Default for PerfRecorder {
    /// The recorder, readout off, recording as the build says.
    fn default() -> Self {
        Self {
            history: FrameHistory::default(),
            always_on: super::frame_history::BUILT_IN,
            readout: false,
            gpu_wanted: false,
            shown_at: None,
            trace: None,
            worst_fns: Vec::new(),
        }
    }
}

impl std::ops::Deref for PerfRecorder {
    type Target = FrameHistory;
    fn deref(&self) -> &FrameHistory {
        &self.history
    }
}

impl std::ops::DerefMut for PerfRecorder {
    fn deref_mut(&mut self) -> &mut FrameHistory {
        &mut self.history
    }
}

impl PerfRecorder {
    /// A recorder with the readout on (the player turns it on in place,
    /// [`show_readout`](Self::show_readout)).
    #[cfg(test)]
    pub(crate) fn new(gpu_wanted: bool) -> Self {
        let mut rec = Self::default();
        rec.show_readout(gpu_wanted);
        rec
    }

    /// A recorder, readout off, recording every frame or not.
    pub(crate) fn recording_always(always_on: bool) -> Self {
        Self {
            always_on,
            ..Self::default()
        }
    }

    /// Whether frames are being recorded: always on, or the readout is.
    pub(crate) fn recording(&self) -> bool {
        self.always_on || self.readout
    }

    // The frame's calls, each nothing while not recording. They shadow the
    // history's own (which tests call through the deref with times given).

    /// A new frame begins now: the one before it ends.
    pub(crate) fn begin_frame(&mut self) {
        // The engine's notes are kept on this thread only while there is a
        // record to put them on (#1077); otherwise each is a flag read
        flint_core::frame_notes::listen(self.recording());
        if self.recording() {
            let now = Instant::now();
            self.history.begin_frame_at(now);
            if let (true, Some(trace)) = (flint_core::profile::COMPILED_IN, &mut self.trace) {
                trace.tracks.frame_began(now);
            }
        }
    }

    /// The end of `phase`, now.
    pub(crate) fn mark(&mut self, phase: Phase) {
        if !self.recording() {
            return;
        }
        let now = Instant::now();
        let began = self.history.mark_at(phase, now);
        // The phase on the trace's own track (#1097), where it ran
        if let (true, Some(trace), Some(began)) =
            (flint_core::profile::COMPILED_IN, &mut self.trace, began)
        {
            trace.tracks.phase(phase, began, now);
        }
    }

    /// This frame's time in each script callback.
    pub(crate) fn record_scripts(&mut self, times: &[(String, f32)]) {
        if self.recording() {
            self.history.record_scripts(times);
        }
    }

    /// What this frame's callbacks spent in each function
    /// (`ScriptSystem::take_function_times`, #1100): each callback's
    /// slowest is kept while it is the window's slowest.
    pub(crate) fn record_functions(&mut self, times: &[flint_script::FnTime]) {
        self.record_functions_at(times, Instant::now());
    }

    fn record_functions_at(&mut self, times: &[flint_script::FnTime], now: Instant) {
        if !self.recording() {
            return;
        }
        let window = std::time::Duration::from_secs_f64(WINDOW_SECS);
        for t in times {
            let seen = SeenFn {
                callback: String::new(),
                function: t.function,
                calls: t.calls,
                ms: t.total_ms,
                at: now,
            };
            match self.worst_fns.iter_mut().find(|w| w.callback == t.callback) {
                Some(w) if w.ms <= t.total_ms || now.duration_since(w.at) > window => {
                    *w = SeenFn {
                        callback: std::mem::take(&mut w.callback),
                        ..seen
                    };
                }
                Some(_) => {}
                None => self.worst_fns.push(SeenFn {
                    callback: t.callback.clone(),
                    ..seen
                }),
            }
        }
    }

    /// `callback`'s slowest function in the window, named without the
    /// callback's own file.
    fn worst_fn(&self, callback: &str, now: Instant) -> Option<WorstFn> {
        let window = std::time::Duration::from_secs_f64(WINDOW_SECS);
        let w = self
            .worst_fns
            .iter()
            .find(|w| w.callback == callback && now.duration_since(w.at) <= window)?;
        let file = callback.rsplit_once(':').map_or("", |(file, _)| file);
        let name = w
            .function
            .strip_prefix(file)
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(w.function);
        Some(WorstFn {
            name: name.to_string(),
            calls: w.calls,
            ms: w.ms,
        })
    }

    /// `present()` returned at `now`.
    pub(crate) fn presented_at(&mut self, now: Instant) {
        if self.recording() {
            self.history.presented_at(now);
        }
    }

    /// The frame's swapchain image, its wait timed as [`Phase::Acquire`]
    /// (#1123). What the frame did before it since the last mark (a look's
    /// fade, the render scale) is the renderer's, so `Render` is marked
    /// first; the encoding after it adds to `Render` at its own mark. A
    /// failed acquire (a timeout after its wait) is timed the same.
    pub(crate) fn acquire<S: flint_render::AcquireFrame + ?Sized>(
        &mut self,
        surface: &S,
    ) -> Result<wgpu::SurfaceTexture, flint_render::SurfaceError> {
        self.mark(Phase::Render);
        let frame = surface.acquire();
        self.mark(Phase::Acquire);
        frame
    }

    /// The fixed physics steps that ran this frame.
    pub(crate) fn fixed_steps(&mut self, steps: u32) {
        if self.recording() {
            self.history.fixed_steps(steps);
        }
    }

    /// A GPU time read back this frame, for the frame numbered `index`
    /// ([`FrameHistory::gpu_time`], #1104).
    pub(crate) fn gpu_time(&mut self, index: Option<u64>, ms: f32) {
        if self.recording() {
            self.history.gpu_time(index, ms);
        }
    }

    /// The number a frame resolved now carries for the history: the frame
    /// under way's while recording, else none, so a time resolved while the
    /// history slept isn't laid on a frame it never timed.
    pub(crate) fn gpu_frame_index(&self) -> Option<u64> {
        self.recording().then(|| self.history.under_way_index())
    }

    /// Turn the readout on, from this frame: the frames already kept stay.
    pub(crate) fn show_readout(&mut self, gpu_wanted: bool) {
        self.readout = true;
        self.gpu_wanted = gpu_wanted;
        self.history.keep_secs(Some(WINDOW_SECS));
    }

    /// The readout's frames: the last [`WINDOW_SECS`] of the history.
    fn window(&self) -> impl Iterator<Item = &Frame> + Clone {
        let frames = self.history.frames();
        let horizon = frames.back().map_or(0.0, |f| f.at - WINDOW_SECS);
        frames.iter().filter(move |f| f.at >= horizon)
    }

    /// The window so far, or `None` before the first whole frame.
    pub(crate) fn summary(&self) -> Option<PerfSummary> {
        let now = Instant::now();
        let window = self.window();
        let ms: Vec<f32> = window.clone().map(|f| f.ms).collect();
        let frames = Spread::of(&ms)?;
        let n = ms.len() as f32;
        let phases: Vec<PhaseTime> = Phase::ALL
            .iter()
            .map(|&phase| {
                let i = phase as usize;
                let (sum, max) = window.clone().fold((0.0f32, 0.0f32), |(s, m), f| {
                    (s + f.phases[i], m.max(f.phases[i]))
                });
                PhaseTime {
                    phase,
                    mean: sum / n,
                    max,
                }
            })
            .collect();
        let worst = window.clone().max_by(|a, b| a.ms.total_cmp(&b.ms))?;
        let mut scripts: Vec<ScriptTime> = self
            .script_names()
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let (sum, max) = window.clone().fold((0.0f32, 0.0f32), |(s, m), f| {
                    let ms = f.scripts.get(i).copied().unwrap_or(0.0);
                    (s + ms, m.max(ms))
                });
                ScriptTime {
                    name: script_name(path),
                    mean: sum / n,
                    max,
                    worst_fn: self.worst_fn(path, now),
                }
            })
            .filter(|s| s.max > 0.0)
            .collect();
        scripts.sort_by(|a, b| b.max.total_cmp(&a.max));
        scripts.truncate(SCRIPTS_SHOWN);
        let secs = window.map(|f| f.ms as f64).sum::<f64>() / 1000.0;
        Some(PerfSummary {
            frames,
            secs,
            cpu_mean: phases.iter().map(|p| p.mean).sum(),
            phases,
            worst_frame: worst.ms,
            worst_frame_cpu: worst.player_ms(),
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
    /// The swapchain's present mode, as wgpu resolved it (#380); `None`
    /// where the player has no surface to ask.
    pub present: Option<wgpu::PresentMode>,
    /// The frame-rate pacing (#140), when the web player paces.
    pub pacing: Option<String>,
    /// The WebGPU calls a frame made, on the web (#139).
    pub webgpu: Option<WebGpuCalls>,
    /// The GPU objects the renderer created and the bytes it wrote a
    /// frame, counted by flint-render itself on every platform (#1091);
    /// `None` until a window has a reading before it.
    pub gpu_calls: Option<GpuCallsPerFrame>,
    /// The renderer's texture memory by class and its mesh buffers, and
    /// the device's own `(buffers, textures)` where wgpu tracks them.
    pub gpu_memory: Option<(flint_render::gpu_counts::GpuMemory, Option<(u64, u64)>)>,
    /// The hitches in the window and the run, and the window's worst with
    /// its cause (#1079); `None` while frames aren't judged.
    pub hitches: Option<super::hitch_watch::HitchReadout>,
    /// The display's refresh period, ms, when it is known: the monitor's
    /// under vsync on the desktop, the pacer's on the web, or read from an
    /// unpaced page's rAF intervals (#1318). It tells a frame the display
    /// holds from one the GPU holds.
    pub refresh_ms: Option<f32>,
}

/// The mean frame's calls into WebGPU, counted by the page (`?perf`
/// wraps them, index.html): each crosses from wasm into JS (#139).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct WebGpuCalls {
    pub submits: f32,
    pub writes: f32,
    /// What the writes carried, bytes.
    pub write_bytes: f32,
    pub bind_groups: f32,
    pub buffers: f32,
    pub passes: f32,
}

impl WebGpuCalls {
    /// The calls between two of the page's running totals, per frame.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn per_frame(now: &Self, then: &Self, frames: u64) -> Option<Self> {
        if frames == 0 {
            return None;
        }
        let n = frames as f32;
        let d = |a: f32, b: f32| (a - b).max(0.0) / n;
        Some(Self {
            submits: d(now.submits, then.submits),
            writes: d(now.writes, then.writes),
            write_bytes: d(now.write_bytes, then.write_bytes),
            bind_groups: d(now.bind_groups, then.bind_groups),
            buffers: d(now.buffers, then.buffers),
            passes: d(now.passes, then.passes),
        })
    }
}

/// The mean frame's GPU objects created and uploads, as flint-render
/// counts them (#1091, `flint_render::gpu_counts`): the native twin of
/// [`WebGpuCalls`], which on the web reads the same frames from the page.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct GpuCallsPerFrame {
    pub buffers: f32,
    pub bind_groups: f32,
    pub textures: f32,
    /// Render and compute pipelines and shader modules.
    pub pipelines: f32,
    pub writes: f32,
    /// What the buffer and texture writes carried, bytes.
    pub write_bytes: f32,
    /// Frames of the window that created any object.
    pub frames_creating: u64,
}

impl GpuCallsPerFrame {
    /// Every object created a frame.
    pub(crate) fn created(&self) -> f32 {
        self.buffers + self.bind_groups + self.textures + self.pipelines
    }

    /// The calls between two of the renderer's running totals, per frame.
    pub(crate) fn per_frame(
        now: &flint_render::gpu_counts::GpuCalls,
        then: &flint_render::gpu_counts::GpuCalls,
        frames: u64,
        frames_creating: u64,
    ) -> Option<Self> {
        if frames == 0 {
            return None;
        }
        let d = now.since(then);
        let n = frames as f32;
        Some(Self {
            buffers: d.buffers as f32 / n,
            bind_groups: d.bind_groups as f32 / n,
            textures: d.textures as f32 / n,
            pipelines: (d.pipelines + d.shader_modules) as f32 / n,
            writes: (d.writes + d.texture_writes) as f32 / n,
            write_bytes: (d.write_bytes + d.texture_write_bytes) as f32 / n,
            frames_creating,
        })
    }
}

/// The renderer's running reading as a readout keeps it (#1091): the
/// totals, the frames submitted and the frames that created any by then.
pub(crate) type GpuCallsReading = (flint_render::gpu_counts::GpuCalls, u64, u64);

/// The window from `then` to `now`, per frame (`None` at the first
/// readout); `then` becomes `now`.
pub(crate) fn gpu_calls_since(
    then: &mut Option<GpuCallsReading>,
    now: GpuCallsReading,
) -> Option<GpuCallsPerFrame> {
    let per_frame = then.as_ref().and_then(|(totals, frames, creating)| {
        GpuCallsPerFrame::per_frame(
            &now.0,
            totals,
            now.1.saturating_sub(*frames),
            now.2.saturating_sub(*creating),
        )
    });
    *then = Some(now);
    per_frame
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// `gpu/frame create 0.0 (buffer 0.0 bindgroup 0.0 texture 0.0 pipeline
/// 0.0) in 0 frames write 15.0 (4.0 KB)`, the copied line's part (#1091).
fn gpu_calls_part(g: &GpuCallsPerFrame) -> String {
    format!(
        "gpu/frame create {:.1} (buffer {:.1} bindgroup {:.1} texture {:.1} pipeline {:.1}) in {} frames write {:.1} ({:.1} KB)",
        g.created(),
        g.buffers,
        g.bind_groups,
        g.textures,
        g.pipelines,
        g.frames_creating,
        g.writes,
        g.write_bytes / 1024.0
    )
}

/// `gpu mem textures 84.2 MB (scene 40.1 style 8.0 shadow 16.0 model 20.1
/// glyph 0.0) meshes 3.1 MB device 12.0/96.3 MB`.
pub(crate) fn gpu_memory_part(
    m: &flint_render::gpu_counts::GpuMemory,
    device: Option<(u64, u64)>,
) -> String {
    let mut part = format!(
        "gpu mem textures {:.1} MB (scene {:.1} style {:.1} shadow {:.1} model {:.1} glyph {:.1}) meshes {:.1} MB",
        mb(m.textures()),
        mb(m.scene_targets),
        mb(m.style_targets),
        mb(m.shadow_maps),
        mb(m.model_textures),
        mb(m.glyph_atlases),
        mb(m.mesh_buffers)
    );
    if let Some((buffers, textures)) = device {
        part.push_str(&format!(
            " device {:.1}/{:.1} MB",
            mb(buffers),
            mb(textures)
        ));
    }
    part
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
    /// Timestamp queries whose later scopes all read zero (#1316, the
    /// Pixel 8 Pro's Mali): said, never listed. `why` is
    /// [`flint_render::TimestampHealth::why`].
    Unusable { why: String },
}

/// Under vsync, a GPU busy for less than this share of the wall clock
/// idles long enough between frames to clock down (#380). How far it
/// reads high depends on the GPU and driver: in #270 (DX12, 1080p) SSAO
/// read 4.4 ms under Fifo against 0.80 with `FLINT_VSYNC=0`, about 5x; in
/// #380 (RTX 4070 laptop, Vulkan, `phase4_runtime` at 1080p, 23% busy)
/// the frame read 3.9 ms against 2.7, about 1.4x.
const IDLE_CLOCK_SHARE: f32 = 0.75;

/// What the readout calls a present mode: wgpu's name, lower case.
pub(super) fn present_name(mode: wgpu::PresentMode) -> &'static str {
    use wgpu::PresentMode as P;
    match mode {
        P::Fifo | P::AutoVsync => "fifo",
        P::FifoRelaxed => "fifo-relaxed",
        P::Mailbox => "mailbox",
        P::Immediate | P::AutoNoVsync => "immediate",
    }
}

/// True when each frame waits for a vblank: the frame rate is the
/// display's and the GPU rests in whatever the frame leaves over.
pub(super) fn under_vsync(mode: wgpu::PresentMode) -> bool {
    use wgpu::PresentMode as P;
    matches!(mode, P::Fifo | P::FifoRelaxed | P::AutoVsync)
}

/// The mean time between frames over the window, ms: the wall time a
/// frame's GPU work is set against. Not the median: on a hybrid-GPU laptop
/// Fifo lets frames through in bursts (p50 5 ms, p90 50 ms at a 60 fps
/// mean), and the GPU's share of the wall clock is what sets its clocks.
fn mean_frame_ms(s: &PerfSummary) -> f32 {
    if s.frames.frames == 0 {
        return 0.0;
    }
    (s.secs * 1000.0 / s.frames.frames as f64) as f32
}

/// Frames this many refreshes apart or more, on average, don't hold the
/// refresh: something other than the display decides when each is drawn.
/// Under it, a frame is the display's, however light the rest (#1318).
const MISSES_REFRESH: f32 = 1.15;

/// The player's own work under this share of the frame interval leaves
/// the frame waiting on something else (#1318).
const LIGHT_WORK_SHARE: f32 = 0.5;

/// The window's throughput (#1318): its frames and the mean time a frame,
/// which, unlike one frame's time, doesn't snap to whole refreshes. When the
/// GPU is what holds the frames back, this is the GPU's time a frame (#568
/// found it by counting submissions, `tools/phone-perf/submits.mjs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Throughput {
    /// The mean time between frames, ms: the window's span over its frames.
    pub ms: f32,
    /// Set when the frames look GPU-bound, with what the guess was made from.
    pub gpu_bound: Option<GpuBound>,
}

/// Why a window reads as GPU-bound: unpaced (no `?fps=` pacer), the
/// player's own work (its phases but the swapchain's waits) under half the
/// frame, and under vsync the frames missing the refresh. A guess from the
/// CPU side alone: the compositor or the browser's own main-thread work can
/// hold frames too, so the readout prints these beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GpuBound {
    /// The mean frame's own work, ms.
    pub work_ms: f32,
    /// The refresh the frames missed, ms; `None` without vsync, where the
    /// display holds nothing.
    pub refresh_ms: Option<f32>,
}

impl GpuBound {
    /// `unpaced, player 2.10 ms, refresh 8.33 ms`: the inputs.
    fn inputs(&self) -> String {
        let display = match self.refresh_ms {
            Some(r) => format!("refresh {r:.2} ms"),
            None => "no vsync".to_string(),
        };
        format!("unpaced, player {:.2} ms, {display}", self.work_ms)
    }
}

/// The window's throughput, and whether the GPU looks to be what holds it.
pub(crate) fn throughput(s: &PerfSummary, c: &PerfContext) -> Option<Throughput> {
    let ms = mean_frame_ms(s);
    if !(ms > 0.0 && ms.is_finite()) {
        return None;
    }
    let work_ms: f32 = s
        .phases
        .iter()
        .filter(|p| !p.phase.is_wait())
        .map(|p| p.mean)
        .sum();
    // A display that waits for vblank holds frames at its refresh; one that
    // doesn't (immediate, mailbox) holds nothing
    let vsync = c.present.is_none_or(under_vsync);
    let display_free = match (vsync, c.refresh_ms) {
        (false, _) => Some(None),
        (true, Some(r)) if r > 0.0 && ms >= r * MISSES_REFRESH => Some(Some(r)),
        (true, _) => None,
    };
    let gpu_bound = match display_free {
        Some(refresh_ms) if c.pacing.is_none() && work_ms < ms * LIGHT_WORK_SHARE => {
            Some(GpuBound {
                work_ms,
                refresh_ms,
            })
        }
        _ => None,
    };
    Some(Throughput { ms, gpu_bound })
}

/// The share of the wall clock the GPU was busy, when its times were
/// taken under vsync with headroom, so at idle clocks: they read higher
/// than the passes cost at full clocks (#380). `None` without vsync, GPU
/// times, or headroom.
fn idle_clock_share(s: &PerfSummary, c: &PerfContext) -> Option<f32> {
    let GpuSource::Timed { frame_ms, .. } = &c.gpu else {
        return None;
    };
    idle_share(c.present, *frame_ms, mean_frame_ms(s))
}

/// The GPU's share of a `frame_ms` frame when it spent `gpu_ms` of it
/// under vsync with headroom (below [`IDLE_CLOCK_SHARE`]).
fn idle_share(present: Option<wgpu::PresentMode>, gpu_ms: f32, frame_ms: f32) -> Option<f32> {
    if !present.is_some_and(under_vsync) || frame_ms <= 0.0 {
        return None;
    }
    let share = gpu_ms / frame_ms;
    (share < IDLE_CLOCK_SHARE).then_some(share)
}

/// What follows a GPU time on one line: whether it was taken under vsync,
/// and at idle clocks when the GPU had headroom (#380). Empty when the
/// present mode isn't known. `--perf`'s line and `--exit-after`'s
/// `[perf] avg GPU` both carry it.
pub(super) fn gpu_vsync_note(
    present: Option<wgpu::PresentMode>,
    gpu_ms: f32,
    frame_ms: f32,
) -> String {
    match (present, idle_share(present, gpu_ms, frame_ms)) {
        (_, Some(share)) => format!(
            " under vsync at idle clocks ({:.0}% busy, reads high; FLINT_VSYNC=0 for pass costs)",
            share * 100.0
        ),
        (Some(mode), None) if under_vsync(mode) => " under vsync".to_string(),
        (Some(_), None) => " no vsync".to_string(),
        (None, None) => String::new(),
    }
}

/// The readout's present line: the mode, and whether it waits for vblank.
fn present_line(mode: wgpu::PresentMode) -> String {
    use wgpu::PresentMode as P;
    let what = match mode {
        P::Fifo | P::FifoRelaxed | P::AutoVsync => "vsync",
        P::Mailbox => "no vsync, newest frame each refresh",
        P::Immediate | P::AutoNoVsync => "no vsync",
    };
    format!("{} ({what})", present_name(mode))
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

/// What the readout and the copied line say in place of pass times that
/// read zero (#1316).
const UNAVAILABLE: &str = flint_render::TIMESTAMPS_UNAVAILABLE;

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
    // The mean frame, which doesn't snap to refreshes, and whether it's the
    // GPU's (#1318)
    if let Some(t) = throughput(s, c) {
        match t.gpu_bound {
            Some(g) => {
                line(format!("mean   {:.2} ms a frame: GPU-bound, a guess", t.ms));
                line(format!("       from {}", g.inputs()));
            }
            None => line(format!("mean   {:.2} ms a frame", t.ms)),
        }
    }
    if let Some(pacing) = &c.pacing {
        line(format!("pace   {pacing}"));
    }
    if let Some(mode) = c.present {
        line(format!("present {}", present_line(mode)));
    }
    line(format!(
        "worst  {:.1} ms, {:.1} of it the player's",
        s.worst_frame, s.worst_frame_cpu
    ));
    if let Some(h) = &c.hitches {
        line(h.rows());
    }
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
    if let Some(w) = &c.webgpu {
        line(format!(
            "webgpu {:.1} submits, {:.1} passes, {:.1} bind groups a frame",
            w.submits, w.passes, w.bind_groups
        ));
        line(format!(
            "       {:.1} writes ({:.1} KB), {:.1} buffers",
            w.writes,
            w.write_bytes / 1024.0,
            w.buffers
        ));
    }
    // The renderer's own count (#1091): objects made a frame, and writes
    if let Some(g) = &c.gpu_calls {
        line(format!(
            "alloc  {:.1} objects a frame ({:.1} buffers, {:.1} bind groups, {:.1} textures, {:.1} pipelines), in {} frames",
            g.created(),
            g.buffers,
            g.bind_groups,
            g.textures,
            g.pipelines,
            g.frames_creating
        ));
        line(format!(
            "       {:.1} writes ({:.1} KB) a frame",
            g.writes,
            g.write_bytes / 1024.0
        ));
    }
    if let Some((m, device)) = &c.gpu_memory {
        line(format!(
            "memory {:.1} MB textures: scene {:.1}, style {:.1}, shadow {:.1}, model {:.1}, glyph {:.1}",
            mb(m.textures()),
            mb(m.scene_targets),
            mb(m.style_targets),
            mb(m.shadow_maps),
            mb(m.model_textures),
            mb(m.glyph_atlases)
        ));
        let mut meshes = format!("       {:.1} MB meshes", mb(m.mesh_buffers));
        if let Some((buffers, textures)) = device {
            meshes.push_str(&format!(
                "; device {:.1} MB buffers, {:.1} MB textures",
                mb(*buffers),
                mb(*textures)
            ));
        }
        line(meshes);
    }
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
        line("scripts                    mean   max   (worst first)".to_string());
        for sc in &s.scripts {
            line(format!("  {:<24}{:5.2} {:5.1}", sc.name, sc.mean, sc.max));
            // Where its worst frame went (#1100)
            if let Some(f) = &sc.worst_fn {
                line(format!("    in {}", f.part()));
            }
        }
    }
    line(String::new());
    match &c.gpu {
        GpuSource::Off => line("gpu ms   off (?perf=cpu)".to_string()),
        GpuSource::Unsupported => {
            line("gpu ms   no timestamp-query here: cpu phases only".to_string())
        }
        GpuSource::Unusable { why } => {
            line(format!("gpu ms   {UNAVAILABLE}"));
            line(format!("  ({why})"));
        }
        GpuSource::Timed { passes, frame_ms } => {
            let vsync = if c.present.is_some_and(under_vsync) {
                ", vsync"
            } else {
                ""
            };
            line(format!(
                "gpu ms   timestamp-query ({frame_ms:.2} a frame{vsync})"
            ));
            for (label, ms) in passes {
                line(format!("  {label:<12}{ms:5.2}"));
            }
            if let Some(share) = idle_clock_share(s, c) {
                line(format!(
                    "  at idle clocks: busy {:.0}% of a {:.1} ms frame,",
                    share * 100.0,
                    mean_frame_ms(s)
                ));
                line("  so these read high (1.4x to 5x measured).".to_string());
                line("  Compare passes with FLINT_VSYNC=0 or".to_string());
                line("  flint render --gpu-time".to_string());
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
    // After the frame times: their mean, and whether it's the GPU's (#1318)
    if let Some(t) = throughput(s, c) {
        parts.insert(
            5,
            match t.gpu_bound {
                Some(g) => format!(
                    "mean {:.2} ms a frame, GPU-bound (guess: {})",
                    t.ms,
                    g.inputs()
                ),
                None => format!("mean {:.2} ms a frame", t.ms),
            },
        );
    }
    if let Some(pacing) = &c.pacing {
        parts.insert(parts.len() - 1, format!("pace {pacing}"));
    }
    // Beside the frame times: how many of them stood out, and why (#1079)
    if let Some(h) = &c.hitches {
        let at = parts.len() - 1;
        parts.insert(at, h.part());
    }
    if let Some(mode) = c.present {
        parts.push(format!("present {}", present_name(mode)));
    }
    if !s.scripts.is_empty() {
        parts.push(format!(
            "worst scripts: {}",
            s.scripts
                .iter()
                .map(|sc| match &sc.worst_fn {
                    // The callback's slowest function in a frame (#1100)
                    Some(f) => format!("{} {:.2}/{:.1} ({})", sc.name, sc.mean, sc.max, f.part()),
                    None => format!("{} {:.2}/{:.1}", sc.name, sc.mean, sc.max),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match &c.gpu {
        GpuSource::Off => parts.push("gpu off".into()),
        GpuSource::Unsupported => parts.push("gpu untimed".into()),
        GpuSource::Unusable { why } => parts.push(format!("{UNAVAILABLE} ({why})")),
        GpuSource::Timed { passes, frame_ms } => parts.push(format!(
            "gpu {frame_ms:.2} ms{}: {}",
            gpu_vsync_note(c.present, *frame_ms, mean_frame_ms(s)),
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
    if let Some(w) = &c.webgpu {
        parts.push(format!(
            "webgpu/frame submit {:.1} pass {:.1} bindgroup {:.1} write {:.1} ({:.1} KB) buffer {:.1}",
            w.submits,
            w.passes,
            w.bind_groups,
            w.writes,
            w.write_bytes / 1024.0,
            w.buffers
        ));
    }
    // What the renderer counted itself (#1091), on every platform
    if let Some(g) = &c.gpu_calls {
        parts.push(gpu_calls_part(g));
    }
    if let Some((m, device)) = &c.gpu_memory {
        parts.push(gpu_memory_part(m, *device));
    }
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
        self.stats.perf.show_readout(gpu);
        // Each load says where its time went too (#1092)
        self.launch.load_report = true;
        self.core.script.set_timing(true);
    }

    /// The end of `phase` in this frame (the frame history, #1076).
    pub(super) fn perf_mark(&mut self, phase: Phase) {
        self.stats.perf.mark(phase);
    }

    /// Whether the readout is due again: at most once every `every`
    /// seconds. Never while not recording.
    pub(super) fn perf_due(&mut self, every: f64) -> bool {
        let perf = &mut self.stats.perf;
        if !perf.readout {
            return false;
        }
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

    /// Take the first reading as soon as there is a renderer, so the first
    /// readout's window has one before it (every frame until then; one
    /// `is_some` after).
    pub(super) fn seed_gpu_calls(&mut self) {
        if self.stats.gpu_calls_then.is_none() {
            if let Some(r) = &self.scene_renderer {
                self.stats.gpu_calls_then = Some(r.gpu_calls_reading());
            }
        }
    }

    /// The renderer's GPU objects and writes a frame since the last
    /// readout (#1091), from the first frame for the first.
    pub(super) fn window_gpu_calls(&mut self) -> Option<GpuCallsPerFrame> {
        self.seed_gpu_calls();
        let now = self.scene_renderer.as_ref()?.gpu_calls_reading();
        gpu_calls_since(&mut self.stats.gpu_calls_then, now)
    }

    /// The window's frames and everything else the readout shows, with the
    /// browser and adapter as the platform names them.
    pub(super) fn perf_snapshot(
        &self,
        browser: String,
        adapter: String,
    ) -> Option<(PerfSummary, PerfContext)> {
        let perf = &self.stats.perf;
        if !perf.readout {
            return None;
        }
        let summary = perf.summary()?;
        let renderer = self.scene_renderer.as_ref()?;
        let context = self.render_context.as_ref()?;
        let stats = renderer.collect_stats();
        let gpu = if !perf.gpu_wanted {
            GpuSource::Off
        } else if !renderer.gpu_timing() {
            GpuSource::Unsupported
        } else if let Some(why) = stats.gpu_timestamps.why() {
            GpuSource::Unusable { why }
        } else {
            GpuSource::Timed {
                passes: stats.gpu_passes.iter().map(|p| (p.label, p.ms)).collect(),
                frame_ms: stats.gpu_frame_ms,
            }
        };
        let game = std::path::Path::new(&self.core.scene_path)
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
                game_time: self.core.clock.total_time,
                audio: self
                    .audio
                    .engine
                    .buffer_stats()
                    .map(|b| (b.late, b.buffers, b.buffer_ms, b.mix_mean_ms)),
                mix: self.audio.mix_counts(),
                gpu,
                present: Some(context.present_mode_in_use()),
                pacing: self.frame_rate_line(),
                webgpu: None,
                // The window's calls are the caller's, which keeps the
                // reading before it (`gpu_calls_since`)
                gpu_calls: None,
                gpu_memory: Some((
                    stats.gpu_memory,
                    flint_render::gpu_counts::device_memory(&context.device),
                )),
                hitches: Some(super::hitch_watch::HitchReadout::of(
                    &self.stats.hitches,
                    perf,
                    WINDOW_SECS,
                )),
                // The monitor's under vsync (desktop), the pacer's (web)
                refresh_ms: self
                    .core
                    .clock
                    .vsync_period()
                    .map(|secs| (secs * 1000.0) as f32),
            },
        ))
    }

    /// `flint-player --perf`: the line a web visitor would copy, printed
    /// every ten seconds, so the desktop can be set beside the browser.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn print_perf(&mut self) {
        self.seed_gpu_calls();
        if !self.perf_due(WINDOW_SECS) {
            return;
        }
        let adapter = self
            .render_context
            .as_ref()
            .map(|c| {
                // DX12 names its shader compiler (#912): `(Dx12, DXC)`
                flint_render::gpu_select::adapter_short(&c.adapter.get_info())
            })
            .unwrap_or_default();
        if let Some((summary, mut context)) = self.perf_snapshot("native".into(), adapter) {
            context.gpu_calls = self.window_gpu_calls();
            // A window that began before the first readout is short; skip it.
            if summary.secs >= WINDOW_SECS * 0.9 {
                let line = summary_line(&summary, &context);
                // Android has no stderr: the line goes to logcat, and to
                // FLINT_PERF_LOG if set, since a device's log can drop it.
                #[cfg(target_os = "android")]
                log::info!("{line}");
                if let Some(path) = flint_core::env::PERF_LOG.path() {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                    {
                        let _ = writeln!(f, "{line}");
                    }
                }
                tracing::info!(target: "perf", "{line}");
            }
        }
    }
}

/// `scripts/biolum.rhai:on_update` -> `biolum.rhai:on_update`: the file
/// without its folder, and the callback (#703).
pub(super) fn script_name(label: &str) -> String {
    label
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(label)
        .to_string()
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
            present: Some(wgpu::PresentMode::Fifo),
            pacing: Some("30 fps, 1 in 4 of 120 Hz (auto)".into()),
            webgpu: Some(WebGpuCalls {
                submits: 1.0,
                writes: 12.5,
                write_bytes: 4096.0,
                bind_groups: 2.0,
                buffers: 0.0,
                passes: 14.0,
            }),
            hitches: Some(super::super::hitch_watch::HitchReadout {
                secs: 10.0,
                window: 2,
                run: 5,
                worst: Some((84.2, "script whale.rhai:on_update 80.1 ms".into())),
            }),
            refresh_ms: Some(1000.0 / 60.0),
            gpu_calls: Some(GpuCallsPerFrame {
                buffers: 0.0,
                bind_groups: 0.5,
                textures: 0.0,
                pipelines: 0.0,
                writes: 15.0,
                write_bytes: 4096.0,
                frames_creating: 2,
            }),
            gpu_memory: Some((
                flint_render::gpu_counts::GpuMemory {
                    scene_targets: 40 << 20,
                    style_targets: 8 << 20,
                    shadow_maps: 16 << 20,
                    model_textures: 20 << 20,
                    glyph_atlases: 0,
                    mesh_buffers: 3 << 20,
                },
                Some((12 << 20, 96 << 20)),
            )),
        };
        (rec.summary().unwrap(), c)
    }

    /// 10 s of frames `frame_ms` apart, each with `work_ms` of the player's
    /// own work and `acquire_ms` waiting for a swapchain image.
    fn window_of(frame_ms: f64, work_ms: f64, acquire_ms: f64) -> PerfSummary {
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        let frames = (10_000.0 / frame_ms) as usize;
        for _ in 0..frames {
            rec.begin_frame_at(t);
            rec.mark_at(Phase::Input, t + ms(work_ms / 2.0));
            rec.mark_at(Phase::Acquire, t + ms(work_ms / 2.0 + acquire_ms));
            rec.mark_at(Phase::Render, t + ms(work_ms + acquire_ms));
            t += ms(frame_ms);
        }
        rec.begin_frame_at(t);
        rec.summary().unwrap()
    }

    /// The phone #568 measured: unpaced (`fps=native`) at 120 Hz, the ASCII
    /// page 340 frames in 10 s (29.4 ms a frame), 2 ms of the player's work.
    fn phone() -> PerfContext {
        PerfContext {
            present: Some(wgpu::PresentMode::Fifo),
            refresh_ms: Some(1000.0 / 120.0),
            ..PerfContext::default()
        }
    }

    /// #1318: unpaced, light on the CPU and missing the refresh, the mean
    /// frame is the GPU's, and the line says what the guess was made from.
    #[test]
    fn an_unpaced_frame_light_on_the_cpu_reads_gpu_bound() {
        let s = window_of(29.4, 2.0, 0.0);
        let c = phone();
        let t = throughput(&s, &c).unwrap();
        assert!((t.ms - 29.4).abs() < 0.05, "{t:?}");
        let g = t.gpu_bound.expect("GPU-bound");
        assert!((g.work_ms - 2.0).abs() < 0.01);
        let line = summary_line(&s, &c);
        assert!(
            line.contains(
                " | mean 29.40 ms a frame, GPU-bound (guess: unpaced, player 2.00 ms, refresh 8.33 ms) | "
            ),
            "{line}"
        );
        let text = readout(&s, &c);
        assert!(
            text.contains(
                "mean   29.40 ms a frame: GPU-bound, a guess\n       from unpaced, player 2.00 ms, refresh 8.33 ms"
            ),
            "{text}"
        );
    }

    /// The desktop's GPU-bound frame waits in `acquire` for a free image: a
    /// wait, not the player's work, so it doesn't make the frame CPU-bound.
    /// Without vsync no refresh holds anything, so none is needed.
    #[test]
    fn a_wait_for_a_swapchain_image_is_not_the_players_work() {
        let s = window_of(12.0, 1.0, 9.0);
        let c = PerfContext {
            present: Some(wgpu::PresentMode::Immediate),
            ..PerfContext::default()
        };
        let line = summary_line(&s, &c);
        assert!(
            line.contains(
                "mean 12.00 ms a frame, GPU-bound (guess: unpaced, player 1.00 ms, no vsync)"
            ),
            "{line}"
        );
    }

    /// The flag is absent on a CPU-bound frame, a paced one, one that holds
    /// the refresh, and under vsync with no refresh known; the mean stays.
    #[test]
    fn a_cpu_bound_paced_or_refresh_held_frame_has_no_flag() {
        let bare = |s: &PerfSummary, c: &PerfContext| {
            let line = summary_line(s, c);
            assert!(!line.contains("GPU-bound"), "{line}");
            assert!(!readout(s, c).contains("GPU-bound"));
            line
        };
        // CPU-bound: 20 of the frame's 29.4 ms is the player's
        let line = bare(&window_of(29.4, 20.0, 0.0), &phone());
        assert!(line.contains(" | mean 29.40 ms a frame | "), "{line}");
        // Paced (`?fps=auto` or a fixed rate) at 30 fps
        let mut paced = phone();
        paced.pacing = Some("30 fps, 1 in 4 of 120 Hz (auto)".into());
        bare(&window_of(33.3, 2.0, 0.0), &paced);
        // Holds 60 Hz: the display's frame, however light the GPU
        let mut sixty = phone();
        sixty.refresh_ms = Some(1000.0 / 60.0);
        let line = bare(&window_of(1000.0 / 60.0, 2.0, 0.0), &sixty);
        assert!(line.contains(" | mean 16.67 ms a frame | "), "{line}");
        // Under vsync with the refresh unknown, there's nothing to judge by
        let mut unknown = phone();
        unknown.refresh_ms = None;
        bare(&window_of(29.4, 2.0, 0.0), &unknown);
    }

    /// #1091: the renderer's own count beside the page's, in the copied
    /// line and the readout, and the memory by class.
    #[test]
    fn the_line_and_the_readout_carry_the_renderers_gpu_count_and_memory() {
        let (s, c) = sample();
        let line = summary_line(&s, &c);
        assert!(
            line.contains(
                " | gpu/frame create 0.5 (buffer 0.0 bindgroup 0.5 texture 0.0 pipeline 0.0) in 2 frames write 15.0 (4.0 KB) | "
            ),
            "{line}"
        );
        assert!(
            line.contains(
                " | gpu mem textures 84.0 MB (scene 40.0 style 8.0 shadow 16.0 model 20.0 glyph 0.0) meshes 3.0 MB device 12.0/96.0 MB | "
            ),
            "{line}"
        );
        let rows = readout(&s, &c);
        assert!(
            rows.contains("alloc  0.5 objects a frame (0.0 buffers, 0.5 bind groups"),
            "{rows}"
        );
        assert!(
            rows.contains("memory 84.0 MB textures: scene 40.0, style 8.0"),
            "{rows}"
        );
        assert!(
            rows.contains("; device 12.0 MB buffers, 96.0 MB textures"),
            "{rows}"
        );
    }

    /// #1091: a window is the difference of two renderer readings, and
    /// the first readout has none before it.
    #[test]
    fn a_readouts_gpu_calls_are_the_window_since_the_last() {
        use flint_render::gpu_counts::GpuCalls;
        let mut then = None;
        let at = |buffers, bind_groups, writes| GpuCalls {
            buffers,
            bind_groups,
            writes,
            ..Default::default()
        };
        assert!(gpu_calls_since(&mut then, (at(10, 30, 100), 50, 3)).is_none());
        let w = gpu_calls_since(&mut then, (at(10, 32, 400), 70, 4)).unwrap();
        assert_eq!(w.bind_groups, 0.1, "2 bind groups over 20 frames");
        assert_eq!(w.buffers, 0.0);
        assert_eq!(w.writes, 15.0);
        assert_eq!(w.frames_creating, 1);
        // No frame between two readings is no window
        assert!(gpu_calls_since(&mut then, (at(10, 32, 400), 70, 4)).is_none());
    }

    /// #1091: reading the counts, a frame's difference and a window cost no
    /// allocation, so a steady frame's count is free of the cliff it guards.
    #[test]
    fn counting_allocates_nothing() {
        use flint_render::gpu_counts::{totals, GpuCalls};
        let mut then = Some((totals(), 1, 0));
        let ((), allocations) = super::super::alloc_count::allocations_in(|| {
            let now = totals();
            let frame = now.since(&GpuCalls::default());
            let mut sum = GpuCalls::default();
            sum += frame;
            std::hint::black_box((frame.created(), sum));
            std::hint::black_box(gpu_calls_since(&mut then, (now, 2, 0)));
        });
        assert_eq!(allocations, 0);
    }

    /// #1079: the copied line and the readout carry the hitches, so
    /// `tools/web-perf/run.mjs` reads them with no change.
    #[test]
    fn the_copied_line_and_the_readout_count_the_hitches() {
        let (s, mut c) = sample();
        let line = summary_line(&s, &c);
        assert!(
            line.contains(" | hitches 2 in 10s (5 in run), worst 84.2 ms: script whale.rhai:on_update 80.1 ms | cpu "),
            "{line}"
        );
        let text = readout(&s, &c);
        assert!(
            text.contains("hitch  2 in 10 s, 5 in the run\n       worst 84.2 ms: script whale.rhai:on_update 80.1 ms"),
            "{text}"
        );
        c.hitches = Some(super::super::hitch_watch::HitchReadout {
            secs: 10.0,
            ..Default::default()
        });
        assert!(summary_line(&s, &c).contains(" | hitches 0 in 10s (0 in run) | "));
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
            "webgpu/frame submit 1.0 pass 14.0 bindgroup 2.0 write 12.5 (4.0 KB)",
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
                ("scripts/weather.rhai:on_update".to_string(), 0.5),
                ("scripts/whale.rhai:on_update".to_string(), spike),
            ]);
            // A frame's later batch adds to what it has
            rec.record_scripts(&[("scripts/weather.rhai:on_update".to_string(), 0.25)]);
            t += ms(16.0);
        }
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert_eq!(s.scripts[0].name, "whale.rhai:on_update");
        assert!((s.scripts[0].max - 40.0).abs() < 0.01);
        assert_eq!(s.scripts[1].name, "weather.rhai:on_update");
        assert!((s.scripts[1].mean - 0.75).abs() < 0.01);
    }

    #[test]
    fn a_slow_script_names_its_slowest_function() {
        let fn_time = |function, calls, total_ms| flint_script::FnTime {
            callback: "scripts/whale.rhai:on_update".to_string(),
            function,
            calls,
            total_ms,
            worst_ms: total_ms,
        };
        let mut rec = PerfRecorder::new(false);
        let mut t = rec.start;
        let now = Instant::now();
        for i in 0..10 {
            rec.begin_frame_at(t);
            let spike = if i == 4 { 40.0 } else { 0.1 };
            rec.record_scripts(&[("scripts/whale.rhai:on_update".to_string(), spike + 3.0)]);
            rec.record_functions_at(
                &[
                    fn_time("scripts/whale.rhai:sing()", 1, spike),
                    fn_time("raycast()", 400, 3.0),
                ],
                now,
            );
            t += ms(16.0);
        }
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        let worst = s.scripts[0].worst_fn.as_ref().unwrap();
        assert_eq!(worst.name, "sing()", "named without its own file");
        assert!((worst.ms - 40.0).abs() < 0.01);
        let c = PerfContext::default();
        let line = summary_line(&s, &c);
        assert!(
            line.contains("worst scripts: whale.rhai:on_update 7.09/43.0 (sing() 40.0)"),
            "{line}"
        );

        // Once the window has passed it, the next frame's slowest is named
        let later = now + std::time::Duration::from_secs_f64(WINDOW_SECS + 1.0);
        rec.record_functions_at(&[fn_time("raycast()", 400, 3.0)], later);
        let w = rec.worst_fn("scripts/whale.rhai:on_update", later).unwrap();
        assert_eq!(w.part(), "raycast() x400 3.0");
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
    fn webgpu_calls_are_the_totals_moved_per_frame() {
        let then = WebGpuCalls {
            submits: 100.0,
            writes: 1000.0,
            write_bytes: 1e6,
            bind_groups: 50.0,
            buffers: 3.0,
            passes: 400.0,
        };
        let now = WebGpuCalls {
            submits: 130.0,
            writes: 1300.0,
            write_bytes: 1.3e6,
            bind_groups: 50.0,
            buffers: 3.0,
            passes: 820.0,
        };
        let w = WebGpuCalls::per_frame(&now, &then, 30).unwrap();
        assert_eq!(w.submits, 1.0);
        assert_eq!(w.writes, 10.0);
        assert_eq!(w.bind_groups, 0.0);
        assert_eq!(w.passes, 14.0);
        assert!(WebGpuCalls::per_frame(&now, &then, 0).is_none());
        let (s, c) = sample();
        assert!(
            readout(&s, &c).contains("webgpu 1.0 submits, 14.0 passes, 2.0 bind groups a frame"),
            "{}",
            readout(&s, &c)
        );
    }

    /// #270's numbers: under Fifo at 60 fps SSAO read 4.4 ms and Scene
    /// 1.9; with `FLINT_VSYNC=0`, 0.80 and 0.37. The line has to say which.
    #[test]
    fn a_perf_line_says_when_its_gpu_times_were_taken_under_vsync() {
        let (s, mut c) = sample();
        c.gpu = GpuSource::Timed {
            passes: vec![("Scene", 1.9), ("SSAO", 4.4)],
            frame_ms: 6.6,
        };
        let line = summary_line(&s, &c);
        assert!(line.contains("present fifo"), "{line}");
        assert!(
            line.contains("gpu 6.60 ms under vsync at idle clocks (40% busy"),
            "{line}"
        );
        assert!(line.contains("FLINT_VSYNC=0"), "{line}");
        let text = readout(&s, &c);
        assert!(text.contains("present fifo (vsync)"), "{text}");
        assert!(text.contains("(6.60 a frame, vsync)"), "{text}");
        assert!(
            text.contains("at idle clocks: busy 40% of a 16.7 ms frame"),
            "{text}"
        );

        // Unlocked: the same passes at full clocks, and no note.
        c.present = Some(wgpu::PresentMode::Immediate);
        c.gpu = GpuSource::Timed {
            passes: vec![("Scene", 0.37), ("SSAO", 0.80)],
            frame_ms: 1.2,
        };
        let line = summary_line(&s, &c);
        assert!(line.contains("present immediate"), "{line}");
        assert!(line.contains("gpu 1.20 ms no vsync: Scene 0.37"), "{line}");
        let text = readout(&s, &c);
        assert!(text.contains("present immediate (no vsync)"), "{text}");
        assert!(!text.contains("idle clocks"), "{text}");

        // A GPU that fills the vsync frame keeps its clocks up: vsync, no note.
        c.present = Some(wgpu::PresentMode::Fifo);
        c.gpu = GpuSource::Timed {
            passes: vec![("Scene", 15.0)],
            frame_ms: 15.0,
        };
        let line = summary_line(&s, &c);
        assert!(line.contains("gpu 15.00 ms under vsync: Scene"), "{line}");
        assert!(!readout(&s, &c).contains("idle clocks"));

        // Mailbox draws uncapped.
        c.present = Some(wgpu::PresentMode::Mailbox);
        assert!(readout(&s, &c).contains("present mailbox (no vsync"));
        assert!(summary_line(&s, &c).contains("gpu 15.00 ms no vsync"));
    }

    /// Fifo on a hybrid-GPU laptop lets frames through in bursts: 6, 6, 6,
    /// then 45 ms. The median frame is 6 ms, but the GPU's 4 ms are a
    /// quarter of the wall clock, and that is what clocks it down.
    #[test]
    fn idle_clocks_are_judged_against_the_wall_clock_not_the_median_frame() {
        let (_, mut c) = sample();
        let mut rec = PerfRecorder::new(true);
        let mut t = rec.start;
        for _ in 0..40 {
            run(&mut rec, &mut t, 3, 6.0, 0.5);
            run(&mut rec, &mut t, 1, 45.0, 0.5);
        }
        rec.begin_frame_at(t);
        let s = rec.summary().unwrap();
        assert!((s.frames.p50 - 6.0).abs() < 0.01);
        c.gpu = GpuSource::Timed {
            passes: vec![("Scene", 4.0)],
            frame_ms: 4.0,
        };
        let line = summary_line(&s, &c);
        assert!(line.contains("at idle clocks (25% busy"), "{line}");
    }

    /// The Pixel 8 Pro (#1316): every pass after Scene read 0.00, and #378
    /// read the list as an answer. Both forms say the times are missing
    /// and list none of them.
    #[test]
    fn unusable_timestamps_are_said_not_listed() {
        let (s, mut c) = sample();
        let real_line = summary_line(&s, &c);
        let real_text = readout(&s, &c);
        c.gpu = GpuSource::Unusable {
            why: flint_render::TimestampHealth::Unusable { zero: 5, scopes: 7 }
                .why()
                .unwrap(),
        };
        let line = summary_line(&s, &c);
        assert!(
            line.contains(
                "| GPU pass times unavailable on this adapter (5 of 7 scopes read 0 ticks for 60 frames) |"
            ),
            "{line}"
        );
        assert!(
            !line.contains("Scene 3.10") && !line.contains("gpu 3.90"),
            "{line}"
        );
        let text = readout(&s, &c);
        assert!(
            text.contains("gpu ms   GPU pass times unavailable on this adapter\n  (5 of 7"),
            "{text}"
        );
        assert!(!text.contains("timestamp-query ("), "{text}");
        // Real times are as they were
        assert!(real_line.contains("gpu 3.90 ms under vsync"), "{real_line}");
        assert!(real_line.contains("Scene 3.10, SSAO 0.80"), "{real_line}");
        assert!(
            real_text.contains("timestamp-query (3.90 a frame, vsync)"),
            "{real_text}"
        );
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
