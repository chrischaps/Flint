//! A frame's description as data (#1084, ADR 0126 decision 3).
//!
//! What one frame drew, in the order [`SceneRenderer::encode_to`] recorded
//! it: each pass with the targets it wrote and read, whether it ran and why
//! not, its draws (each named by its entity), the draws a cull set aside
//! and why, and its GPU time; then the frame's settings, its targets, its
//! lights and [`RenderStats`] whole. `flint render --frame-dump <file.json>`
//! writes one; a capture's `manifest.json` (#1085, #1086) holds the same
//! type, so `flint frame` (#1087) reads both with one reader.
//!
//! **Built only when asked.** A host arms it with
//! [`SceneRenderer::describe_next_frame`] before the frame, and takes it with
//! [`SceneRenderer::describe_frame`] after. Until then the renderer keeps no
//! log, and every site that would write one checks an `Option` and moves
//! on: a frame nobody describes formats nothing and allocates nothing.
//!
//! **Recorded where the frame is.** Every pass is logged by the
//! [`FrameScopes`] that opens its GPU timer scope and debug group (#1083),
//! and a pass that is skipped is logged at the branch that skips it, so the
//! description can't walk a different frame than the one drawn. A pass's
//! draws come from the same selector the pass draws with
//! (`main_pass_draws`, `shadow_caster`).
//!
//! **Versioned as ADR 0126 says.** `format` is `"flintframe"`, `version` an
//! integer, 1. A field added is no bump: readers ignore what they don't
//! know and default what's missing (`#[serde(default)]`, no
//! `deny_unknown_fields`). A field removed, renamed or changed in meaning is.
//!
//! **Entities by name**, the id beside it for display only: ids move between
//! a captured world and its replay (ADR 0126), so nothing here is keyed by
//! one.
//!
//! [`SceneRenderer::encode_to`]: crate::SceneRenderer::encode_to
//! [`SceneRenderer::describe_next_frame`]: crate::SceneRenderer::describe_next_frame
//! [`SceneRenderer::describe_frame`]: crate::SceneRenderer::describe_frame

use crate::gpu_timer::{GpuScope, GpuTimer, GpuTimings};
use crate::render_stats::RenderStats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(not(target_arch = "wasm32"))]
pub use crate::frame_capture_write::{
    write_capture, write_capture_on, write_replay, ReplayFiles, WrittenCapture, REPLAY_FILES,
};
pub use crate::frame_taps::{CaptureTargets, CapturedFrame, CapturedImage, FrameCapture};

/// What `format` says: this is a Flint frame.
pub const FORMAT: &str = "flintframe";

/// The manifest version this engine writes and reads.
pub const VERSION: u32 = 1;

/// One frame, described; see the module docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameDescription {
    /// Always [`FORMAT`].
    pub format: String,
    /// [`VERSION`] when written by this engine.
    pub version: u32,
    pub frame: FrameInfo,
    /// The frame's totals, as the F2 overlay counts them. `gpu_passes` is
    /// left out: each pass carries its own `gpu_ms`.
    pub stats: RenderStats,
    pub lights: LightsDescription,
    /// Every texture the frame drew into or read, each once.
    pub targets: Vec<TargetDescription>,
    /// What the GPU memory the frame's targets hold adds up to.
    pub memory: MemoryDescription,
    /// The passes in the order the frame recorded them. With comic panels
    /// (#880) these are the page's own (the compositor) and each panel's
    /// passes are under [`panels`](Self::panels).
    pub passes: Vec<PassDescription>,
    /// A comic page's panels, each a whole frame drawn again (#880): empty
    /// for a frame with one view.
    pub panels: Vec<PanelDescription>,
    /// What a capture (#1085) read back and wrote: `None` for a frame
    /// described without its images (`--frame-dump`).
    pub capture: Option<CaptureDescription>,
    /// What wrote it: `flint-player`, `flint render`. `None` in a
    /// description written before #1086.
    pub written_by: Option<String>,
    /// When it was captured, UTC, RFC 3339 (`2026-11-16T22:30:00Z`).
    pub captured_at: Option<String>,
    /// The engine that drew it.
    pub engine: Option<EngineDescription>,
    /// The checkout the run's project was in, when it was one (#1290).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectDescription>,
    /// The scene it drew, by path and hash.
    pub scene: Option<SceneDescription>,
    /// Why the frame was captured (#1086): a time, a key, a script, a hitch.
    pub trigger: Option<TriggerDescription>,
    /// The player's side of the frame (#1086, ADR 0126 decision 3): the
    /// frame history's window around it, and the hitch that asked for it.
    pub cpu: Option<CpuDescription>,
    /// What the capture holds for drawing the frame again (#1089): the
    /// files under `replay/`, the assets by hash, and what a replay draws
    /// fresh. `None` when no replay inputs were captured.
    pub replay: Option<ReplayDescription>,
    /// What a reader should know about this frame, a line each: a Living
    /// Props split, GPU timing that didn't arrive.
    pub notes: Vec<String>,
    /// The scope recorder's look-back around the frame (#1097), a Chrome
    /// trace-event JSON file in the capture's folder (`trace.json`), when
    /// the player was recording.
    pub trace: Option<String>,
}

impl Default for FrameDescription {
    fn default() -> Self {
        Self {
            format: FORMAT.to_string(),
            version: VERSION,
            frame: FrameInfo::default(),
            stats: RenderStats::default(),
            lights: LightsDescription::default(),
            targets: Vec::new(),
            memory: MemoryDescription::default(),
            passes: Vec::new(),
            panels: Vec::new(),
            capture: None,
            written_by: None,
            captured_at: None,
            engine: None,
            project: None,
            scene: None,
            trigger: None,
            cpu: None,
            replay: None,
            notes: Vec::new(),
            trace: None,
        }
    }
}

impl FrameDescription {
    /// Every pass of the frame, a panel's included, in the order recorded,
    /// each followed by its sub-passes (a shadow map's layers).
    pub fn all_passes(&self) -> Vec<&PassDescription> {
        fn walk<'a>(passes: &'a [PassDescription], out: &mut Vec<&'a PassDescription>) {
            for pass in passes {
                out.push(pass);
                walk(&pass.passes, out);
            }
        }
        let mut out = Vec::new();
        for panel in &self.panels {
            walk(&panel.passes, &mut out);
        }
        walk(&self.passes, &mut out);
        out
    }

    /// Visit every pass in [`all_passes`](Self::all_passes)'s order, each
    /// mutably: a pass, then the passes inside it.
    pub fn each_pass_mut(&mut self, visit: &mut dyn FnMut(&mut PassDescription)) {
        fn walk(passes: &mut [PassDescription], visit: &mut dyn FnMut(&mut PassDescription)) {
            for pass in passes {
                visit(pass);
                walk(&mut pass.passes, visit);
            }
        }
        for panel in &mut self.panels {
            walk(&mut panel.passes, visit);
        }
        walk(&mut self.passes, visit);
    }

    /// Every image the frame's capture wrote, in pass order.
    pub fn images(&self) -> Vec<&ImageDescription> {
        self.all_passes()
            .into_iter()
            .flat_map(|p| p.images.iter())
            .collect()
    }

    /// The first pass named `name` (a panel's included).
    pub fn pass(&self, name: &str) -> Option<&PassDescription> {
        self.all_passes().into_iter().find(|p| p.name == name)
    }

    /// Lay one frame's raw GPU times (`GpuTimer::recorded`'s, not the
    /// smoothed figure) on the passes that were timed, in the order the
    /// timer's scopes were begun: the n-th timed pass named "Bloom" takes
    /// the n-th "Bloom" scope. Passes the timer didn't time keep `None`.
    pub fn lay_gpu_times(&mut self, timings: &GpuTimings) {
        let mut taken = vec![false; timings.passes.len()];
        let mut lay = |pass: &mut PassDescription| {
            if !pass.timed {
                return;
            }
            let found = timings
                .passes
                .iter()
                .enumerate()
                .find(|(i, t)| !taken[*i] && t.label == pass.name);
            if let Some((i, t)) = found {
                taken[i] = true;
                pass.gpu_ms = Some(t.ms);
            }
        };
        fn each(passes: &mut [PassDescription], lay: &mut dyn FnMut(&mut PassDescription)) {
            for pass in passes {
                lay(pass);
                each(&mut pass.passes, lay);
            }
        }
        // Only the last panel's scopes are timed (#880), and the page's
        // after them
        if let Some(last) = self.panels.last_mut() {
            each(&mut last.passes, &mut lay);
        }
        each(&mut self.passes, &mut lay);
        self.frame.gpu_frame_ms = Some(timings.frame_ms);
    }
}

/// The frame's settings: what it was drawn at and on.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameInfo {
    /// The frame's number in the host's frame history (the player's), when
    /// it keeps one.
    pub index: Option<u64>,
    /// The game time it was drawn at, seconds (what scripts read as
    /// `total_time()`).
    pub game_time: Option<f64>,
    /// The game step it took, seconds.
    pub dt: Option<f64>,
    /// The output's size in pixels.
    pub size: [u32; 2],
    /// What the 3D scene is drawn at: `size` at render scale 1 (#122).
    pub scene_size: [u32; 2],
    pub render_scale: f32,
    /// Samples per pixel of the scene passes: 1 or 4 (ADR 0058).
    pub msaa: u32,
    /// The output's texture format.
    pub surface_format: String,
    /// The host's adapter line (`gpu_select::adapter_line`), when it said.
    pub adapter: Option<String>,
    /// `Vulkan`, `Dx12`, ...: the adapter's backend, when the host said.
    pub backend: Option<String>,
    /// The window's present mode; `None` for a headless frame.
    pub present_mode: Option<String>,
    /// The shading debug view (`DebugMode`), `normal` when none.
    pub debug_mode: String,
    /// Each quality setting (`QualitySettings::entries`), as written on
    /// `--quality`.
    pub quality: BTreeMap<String, String>,
    /// The drawn post-process config (`effective_post_process_config`:
    /// the scene's, scripts' overrides and the quality caps laid), every
    /// `[post_process]` key of `post_effects` by name.
    pub post_process: BTreeMap<String, serde_json::Value>,
    /// The styles drawn this frame, in chain order, with their stage.
    pub styles: Vec<StyleDescription>,
    /// The frame's GPU busy time when it was timed: the scopes' sum.
    pub gpu_frame_ms: Option<f32>,
    /// Whether this frame was captured (#1085): its targets were copied
    /// out as it drew, so its GPU time holds those copies, and a hitch
    /// detector (#1076) should pass over it.
    pub capturing: bool,
}

/// The engine a capture was drawn with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineDescription {
    /// The crate version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// The commit the binary that drew it was built from, stamped at
    /// compile time (#1290; before it, the HEAD of the project it ran in,
    /// which in a game's repository is the game's). `None` when the build
    /// didn't know.
    pub commit: Option<String>,
    /// Whether the tree it was built from has changes to tracked files,
    /// asked of the build's source folder while that is still at `commit`;
    /// `None` when it couldn't be asked.
    pub dirty: Option<bool>,
    /// Where the engine's sources sat in the checkout `commit` is of, when
    /// not at its root: `engine/` for a player a game built from its
    /// subtree, whose commit `commit` then is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
}

/// The checkout the captured run's project was in (#1290): a game's own
/// repository, at run time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectDescription {
    /// Its HEAD, `None` where git couldn't say.
    pub commit: Option<String>,
    /// Whether it had changes to tracked files.
    pub dirty: Option<bool>,
}

/// The scene a capture drew.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneDescription {
    /// As the host was handed it (relative to where it ran).
    pub path: String,
    /// Its `[scene] name`.
    pub name: String,
    /// The file's SHA-256, hex, when it could be read.
    pub sha256: Option<String>,
}

/// Why a frame was captured (#1086).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerDescription {
    /// `at` (`--capture-frame-at`), `key` (the debug key), `script` (an
    /// `--input-script` line) or `hitch` (`--capture-on-hitch`).
    pub kind: String,
    /// In a line: `--capture-frame-at 2`, `Shift+F2`, `input script at
    /// 1.5 s`, `the hitch at frame 312`.
    pub detail: String,
}

/// The player's side of a captured frame: its frame history around it.
/// Times are ms.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuDescription {
    /// The captured frame's own record, when the history kept it.
    pub frame: Option<CpuFrame>,
    /// The frames around it, oldest first: the capture's own and those
    /// before it, or, for a hitch, those before the hitch and after it up
    /// to the capture's read-back.
    pub window: Vec<CpuFrame>,
    /// Whether script callbacks were timed (each frame's `scripts_ms` holds
    /// their times only then).
    pub scripts_timed: bool,
    /// The hitch that asked for the capture (`--capture-on-hitch`). The
    /// captured frame is the one after it: a hitch is over by the time it
    /// is judged, so its own frame can't be drawn again. What it cost by
    /// phase is its record in `window`; what the scene looked like is the
    /// next frame's images.
    pub hitch: Option<HitchDescription>,
    /// Why there is no window, when there isn't one (a build without the
    /// frame history and no `--perf`).
    pub missing: Option<String>,
}

/// One frame of the player's history (#1076).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuFrame {
    pub index: u64,
    pub game_time: f64,
    /// Wall time from its start to the next frame's.
    pub ms: f32,
    /// What the hitch detector judges it by: paced under vsync, else `ms`.
    pub judged_ms: f32,
    /// Each phase of the player's work by name (`scripts`, `physics`,
    /// `render`, `present`...).
    pub phases_ms: BTreeMap<String, f32>,
    /// Each script callback's time, when scripts were timed.
    pub scripts_ms: BTreeMap<String, f32>,
    pub present_ms: Option<f32>,
    pub fixed_steps: u32,
    /// The GPU's busy time on its own work (#1104); `None` when it wasn't
    /// timed or its read-back hadn't come back when the capture was made.
    pub gpu_ms: Option<f32>,
    /// What the engine did in it (#1077).
    pub notes: Vec<CpuNote>,
    /// A capture was in flight on it, so the hitch detector passed over it.
    pub capturing: bool,
    /// The detector found a hitch on it.
    pub hitch: bool,
}

/// One frame note (`flint_core::frame_notes`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuNote {
    pub kind: String,
    pub detail: String,
    pub ms: Option<f32>,
    pub depth: u8,
}

/// A hitch, as the hitch log writes it (#1079).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HitchDescription {
    /// Its frame's [`CpuFrame::index`].
    pub frame: u64,
    pub game_time: f64,
    pub ms: f32,
    pub judged_ms: f32,
    /// The median of the frames before it, which it stood out from.
    pub median_ms: f32,
    /// `player` (the engine's own work) or `elsewhere`.
    pub class: String,
    /// In a line: its notes' likely cause, else the script or phase.
    pub cause: String,
    /// The player's phase that grew most, and by how much.
    pub phase: Option<String>,
    pub phase_ms: Option<f32>,
    /// The script callback that took longest, when scripts were timed.
    pub script: Option<String>,
    pub script_ms: Option<f32>,
}

/// What a capture holds for replay (ADR 0126 decision 4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayDescription {
    /// Whether the replay inputs were written.
    pub available: bool,
    /// The files under the capture's folder, `replay/...`.
    pub files: Vec<String>,
    /// What a replay draws fresh rather than as captured, a line each.
    pub fresh_only: Vec<String>,
    /// Each file the load resolved, by path and SHA-256, never a copy.
    pub assets: Vec<AssetDescription>,
}

/// One asset a captured frame drew from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetDescription {
    /// `model` or `texture`.
    pub kind: String,
    /// What the scene named it by (`tavern`, `textures/wood.png`).
    pub name: String,
    /// Where it was found, as the host's search returned it.
    pub path: String,
    /// Its SHA-256, hex; `None` when it couldn't be read.
    pub sha256: Option<String>,
    pub bytes: u64,
}

/// A style the post chain ran, in the order it ran.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleDescription {
    pub name: String,
    /// `hdr` or `display`.
    pub stage: String,
    /// Its table's `blend` (1 when it has none).
    pub blend: f32,
}

/// Which lights the frame drew and which it left out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LightsDescription {
    /// Each light drawn: `directional`, `point` or `spot`, with its entity.
    pub kept: Vec<LightEntry>,
    /// Lights past a kind's cap, left out of this frame, by entity name.
    pub left_out: Vec<LightEntry>,
    /// Whether the built-in pair stood in (no usable light in the scene).
    pub default_lights: bool,
}

/// One light of the frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LightEntry {
    pub kind: String,
    pub entity: Option<String>,
    pub entity_id: Option<u64>,
    /// A spot that draws its own shadow map (#504).
    pub casts_shadow: bool,
}

/// One texture of the frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetDescription {
    /// A stable name passes refer to it by: `hdr_scene`, `ssao`,
    /// `shadow_map`, `surface`, a style's `kuwahara.tensor`...
    pub id: String,
    pub format: String,
    pub size: [u32; 2],
    /// Array layers (a shadow map's cascades), 1 for most.
    pub layers: u32,
    pub samples: u32,
    /// Its size in GPU memory, as `width x height x layers x samples x
    /// texel` (a compressed or tiled layout may differ).
    pub bytes: u64,
}

/// What the frame's targets hold in GPU memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryDescription {
    /// Every target listed, summed.
    pub target_bytes: u64,
    /// Of those, the style chain's (#302, `style_target_entries`).
    pub style_target_bytes: u64,
}

/// One pass of the frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PassDescription {
    /// The GPU timer scope's label, which `--gpu-time` and the F2 overlay
    /// name it by (`Scene`, `Bloom`, `Kuwahara`...), or a shadow map
    /// layer's pass label (`Shadow cascade 0`).
    pub name: String,
    /// Whether it drew anything this frame.
    pub ran: bool,
    /// Why it didn't, when it didn't.
    pub skipped: Option<String>,
    /// Whether the GPU timer has a scope of this name for it: a pass the
    /// frame records without a scope (a shadow layer inside `Shadows`, the
    /// composite while FXAA ends the chain) has none.
    pub timed: bool,
    /// Its GPU time this frame, ms, raw (not the F2 overlay's smoothed
    /// figure); `None` while timing is off or before the read-back.
    pub gpu_ms: Option<f32>,
    /// The target ids it reads ([`TargetDescription::id`]).
    pub reads: Vec<String>,
    /// The target ids it writes.
    pub writes: Vec<String>,
    /// The wgpu render passes it is made of, by label, in order (what a
    /// GPU debugger lists).
    pub render_passes: Vec<String>,
    /// What it draws besides its draw list, in order: `Sky`, `Grass`,
    /// `Water`, `Grid`, `2D sprites`...
    pub also: Vec<String>,
    /// Its draws, in the order drawn.
    pub draws: Vec<DrawDescription>,
    /// Draws a cull set aside for this pass, each with `culled` saying why.
    pub culled: Vec<DrawDescription>,
    /// Passes inside it: the shadow map's layers.
    pub passes: Vec<PassDescription>,
    /// What a capture (#1085) read of each target it wrote, as the pass
    /// left it, in the order read: empty unless the frame was captured.
    pub images: Vec<ImageDescription>,
}

/// One target as a pass left it, read back by a capture (#1085, ADR 0126
/// decision 2): stored once, exactly, in `file`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageDescription {
    /// The target's id ([`TargetDescription::id`]).
    pub target: String,
    /// The step of a pass that writes one target more than once: bloom's
    /// `threshold`, `down` and `up`. `None` for most.
    pub step: Option<String>,
    /// The array layer read: a shadow map's cascade (0-2) or the casting
    /// spot's (3). `None` for a plain texture.
    pub layer: Option<u32>,
    /// The texture's format, as wgpu names it (`Rgba16Float`).
    pub format: String,
    pub size: [u32; 2],
    /// Where it is, from the capture's folder: `targets/NN-<target>.<ext>`,
    /// `NN` the pass's place in the frame.
    pub file: String,
    /// How the texels are kept: `png` (8-bit, exact), `png16` (16-bit
    /// integers, exact), `exr-half` (float, OpenEXR half, ZIP), `exr-f32`
    /// (depth or a 32-bit float, ZIP), `exr-u32` or `raw` (the texels as
    /// copied, for a format nothing else holds).
    pub stored: String,
    /// The read-back this came from, while the capture is in memory.
    #[serde(skip)]
    pub(crate) tap: Option<usize>,
}

/// What a frame's capture read back and wrote (#1085).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureDescription {
    /// Images written, one per [`ImageDescription`].
    pub images: u32,
    /// Bytes read back from the GPU: every image's texels.
    pub raw_bytes: u64,
    /// Bytes the image files take on disk.
    pub file_bytes: u64,
    /// Targets a pass wrote that weren't read, each with why: a buffer, a
    /// multisampled texture (its resolve is read), one the renderer
    /// doesn't hold.
    pub untapped: Vec<String>,
    /// Recording the copies into the frame's encoder, on the frame thread, ms.
    pub record_ms: f64,
    /// Waiting for the GPU to finish the frame and its copies, ms (headless
    /// only: a window polls and never waits, ADR 0126 decision 5).
    pub wait_ms: Option<f64>,
    /// Copying out of mapped memory, ms.
    pub copy_out_ms: f64,
    /// Encoding and writing the files, ms (on the writer's threads).
    pub encode_ms: f64,
}

/// One draw.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawDescription {
    /// The entity it draws, by name (ADR 0126: names, not ids).
    pub entity: Option<String>,
    /// That entity's id this run, for display only.
    pub entity_id: Option<u64>,
    /// `Static`, `Wire box`, `Skinned`, `Echo`, `Billboard`, `Terrain
    /// chunk`, `Particles`, `Sprite batch`, `Grass`.
    pub kind: String,
    /// The draw list's layer (`Opaque static`, `Blended skinned`...).
    pub layer: String,
    /// `opaque`, `alpha`, `premultiplied`, `additive`, `multiply`, `lighten`.
    pub blend: String,
    /// `plain`, or `wooze` (shallow wooze's vertex stage, #215).
    pub vertex: String,
    /// The pipeline it draws through.
    pub pipeline: String,
    /// The mesh: a model's sub-mesh as `<asset>#<n>`, `box`, `terrain
    /// chunk <n>`, a particle draw's emitter.
    pub mesh: Option<String>,
    /// Its material's name when the mesh has one.
    pub material: Option<String>,
    /// Draws with one number share one material buffer this frame.
    pub material_id: Option<u32>,
    pub triangles: u32,
    pub instances: u32,
    /// The entity's `material.cast_shadows`.
    pub casts_shadow: bool,
    /// Drawn double-sided (glTF `doubleSided`, #623).
    pub double_sided: bool,
    /// Set aside: `frustum`, `lod` (another level of detail draws from
    /// here) or `draw distance` (#56). `None` for a draw drawn.
    pub culled: Option<String>,
}

/// A comic panel's frame (#880).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelDescription {
    pub name: String,
    /// Its picture's size in pixels.
    pub size: [u32; 2],
    pub passes: Vec<PassDescription>,
}

// ── The log a described frame writes as it is recorded ──

/// A draw as the frame logs it: the entity by id, named when the frame is
/// described (the world isn't at hand while the frame is recorded).
pub(crate) struct LoggedDraw {
    pub entity: Option<flint_core::EntityId>,
    /// The mesh's vertex buffer and the material's buffer, by address, for
    /// naming the mesh and numbering the material when described.
    pub mesh_key: Option<usize>,
    pub material_key: Option<usize>,
    pub draw: DrawDescription,
}

/// A pass as the frame logs it.
#[derive(Default)]
pub(crate) struct LoggedPass {
    pub pass: PassDescription,
    pub draws: Vec<LoggedDraw>,
    pub culled: Vec<LoggedDraw>,
    pub passes: Vec<LoggedPass>,
}

/// One `encode_to`'s passes: a frame, or one comic panel of it.
#[derive(Default)]
pub(crate) struct LoggedView {
    pub size: [u32; 2],
    pub passes: Vec<LoggedPass>,
    /// The styles its post chain ran, in order.
    pub styles: Vec<StyleDescription>,
}

/// What a described frame has logged so far: one view an `encode_to`
/// (a comic page has one a panel), and the page's own passes after them.
#[derive(Default)]
pub(crate) struct FrameLog {
    pub views: Vec<LoggedView>,
    /// Passes of the page (#880's compositor), after its panels' views.
    pub page: Vec<LoggedPass>,
    /// Whether the views are a comic page's panels, by name.
    pub panels: Vec<String>,
    /// Whether the next pass goes to the page rather than a view.
    pub on_page: bool,
    /// What the description's `notes` say of the frame (a split view).
    pub notes: Vec<String>,
}

impl FrameLog {
    fn passes(&mut self) -> &mut Vec<LoggedPass> {
        if self.on_page {
            return &mut self.page;
        }
        if self.views.is_empty() {
            self.views.push(LoggedView::default());
        }
        &mut self.views.last_mut().expect("pushed above").passes
    }

    /// A new view: the next `encode_to`'s passes go into it.
    pub fn begin_view(&mut self, size: [u32; 2]) {
        self.on_page = false;
        self.views.push(LoggedView {
            size,
            ..Default::default()
        });
    }

    /// Log a pass that ran and return it, for its draws and targets.
    pub fn ran(&mut self, name: &str, timed: bool) -> &mut LoggedPass {
        let passes = self.passes();
        passes.push(LoggedPass {
            pass: PassDescription {
                name: name.to_string(),
                ran: true,
                timed,
                ..Default::default()
            },
            ..Default::default()
        });
        passes.last_mut().expect("pushed above")
    }

    /// Log a pass the frame didn't draw, and why.
    pub fn skipped(&mut self, name: &str, why: impl Into<String>) {
        self.passes().push(LoggedPass {
            pass: PassDescription {
                name: name.to_string(),
                skipped: Some(why.into()),
                ..Default::default()
            },
            ..Default::default()
        });
    }

    /// The view being logged's styles, for the post chain to fill.
    pub fn styles(&mut self) -> Option<&mut Vec<StyleDescription>> {
        self.views.last_mut().map(|v| &mut v.styles)
    }

    /// The last logged pass named `name`.
    pub fn named(&mut self, name: &str) -> Option<&mut LoggedPass> {
        self.passes().iter_mut().rev().find(|p| p.pass.name == name)
    }
}

impl LoggedPass {
    pub fn reads(&mut self, ids: &[&str]) -> &mut Self {
        push_ids(&mut self.pass.reads, ids);
        self
    }

    pub fn writes(&mut self, ids: &[&str]) -> &mut Self {
        push_ids(&mut self.pass.writes, ids);
        self
    }

    /// Say it drew nothing after all (an empty shadow pass), and why.
    pub fn drew_nothing(&mut self, why: impl Into<String>) {
        self.pass.ran = false;
        self.pass.skipped = Some(why.into());
    }

    pub fn render_pass(&mut self, label: &str) {
        self.pass.render_passes.push(label.to_string());
    }
}

fn push_ids(list: &mut Vec<String>, ids: &[&str]) {
    for id in ids {
        if !list.iter().any(|i| i == id) {
            list.push((*id).to_string());
        }
    }
}

// ── The scopes a frame's parts open ──

/// The GPU timer and, while a frame is described, its log, carried through
/// the frame together so every part that opens a timer scope and debug
/// group (`gpu_labels::begin`, #1083) logs its pass at the same call: the
/// description and `--gpu-time`'s rows can't drift apart.
#[derive(Default)]
pub(crate) struct FrameScopes {
    pub timer: Option<GpuTimer>,
    pub log: Option<Box<FrameLog>>,
    /// While the frame is captured (#1085): each pass's targets, copied as
    /// it ends (`SceneRenderer::tap_pass`).
    pub taps: Option<Box<crate::frame_taps::FrameTaps>>,
    /// While a host skips or repeats parts by label (#1317): the knobs and
    /// what they've said. `None` on every frame a game draws.
    pub knobs: Option<Box<crate::pass_knobs::KnobRun>>,
}

/// Why a part a knob skipped didn't run, in a frame's description.
pub(crate) const SKIPPED_BY_KNOB: &str = "skip_pass names it (#1317)";

/// A part [`FrameScopes::begin`] opened.
#[must_use = "a part of the frame is closed only by FrameScopes::end"]
pub(crate) struct ScopeGuard(crate::gpu_labels::FrameScope);

impl FrameScopes {
    /// Open the part named `label`: its debug group, its timer scope, and
    /// its pass in the log.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder, label: &'static str) -> ScopeGuard {
        let scope = crate::gpu_labels::begin(self.timer.as_mut(), encoder, label);
        if let Some(log) = &mut self.log {
            log.ran(label, scope_timed(&scope));
        }
        ScopeGuard(scope)
    }

    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder, scope: ScopeGuard) {
        crate::gpu_labels::end(self.timer.as_mut(), encoder, scope.0);
    }

    /// `run` inside the part named `label`: once, or as the pass knobs
    /// say (#1317).
    pub fn run(
        &mut self,
        label: &'static str,
        encoder: &mut wgpu::CommandEncoder,
        run: &mut dyn FnMut(&mut wgpu::CommandEncoder),
    ) {
        self.run_refusing(label, encoder, run, None);
    }

    /// [`run`](Self::run), drawing once whatever a repeat asks when
    /// `refuse` says why the part can't be repeated.
    pub fn run_refusing(
        &mut self,
        label: &'static str,
        encoder: &mut wgpu::CommandEncoder,
        run: &mut dyn FnMut(&mut wgpu::CommandEncoder),
        refuse: Option<&str>,
    ) {
        let n = self.times_refusing(label, refuse);
        if n == 0 {
            self.skip(label, || SKIPPED_BY_KNOB.to_string());
            return;
        }
        // The CPU's side of each post effect and style part, by its label:
        // a dozen a frame at most (#1096)
        flint_core::scope!("encode_part", label);
        let scope = self.begin(encoder, label);
        for _ in 0..n {
            run(encoder);
        }
        self.end(encoder, scope);
    }

    /// How many times the part named `label` records its work this frame
    /// (#1317): 1 unless a pass knob skips (0) or repeats (N) it.
    pub fn times(&mut self, label: &'static str) -> u32 {
        self.times_refusing(label, None)
    }

    /// [`times`](Self::times), at most 1 when `refuse` says why the part
    /// can't be repeated (said once).
    pub fn times_refusing(&mut self, label: &'static str, refuse: Option<&str>) -> u32 {
        match self.knobs.as_deref_mut() {
            None => 1,
            Some(knobs) => knobs.times(label, refuse),
        }
    }

    /// Open the part named `label` for `n` recordings ([`times`](Self::times)):
    /// `None`, logged as skipped, when a knob skips it.
    pub fn begin_times(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        label: &'static str,
        n: u32,
    ) -> Option<ScopeGuard> {
        if n == 0 {
            self.skip(label, || SKIPPED_BY_KNOB.to_string());
            return None;
        }
        Some(self.begin(encoder, label))
    }

    /// `work` `n` times, the log and taps set aside after the first, so a
    /// repeated part is described and read once (#1317).
    pub fn repeat(&mut self, n: u32, mut work: impl FnMut(&mut Self)) {
        let mut held = None;
        for i in 0..n {
            if i == 1 {
                held = Some((self.log.take(), self.taps.take()));
            }
            work(self);
        }
        if let Some((log, taps)) = held {
            self.log = log;
            self.taps = taps;
        }
    }

    /// Log the part named `label` as skipped this frame, and why. `why`
    /// is only called while the frame is described.
    pub fn skip(&mut self, label: &str, why: impl FnOnce() -> String) {
        if let Some(log) = &mut self.log {
            log.skipped(label, why());
        }
    }

    /// The log, while the frame is described.
    pub fn log(&mut self) -> Option<&mut FrameLog> {
        self.log.as_deref_mut()
    }

    /// Add to the last logged pass named `pass`, while the frame is
    /// described: what it found once it began (its render passes, targets,
    /// draws).
    pub fn note(&mut self, pass: &str, add: impl FnOnce(&mut LoggedPass)) {
        if let Some(found) = self.log.as_deref_mut().and_then(|l| l.named(pass)) {
            add(found);
        }
    }
}

fn scope_timed(scope: &crate::gpu_labels::FrameScope) -> bool {
    scope.timed().is_some_and(|s: GpuScope| s.is_timed())
}

/// A texture's description.
pub(crate) fn target(id: &str, texture: &wgpu::Texture) -> TargetDescription {
    let size = texture.size();
    let format = texture.format();
    let layers = size.depth_or_array_layers.max(1);
    let samples = texture.sample_count().max(1);
    let texel = format
        .block_copy_size(Some(wgpu::TextureAspect::All))
        .or_else(|| format.block_copy_size(Some(wgpu::TextureAspect::DepthOnly)))
        .unwrap_or(4) as u64;
    TargetDescription {
        id: id.to_string(),
        format: format!("{format:?}"),
        size: [size.width, size.height],
        layers,
        samples,
        bytes: size.width as u64 * size.height as u64 * layers as u64 * samples as u64 * texel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_reads_back_and_ignores_what_it_doesnt_know() {
        let mut d = FrameDescription::default();
        d.passes.push(PassDescription {
            name: "Scene".into(),
            ran: true,
            ..Default::default()
        });
        let mut json: serde_json::Value = serde_json::to_value(&d).unwrap();
        assert_eq!(json["format"], "flintframe");
        assert_eq!(json["version"], 1);
        json["a_field_from_later"] = serde_json::json!(3);
        json.as_object_mut().unwrap().remove("lights");
        let back: FrameDescription = serde_json::from_value(json).unwrap();
        assert_eq!(back.passes[0].name, "Scene");
        assert_eq!(back.lights.kept.len(), 0);
    }

    #[test]
    fn gpu_times_go_to_the_timed_passes_in_order() {
        let mut d = FrameDescription::default();
        let pass = |name: &str, timed: bool| PassDescription {
            name: name.into(),
            timed,
            ran: true,
            ..Default::default()
        };
        d.passes = vec![
            pass("Shadows", true),
            pass("Scene", true),
            pass("Composite", false),
            pass("Bloom", true),
            pass("Bloom", true),
        ];
        let t = |label: &'static str, ms: f32| crate::GpuPassTime { label, ms };
        d.lay_gpu_times(&GpuTimings {
            passes: vec![
                t("Shadows", 0.5),
                t("Scene", 1.0),
                t("Bloom", 0.25),
                t("Bloom", 0.125),
            ],
            frame_ms: 2.0,
        });
        let ms: Vec<Option<f32>> = d.passes.iter().map(|p| p.gpu_ms).collect();
        assert_eq!(ms, [Some(0.5), Some(1.0), None, Some(0.25), Some(0.125)]);
        assert_eq!(d.frame.gpu_frame_ms, Some(2.0));
    }

    #[test]
    fn a_log_keeps_each_view_apart() {
        let mut log = FrameLog::default();
        log.begin_view([4, 4]);
        log.ran("Scene", false);
        log.begin_view([8, 8]);
        log.skipped("Bloom", "off");
        assert_eq!(log.views.len(), 2);
        assert_eq!(log.views[1].passes[0].pass.skipped.as_deref(), Some("off"));
        assert!(!log.views[1].passes[0].pass.ran);
    }
}
