//! Capturing a frame from the running player (#1086, ADR 0126): on a key, at
//! a game time, from an input script, and on a hitch. Each capture is a
//! `<dir>/<scene>-t<seconds>.flintframe/` folder: the renderer's description
//! of the frame and every pass's image (#1084, #1085), with the player's side
//! added (the frame history around it, the HUD pass, the scene, the game
//! time, why it was taken) and the replay inputs ADR 0126 decision 4 names
//! (`replay/world.toml`, `inputs.json`, `skinning.bin`, `particles.bin`, the
//! assets by hash).
//!
//! **One frame, never waited on.** A request waits until no capture is in
//! flight. Then, on the next frame: `sync_renderer` keeps the frame's inputs
//! ([`FrameCaptures::keep_inputs`]); `render` arms the renderer before
//! `encode_frame_to` ([`FrameCaptures::arm`]), copies the HUD's output and the
//! bone palettes into the same encoder ([`FrameCaptures::host_pass`],
//! [`FrameCaptures::record_replay`]), and after the one submission takes the
//! copies, the description and the world as text ([`FrameCaptures::submitted`]).
//! The frames after poll the copies ([`FrameCaptures::poll`]) and, once
//! they are mapped and the GPU timer has read the frame back (or
//! [`TIMING_WAIT_FRAMES`] have passed), a writer thread copies them out,
//! encodes the images, hashes the assets and writes the folder, manifest
//! last. The capturing frame and every frame until its writer starts are
//! marked `capturing` in the frame history, so the hitch detector passes
//! over them: the capture is not counted as the hitch it may be.
//!
//! **A hitch can't be captured itself.** It is over by the time the
//! detector judges it (its clock comes in on the next tick), so
//! `--capture-on-hitch` captures the frame after it and puts the history's
//! window around the hitch in the manifest's `cpu`: what the hitch cost by
//! phase is its record there, and the images show the scene a frame later.
//! The hitches of the first [`STARTUP_SECS`] of game time are passed over
//! unless `--capture-on-hitch-startup` asks for them (#1207): the exit
//! report and `--max-hitches` set them apart as loading's, and a capture
//! spent on one is a capture of the load.
//!
//! Desktop only: the web player declines with a note (no files, and no `exr`
//! in its `image`).

// The web build keeps only the triggers' decline: the rest is desktop's
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use flint_render::frame_capture::{
    CpuDescription, CpuFrame, CpuNote, FrameDescription, HitchDescription, TriggerDescription,
};
use flint_render::frame_replay::{FrameInputsState, ReplayStateCapture};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use super::frame_history::{Frame, FrameHistory};
use super::hitch::{Cause, Hitch};
use super::perf::Phase;
use crate::hitch_args::STARTUP_SECS;

/// Frames a capture waits for its frame's GPU times once its images are
/// read back, before it is written without them.
pub(crate) const TIMING_WAIT_FRAMES: u32 = 30;

/// Frames of history the manifest keeps before the captured frame (or the
/// hitch): a second at 60 fps.
pub(crate) const WINDOW_BEFORE: usize = 60;

/// Why a frame is captured.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Trigger {
    /// `--capture-frame-at <seconds>`.
    At(f64),
    /// The debug key.
    Key,
    /// An `--input-script` `capture` line at this game time.
    Script(f64),
    /// `--capture-on-hitch`: the hitch just judged.
    Hitch(Box<HitchDescription>),
}

impl Trigger {
    fn describe(&self) -> TriggerDescription {
        let (kind, detail) = match self {
            Trigger::At(t) => ("at", format!("--capture-frame-at {t}")),
            Trigger::Key => (
                "key",
                "the debug key (capture_frame, Shift+F2 unless rebound)".to_string(),
            ),
            Trigger::Script(t) => ("script", format!("input script line at {t} s")),
            Trigger::Hitch(h) => (
                "hitch",
                format!(
                    "the hitch at frame {} ({:.1} ms, {})",
                    h.frame, h.ms, h.cause
                ),
            ),
        };
        TriggerDescription {
            kind: kind.into(),
            detail,
        }
    }
}

/// A request taken for the next frame: its inputs kept at `sync_renderer`.
struct Armed {
    trigger: Trigger,
    /// The frame's inputs, kept as data when the renderer was prepared.
    inputs: Option<FrameInputsState>,
}

/// A capture whose copies are on their way back.
#[cfg(not(target_arch = "wasm32"))]
struct InFlight {
    job: Job,
    /// Frames polled since its images were ready.
    waited: u32,
    /// The GPU timer was turned on for it, to be turned off after.
    timing_turned_on: bool,
}

/// What a writer thread needs.
#[cfg(not(target_arch = "wasm32"))]
struct Job {
    dir: PathBuf,
    trigger: Trigger,
    images: flint_render::frame_capture::FrameCapture,
    replay: ReplayStateCapture,
    description: FrameDescription,
    inputs: FrameInputsState,
    camera: flint_render::frame_replay::CameraState,
    particles: flint_render::frame_replay::ParticlesState,
    world_toml: String,
    /// The scene's path, to hash.
    scene_path: PathBuf,
    /// Each model and texture the world names, to resolve and hash.
    assets: Vec<(&'static str, String)>,
    config: flint_render::model_loader::ModelLoadConfig,
    project_root: PathBuf,
    /// What a replay draws fresh, found on the frame thread.
    fresh_only: Vec<String>,
}

/// What a writer thread wrote.
#[cfg(not(target_arch = "wasm32"))]
struct Written {
    dir: PathBuf,
    line: String,
}

/// The player's frame captures (#1086): what's asked for, what's in flight,
/// what's been written. See the module docs.
#[derive(Default)]
pub(crate) struct FrameCaptures {
    /// `--capture-frame-at` times not reached yet, earliest first.
    pub at: Vec<f64>,
    /// `--capture-on-hitch`: hitches still to capture.
    pub on_hitch: u32,
    /// `--capture-on-hitch-startup`: take the hitches of the first
    /// [`STARTUP_SECS`] too. Off, they are passed over, as the exit report
    /// and `--max-hitches` set them apart (#1207): loading's noise, not the
    /// hitch someone felt mid-game.
    pub on_hitch_startup: bool,
    /// Startup hitches passed over, so the first is said once.
    passed_startup: u32,
    /// `--capture-dir`; `None` is `captures/` at the project's root.
    pub dir: Option<PathBuf>,
    queue: VecDeque<Trigger>,
    armed: Option<Armed>,
    #[cfg(not(target_arch = "wasm32"))]
    in_flight: Option<InFlight>,
    #[cfg(not(target_arch = "wasm32"))]
    writers: Vec<std::thread::JoinHandle<Result<Written, String>>>,
    /// Folder names taken this run, so two captures at one game time don't
    /// share one.
    names: Vec<String>,
    /// Captures written, by folder.
    pub(crate) written: Vec<PathBuf>,
    /// Captures that failed, each with why.
    pub(crate) failures: Vec<String>,
    /// Lines for the console and the key note: what was written, what
    /// failed. The player takes them once a frame.
    said: Vec<String>,
    /// The armed frame turned the GPU timer on.
    armed_timing: bool,
    /// The player records scopes (#1097): a capture handed to its writer
    /// names `trace.json` in its manifest, and its folder waits in
    /// `handed` for the player to write the trace into.
    pub(crate) with_trace: bool,
    pub(crate) handed: Vec<PathBuf>,
    /// The armed frame's bone palettes and renderer state, recorded last
    /// in its encoder.
    #[cfg(not(target_arch = "wasm32"))]
    armed_replay: Option<ReplayStateCapture>,
}

impl FrameCaptures {
    /// Whether anything is asked for, armed or on its way: what the frame
    /// checks before doing any of this.
    pub(crate) fn busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.in_flight.is_some() || !self.writers.is_empty() {
            return true;
        }
        !self.queue.is_empty() || self.armed.is_some()
    }

    /// The hitches asked for and not taken yet, oldest first.
    #[cfg(test)]
    pub(crate) fn queued_hitches(&self) -> Vec<&HitchDescription> {
        self.queue
            .iter()
            .filter_map(|t| match t {
                Trigger::Hitch(h) => Some(&**h),
                _ => None,
            })
            .collect()
    }

    /// Ask for the next frame to be captured. One in flight at a time: the
    /// request waits for it.
    pub(crate) fn request(&mut self, trigger: Trigger) {
        self.queue.push_back(trigger);
    }

    /// The `--capture-frame-at` times reached by `now`, each asked for once.
    pub(crate) fn times_due(&mut self, now: f64) {
        // The same tolerance as `--screenshot-at`: 120 steps of 1/60 add
        // up to a hair under 2
        while self.at.first().is_some_and(|t| now + 1e-6 >= *t) {
            let t = self.at.remove(0);
            self.request(Trigger::At(t));
        }
    }

    /// Whether this frame is captured: the next request is taken, if
    /// nothing is in flight. Call before the frame's inputs are gathered.
    pub(crate) fn take_for_frame(&mut self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.in_flight.is_some() {
            return false;
        }
        if self.armed.is_none() {
            let Some(trigger) = self.queue.pop_front() else {
                return false;
            };
            self.armed = Some(Armed {
                trigger,
                inputs: None,
            });
        }
        true
    }

    /// The frame's inputs, kept while it is armed: what `prepare` drew with.
    pub(crate) fn keep_inputs(&mut self, inputs: FrameInputsState) {
        if let Some(armed) = &mut self.armed {
            armed.inputs = Some(inputs);
        }
    }

    /// Whether the frame being drawn is captured.
    pub(crate) fn armed(&self) -> bool {
        self.armed.is_some()
    }

    /// What was written or failed since the last call, a line each.
    pub(crate) fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.said)
    }

    /// Why the run failed for its captures, if any did: a run that asks for
    /// one writes it or says why it didn't.
    pub(crate) fn failure(&self) -> Option<String> {
        if let Some(first) = self.failures.first() {
            return Some(format!(
                "{} frame capture{} failed: {first}",
                self.failures.len(),
                if self.failures.len() == 1 { "" } else { "s" }
            ));
        }
        if let Some(t) = self.at.first() {
            return Some(format!(
                "--capture-frame-at {t}: the run ended before game time reached it"
            ));
        }
        None
    }
}

// ── The frame's share, desktop ──

/// The window's targets a capture reads, and what describes them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Targets<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// The presented texture, when it can be copied (`COPY_SRC`).
    pub surface: Option<&'a wgpu::Texture>,
    pub depth: &'a wgpu::Texture,
    pub host: flint_render::FrameHost<'a>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> Targets<'a> {
    /// A window's: its swapchain texture (when copyable) and depth buffer.
    pub(crate) fn of(
        context: &'a flint_render::RenderContext,
        surface: &'a wgpu::Texture,
        adapter: &'a wgpu::AdapterInfo,
    ) -> Self {
        Self {
            device: &context.device,
            queue: &context.queue,
            surface: context.surface_copyable.then_some(surface),
            depth: &context.depth_texture,
            host: flint_render::FrameHost {
                adapter: Some(adapter),
                present_mode: Some(context.present_mode_in_use()),
            },
        }
    }
}

/// What the player knows of the frame being captured.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct FrameFacts<'a> {
    pub world: &'a flint_ecs::FlintWorld,
    pub camera: &'a flint_render::Camera,
    pub particles: Option<&'a flint_particles::ParticleSync>,
    pub scene_path: &'a str,
    pub project: &'a flint_core::project::Project,
    /// The history's number for the frame under way, when it records.
    pub index: Option<u64>,
    pub game_time: f64,
    pub dt: f64,
}

#[cfg(not(target_arch = "wasm32"))]
impl FrameCaptures {
    /// Before the frame is recorded: arm the renderer, and time this frame
    /// on the GPU where it can be timed. Returns whether it armed.
    pub(crate) fn arm(
        &mut self,
        renderer: &mut flint_render::SceneRenderer,
        targets: &Targets<'_>,
    ) -> bool {
        if self.armed.is_none() {
            return false;
        }
        renderer.capture_next_frame(flint_render::frame_capture::CaptureTargets {
            surface: targets.surface.cloned(),
            depth: Some(targets.depth.clone()),
        });
        let was_on = renderer.gpu_timing();
        let on = renderer.set_gpu_timing(targets.device, targets.queue, true);
        if let Some(timer) = renderer.gpu_timer_mut() {
            timer.record(true);
        }
        self.armed_timing = on && !was_on;
        true
    }

    /// After the host's HUD pass: log it, and copy what it left.
    pub(crate) fn host_pass(
        &self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface: &wgpu::Texture,
    ) {
        if self.armed.is_none() {
            return;
        }
        let timed = renderer.gpu_timing();
        renderer.capture_host_pass(device, encoder, "UI", "surface", surface, timed);
    }

    /// Last in the frame's encoder: the bone palettes as the frame drew
    /// with them.
    pub(crate) fn record_replay(
        &mut self,
        renderer: &flint_render::SceneRenderer,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        world: &flint_ecs::FlintWorld,
    ) {
        if self.armed.is_none() {
            return;
        }
        self.armed_replay = Some(renderer.capture_replay_state(device, encoder, world));
    }

    /// The frame is submitted: take what it copied, describe it, and keep
    /// the world as it stood. Says on screen where it will be written.
    pub(crate) fn submitted(
        &mut self,
        renderer: &mut flint_render::SceneRenderer,
        targets: &Targets<'_>,
        facts: &FrameFacts<'_>,
    ) -> Option<String> {
        let armed = self.armed.take()?;
        let replay = self.armed_replay.take();
        let timing_turned_on = std::mem::take(&mut self.armed_timing);
        let Some(mut images) = renderer.take_frame_capture() else {
            self.failures.push("the renderer made no capture".into());
            return None;
        };
        images.map();
        let mut replay = replay?;
        replay.map();
        let mut description = renderer.describe_frame(Some(facts.world), &targets.host)?;
        description.written_by = Some("flint-player".into());
        description.trigger = Some(armed.trigger.describe());
        description.frame.index = facts.index;
        description.frame.game_time = Some(facts.game_time);
        description.frame.dt = Some(facts.dt);
        description.engine = Some(flint_render::frame_capture::EngineDescription {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: None,
            dirty: None,
            subdir: None,
        });
        description.scene = Some(flint_render::frame_capture::SceneDescription {
            path: facts.scene_path.to_string(),
            name: String::new(),
            sha256: None,
        });
        if targets.surface.is_none() {
            description.notes.push(
                "the window's surface has no COPY_SRC on this adapter: the surface and UI \
                 images are missing"
                    .into(),
            );
        }

        let scene_dir = Path::new(facts.scene_path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let root = facts.project.root_for(&scene_dir);
        let base = self.dir.clone().unwrap_or_else(|| root.join("captures"));
        let dir = base.join(self.folder_name(facts.scene_path, facts.game_time));

        let world = facts.world;
        let world_toml = flint_scene::save_scene_string(world, scene_stem(facts.scene_path))
            .unwrap_or_else(|e| format!("# the world couldn't be written as a scene: {e}\n"));
        let particles = facts
            .particles
            .map(|p| flint_render::frame_replay::ParticlesState::of(p, world))
            .unwrap_or_default();
        let fresh_only = flint_render::frame_replay::fresh_only(renderer, world);
        let job = Job {
            dir: dir.clone(),
            trigger: armed.trigger,
            images,
            replay,
            description,
            inputs: armed.inputs.unwrap_or_default(),
            camera: flint_render::frame_replay::CameraState::of(facts.camera),
            particles,
            world_toml,
            scene_path: PathBuf::from(facts.scene_path),
            assets: flint_render::frame_replay::assets_named(world),
            config: flint_render::model_loader::ModelLoadConfig::in_project(
                facts.project.clone(),
                facts.scene_path,
            ),
            project_root: root,
            fresh_only,
        };
        self.in_flight = Some(InFlight {
            job,
            waited: 0,
            timing_turned_on,
        });
        Some(format!(
            "capturing the frame at t={:.2} s to {}",
            facts.game_time,
            flint_core::project::shown(&dir)
        ))
    }

    /// Once a frame: poll the capture in flight, and once it's back hand
    /// it to a writer thread with the history's window; collect writers
    /// that finished. Returns whether this frame did a capture's work (polled
    /// one, or handed one to its writer): the frame under way is then
    /// marked `capturing`, so the hand-off isn't judged a hitch either.
    pub(crate) fn poll(
        &mut self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        history: &FrameHistory,
        hitches: &std::collections::VecDeque<Hitch>,
        scripts_timed: bool,
    ) -> bool {
        self.collect_writers(false);
        let Some(flight) = &mut self.in_flight else {
            return false;
        };
        let back = flight.job.images.poll(device) & flight.job.replay.poll(device);
        if !back {
            return true;
        }
        let timings = renderer
            .gpu_timer_mut()
            .and_then(|t| t.recorded().first().cloned());
        let timer_on = renderer.gpu_timing();
        if timings.is_none() && timer_on && flight.waited < TIMING_WAIT_FRAMES {
            flight.waited += 1;
            return true;
        }
        let mut flight = self.in_flight.take().expect("checked above");
        if let Some(timer) = renderer.gpu_timer_mut() {
            timer.record(false);
        }
        if flight.timing_turned_on {
            renderer.set_gpu_timing(device, queue, false);
        }
        let job = &mut flight.job;
        match &timings {
            Some(t) => job.description.lay_gpu_times(t),
            None if timer_on => job.description.notes.push(format!(
                "the frame's GPU times didn't arrive within {TIMING_WAIT_FRAMES} frames"
            )),
            None => job
                .description
                .notes
                .push("untimed: the adapter has no TIMESTAMP_QUERY".into()),
        }
        job.description.cpu = Some(cpu_section(
            history,
            hitches,
            &job.trigger,
            job.description.frame.index,
            scripts_timed,
        ));
        if self.with_trace {
            job.description.trace = Some("trace.json".into());
            self.handed.push(job.dir.clone());
        }
        let job = flight.job;
        self.writers.push(std::thread::spawn(move || write(job)));
        // The hand-off is this frame's work too
        true
    }

    /// Wait for the capture in flight and every writer: the run is ending.
    pub(crate) fn finish(
        &mut self,
        renderer: &mut flint_render::SceneRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        history: &FrameHistory,
        hitches: &std::collections::VecDeque<Hitch>,
        scripts_timed: bool,
    ) {
        if let Some(flight) = &mut self.in_flight {
            flight.job.images.wait(device);
            flight.job.replay.wait(device);
            if let Some(timer) = renderer.gpu_timer_mut() {
                timer.flush(device);
            }
            flight.waited = TIMING_WAIT_FRAMES;
            self.poll(renderer, device, queue, history, hitches, scripts_timed);
        }
        self.collect_writers(true);
    }

    /// Join the writers that finished (all of them with `wait`).
    fn collect_writers(&mut self, wait: bool) {
        let mut i = 0;
        while i < self.writers.len() {
            if !wait && !self.writers[i].is_finished() {
                i += 1;
                continue;
            }
            let writer = self.writers.remove(i);
            match writer.join() {
                Ok(Ok(written)) => {
                    self.said.push(written.line);
                    self.written.push(written.dir);
                }
                Ok(Err(e)) => {
                    self.said.push(format!("failed: {e}"));
                    self.failures.push(e);
                }
                Err(_) => {
                    let e = "the capture's writer thread panicked".to_string();
                    self.said.push(format!("failed: {e}"));
                    self.failures.push(e);
                }
            }
        }
    }

    /// `<scene stem>-t<seconds>.flintframe`, a number added for a second
    /// capture at one game time.
    fn folder_name(&mut self, scene_path: &str, game_time: f64) -> String {
        let stem = scene_stem(scene_path);
        let base = format!("{stem}-t{game_time:.3}");
        let mut name = format!("{base}.flintframe");
        let mut n = 2;
        while self.names.contains(&name) {
            name = format!("{base}-{n}.flintframe");
            n += 1;
        }
        self.names.push(name.clone());
        name
    }
}

/// `levels/tavern_hub.scene.toml` -> `tavern_hub`.
fn scene_stem(scene_path: &str) -> &str {
    Path::new(scene_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches(".toml").trim_end_matches(".scene"))
        .filter(|n| !n.is_empty())
        .unwrap_or("scene")
}

/// The manifest's `cpu`: the history's window around the captured frame
/// (or the hitch), and the hitch when one asked.
pub(crate) fn cpu_section(
    history: &FrameHistory,
    hitches: &std::collections::VecDeque<Hitch>,
    trigger: &Trigger,
    captured: Option<u64>,
    scripts_timed: bool,
) -> CpuDescription {
    let frames = history.frames();
    if frames.is_empty() {
        return CpuDescription {
            missing: Some(
                "the frame history records nothing in this build without --perf \
                 (the `frame-history` feature, FLINT_FRAME_HISTORY=1)"
                    .into(),
            ),
            ..Default::default()
        };
    }
    let hitch = match trigger {
        Trigger::Hitch(h) => Some((**h).clone()),
        _ => None,
    };
    let around = hitch.as_ref().map(|h| h.frame).or(captured);
    let from = around.map_or(0, |a| a.saturating_sub(WINDOW_BEFORE as u64));
    let names = history.script_names();
    let hitch_frames: Vec<u64> = hitches.iter().map(|h| h.index).collect();
    let window: Vec<CpuFrame> = frames
        .iter()
        .filter(|f| f.index >= from)
        .map(|f| cpu_frame(f, names, hitch_frames.contains(&f.index)))
        .collect();
    CpuDescription {
        frame: captured.and_then(|i| window.iter().find(|f| f.index == i).cloned()),
        window,
        scripts_timed,
        hitch,
        missing: None,
    }
}

fn cpu_frame(f: &Frame, script_names: &[String], hitch: bool) -> CpuFrame {
    CpuFrame {
        index: f.index,
        game_time: f.game_time,
        ms: f.ms,
        judged_ms: f.judged_ms(),
        phases_ms: Phase::ALL
            .iter()
            .map(|&p| (p.label().to_string(), f.phases[p as usize]))
            .collect(),
        scripts_ms: f
            .scripts
            .iter()
            .enumerate()
            .filter(|(_, ms)| **ms > 0.0)
            .filter_map(|(i, ms)| Some((script_names.get(i)?.clone(), *ms)))
            .collect(),
        present_ms: f.present_ms,
        fixed_steps: f.fixed_steps,
        gpu_ms: f.gpu_ms,
        notes: f
            .notes
            .iter()
            .map(|n| CpuNote {
                kind: n.kind.label().to_string(),
                detail: n.detail.clone(),
                ms: n.ms,
                depth: n.depth,
            })
            .collect(),
        capturing: f.capturing,
        hitch,
    }
}

/// A hitch as the manifest names it.
pub(crate) fn hitch_description(h: &Hitch) -> HitchDescription {
    let (phase, phase_ms, script, script_ms) = match &h.cause {
        Cause::Player {
            phase,
            phase_ms,
            script,
        } => (
            Some(phase.label().to_string()),
            Some(*phase_ms),
            script.as_ref().map(|(n, _)| n.clone()),
            script.as_ref().map(|(_, ms)| *ms),
        ),
        Cause::Elsewhere { .. } | Cause::Tooling { .. } => (None, None, None, None),
    };
    HitchDescription {
        frame: h.index,
        game_time: h.game_time,
        ms: h.ms,
        judged_ms: h.judged_ms,
        median_ms: h.median_ms,
        class: h.cause.label().to_string(),
        cause: h.cause_line(),
        phase,
        phase_ms,
        script,
        script_ms,
    }
}

// ── The player's share ──

impl super::PlayerApp {
    /// Capture the frames at `times` (game seconds, earliest first) and
    /// after the run's first `on_hitch` hitches (past the start, unless
    /// [`capture_startup_hitches`](Self::capture_startup_hitches)), into
    /// `dir` (`None`: `captures/` at the project's root). Hitches need the frame history
    /// and named scripts, so this turns both on, as `--hitch-log` does.
    pub fn capture_frames(&mut self, times: Vec<f64>, on_hitch: u32, dir: Option<PathBuf>) {
        self.frame_captures.at = times;
        self.frame_captures.on_hitch = on_hitch;
        self.frame_captures.dir = dir;
        if on_hitch > 0 {
            self.stats.perf.always_on = true;
            self.core.script.set_timing(true);
        }
    }

    /// Whether `--capture-on-hitch` takes the hitches of the first
    /// [`STARTUP_SECS`] of game time too (`--capture-on-hitch-startup`).
    /// By default it passes over them, as the exit report and
    /// `--max-hitches` do (#1207).
    pub fn capture_startup_hitches(&mut self, take: bool) {
        self.frame_captures.on_hitch_startup = take;
    }

    /// In `tick`, once the clock and the hitch detector have run: the
    /// hitches it just found (`found`, the newest) for `--capture-on-hitch`,
    /// past the start unless `on_hitch_startup` and never the tools' own
    /// (`Cause::Tooling`), and the
    /// `--capture-frame-at` times reached.
    pub(super) fn frame_capture_triggers(&mut self, found: usize) {
        let captures = &mut self.frame_captures;
        if captures.on_hitch > 0 && found > 0 {
            let hitches = self.stats.hitches.hitches();
            for h in hitches.iter().skip(hitches.len().saturating_sub(found)) {
                if captures.on_hitch == 0 {
                    break;
                }
                // A screenshot's or the F2 overlay's own cost (#1129) is
                // not the game's hitch: a capture of it shows nothing. Nor
                // is a load behind a script's fade (#1125): it shows the fade
                if !h.is_game() {
                    continue;
                }
                if !captures.on_hitch_startup && h.game_time < STARTUP_SECS {
                    if captures.passed_startup == 0 {
                        tracing::info!(
                            target: "player",
                            "[capture] passed over a startup hitch at t={:.2} s (before t={STARTUP_SECS:.0} s; \
                             --capture-on-hitch-startup takes them)",
                            h.game_time
                        );
                    }
                    captures.passed_startup += 1;
                    continue;
                }
                captures.on_hitch -= 1;
                captures.request(Trigger::Hitch(Box::new(hitch_description(h))));
            }
        }
        captures.times_due(self.core.clock.total_time);
    }

    /// The debug key (`key`, the chord it is on) or an input script's
    /// `capture` line asks for the next frame. Says so when it can't, or
    /// when it waits.
    pub(super) fn ask_frame_capture(&mut self, trigger: Trigger, key: &str) {
        if cfg!(target_arch = "wasm32") {
            self.hud.key_note.say(
                key,
                "frame capture is desktop only: the web player writes no files",
            );
            return;
        }
        if self.scene_renderer.is_none() {
            self.hud
                .key_note
                .say(key, "no renderer active — no frame to capture");
            return;
        }
        let waits = self.frame_captures.busy();
        self.frame_captures.request(trigger);
        if waits {
            self.hud
                .key_note
                .say(key, "waits for the capture already on its way");
        }
    }

    /// What the captures said since the last frame (written, failed), on
    /// screen and on the console.
    pub(super) fn say_frame_captures(&mut self) {
        for line in self.frame_captures.take_said() {
            self.hud.key_note.say("capture", &line);
        }
    }

    /// The run is ending: write what's in flight and wait for every writer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn finish_frame_captures(&mut self) {
        if !self.frame_captures.busy() {
            return;
        }
        let device_queue = match &self.render_context {
            Some(c) => Some((&c.device, &c.queue)),
            #[cfg(test)]
            None => self.test_gpu.map(|g| (&g.device, &g.queue)),
            #[cfg(not(test))]
            None => None,
        };
        if let (Some((device, queue)), Some(renderer)) = (device_queue, &mut self.scene_renderer) {
            let timed = self.core.script.timing();
            self.frame_captures.finish(
                renderer,
                device,
                queue,
                &self.stats.perf,
                self.stats.hitches.hitches(),
                timed,
            );
        }
        self.traces_into_captures();
        for line in self.frame_captures.take_said() {
            tracing::info!(target: "player", "[capture] {line}");
        }
    }
}

// ── The writer thread ──

/// Read the copies out, write `replay/`, hash the assets, then the images
/// and the manifest (last).
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)] // a write path, into the capture's own folder
fn write(job: Job) -> Result<Written, String> {
    use flint_render::frame_capture::ReplayDescription;
    let Job {
        dir,
        trigger: _,
        images,
        replay,
        mut description,
        inputs,
        camera,
        particles,
        world_toml,
        scene_path,
        assets,
        config,
        project_root,
        fresh_only,
    } = job;
    let shown = flint_core::project::shown(&dir);
    let fail = |what: &str, e: &dyn std::fmt::Display| format!("{shown}: {what}: {e}");
    let captured = images.read().map_err(|e| fail("reading the images", &e))?;
    let (renderer, palettes) = replay
        .read()
        .map_err(|e| fail("reading the bone palettes", &e))?;

    // replay/ first: write_capture keeps it, and writes the manifest last
    let inputs = flint_render::frame_replay::ReplayInputs {
        version: 1,
        frame: inputs,
        camera,
        renderer,
        particles: particles.emitters,
    };
    let files = flint_render::frame_capture::ReplayFiles {
        world_toml: &world_toml,
        inputs: &inputs,
        skinning: &palettes.bytes,
        particles: &particles.bytes,
        history: &palettes.history,
    };
    flint_render::frame_capture::write_replay(&dir, &files)
        .map_err(|e| fail("writing replay/", &e))?;
    let listed = files.listed();
    let found = flint_render::frame_replay::hash_assets(assets, &config);
    if let Some(scene) = &mut description.scene {
        scene.sha256 = flint_render::frame_replay::hash_file(&scene_path).map(|(h, _)| h);
        // Its `[scene] name`, read here rather than on the frame thread
        scene.name = flint_core::vfs::read_to_string(&scene_path)
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|t| t.get("scene")?.get("name")?.as_str().map(str::to_string))
            .unwrap_or_default();
    }
    // The engine the binary was built from, and the project's checkout
    // beside it (#1290: the project's HEAD alone named a game's commit)
    let origin = crate::provenance::Provenance::of(&project_root);
    if let Some(engine) = &mut description.engine {
        engine.commit = origin.engine.commit;
        engine.dirty = origin.engine.dirty;
        engine.subdir = origin.engine_subdir;
    }
    let project = origin.project;
    description.project =
        project
            .commit
            .is_some()
            .then_some(flint_render::frame_capture::ProjectDescription {
                commit: project.commit,
                dirty: project.dirty,
            });
    description.captured_at = Some(utc_now());
    description.replay = Some(ReplayDescription {
        available: true,
        files: listed,
        fresh_only,
        assets: found,
    });
    // A quarter of the cores: the game plays on while it encodes
    let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).max(1));
    let written =
        flint_render::frame_capture::write_capture_on(&dir, &mut description, captured, threads)
            .map_err(|e| fail("writing", &e))?;
    let passes = description
        .all_passes()
        .iter()
        .filter(|p| !p.images.is_empty())
        .count();
    Ok(Written {
        line: format!(
            "wrote {shown}: {} images from {passes} passes, {:.1} MB",
            written.images,
            written.file_bytes as f64 / 1e6
        ),
        dir,
    })
}

/// Now, UTC, as RFC 3339 to the second.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// `secs` since the epoch as `YYYY-MM-DDTHH:MM:SSZ` (Howard Hinnant's
/// days-to-civil).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_794_787_200), "2026-11-16T00:00:00Z");
        assert_eq!(rfc3339(951_782_400 + 3661), "2000-02-29T01:01:01Z");
    }

    #[test]
    fn times_are_asked_for_once_each_as_they_are_reached() {
        let mut c = FrameCaptures {
            at: vec![1.0, 2.0],
            ..Default::default()
        };
        c.times_due(0.5);
        assert!(!c.take_for_frame());
        // 60 steps of 1/60 fall a hair short of 1
        c.times_due(0.999_999_9);
        assert!(c.take_for_frame());
        assert_eq!(
            c.armed.as_ref().map(|a| &a.trigger),
            Some(&Trigger::At(1.0))
        );
        c.times_due(5.0);
        assert!(c.at.is_empty());
        assert_eq!(c.queue.len(), 1, "2 waits while 1 is armed");
    }

    #[test]
    fn two_captures_at_one_time_get_two_folders() {
        let mut c = FrameCaptures::default();
        let a = c.folder_name("levels/tavern_hub.scene.toml", 2.0);
        let b = c.folder_name("levels/tavern_hub.scene.toml", 2.0);
        assert_eq!(a, "tavern_hub-t2.000.flintframe");
        assert_eq!(b, "tavern_hub-t2.000-2.flintframe");
    }

    #[test]
    fn a_time_never_reached_fails_the_run() {
        let c = FrameCaptures {
            at: vec![9.0],
            ..Default::default()
        };
        assert!(c.failure().unwrap().contains("--capture-frame-at 9"));
    }
}
