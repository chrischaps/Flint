//! The editors' one host (#712): the window, the GPU, egui, the event loop,
//! frame timing, the orbit camera's input, a file watch, the debug keys and
//! F1's card of them (#1170), and the offscreen `--render` still, for every
//! editor `flint edit` opens.
//!
//! Each editor (the scene viewer and track editor here; the model,
//! procgen, texture-graph, terrain and particle editors in flint-cli) is an
//! [`Editor`]: it builds what it draws in [`Editor::init`], draws its view
//! in [`Editor::draw`] and its panels in [`Editor::ui`]. [`run`] puts it in
//! a window; [`render_still`] draws the same `init` and `draw` offscreen,
//! so `flint edit <any file> --render out.png` shows what the window would,
//! without the panels. A fix to resizing, surface loss, DPI or the camera
//! flags is made here once, for all of them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use flint_render::{
    AcquireFrame, Camera, DepthTarget, HeadlessContext, OrbitCameraController, RenderContext,
    RendererConfig, SceneRenderer,
};
use flint_runtime::{DebugKey, DebugKeyGroup, DebugKeyTable, InputConfig};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

/// The device an editor draws with, a window's or an offscreen target's.
/// Cheap to clone (wgpu's handles are shared), so an editor keeps one.
#[derive(Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The format the scene's last pass writes (`RenderContext::view_format`,
    /// or the headless target's)
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

impl Gpu {
    fn of_window(context: &RenderContext) -> Self {
        Self {
            device: context.device.clone(),
            queue: context.queue.clone(),
            format: context.view_format,
            width: context.config.width,
            height: context.config.height,
        }
    }

    fn of_headless(context: &HeadlessContext) -> Self {
        Self {
            device: context.device.clone(),
            queue: context.queue.clone(),
            format: context.format,
            width: context.width,
            height: context.height,
        }
    }

    /// A scene renderer for this target, the same for a window and a still.
    pub fn scene_renderer(&self, config: RendererConfig) -> SceneRenderer {
        SceneRenderer::new_headless(
            &self.device,
            &self.queue,
            self.format,
            self.width,
            self.height,
            config,
        )
    }

    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

/// What an editor is given once its target exists.
pub struct InitCx<'a> {
    pub gpu: &'a Gpu,
    /// The context [`Editor::ui`] runs in; an editor keeps a clone to ask
    /// it for modifiers or to load textures
    pub egui: &'a egui::Context,
    /// The window, or `None` for a `--render` still
    pub window: Option<Arc<Window>>,
}

/// One frame's target.
pub struct Target<'a> {
    pub view: &'a wgpu::TextureView,
    pub depth: DepthTarget<'a>,
    pub gpu: &'a Gpu,
}

impl Target<'_> {
    /// Draw `renderer`'s scene from `camera` into this target.
    pub fn render(&self, renderer: &mut SceneRenderer, camera: &Camera) {
        renderer.render_to(
            &self.gpu.device,
            &self.gpu.queue,
            self.depth,
            camera,
            self.view,
        );
    }

    /// Fill this target with one colour (an editor with no 3D view).
    pub fn clear(&self, rgb: [f64; 3]) {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Editor Clear"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Editor Clear Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: rgb[0],
                            g: rgb[1],
                            b: rgb[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
    }
}

/// The frame's clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tick {
    /// Seconds since the last frame (0 for a still)
    pub dt: f32,
    /// Frames drawn in the last second
    pub fps: f32,
}

/// One frame's phases as the host timed them (#1110), for an editor that
/// asks ([`Editor::times_frames`]). Handed over when the next frame begins,
/// since a frame's wall time runs to there. Times are ms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameTimes {
    /// When it began, seconds since the host first timed a frame
    pub at: f64,
    /// Its begin to the next frame's begin
    pub wall_ms: f32,
    /// The file watch, the orbit camera and [`Editor::update`]
    pub update_ms: f32,
    /// The wait for a swapchain image (under vsync, often the display's)
    pub acquire_ms: f32,
    /// [`Editor::draw`]: the scene drawn and submitted
    pub draw_ms: f32,
    /// egui: [`Editor::ui`] run, tessellated, painted and submitted
    pub ui_ms: f32,
    /// Handing the frame to the swapchain
    pub present_ms: f32,
    /// The display's refresh period, if its monitor says
    pub refresh_ms: Option<f32>,
}

impl FrameTimes {
    /// The wall time no phase accounts for: the event loop, the OS, the
    /// driver, the compositor.
    pub fn unaccounted_ms(&self) -> f32 {
        let phases = self.update_ms + self.acquire_ms + self.draw_ms + self.ui_ms + self.present_ms;
        (self.wall_ms - phases).max(0.0)
    }
}

/// What an editor did with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Not used: the host may act on it (orbit input, O, `[`, `]`)
    Pass,
    /// Used; the host does nothing more with it
    Handled,
    /// Close the editor
    Exit,
}

/// The camera flags every editor with a 3D view takes the same way:
/// `--distance`, `--yaw`, `--pitch` (degrees), `--target`, `--fov`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraFlags {
    pub distance: Option<f32>,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub target: Option<[f32; 3]>,
    pub fov: Option<f32>,
}

impl CameraFlags {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Lay the flags over the editor's own framing.
    pub fn apply(&self, camera: &mut Camera) {
        if let Some(d) = self.distance {
            camera.distance = d;
        }
        if let Some(y) = self.yaw {
            camera.yaw = y.to_radians();
        }
        if let Some(p) = self.pitch {
            camera.pitch = p.to_radians();
        }
        if let Some(t) = self.target {
            camera.target = flint_core::Vec3::new(t[0], t[1], t[2]);
        }
        if let Some(f) = self.fov {
            camera.fov = f;
        }
        camera.update_orbit();
    }
}

/// How the host opens an editor.
#[derive(Clone, Debug)]
pub struct HostOptions {
    /// The window's (or the still's) size in physical pixels
    pub width: u32,
    pub height: u32,
    pub camera: CameraFlags,
    /// Start the orbit camera turning (`--auto-orbit`)
    pub auto_orbit: bool,
}

/// What `--render` asks of an editor.
#[derive(Clone, Copy, Debug, Default)]
pub struct StillOptions {
    /// `--anim-time`: the moment to show, for an editor with a clock
    pub time: Option<f32>,
}

/// One of `flint edit`'s editors.
pub trait Editor {
    /// The window's title.
    fn title(&self) -> String;

    /// Build what the editor draws, on `cx.gpu`. Called once, for a window
    /// or a still; the camera flags are laid over the camera afterwards.
    fn init(&mut self, cx: InitCx) -> Result<()>;

    /// The orbit camera, for an editor the shared controller drives: the
    /// host feeds it the mouse and WASD/QE, turns it with `--auto-orbit`,
    /// O, `[` and `]`, and keeps its aspect.
    fn orbit(&mut self) -> Option<(&mut Camera, &mut OrbitCameraController)> {
        None
    }

    /// The camera the camera flags set and resizes keep in aspect; the
    /// orbit camera's unless the editor drives its own.
    fn camera(&mut self) -> Option<&mut Camera> {
        self.orbit().map(|(camera, _)| camera)
    }

    /// Files whose change calls [`Editor::on_file_changed`] (debounced).
    fn watched_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Folders watched with everything under them, the same way (the scene
    /// viewer's `--watch`: the scene's folder and the schema folders).
    fn watched_folders(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Whether a change to `path` counts. A debounced batch of changes none
    /// of which counts calls nothing (#429: the scene viewer leaves a user
    /// style's WGSL and description to the style's own reload). Asked on
    /// the frame's thread, so it may read the editor.
    fn file_change_counts(&self, _path: &Path) -> bool {
        true
    }

    /// A watched file changed on disk.
    fn on_file_changed(&mut self) {}

    /// An event before egui sees it, for shortcuts that must win over a
    /// focused panel.
    fn early_event(&mut self, _event: &WindowEvent) -> Flow {
        Flow::Pass
    }

    /// An event egui didn't consume.
    fn event(&mut self, _event: &WindowEvent) -> Flow {
        Flow::Pass
    }

    /// The target changed size (`gpu` carries the new one).
    fn resized(&mut self, _gpu: &Gpu) {}

    /// Advance the editor's state for this frame.
    fn update(&mut self, _tick: Tick) {}

    /// Whether the host times this frame's phases for
    /// [`Editor::frame_timed`]. Asked once a frame; while it says no, the
    /// host takes no time and keeps nothing for it.
    fn times_frames(&self) -> bool {
        false
    }

    /// The last frame's phases, as a frame the host timed begins.
    fn frame_timed(&mut self, _times: &FrameTimes) {}

    /// Draw the editor's view into the target.
    fn draw(&mut self, target: &Target);

    /// Whether the panels are drawn this frame.
    fn show_ui(&self) -> bool {
        true
    }

    /// The panels, drawn over the view, and what they asked for. Called
    /// once a frame (the host runs egui in one pass).
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    /// What the editor paints over its view that is part of the picture,
    /// not a panel: the track editor's spline and its handles (#1154). The
    /// window paints it after [`Editor::ui`], in the same egui pass (and
    /// hides it with the panels); a `--render` still paints it over the
    /// view, where the panels are left out. Paint it in screen points over
    /// the whole context (`ctx.content_rect()`): a still is one pixel a
    /// point.
    fn overlay(&mut self, _ctx: &egui::Context) {}

    /// Prepare a `--render` still: by default one frame at rest. An editor
    /// whose still is not its 3D view (a texture) returns the picture.
    fn still(&mut self, _options: &StillOptions) -> Result<Option<image::RgbaImage>> {
        self.update(Tick::default());
        Ok(None)
    }

    /// A line `--render` prints after writing the still.
    fn still_note(&self) -> Option<String> {
        None
    }

    /// The editor's debug keys (#1170), declared as the player declares
    /// its own: an action name, a default key, what it does. The host looks
    /// a press up in the table (its own F1 first) and hands the editor the
    /// action in [`Editor::on_debug_key`]; F1 shows the whole table. No
    /// editor matches an F-key by hand.
    fn debug_keys(&self) -> Option<DebugKeyGroup> {
        None
    }

    /// Whether the editor's debug keys act now (the procgen previewer's
    /// need a mesh). Off, F1 shows them dimmed, with the group's `when`.
    fn debug_keys_live(&self) -> bool {
        true
    }

    /// One of [`Editor::debug_keys`]' actions, pressed (never a repeat).
    fn on_debug_key(&mut self, _action: &str) {}
}

/// The host's own debug key action: show or hide the table.
pub const SHOW_KEYS_ACTION: &str = "keys";

/// The host's debug keys, in every editor: F1 is the list of keys, as it is
/// in the player, so it means one thing in every tool.
pub const HOST_KEYS: DebugKeyGroup = DebugKeyGroup {
    owner: "editor",
    when: "always",
    keys: &[DebugKey {
        action: SHOW_KEYS_ACTION,
        key: "F1",
        does: "Show or hide this list of debug keys",
    }],
};

/// What the F1 card says under an editor's keys.
const CARD_FOOTER: &str = "F1 again closes this \u{b7} flint play's F1 lists a game's keys";

/// An editor's debug keys: the host's F1, then the editor's own group
/// (live or not). No input config: the editors read none, so the defaults
/// stand.
pub fn debug_key_table(editor: Option<DebugKeyGroup>, live: bool) -> DebugKeyTable {
    let always: &dyn Fn(&str) -> bool = &|_| true;
    let editor_live: &dyn Fn(&str) -> bool = &move |_| live;
    let groups = std::iter::once((HOST_KEYS, always)).chain(editor.map(|g| (g, editor_live)));
    DebugKeyTable::build(groups, &InputConfig::default())
}

/// Who a press of `key` belongs to, if it is a debug key: the host's own
/// action, or the editor's.
#[derive(Debug, PartialEq, Eq)]
pub enum DebugPress {
    Host(&'static str),
    Editor(&'static str),
}

/// Look a press up in `table`.
pub fn debug_press(table: &DebugKeyTable, key: KeyCode, shift: bool) -> Option<DebugPress> {
    let entry = table.lookup(key, shift)?;
    Some(if entry.owner == HOST_KEYS.owner {
        DebugPress::Host(entry.action)
    } else {
        DebugPress::Editor(entry.action)
    })
}

/// Open `editor` in a window and run it until it closes.
pub fn run(editor: impl Editor, options: HostOptions) -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let egui = egui::Context::default();
    // One pass a frame: an editor's `ui` acts on what its panels report
    // (a click saves, a key toggles), so a discarded second pass would act
    // twice. The editors redraw every frame, so a layout egui would have
    // redone settles on the next one.
    egui.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN);
    let mut host = Host {
        editor,
        options,
        window: None,
        context: None,
        gpu: None,
        egui,
        egui_winit: None,
        egui_renderer: None,
        clock: FrameClock::new(),
        timer: FrameTimer::default(),
        refresh_ms: None,
        watch: None,
        modifiers: ModifiersState::empty(),
        key_table_open: false,
        error: None,
    };
    event_loop.run_app(&mut host)?;
    match host.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Draw `editor` offscreen at the options' size: its `init`, the camera
/// flags, its `still` and its `draw`, then its [`Editor::overlay`], the
/// panels left out.
pub fn render_still(
    editor: &mut impl Editor,
    options: &HostOptions,
    still: &StillOptions,
) -> Result<image::RgbaImage> {
    let (width, height) = (options.width.max(1), options.height.max(1));
    let context = pollster::block_on(HeadlessContext::new(width, height))
        .context("Failed to create headless render context")?;
    let gpu = Gpu::of_headless(&context);
    let egui = egui::Context::default();
    editor.init(InitCx {
        gpu: &gpu,
        egui: &egui,
        window: None,
    })?;
    lay_camera(editor, options, &gpu);
    if let Some(picture) = editor.still(still)? {
        return Ok(picture);
    }
    editor.draw(&Target {
        view: &context.color_view,
        depth: context.depth_target(),
        gpu: &gpu,
    });
    paint_still_overlay(editor, &egui, &context);
    let pixels =
        pollster::block_on(context.read_pixels()).context("Failed to read rendered pixels")?;
    image::RgbaImage::from_raw(width, height, pixels)
        .context("Failed to create image from pixel data")
}

/// Paint `editor`'s [`Editor::overlay`] over a still's view, as the window
/// paints it: one egui pass at one pixel a point on a clock of 0, into the
/// target's plain view so it blends in display space as the window's does
/// (#428, `HeadlessContext::hud_view`). An editor that paints nothing over
/// its view gets no egui renderer and no pass, so its still is the bare
/// view, byte for byte.
fn paint_still_overlay(editor: &mut impl Editor, egui: &egui::Context, context: &HeadlessContext) {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(context.width as f32, context.height as f32),
        )),
        time: Some(0.0),
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(1.0);
    // A real frame: its texture updates are uploaded below (#944)
    #[allow(clippy::disallowed_methods)]
    let mut output = egui.run_ui(input, |ui| editor.overlay(ui.ctx()));
    let jobs = egui.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    if jobs.is_empty() {
        // Nothing to upload them for: egui 0.36 asserts on a dropped delta
        output.textures_delta.clear();
        return;
    }
    let (device, queue) = (&context.device, &context.queue);
    let mut renderer = egui_wgpu::Renderer::new(
        device,
        context.format.remove_srgb_suffix(),
        egui_wgpu::RendererOptions {
            depth_stencil_format: None,
            msaa_samples: 1,
            dithering: false,
            ..Default::default()
        },
    );
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [context.width, context.height],
        pixels_per_point: output.pixels_per_point,
    };
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            renderer.update_texture(device, queue, *id, delta);
        }
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Editor still overlay"),
    });
    renderer.update_buffers(device, queue, &mut encoder, &jobs, &screen);
    let view = context.hud_view();
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Editor still overlay Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    queue.submit(std::iter::once(encoder.finish()));
    // Applied above: egui 0.36 asserts when a delta drops unhandled (#944)
    output.textures_delta.clear();
}

/// The camera flags over the editor's framing, its aspect the target's.
fn lay_camera(editor: &mut impl Editor, options: &HostOptions, gpu: &Gpu) {
    if let Some((_, orbit)) = editor.orbit() {
        orbit.auto_orbit = options.auto_orbit;
    }
    if let Some(camera) = editor.camera() {
        camera.aspect = gpu.aspect();
        options.camera.apply(camera);
    }
}

/// Frame times and the frames-a-second readout.
struct FrameClock {
    last: Instant,
    frames: std::collections::VecDeque<Instant>,
    fps: f32,
    last_fps: Instant,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            last: Instant::now(),
            frames: Default::default(),
            fps: 0.0,
            last_fps: Instant::now(),
        }
    }

    fn tick(&mut self) -> Tick {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.frames.push_back(now);
        while self
            .frames
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
        {
            self.frames.pop_front();
        }
        if now.duration_since(self.last_fps) > Duration::from_millis(250) {
            self.fps = self.frames.len() as f32;
            self.last_fps = now;
        }
        Tick { dt, fps: self.fps }
    }
}

/// The phases of frames an editor asked to have timed (#1110). Holds a
/// frame only while it is being timed: one the editor didn't ask for opens
/// nothing, and each [`lap`](Self::lap) on it is one branch.
#[derive(Default)]
struct FrameTimer {
    /// The first timed frame's begin, the epoch of every `at`
    epoch: Option<Instant>,
    /// The frame under way: its times so far and the last lap's end
    open: Option<(FrameTimes, Instant)>,
}

impl FrameTimer {
    /// A frame begins at `now`, timed if `timed`. Returns the frame before,
    /// closed at `now`, when it was timed and this one is too (an editor
    /// that stopped asking is handed nothing more).
    fn begin(&mut self, now: Instant, timed: bool, refresh_ms: Option<f32>) -> Option<FrameTimes> {
        let last = self.open.take();
        if !timed {
            return None;
        }
        let epoch = *self.epoch.get_or_insert(now);
        let at = now.duration_since(epoch).as_secs_f64();
        let times = FrameTimes {
            at,
            refresh_ms,
            ..Default::default()
        };
        self.open = Some((times, now));
        last.map(|(mut done, _)| {
            done.wall_ms = ((at - done.at) * 1000.0) as f32;
            done
        })
    }

    /// The time since the last lap, added to the open frame's `phase`.
    fn lap(&mut self, phase: fn(&mut FrameTimes) -> &mut f32) {
        if let Some((times, last)) = &mut self.open {
            let now = Instant::now();
            *phase(times) += now.duration_since(*last).as_secs_f32() * 1000.0;
            *last = now;
        }
    }

    /// The frame drew nothing (its surface was lost): it is not handed on.
    fn abandon(&mut self) {
        self.open = None;
    }
}

/// A debounced watch on the editor's files and folders, keeping the paths
/// that changed for the frame to hand to [`Editor::file_change_counts`].
struct FileWatch {
    /// Set with each batch, so a frame with nothing new takes no lock
    changed: Arc<AtomicBool>,
    paths: Arc<std::sync::Mutex<Vec<PathBuf>>>,
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

impl FileWatch {
    fn new(files: &[PathBuf], folders: &[PathBuf]) -> Result<Self> {
        use notify_debouncer_mini::notify::RecursiveMode;
        let changed = Arc::new(AtomicBool::new(false));
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (flag, sink) = (Arc::clone(&changed), Arc::clone(&paths));
        let mut debouncer = notify_debouncer_mini::new_debouncer(
            Duration::from_millis(400),
            move |result: notify_debouncer_mini::DebounceEventResult| match result {
                Ok(events) => {
                    if let Ok(mut paths) = sink.lock() {
                        paths.extend(events.into_iter().map(|e| e.path));
                    }
                    flag.store(true, Ordering::Release);
                }
                Err(e) => tracing::warn!("Watch error: {:?}", e),
            },
        )?;
        let watched = files
            .iter()
            .map(|f| (f, RecursiveMode::NonRecursive))
            .chain(folders.iter().map(|d| (d, RecursiveMode::Recursive)));
        for (path, mode) in watched {
            debouncer
                .watcher()
                .watch(path, mode)
                .with_context(|| format!("can't watch {}", path.display()))?;
        }
        Ok(Self {
            changed,
            paths,
            _debouncer: debouncer,
        })
    }

    /// The paths changed since the last take, if any batch came.
    fn take(&self) -> Option<Vec<PathBuf>> {
        if !self.changed.swap(false, Ordering::AcqRel) {
            return None;
        }
        Some(
            self.paths
                .lock()
                .map(|mut p| std::mem::take(&mut *p))
                .unwrap_or_default(),
        )
    }
}

/// Whether a batch of changed paths calls [`Editor::on_file_changed`]:
/// any one that counts does.
fn batch_counts(paths: &[PathBuf], counts: impl Fn(&Path) -> bool) -> bool {
    paths.iter().any(|p| counts(p))
}

struct Host<E: Editor> {
    editor: E,
    options: HostOptions,
    window: Option<Arc<Window>>,
    context: Option<RenderContext>,
    gpu: Option<Gpu>,
    egui: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    clock: FrameClock,
    /// The phases of frames the editor asked to have timed
    timer: FrameTimer,
    /// The window's monitor's refresh period, ms, once asked; forgotten
    /// when the window moves, so it is asked of the monitor it moved to
    refresh_ms: Option<Option<f32>>,
    watch: Option<FileWatch>,
    /// Shift held, for a debug key's chord
    modifiers: ModifiersState,
    /// F1's card is up
    key_table_open: bool,
    /// Why the editor couldn't open, returned from [`run`]
    error: Option<anyhow::Error>,
}

impl<E: Editor> Host<E> {
    /// The one place a window, a GPU and egui are made for an editor.
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(self.editor.title())
                        .with_inner_size(PhysicalSize::new(
                            self.options.width.max(1),
                            self.options.height.max(1),
                        )),
                )
                .context("Failed to create the editor's window")?,
        );
        let context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to create the editor's render context")?;
        let gpu = Gpu::of_window(&context);

        self.editor.init(InitCx {
            gpu: &gpu,
            egui: &self.egui,
            window: Some(window.clone()),
        })?;
        lay_camera(&mut self.editor, &self.options, &gpu);

        let (files, folders) = (self.editor.watched_files(), self.editor.watched_folders());
        if !files.is_empty() || !folders.is_empty() {
            self.watch = Some(FileWatch::new(&files, &folders)?);
        }

        self.egui_winit = Some(egui_winit::State::new(
            self.egui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        ));
        // egui draws into the plain view (RenderContext::hud_view, #428),
        // so its translucent panels blend in display space in every editor
        self.egui_renderer = Some(egui_wgpu::Renderer::new(
            &context.device,
            context.hud_format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                dithering: false,
                ..Default::default()
            },
        ));
        self.window = Some(window);
        self.context = Some(context);
        self.gpu = Some(gpu);
        Ok(())
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        let Some(context) = &mut self.context else {
            return;
        };
        context.resize(size);
        let gpu = Gpu::of_window(context);
        if let Some(camera) = self.editor.camera() {
            camera.aspect = gpu.aspect();
        }
        self.editor.resized(&gpu);
        self.gpu = Some(gpu);
    }

    /// The editor's debug keys now: the host's F1 and the editor's group.
    fn debug_key_table(&self) -> DebugKeyTable {
        debug_key_table(self.editor.debug_keys(), self.editor.debug_keys_live())
    }

    /// A key pressed: whether it was a debug key, acted on (unless it is a
    /// repeat) and so kept from the editor and the orbit.
    fn debug_key(&mut self, key: KeyCode, repeat: bool) -> bool {
        let table = self.debug_key_table();
        let Some(press) = debug_press(&table, key, self.modifiers.shift_key()) else {
            return false;
        };
        if !repeat {
            match press {
                DebugPress::Host(SHOW_KEYS_ACTION) => self.key_table_open = !self.key_table_open,
                DebugPress::Host(_) => {}
                DebugPress::Editor(action) => self.editor.on_debug_key(action),
            }
        }
        true
    }

    /// The host's own keys and the orbit camera's input, for an event the
    /// editor passed on.
    fn default_event(&mut self, event: &WindowEvent) {
        let over_ui = self.egui.is_pointer_over_egui();
        let Some((camera, orbit)) = self.editor.orbit() else {
            return;
        };
        match event {
            WindowEvent::MouseInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseWheel { .. } => {
                orbit.handle_event(event, camera, over_ui);
            }
            WindowEvent::KeyboardInput { event: key, .. }
                if key.state == ElementState::Pressed && !key.repeat =>
            {
                match key.physical_key {
                    PhysicalKey::Code(KeyCode::KeyO) => {
                        orbit.auto_orbit = !orbit.auto_orbit;
                        tracing::info!(
                            target: "viewer",
                            "Auto-orbit: {}",
                            if orbit.auto_orbit { "ON" } else { "OFF" }
                        );
                    }
                    PhysicalKey::Code(KeyCode::BracketRight) => {
                        orbit.adjust_auto_orbit_speed(1.5);
                        tracing::info!(target: "viewer", "Auto-orbit speed: {:.2} rad/s", orbit.auto_orbit_speed);
                    }
                    PhysicalKey::Code(KeyCode::BracketLeft) => {
                        orbit.adjust_auto_orbit_speed(1.0 / 1.5);
                        tracing::info!(target: "viewer", "Auto-orbit speed: {:.2} rad/s", orbit.auto_orbit_speed);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The window's monitor's refresh period, ms, asked of the monitor
    /// once and again after the window moves.
    fn refresh_ms(&mut self) -> Option<f32> {
        if self.refresh_ms.is_none() {
            let mhz = self
                .window
                .as_ref()
                .and_then(|w| w.current_monitor())
                .and_then(|m| m.refresh_rate_millihertz())
                .filter(|&mhz| mhz > 0);
            self.refresh_ms = Some(mhz.map(|mhz| 1_000_000.0 / mhz as f32));
        }
        self.refresh_ms.flatten()
    }

    fn frame(&mut self) {
        let tick = self.clock.tick();
        // The last frame's phases for an editor that times them (F2's graph)
        let timed = self.editor.times_frames();
        let refresh = if timed { self.refresh_ms() } else { None };
        if let Some(done) = self.timer.begin(self.clock.last, timed, refresh) {
            self.editor.frame_timed(&done);
        }
        if let Some(paths) = self.watch.as_ref().and_then(FileWatch::take) {
            let editor = &self.editor;
            if batch_counts(&paths, |p| editor.file_change_counts(p)) {
                tracing::debug!(?paths, "watched files changed");
                self.editor.on_file_changed();
            } else {
                tracing::debug!(?paths, "watched files changed, none the editor counts");
            }
        }
        if let Some((camera, orbit)) = self.editor.orbit() {
            orbit.update(camera, tick.dt);
        }
        self.editor.update(tick);
        self.timer.lap(|t| &mut t.update_ms);

        let (Some(context), Some(gpu)) = (&self.context, &self.gpu) else {
            return self.timer.abandon();
        };
        let output = match context.surface.acquire() {
            Ok(output) => output,
            Err(flint_render::SurfaceError::Lost | flint_render::SurfaceError::Outdated) => {
                // Configure the surface again at its size; the next frame draws
                let size = context.size;
                self.resize(size);
                return self.timer.abandon();
            }
            Err(e) => {
                tracing::warn!("Surface error: {:?}", e);
                return self.timer.abandon();
            }
        };
        self.timer.lap(|t| &mut t.acquire_ms);
        // The scene into the sRGB view, egui into the plain one, as the
        // player does (#428)
        let view = context.frame_view(&output.texture);
        let hud_view = context.hud_view(&output.texture);
        self.editor.draw(&Target {
            view: &view,
            depth: context.depth_target(),
            gpu,
        });
        self.timer.lap(|t| &mut t.draw_ms);
        if self.editor.show_ui() {
            self.paint_ui(&hud_view);
        }
        self.timer.lap(|t| &mut t.ui_ms);
        if let Some(context) = &self.context {
            context.queue.present(output);
        }
        self.timer.lap(|t| &mut t.present_ms);
    }

    /// Run the editor's panels and paint them over the frame.
    fn paint_ui(&mut self, target: &wgpu::TextureView) {
        let (Some(window), Some(context), Some(egui_winit), Some(egui_renderer)) = (
            &self.window,
            &self.context,
            &mut self.egui_winit,
            &mut self.egui_renderer,
        ) else {
            return;
        };
        let raw_input = egui_winit.take_egui_input(window);
        // F1's card (#1170), over the panels
        let key_table = self
            .key_table_open
            .then(|| debug_key_table(self.editor.debug_keys(), self.editor.debug_keys_live()));
        let editor = &mut self.editor;
        // A real frame: its texture updates are uploaded below (#944)
        #[allow(clippy::disallowed_methods)]
        let mut output = self.egui.run_ui(raw_input, |ui| {
            editor.ui(ui);
            editor.overlay(ui.ctx());
            if let Some(table) = &key_table {
                flint_runtime::debug_key_card::paint(ui.ctx(), table, CARD_FOOTER);
            }
        });
        egui_winit.handle_platform_output(window, std::mem::take(&mut output.platform_output));

        let paint_jobs = self
            .egui
            .tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Editor egui"),
            });
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&context.device, &context.queue, *id, delta);
            }
        }
        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Editor egui Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }
        context.queue.submit(std::iter::once(encoder.finish()));
        for id in &output.textures_delta.free {
            egui_renderer.free_texture(id);
        }
        // Applied above: egui 0.36 asserts when a delta drops unhandled (#944)
        output.textures_delta.clear();
    }
}

impl<E: Editor> ApplicationHandler for Host<E> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // User styles and glyph sets ask a watcher thread from here on,
        // never the disk in a frame (#633)
        flint_core::file_watch::start();
        if let Err(e) = self.initialize(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match self.editor.early_event(&event) {
            Flow::Pass => {}
            Flow::Handled => return,
            Flow::Exit => return event_loop.exit(),
        }
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.modifiers = modifiers.state();
        }
        if let (Some(egui_winit), Some(window)) = (&mut self.egui_winit, &self.window) {
            if egui_winit.on_window_event(window, &event).consumed {
                return;
            }
        }
        match &event {
            WindowEvent::CloseRequested => return event_loop.exit(),
            WindowEvent::Resized(size) => return self.resize(*size),
            WindowEvent::RedrawRequested => return self.frame(),
            // Perhaps onto another monitor, with another refresh
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.refresh_ms = None;
            }
            WindowEvent::KeyboardInput { event: key, .. } => {
                // WASD/QE held for the keyboard orbit, whatever the editor does
                if let Some((_, orbit)) = self.editor.orbit() {
                    orbit.handle_key_event(key);
                }
                // A debug key (#1170) is the table's: the editor never sees it
                if let (ElementState::Pressed, PhysicalKey::Code(code)) =
                    (key.state, key.physical_key)
                {
                    if self.debug_key(code, key.repeat) {
                        return;
                    }
                }
            }
            _ => {}
        }
        match self.editor.event(&event) {
            Flow::Pass => self.default_event(&event),
            Flow::Handled => {}
            Flow::Exit => event_loop.exit(),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Textures as one picture `width` x `height`, for a still of an editor
/// whose view is images rather than a scene: each scaled (nearest) to fit
/// its cell of a grid with the fewest empty cells, on `background`.
pub fn image_sheet(
    images: &[image::RgbaImage],
    width: u32,
    height: u32,
    background: [u8; 3],
) -> image::RgbaImage {
    let mut sheet = image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([background[0], background[1], background[2], 255]),
    );
    let n = images.len() as u32;
    if n == 0 || width == 0 || height == 0 {
        return sheet;
    }
    // The column count whose cells are largest
    let (cols, rows) = (1..=n)
        .map(|cols| (cols, n.div_ceil(cols)))
        .max_by(|a, b| {
            let side = |(c, r): (u32, u32)| (width / c).min(height / r);
            side(*a).cmp(&side(*b)).then(b.0.cmp(&a.0))
        })
        .unwrap_or((1, 1));
    let (cell_w, cell_h) = (width / cols, height / rows);
    let gap = 4.min(cell_w / 8).min(cell_h / 8);
    for (i, img) in images.iter().enumerate() {
        let (iw, ih) = img.dimensions();
        if iw == 0 || ih == 0 {
            continue;
        }
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        let (room_w, room_h) = (cell_w - 2 * gap, cell_h - 2 * gap);
        let scale = (room_w as f32 / iw as f32).min(room_h as f32 / ih as f32);
        let (dw, dh) = (
            ((iw as f32 * scale) as u32).max(1),
            ((ih as f32 * scale) as u32).max(1),
        );
        let x0 = col * cell_w + (cell_w - dw) / 2;
        let y0 = row * cell_h + (cell_h - dh) / 2;
        for y in 0..dh {
            let sy = ((y as f32 + 0.5) / scale) as u32;
            for x in 0..dw {
                let sx = ((x as f32 + 0.5) / scale) as u32;
                let p = img.get_pixel(sx.min(iw - 1), sy.min(ih - 1));
                // Over the background, so a transparent texel shows it
                let a = p[3] as u32;
                let mix = |c: u8, b: u8| ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8;
                sheet.put_pixel(
                    x0 + x,
                    y0 + y,
                    image::Rgba([
                        mix(p[0], background[0]),
                        mix(p[1], background[1]),
                        mix(p[2], background[2]),
                        255,
                    ]),
                );
            }
        }
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1170: F1 is the host's list of keys in every editor, one with no
    /// keys of its own too; the scene viewer's F2-F4 reach it by action.
    #[test]
    fn f1_is_the_list_in_every_editor_and_the_viewers_keys_are_its_own() {
        let bare = debug_key_table(None, true);
        assert_eq!(
            debug_press(&bare, KeyCode::F1, false),
            Some(DebugPress::Host(SHOW_KEYS_ACTION))
        );
        assert_eq!(debug_press(&bare, KeyCode::F2, false), None);

        let viewer = debug_key_table(Some(crate::app::VIEWER_KEYS), true);
        for (key, action) in [
            (KeyCode::F2, "stats"),
            (KeyCode::F3, "normal_arrows"),
            (KeyCode::F4, "render_panel"),
        ] {
            assert_eq!(
                debug_press(&viewer, key, false),
                Some(DebugPress::Editor(action))
            );
        }
        assert_eq!(
            debug_press(&viewer, KeyCode::F1, false),
            Some(DebugPress::Host(SHOW_KEYS_ACTION))
        );
        assert!(
            viewer
                .entries
                .iter()
                .all(|e| e.standing == flint_runtime::Standing::Active),
            "no clash, nothing the built-in actions take: {viewer:#?}"
        );
        assert!(
            viewer.text().contains("normal_arrows  viewer"),
            "{}",
            viewer.text()
        );
    }

    /// An editor whose keys don't act now (a texture in the previewer)
    /// keeps them out of the way, and F1 still answers.
    #[test]
    fn keys_not_live_pass_on_and_f1_still_lists_them() {
        let table = debug_key_table(Some(crate::app::VIEWER_KEYS), false);
        assert_eq!(debug_press(&table, KeyCode::F2, false), None);
        assert_eq!(
            debug_press(&table, KeyCode::F1, false),
            Some(DebugPress::Host(SHOW_KEYS_ACTION))
        );
        assert_eq!(table.entries.len(), 4, "listed, dimmed");
    }

    #[test]
    fn camera_flags_lay_over_the_editors_framing() {
        let mut camera = Camera::new();
        camera.distance = 3.0;
        camera.yaw = 0.25;
        CameraFlags {
            distance: Some(9.0),
            pitch: Some(30.0),
            ..Default::default()
        }
        .apply(&mut camera);
        assert_eq!(camera.distance, 9.0);
        assert_eq!(camera.yaw, 0.25, "a flag left out keeps the editor's");
        assert!((camera.pitch - 30f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn a_frame_nobody_asked_to_time_keeps_nothing() {
        let mut timer = FrameTimer::default();
        let t0 = Instant::now();
        assert_eq!(timer.begin(t0, false, None), None);
        timer.lap(|t| &mut t.update_ms);
        assert!(timer.open.is_none() && timer.epoch.is_none());
    }

    #[test]
    fn a_timed_frame_is_handed_on_with_its_wall_time_when_the_next_begins() {
        let mut timer = FrameTimer::default();
        let t0 = Instant::now();
        assert_eq!(timer.begin(t0, true, Some(16.7)), None, "nothing before it");
        timer.lap(|t| &mut t.update_ms);
        timer.lap(|t| &mut t.draw_ms);
        let t1 = t0 + Duration::from_millis(20);
        let done = timer.begin(t1, true, Some(16.7)).expect("the first frame");
        assert_eq!(done.at, 0.0);
        assert!((done.wall_ms - 20.0).abs() < 1e-3, "{}", done.wall_ms);
        assert_eq!(done.refresh_ms, Some(16.7));
        assert!(done.update_ms + done.draw_ms <= done.wall_ms + 1.0);
        assert!(done.unaccounted_ms() >= 0.0);
        // An abandoned frame isn't handed on; nor is one once F2 closes
        timer.abandon();
        assert_eq!(
            timer.begin(t1 + Duration::from_millis(17), true, None),
            None
        );
        assert_eq!(
            timer.begin(t1 + Duration::from_millis(34), false, None),
            None
        );
        assert!(timer.open.is_none());
        let next = timer.begin(t1 + Duration::from_millis(51), true, None);
        assert_eq!(next, None, "a frame after a closed one starts afresh");
    }

    /// What a watch hands the frame within `secs`, polled as a frame would.
    fn changes_within(watch: &FileWatch, secs: u64) -> Vec<PathBuf> {
        let until = Instant::now() + Duration::from_secs(secs);
        let mut seen = Vec::new();
        while Instant::now() < until {
            if let Some(paths) = watch.take() {
                seen.extend(paths);
            }
            if !seen.is_empty() {
                // One more debounce window, for a second batch
                std::thread::sleep(Duration::from_millis(600));
                seen.extend(watch.take().unwrap_or_default());
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        seen
    }

    #[test]
    fn a_watched_folder_sees_a_file_deep_inside_it_and_the_editor_filters() {
        let dir = std::env::temp_dir().join(format!("flint-host-watch-{}", std::process::id()));
        let deep = dir.join("styles").join("lib");
        std::fs::create_dir_all(&deep).unwrap();
        let watch = FileWatch::new(&[], std::slice::from_ref(&dir)).unwrap();
        assert!(watch.take().is_none(), "nothing changed yet");

        let wgsl = deep.join("noise.wgsl");
        std::fs::write(&wgsl, "// one").unwrap();
        let paths = changes_within(&watch, 10);
        let _ = std::fs::remove_dir_all(&dir);

        let named = |p: &PathBuf| p.file_name().is_some_and(|n| n == "noise.wgsl");
        assert!(paths.iter().any(named), "{paths:?}");
        // A filter that leaves WGSL out counts none of the batch; one that
        // takes everything counts it
        let not_wgsl = |p: &Path| p.extension().is_none_or(|e| e != "wgsl");
        let only_wgsl: Vec<PathBuf> = paths.iter().filter(|p| named(p)).cloned().collect();
        assert!(!batch_counts(&only_wgsl, not_wgsl));
        assert!(batch_counts(&only_wgsl, |_| true));
        assert!(!batch_counts(&[], |_| true), "an empty batch calls nothing");
    }

    #[test]
    fn a_sheet_fits_every_image_and_keeps_its_size() {
        let red = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        let blue = image::RgbaImage::from_pixel(16, 8, image::Rgba([0, 0, 255, 255]));
        let sheet = image_sheet(&[red, blue], 200, 100, [10, 10, 10]);
        assert_eq!(sheet.dimensions(), (200, 100));
        // Two across: red's cell is the left half, blue's the right
        assert_eq!(sheet.get_pixel(50, 50).0, [255, 0, 0, 255]);
        assert_eq!(sheet.get_pixel(150, 50).0, [0, 0, 255, 255]);
        // A corner is the background
        assert_eq!(sheet.get_pixel(0, 0).0, [10, 10, 10, 255]);
        // None: all background
        let empty = image_sheet(&[], 4, 4, [1, 2, 3]);
        assert_eq!(empty.get_pixel(2, 2).0, [1, 2, 3, 255]);
    }
}
