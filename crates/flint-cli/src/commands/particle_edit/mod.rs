//! `flint edit fx.particles.toml` — the particle effect editor (ADR 0068).
//!
//! Three modes, like the model previewer:
//! - interactive: a 3D viewport with the effect playing at the origin, an
//!   emitter list + sections panel on the left, a scrub timeline below;
//! - bootstrap: a missing file is created from a preset before opening;
//! - `--render out.png [--anim-time T]`: deterministic headless snapshot.
//!
//! Scrubbing re-simulates from t = 0 in fixed 1/120 s steps, and play uses
//! the same step, so a paused frame and a scrubbed frame at the same time
//! are bit-identical. Ctrl+S patches only the keys that changed
//! (`save.rs`), keeping comments intact.

mod gizmo;
mod presets;
mod save;
mod sim;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Args;
use flint_particles::{load_effect_from_file, ParticleEffect};
use flint_render::{Camera, OrbitCameraController, RendererConfig, SceneRenderer};
use flint_viewer::host::{Editor, Flow, Gpu, InitCx, StillOptions, Target, Tick};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

use sim::PreviewSim;

// ─── CLI args ───────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ParticleEditArgs {
    /// Path to a .particles.toml effect (created from --preset if missing)
    pub file: String,
    #[arg(long, default_value = "1500")]
    pub width: u32,
    #[arg(long, default_value = "940")]
    pub height: u32,
    #[arg(long)]
    pub no_grid: bool,
    #[arg(long)]
    pub auto_orbit: bool,
    /// Render a PNG instead of opening a window
    #[arg(long)]
    pub render: Option<String>,
    /// Simulation time for --render (default 1.0 s)
    #[arg(long)]
    pub anim_time: Option<f32>,
    /// Preset for a new file: fire, smoke, sparks, rain
    #[arg(long)]
    pub preset: Option<String>,
    #[arg(long)]
    pub distance: Option<f32>,
    #[arg(long)]
    pub yaw: Option<f32>,
    #[arg(long)]
    pub pitch: Option<f32>,
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub target: Option<[f32; 3]>,
    #[arg(long)]
    pub fov: Option<f32>,
    /// `--style` / `--style-time` for --render (#271)
    #[command(flatten)]
    pub style: crate::commands::common_args::StyleArgs,
}

// ─── Entry ──────────────────────────────────────────────────────────────────

/// The particle editor for `args.file` (made from `--preset` when it
/// isn't there), for [`flint_viewer::host`] to open in a window or draw as a
/// `--render` still (#712).
pub fn editor(args: ParticleEditArgs) -> Result<ParticleEditApp> {
    let path = PathBuf::from(&args.file);
    let effect = if path.exists() {
        load_effect_from_file(&path).map_err(|e| anyhow::anyhow!(e))?
    } else {
        let preset = args.preset.as_deref().unwrap_or("sparks");
        let fx = presets::bootstrap(&path, preset)?;
        crate::output::say!(
            "Created new particle effect '{}' from the '{}' preset at {}",
            fx.name,
            preset,
            path.display()
        );
        fx
    };

    Ok(editor_for(args, path, effect))
}

/// The editor's own framing; the host lays the camera flags over it.
fn default_camera(aspect: f32) -> Camera {
    let mut camera = Camera::new();
    camera.aspect = aspect;
    camera.target = flint_core::Vec3::new(0.0, 1.0, 0.0);
    camera.distance = 6.0;
    camera.pitch = 20.0f32.to_radians();
    camera.yaw = 35.0f32.to_radians();
    camera.update_orbit();
    camera
}

fn effect_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

// ─── Interactive ────────────────────────────────────────────────────────────

/// Preview backdrop options (the clear colour behind the effect).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    Dark,
    Light,
    Black,
}

impl Backdrop {
    fn next(self) -> Self {
        match self {
            Backdrop::Dark => Backdrop::Light,
            Backdrop::Light => Backdrop::Black,
            Backdrop::Black => Backdrop::Dark,
        }
    }
    fn color(self) -> [f32; 4] {
        match self {
            Backdrop::Dark => flint_render::DEFAULT_CLEAR_COLOR,
            Backdrop::Light => [0.72, 0.74, 0.78, 1.0],
            Backdrop::Black => [0.0, 0.0, 0.0, 1.0],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Backdrop::Dark => "dark",
            Backdrop::Light => "light",
            Backdrop::Black => "black",
        }
    }
}

/// Per-emitter preview toggles (never saved).
#[derive(Clone, Copy, Debug, Default)]
pub struct EmitterView {
    pub muted: bool,
    pub show_gizmo: bool,
}

/// Transport state.
pub struct PlayState {
    pub playing: bool,
    pub speed: f32,
    pub looping: bool,
    pub loop_end: f32,
}

/// Every mutable thing the UI can ask for; applied after the egui frame.
#[derive(Default)]
pub struct UiActions {
    pub changed: bool,
    pub structural: bool,
    pub seek: Option<f32>,
    pub toggle_play: bool,
    pub restart: bool,
    pub save: bool,
    pub reload: bool,
    pub undo: bool,
    pub redo: bool,
    pub select: Option<Option<usize>>,
    pub browse_texture: Option<usize>,
    pub add_preset: Option<&'static str>,
    pub set_loop: Option<bool>,
    pub set_loop_end: Option<f32>,
    pub set_speed: Option<f32>,
    pub toggle_grid: bool,
    pub toggle_gizmos: bool,
    pub cycle_backdrop: bool,
    pub toggle_orbit: bool,
}

pub struct ParticleEditApp {
    args: ParticleEditArgs,
    path: PathBuf,

    // Document
    pub effect: ParticleEffect,
    pub saved: ParticleEffect,
    pub selected: Option<usize>,
    pub views: Vec<EmitterView>,
    pub solo: Option<usize>,
    undo: Vec<ParticleEffect>,
    redo: Vec<ParticleEffect>,
    /// State before the current run of edits (pushed to `undo` on commit).
    edit_start: Option<ParticleEffect>,
    /// State captured just before the latest discrete mutation.
    pre_change_snapshot: Option<ParticleEffect>,
    last_change: Instant,

    // Simulation + transport
    sim: PreviewSim,
    pub play: PlayState,
    sim_dirty: bool,

    // Viewport
    pub backdrop: Backdrop,
    pub show_grid: bool,
    pub show_gizmos: bool,
    show_ui: bool,
    gizmo_hash: u64,

    // Window / GPU (the host's, #712)
    window: Option<Arc<Window>>,
    render_context: Option<Gpu>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,
    orbit: OrbitCameraController,

    // egui
    egui_ctx: egui::Context,

    // Watch / save
    watcher_paused_until: Option<Instant>,
    pub status: Option<(String, Instant)>,
    quit_armed: Option<Instant>,
}

fn editor_for(args: ParticleEditArgs, path: PathBuf, effect: ParticleEffect) -> ParticleEditApp {
    let loop_end = effect.preview_length().max(1.0);
    let views = vec![
        EmitterView {
            muted: false,
            show_gizmo: true,
        };
        effect.emitters.len()
    ];
    let show_grid = !args.no_grid;
    ParticleEditApp {
        args,
        path,
        saved: effect.clone(),
        effect,
        selected: Some(0),
        views,
        solo: None,
        undo: Vec::new(),
        redo: Vec::new(),
        edit_start: None,
        pre_change_snapshot: None,
        last_change: Instant::now(),
        sim: PreviewSim::new(),
        play: PlayState {
            playing: true,
            speed: 1.0,
            looping: true,
            loop_end,
        },
        sim_dirty: true,
        backdrop: Backdrop::Dark,
        show_grid,
        show_gizmos: true,
        show_ui: true,
        gizmo_hash: 0,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),
        orbit: OrbitCameraController::new(),
        egui_ctx: egui::Context::default(),
        watcher_paused_until: None,
        status: None,
        quit_armed: None,
    }
}

impl ParticleEditApp {
    /// The window's or the still's GPU is here (#712). A still keeps the
    /// renderer's clear colour, as `--render` always has.
    fn initialize(&mut self, cx: InitCx) -> Result<()> {
        let still = cx.window.is_none();
        let context = cx.gpu.clone();
        self.camera = default_camera(context.aspect());
        let mut renderer = context.scene_renderer(RendererConfig {
            show_grid: self.show_grid,
            ..Default::default()
        });
        if !still {
            renderer.set_clear_color(self.backdrop.color());
            self.sim.load_textures(
                &mut renderer,
                &context.device,
                &context.queue,
                &effect_dir(&self.path),
            );
        }
        self.egui_ctx = cx.egui.clone();
        self.window = cx.window;
        self.render_context = Some(context);
        self.scene_renderer = Some(renderer);
        Ok(())
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::info!("{}", msg);
        self.status = Some((msg, Instant::now()));
    }

    pub fn is_dirty(&self) -> bool {
        self.effect != self.saved
    }

    fn update_title(&self) {
        if let Some(w) = &self.window {
            let star = if self.is_dirty() { " *" } else { "" };
            w.set_title(&format!(
                "Flint Particles \u{2014} {}{star}",
                self.effect.name
            ));
        }
    }

    // ─── Undo ───────────────────────────────────────────────────────────

    fn note_change(&mut self) {
        if self.edit_start.is_none() {
            // The state before this edit run; committed once the pointer is up.
            self.edit_start = Some(
                self.pre_change_snapshot
                    .take()
                    .unwrap_or_else(|| self.saved.clone()),
            );
        }
        self.last_change = Instant::now();
        self.redo.clear();
        self.sim_dirty = true;
        self.update_title();
    }

    fn commit_pending_undo(&mut self, pointer_down: bool) {
        if let Some(start) = &self.edit_start {
            if !pointer_down && self.last_change.elapsed() > Duration::from_millis(250) {
                if *start != self.effect {
                    self.undo.push(start.clone());
                    if self.undo.len() > 100 {
                        self.undo.remove(0);
                    }
                }
                self.edit_start = None;
            }
        }
    }

    fn undo(&mut self) {
        self.edit_start = None;
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.effect.clone());
            self.effect = prev;
            self.after_structural();
            self.set_status("Undo");
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.effect.clone());
            self.effect = next;
            self.after_structural();
            self.set_status("Redo");
        }
    }

    /// Emitters were added/removed/reordered: fix up parallel state.
    fn after_structural(&mut self) {
        self.views.resize(
            self.effect.emitters.len(),
            EmitterView {
                muted: false,
                show_gizmo: true,
            },
        );
        if let Some(s) = self.selected {
            if s >= self.effect.emitters.len() {
                self.selected = self.effect.emitters.len().checked_sub(1);
            }
        }
        if let Some(s) = self.solo {
            if s >= self.effect.emitters.len() {
                self.solo = None;
            }
        }
        self.play.loop_end = self.effect.preview_length().max(1.0);
        self.sim_dirty = true;
        self.update_title();
    }

    // ─── Save / reload ──────────────────────────────────────────────────

    fn save(&mut self) {
        if let Err(e) = self.effect.validate() {
            self.set_status(format!("Not saved: {e}"));
            return;
        }
        match save::save_effect(&self.path, &self.saved, &self.effect) {
            Ok(mode) => {
                self.saved = self.effect.clone();
                self.watcher_paused_until = Some(Instant::now() + Duration::from_millis(1500));
                self.set_status(format!("Saved {} ({mode})", self.path.display()));
                self.update_title();
            }
            Err(e) => self.set_status(format!("Save failed: {e}")),
        }
    }

    fn reload_from_disk(&mut self) {
        match load_effect_from_file(&self.path) {
            Ok(fx) => {
                self.effect = fx.clone();
                self.saved = fx;
                self.undo.clear();
                self.redo.clear();
                self.edit_start = None;
                self.after_structural();
                self.set_status("Reloaded from disk");
                if let (Some(renderer), Some(ctx)) =
                    (self.scene_renderer.as_mut(), self.render_context.as_ref())
                {
                    self.sim.load_textures(
                        renderer,
                        &ctx.device,
                        &ctx.queue,
                        &effect_dir(&self.path),
                    );
                }
            }
            Err(e) => self.set_status(format!("Reload failed: {e}")),
        }
    }

    /// The file changed on disk (the host's watch), unless by our own save.
    fn check_file_reload(&mut self) {
        if let Some(until) = self.watcher_paused_until {
            if Instant::now() < until {
                return;
            }
            self.watcher_paused_until = None;
        }
        self.reload_from_disk();
    }

    // ─── Simulation ─────────────────────────────────────────────────────

    fn muted_flags(&self) -> Vec<bool> {
        self.views.iter().map(|v| v.muted).collect()
    }

    fn rebuild_sim(&mut self) {
        let t = self.sim.time();
        let muted = self.muted_flags();
        self.sim.rebuild(&self.effect, &muted, self.solo);
        self.sim.seek(t.min(self.play.loop_end));
        self.sim_dirty = false;
        self.gizmo_hash = 0; // force gizmo refresh
        if let (Some(renderer), Some(ctx)) =
            (self.scene_renderer.as_mut(), self.render_context.as_ref())
        {
            self.sim
                .load_textures(renderer, &ctx.device, &ctx.queue, &effect_dir(&self.path));
        }
    }

    fn seek(&mut self, t: f32) {
        let muted = self.muted_flags();
        if self.sim_dirty {
            self.sim.rebuild(&self.effect, &muted, self.solo);
            self.sim_dirty = false;
        }
        self.sim.seek(t.clamp(0.0, self.play.loop_end.max(0.0)));
    }

    fn tick(&mut self, real_dt: f32) {
        if self.sim_dirty {
            self.rebuild_sim();
        }
        if self.play.playing {
            let wrapped = self.sim.advance(
                real_dt,
                self.play.speed,
                self.play.looping,
                self.play.loop_end,
            );
            if wrapped {
                let muted = self.muted_flags();
                self.sim.rebuild(&self.effect, &muted, self.solo);
                self.sim.seek(0.0);
            } else if !self.play.looping && self.sim.time() >= self.play.loop_end {
                self.play.playing = false;
            }
        }
    }

    /// While auto-orbit runs, ease the orbit centre onto the selected
    /// emitter's spawn point (its `shape_offset` at the effect origin) so the
    /// turntable stays framed on it. A fixed point, not the particle
    /// centroid: a moving centroid made the camera wander.
    fn follow_selected_emitter(&mut self, dt: f32) {
        if !self.orbit.auto_orbit {
            return;
        }
        let Some(i) = self.selected else {
            return;
        };
        let focus = self
            .effect
            .emitters
            .get(i)
            .and_then(|em| em.resolve(flint_particles::ResolveContext::asset()).ok())
            .map(|cfg| cfg.shape_offset);
        if let Some(p) = focus {
            let t = 1.0 - (-4.0 * dt).exp();
            self.camera.target.x += (p[0] - self.camera.target.x) * t;
            self.camera.target.y += (p[1] - self.camera.target.y) * t;
            self.camera.target.z += (p[2] - self.camera.target.z) * t;
            self.camera.update_orbit();
        }
    }

    /// Shift the projection so the camera target sits in the middle of the
    /// region the panels leave uncovered (the panels are drawn over the 3D
    /// pass, which spans the whole window). Uses last frame's layout.
    fn center_in_viewport(&mut self) {
        let screen = self.egui_ctx.content_rect();
        let view = self
            .egui_ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(ui::VIEW_RECT_ID)))
            .unwrap_or(screen);
        if screen.width() <= 0.0 || screen.height() <= 0.0 {
            return;
        }
        // NDC spans [-1, 1]; offset = 2 × (viewport centre − window centre) / size.
        let dx = (view.center().x - screen.center().x) / screen.width() * 2.0;
        let dy = -(view.center().y - screen.center().y) / screen.height() * 2.0;
        self.camera.ndc_offset = [dx, dy];
    }

    fn refresh_gizmo(&mut self) {
        let (Some(renderer), Some(ctx)) =
            (self.scene_renderer.as_mut(), self.render_context.as_ref())
        else {
            return;
        };
        if !self.show_gizmos {
            if self.gizmo_hash != u64::MAX {
                renderer.clear_debug_overlay();
                self.gizmo_hash = u64::MAX;
            }
            return;
        }
        let hash = gizmo::shape_hash(&self.effect, self.selected, &self.views);
        if hash == self.gizmo_hash {
            return;
        }
        self.gizmo_hash = hash;
        let mesh = gizmo::build_overlay(&self.effect, self.selected, &self.views);
        renderer.set_debug_overlay(&ctx.device, &mesh);
    }

    // ─── Actions from the UI ────────────────────────────────────────────

    fn apply_actions(&mut self, a: UiActions) {
        if a.changed || a.structural {
            self.note_change();
        }
        if a.structural {
            self.after_structural();
        }
        if let Some(sel) = a.select {
            self.selected = sel;
            self.gizmo_hash = 0;
        }
        if let Some(t) = a.seek {
            self.seek(t);
        }
        if a.toggle_play {
            self.play.playing = !self.play.playing;
            if self.play.playing && self.sim.time() >= self.play.loop_end {
                self.seek(0.0);
            }
        }
        if a.restart {
            self.seek(0.0);
            self.play.playing = true;
        }
        if let Some(l) = a.set_loop {
            self.play.looping = l;
        }
        if let Some(end) = a.set_loop_end {
            self.play.loop_end = end.clamp(0.1, 600.0);
        }
        if let Some(s) = a.set_speed {
            self.play.speed = s.clamp(0.05, 8.0);
        }
        if a.toggle_grid {
            self.show_grid = !self.show_grid;
            if let (Some(r), Some(ctx)) =
                (self.scene_renderer.as_mut(), self.render_context.as_ref())
            {
                r.set_show_grid(&ctx.device, self.show_grid);
            }
        }
        if a.toggle_gizmos {
            self.show_gizmos = !self.show_gizmos;
        }
        if a.cycle_backdrop {
            self.backdrop = self.backdrop.next();
            if let Some(r) = self.scene_renderer.as_mut() {
                r.set_clear_color(self.backdrop.color());
            }
        }
        if a.toggle_orbit {
            self.orbit.auto_orbit = !self.orbit.auto_orbit;
        }
        if let Some(name) = a.add_preset {
            self.snapshot_for_edit();
            if let Some(fx) = presets::preset(name) {
                for em in fx.emitters {
                    let mut em = em;
                    em.name = unique_name(&self.effect, &em.name);
                    self.effect.emitters.push(em);
                }
                self.note_change();
                self.after_structural();
                self.selected = self.effect.emitters.len().checked_sub(1);
                self.set_status(format!("Added '{name}' preset emitters"));
            }
        }
        if let Some(i) = a.browse_texture {
            self.browse_texture(i);
        }
        if a.save {
            self.save();
        }
        if a.reload {
            self.reload_from_disk();
        }
        if a.undo {
            self.undo();
        }
        if a.redo {
            self.redo();
        }
    }

    /// Record the pre-edit state right before a discrete UI mutation.
    pub fn snapshot_for_edit(&mut self) {
        if self.edit_start.is_none() {
            self.pre_change_snapshot = Some(self.effect.clone());
        }
    }

    fn browse_texture(&mut self, emitter: usize) {
        let dir = effect_dir(&self.path);
        let picked = crate::file_dialog::FileDialog::new()
            .add_filter("image", &["png", "jpg", "jpeg", "ktx2"])
            .set_directory(&dir)
            .pick_file();
        if let Some(file) = picked {
            let rel = pathdiff_relative(&file, &dir);
            if emitter < self.effect.emitters.len() {
                self.snapshot_for_edit();
                self.effect.emitters[emitter].texture = rel.clone();
                self.note_change();
                self.set_status(format!("Texture: {rel}"));
            }
        }
    }
}

/// Relative path from `base` to `file` when `file` lives under it, else the
/// file name alone (textures are searched next to the effect file).
fn pathdiff_relative(file: &Path, base: &Path) -> String {
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let file_c = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    match file_c.strip_prefix(&base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

fn unique_name(effect: &ParticleEffect, base: &str) -> String {
    if effect.emitter_index(base).is_none() {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}_{i}");
        if effect.emitter_index(&candidate).is_none() {
            return candidate;
        }
    }
    unreachable!()
}

// ─── winit ──────────────────────────────────────────────────────────────────

impl Editor for ParticleEditApp {
    fn title(&self) -> String {
        format!("Flint Particles \u{2014} {}", self.effect.name)
    }

    fn init(&mut self, cx: InitCx) -> Result<()> {
        self.initialize(cx)
    }

    fn orbit(&mut self) -> Option<(&mut Camera, &mut OrbitCameraController)> {
        Some((&mut self.camera, &mut self.orbit))
    }

    fn watched_files(&self) -> Vec<PathBuf> {
        vec![self.path.clone()]
    }

    fn on_file_changed(&mut self) {
        self.check_file_reload();
    }

    fn resized(&mut self, gpu: &Gpu) {
        if let Some(r) = &mut self.scene_renderer {
            r.resize_postprocess(&gpu.device, gpu.width, gpu.height);
        }
        self.render_context = Some(gpu.clone());
    }

    /// Every key is the editor's own (O, `[` and `]` too: they set the
    /// playback speed unless the turntable is on).
    fn event(&mut self, event: &WindowEvent) -> Flow {
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return Flow::Pass;
        };
        if event.state != ElementState::Pressed || event.repeat {
            return Flow::Handled;
        }
        if self.egui_ctx.egui_wants_keyboard_input() {
            return Flow::Handled;
        }
        let mods = self.egui_ctx.input(|i| i.modifiers);
        let mut a = UiActions::default();
        match event.physical_key {
            PhysicalKey::Code(KeyCode::Escape) => {
                if self.is_dirty() {
                    match self.quit_armed {
                        Some(t) if t.elapsed() < Duration::from_secs(3) => {
                            return Flow::Exit;
                        }
                        _ => {
                            self.quit_armed = Some(Instant::now());
                            self.set_status("Unsaved changes — press Escape again to quit");
                        }
                    }
                } else {
                    return Flow::Exit;
                }
            }
            PhysicalKey::Code(KeyCode::Space) => a.toggle_play = true,
            PhysicalKey::Code(KeyCode::KeyR) if mods.command => a.reload = true,
            PhysicalKey::Code(KeyCode::KeyR) => a.restart = true,
            PhysicalKey::Code(KeyCode::Home) => a.seek = Some(0.0),
            PhysicalKey::Code(KeyCode::End) => a.seek = Some(self.play.loop_end),
            PhysicalKey::Code(KeyCode::ArrowLeft) => {
                let step = if mods.shift { 0.1 } else { sim::FIXED_DT };
                a.seek = Some((self.sim.time() - step).max(0.0));
                self.play.playing = false;
            }
            PhysicalKey::Code(KeyCode::ArrowRight) => {
                let step = if mods.shift { 0.1 } else { sim::FIXED_DT };
                a.seek = Some(self.sim.time() + step);
                self.play.playing = false;
            }
            PhysicalKey::Code(KeyCode::KeyL) => a.set_loop = Some(!self.play.looping),
            PhysicalKey::Code(KeyCode::BracketLeft) => {
                if self.orbit.auto_orbit {
                    self.orbit.adjust_auto_orbit_speed(0.5);
                } else {
                    a.set_speed = Some(self.play.speed * 0.5);
                }
            }
            PhysicalKey::Code(KeyCode::BracketRight) => {
                if self.orbit.auto_orbit {
                    self.orbit.adjust_auto_orbit_speed(2.0);
                } else {
                    a.set_speed = Some(self.play.speed * 2.0);
                }
            }
            PhysicalKey::Code(KeyCode::KeyO) => a.toggle_orbit = true,
            PhysicalKey::Code(KeyCode::KeyG) => a.toggle_grid = true,
            PhysicalKey::Code(KeyCode::KeyX) => a.toggle_gizmos = true,
            PhysicalKey::Code(KeyCode::KeyB) => a.cycle_backdrop = true,
            PhysicalKey::Code(KeyCode::KeyH) => self.show_ui = !self.show_ui,
            PhysicalKey::Code(KeyCode::KeyS) if mods.command => a.save = true,
            PhysicalKey::Code(KeyCode::KeyZ) if mods.command && mods.shift => a.redo = true,
            PhysicalKey::Code(KeyCode::KeyZ) if mods.command => a.undo = true,
            PhysicalKey::Code(KeyCode::KeyY) if mods.command => a.redo = true,
            PhysicalKey::Code(KeyCode::KeyD) if mods.command => {
                if let Some(i) = self.selected {
                    if let Some(em) = self.effect.emitters.get(i).cloned() {
                        self.snapshot_for_edit();
                        let mut copy = em;
                        copy.name = unique_name(&self.effect, &copy.name);
                        self.effect.emitters.insert(i + 1, copy);
                        a.structural = true;
                        a.select = Some(Some(i + 1));
                    }
                }
            }
            PhysicalKey::Code(KeyCode::Delete) => {
                if let Some(i) = self.selected {
                    if self.effect.emitters.len() > 1 && i < self.effect.emitters.len() {
                        self.snapshot_for_edit();
                        self.effect.emitters.remove(i);
                        self.views.remove(i);
                        a.structural = true;
                    }
                }
            }
            _ => {}
        }
        self.apply_actions(a);
        Flow::Handled
    }

    fn update(&mut self, tick: Tick) {
        let dt = tick.dt.min(0.1);
        self.follow_selected_emitter(dt);
        let pointer_down = self.egui_ctx.input(|i| i.pointer.any_down());
        self.commit_pending_undo(pointer_down);
        self.tick(dt);
        self.refresh_gizmo();
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            self.sim.upload(
                renderer,
                &context.device,
                &context.queue,
                self.camera.position_array(),
                None,
            );
        }
        if !self.show_ui && self.camera.ndc_offset != [0.0, 0.0] {
            self.camera.ndc_offset = [0.0, 0.0];
        }
    }

    fn draw(&mut self, target: &Target) {
        if let Some(renderer) = &mut self.scene_renderer {
            target.render(renderer, &self.camera);
        }
    }

    fn show_ui(&self) -> bool {
        self.show_ui
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let actions = ui::render_egui(self, ui);
        self.apply_actions(actions);
        self.center_in_viewport();
    }

    /// `--render`: the effect simulated to `--anim-time` (1 s if not
    /// given) in the editor's fixed steps, every emitter on.
    fn still(&mut self, options: &StillOptions) -> Result<Option<image::RgbaImage>> {
        let (Some(renderer), Some(ctx)) = (&mut self.scene_renderer, &self.render_context) else {
            anyhow::bail!("the particle editor has no GPU");
        };
        self.sim.rebuild(&self.effect, &[], None);
        self.sim
            .load_textures(renderer, &ctx.device, &ctx.queue, &effect_dir(&self.path));
        let t = options.time.unwrap_or(1.0).max(0.0);
        self.sim.seek(t);
        self.sim.upload(
            renderer,
            &ctx.device,
            &ctx.queue,
            self.camera.position_array(),
            self.args.style.look_time(),
        );
        self.args.style.apply_to_renderer(renderer, &ctx.device);
        Ok(None)
    }

    fn still_note(&self) -> Option<String> {
        Some(format!(
            "'{}' at t = {:.2}s ({} particles alive)",
            self.effect.name,
            self.sim.time(),
            self.sim.alive()
        ))
    }
}
