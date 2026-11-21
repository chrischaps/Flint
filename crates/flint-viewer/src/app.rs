//! Main viewer application — combines wgpu scene rendering with egui panels.
//! Supports an optional spline editor mode for interactive track editing.

use crate::edits::FieldEdits;
use crate::host::{self, Editor, Flow, Gpu, HostOptions, InitCx, Target, Tick};
use crate::panels::transform_gizmo::apply_gizmo_delta;
use crate::panels::{
    CameraView, DiagnosticsPanel, EntityInspector, FrameStats, GizmoAction, GizmoMode, SceneTree,
    SplinePanelAction, TransformGizmo, ViewGizmo,
};
use crate::picking::{build_pick_targets, pick_entity};
use crate::spline_editor::{DragMode, SplineEditor, SplineEditorConfig};
use crate::undo::{EditAction, UndoCommand, UndoStack};
use crate::watch::StyleSources;
use anyhow::{Context, Result};
use flint_constraint::{ConstraintEvaluator, ConstraintRegistry};
use flint_core::components as comp;
use flint_ecs::FlintWorld;
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::shallow_wooze::{GrassWooze, ShallowWooze};
use flint_render::{
    post_process_config_from_def, Camera, FrameInputs, PostProcessConfig, RendererConfig,
    SceneBinding, SceneRenderer,
};
use flint_scene::{
    load_failed, load_scene, load_scene_after, CameraDef, PostProcessDef, SceneDocument,
};
use flint_schema::{SchemaDirs, SchemaRegistry};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, Debouncer};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::Window;

struct ViewerState {
    world: FlintWorld,
    registry: SchemaRegistry,
    constraint_registry: ConstraintRegistry,
    scene_path: String,
    needs_reload: bool,
    scene_doc: Option<SceneDocument>,
    /// The scene's authored `[post_process]` block, if any — applied on load
    /// so the viewer shows the authored look (DoF, vignette, fog, …);
    /// F4's "Authored post" button toggles between it and the viewer's
    /// default config.
    scene_post: Option<PostProcessDef>,
    /// The scene's wooze field, if it has a `[wooze]` table. The viewer reads it
    /// for the shading boil only (#214); it doesn't run living props.
    wooze: Option<flint_wooze::WoozeField>,
    /// The scene's authored `[camera]` block, if any — seeds the orbit camera
    /// on load (and on Space reset) so the viewer opens on the authored
    /// framing instead of the generic default.
    scene_camera: Option<CameraDef>,
    /// The scene's `[environment]`, whose lighting levers binding the scene
    /// sets (#739: the viewer never read it before).
    environment: Option<flint_scene::EnvironmentDef>,
}

/// `flint edit scene.toml --style ... --style-time ...` (#328): style words
/// written over the scene's post-processing at load and at every hot
/// reload, and the post time styles see. `--motion-hold` (#871) is the
/// whole-frame motion on twos rate, written over the scene's `[post_process]
/// motion_hold` the same way.
#[derive(Debug, Clone, Default)]
pub struct ViewerStyle {
    pub specs: Vec<String>,
    pub time: Option<f32>,
    pub motion_hold: Option<f32>,
}

/// The viewer window's size in physical pixels and whether it draws the
/// editor grid: `flint edit scene.toml --width --height --no-grid` (#423).
/// The grid writes depth, so a depth-reading style traces it; `grid: false`
/// keeps it out of the frame from the first one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerWindow {
    pub width: u32,
    pub height: u32,
    pub grid: bool,
}

impl Default for ViewerWindow {
    fn default() -> Self {
        Self {
            width: 1600,
            height: 900,
            grid: true,
        }
    }
}

impl ViewerWindow {
    /// The host's options for this window: its size, the camera flags
    /// left to the scene's framing.
    pub fn host_options(&self) -> HostOptions {
        HostOptions {
            width: self.width,
            height: self.height,
            camera: Default::default(),
            auto_orbit: false,
        }
    }
}

/// What the viewer loads for a scene: the schemas and constraints of every
/// directory the one resolver names (ADR 0114: `--schemas` flags if given,
/// else the project manifest's list, else `schemas/` in the working
/// directory, beside the scene and one level up), and those directories, to
/// watch. Every flag counts (#704: the viewer used to take the first).
pub fn load_schemas(
    scene_path: &str,
    schemas: &[String],
) -> Result<(SchemaRegistry, ConstraintRegistry, SchemaDirs)> {
    let (registry, dirs) = SchemaRegistry::resolve(Some(Path::new(scene_path)), schemas)?;
    if dirs.dirs.is_empty() {
        eprintln!("Warning: No schemas directories found");
    }
    let mut constraints = ConstraintRegistry::new();
    for dir in &dirs.dirs {
        // A file that doesn't read leaves the viewer without that
        // directory's constraints, and says so (#687); a constraint whose
        // query doesn't parse is kept with its file, and the evaluator
        // shows it in the status panel as an error
        match ConstraintRegistry::load_from_directory(dir) {
            Ok(loaded) => constraints.extend(loaded),
            Err(e) => eprintln!("Warning: constraints not loaded: {e}"),
        }
    }
    Ok((registry, constraints, dirs))
}

/// What the status panel lists: one line a violation, and one for each
/// constraint that can't be evaluated (a query that doesn't parse), which
/// the evaluator reports as an error naming the constraint and its file
/// (#687). It used to show nothing for those, so a typo read as a pass.
fn violation_lines(
    world: &FlintWorld,
    registry: &SchemaRegistry,
    constraints: &ConstraintRegistry,
) -> Vec<String> {
    ConstraintEvaluator::new(world, registry, constraints)
        .validate()
        .violations
        .iter()
        .map(|v| format!("[{:?}] {}: {}", v.severity, v.entity_name, v.message))
        .collect()
}

#[cfg(test)]
mod constraint_tests {
    use super::*;

    #[test]
    fn a_constraint_with_a_bad_query_is_listed_as_an_error() {
        let mut constraints = ConstraintRegistry::new();
        constraints
            .load_string(
                "[[constraint]]\nname = \"doors_shut\"\n\
                 query = \"entities where archetype == 'door' and door.locked == true\"\n\
                 severity = \"warning\"\nmessage = \"m\"\n\
                 [constraint.kind]\ntype = \"required_child\"\nchild_archetype = \"handle\"\n",
            )
            .unwrap();
        let lines = violation_lines(&FlintWorld::new(), &SchemaRegistry::new(), &constraints);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("[Error] [constraint]: constraint 'doors_shut': its query"),
            "{lines:?}"
        );
        assert!(lines[0].contains("can't be evaluated"), "{lines:?}");
    }
}

/// Run the viewer application (standard viewer mode). `schemas` are the
/// `--schemas` flags, empty when none were given.
pub fn run(
    scene_path: &str,
    watch: bool,
    schemas: &[String],
    inspector: bool,
    auto_orbit: bool,
    style: ViewerStyle,
    window: ViewerWindow,
) -> Result<()> {
    let options = window.host_options();
    host::run(
        viewer(
            scene_path, watch, schemas, inspector, auto_orbit, style, window,
        )?,
        options,
    )
}

/// The scene viewer on `scene_path`, for [`crate::host`] to open in a
/// window or draw as a `--render` still (#712).
pub fn viewer(
    scene_path: &str,
    watch: bool,
    schemas: &[String],
    inspector: bool,
    auto_orbit: bool,
    style: ViewerStyle,
    window: ViewerWindow,
) -> Result<ViewerApp> {
    let (registry, constraint_registry, schema_dirs) = load_schemas(scene_path, schemas)?;

    let binding = scene_binding(scene_path);
    // A word may name one of the scene's styles, registered just now
    flint_render::stylize::check_style_specs(&style.specs, binding.styles())
        .map_err(anyhow::Error::msg)?;
    // A failure names the file (#1044)
    let (world, mut scene_file) =
        load_scene(scene_path, &registry).with_context(|| load_failed(scene_path))?;
    // Living props' tables: checked, and the preset [wooze] names filled in (#686)
    flint_procgen::settle_scene(&mut scene_file, std::path::Path::new(scene_path))
        .with_context(|| load_failed(scene_path))?;

    eprintln!("Loaded scene: {}", scene_file.scene.name);
    eprintln!("Entities: {}", world.entity_count());

    // Parse the scene file as an editable document for structure-preserving saves
    let scene_doc = SceneDocument::from_file(scene_path).ok();

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
        scene_doc,
        scene_post: scene_file
            .post_process
            .as_ref()
            .map(|d| flint_render::look::resolve_scene(d, scene_path)),
        wooze: field_of(&scene_file),
        scene_camera: scene_file.camera.clone(),
        environment: scene_file.environment.clone(),
    }));

    let watcher = if watch {
        let watcher = watch_scene(scene_path, &schema_dirs.dirs, &state)?;
        eprintln!("Watching for changes in scene and related assets...");
        Some(watcher)
    } else {
        None
    };

    let mut app = ViewerApp::new(state, inspector);
    app.auto_orbit = auto_orbit;
    app.style_specs = style.specs;
    app.style_time = style.time;
    app.motion_hold = style.motion_hold.map(|rate| rate.max(0.0));
    app.window_opts = window;
    app.scene_binding = Some(binding);
    app._watcher = watcher;
    Ok(app)
}

/// Run the viewer with a pre-built world (no file loading or hot-reload).
/// `scene_path_anchor` is used for model path resolution only.
pub fn run_with_world(
    world: FlintWorld,
    registry: SchemaRegistry,
    scene_path_anchor: &str,
    inspector: bool,
) -> Result<()> {
    let constraint_registry = ConstraintRegistry::default();

    eprintln!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path_anchor.to_string(),
        needs_reload: false,
        scene_doc: None,
        scene_post: None,
        wooze: None,
        scene_camera: None,
        environment: None,
    }));

    let app = ViewerApp::new(state, inspector);
    host::run(app, ViewerWindow::default().host_options())
}

/// Run the viewer in editor mode with a spline editor configuration.
/// `schemas` are the `--schemas` flags, empty when none were given.
pub fn run_editor(
    scene_path: &str,
    watch: bool,
    schemas: &[String],
    editor_config: SplineEditorConfig,
    window: ViewerWindow,
) -> Result<()> {
    let options = window.host_options();
    host::run(
        track_editor(scene_path, watch, schemas, editor_config, window)?,
        options,
    )
}

/// The viewer in editor mode with a spline editor configuration, for
/// [`crate::host`] (#712).
pub fn track_editor(
    scene_path: &str,
    watch: bool,
    schemas: &[String],
    editor_config: SplineEditorConfig,
    window: ViewerWindow,
) -> Result<ViewerApp> {
    let (registry, constraint_registry, schema_dirs) = load_schemas(scene_path, schemas)?;

    let binding = scene_binding(scene_path);
    let (world, mut scene_file) =
        load_scene(scene_path, &registry).with_context(|| load_failed(scene_path))?;
    flint_procgen::settle_scene(&mut scene_file, std::path::Path::new(scene_path))
        .with_context(|| load_failed(scene_path))?;

    eprintln!("Loaded scene: {}", scene_file.scene.name);
    eprintln!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
        scene_doc: None,
        scene_post: scene_file
            .post_process
            .as_ref()
            .map(|d| flint_render::look::resolve_scene(d, scene_path)),
        wooze: field_of(&scene_file),
        // Editor mode overrides the camera with its birds-eye view; never seeded.
        scene_camera: None,
        environment: scene_file.environment.clone(),
    }));

    let watcher = if watch {
        let watcher = watch_scene(scene_path, &schema_dirs.dirs, &state)?;
        eprintln!("Watching for changes...");
        Some(watcher)
    } else {
        None
    };

    let editor = SplineEditor::from_config(editor_config);
    let mut app = ViewerApp::new_editor(state, editor);
    app.window_opts = window;
    app.scene_binding = Some(binding);
    app._watcher = watcher;
    Ok(app)
}

/// The main viewer application
pub struct ViewerApp {
    state: Arc<Mutex<ViewerState>>,
    window: Option<Arc<Window>>,
    /// The host's GPU (#712)
    render_context: Option<Gpu>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input state
    mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    right_mouse_pressed: bool,
    middle_mouse_pressed: bool,
    modifiers: ModifiersState,
    held_orbit_keys: std::collections::HashSet<KeyCode>,

    // egui state
    egui_ctx: egui::Context,
    show_inspector: bool,

    // Panel state
    scene_tree: SceneTree,
    entity_inspector: EntityInspector,
    /// F2's frame history and graph, fed only while F2 is open (#1110)
    frame_stats: FrameStats,
    show_stats: bool,
    view_gizmo: ViewGizmo,
    transform_gizmo: TransformGizmo,

    // Camera snap animation
    camera_snap_target: Option<(f32, f32)>,
    last_frame_time: Instant,

    // Constraint violations cache
    violation_count: usize,
    violation_messages: Vec<String>,

    /// The scene's diagnostics, as `flint validate` lists them (#795)
    diagnostics: DiagnosticsPanel,

    // Editor mode (None = standard viewer)
    editor: Option<SplineEditor>,

    // Undo/redo
    undo_stack: UndoStack,

    // Dirty tracking + save
    dirty: bool,
    last_save_time: Option<Instant>,
    suppress_reload_until: Option<Instant>,
    /// Until when the next reload is the one F4's Commit to scene caused
    /// (#516): it leaves the post config on screen as it is, so tweaks
    /// never written anywhere (bloom, fog, another style) survive it.
    own_commit_until: Option<Instant>,
    /// Frames counted for the user style poll in `check_reload` (#275).
    user_style_polls: u32,

    // Pending edit actions from inspector (collected inside egui closure, applied after)
    pending_edits: Vec<EditAction>,

    // Gizmo undo coalescing: when a drag ends, we push a single undo command
    // from the stored start transform to the current transform
    gizmo_drag_ended: bool,

    /// Every field edited since the file was read or saved, with the value
    /// it had then (#679): `dirty` is whether the world differs from it
    edits: FieldEdits,
    /// Why the last save couldn't write some of those edits, shown in the
    /// status bar while they're still unsaved
    save_problems: Vec<String>,

    // Status message (e.g., "Saved!" in editor mode)
    status_message: Option<(String, Instant)>,

    // Scene [post_process] application (authored <-> viewer default, toggled
    // from the Rendering & Effects menu)
    scene_post_enabled: bool,
    /// A look picked in F4 (#356; `Some(None)`: none), kept over hot
    /// reloads so a look file can be tuned while it is on screen.
    picked_look: Option<Option<String>>,
    default_post_config: Option<PostProcessConfig>,

    // Viewer DoF follow: focus tracks the last selected entity; toggled from
    // the Rendering & Effects menu, strength/range live in its DoF section.
    dof_follow: bool,
    dof_focus_entity: Option<flint_core::EntityId>,

    // Rendering & Effects menu (F4, ADR 0053): the consolidated render/post
    // debug controls, shared with the player. `open` tracked viewer-side so
    // the egui Window can own a close button.
    render_panel: flint_debug_ui::RenderDebugPanel,
    render_panel_open: bool,

    // Auto-orbit turntable (O toggles, [/] adjust speed, any manual orbit
    // input cancels — same convention as the model/procgen previewers).
    auto_orbit: bool,
    auto_orbit_speed: f32,

    /// `--style name[:key=value,...]` words (#328), written over the
    /// config at load, at each hot reload and at the authored/default
    /// swap, so the command line keeps winning over the scene file.
    style_specs: Vec<String>,
    /// `--style-time`: the post time animated styles see.
    style_time: Option<f32>,
    /// `--motion-hold` (#871): the whole-frame motion on twos rate, kept
    /// over the scene's as `--style` is.
    motion_hold: Option<f32>,
    /// The clock motion held on twos ticks by (#871): seconds since the
    /// viewer opened. The viewer's post time stands still (at 0, or
    /// `--style-time`), so a hold stamped with it would never turn and a
    /// dragged held entity would stay where it was first drawn; on its own
    /// wall clock it steps, `rate` times a second, as it will in the player.
    motion_clock: Instant,
    /// `--width` / `--height` / `--no-grid` (#423): the window's first size
    /// and whether the renderer draws the editor grid.
    window_opts: ViewerWindow,

    // Live particles (ADR 0068): the viewer steps the same ParticleSystem the
    // player uses, so inspector edits to particle_emitter / particle_effect
    // components show immediately. Controls live in the F4 menu.
    particles: flint_particles::ParticleSystem,
    show_particles: bool,
    particles_paused: bool,
    particle_speed: f32,
    particle_alive: usize,
    last_particle_tick: Instant,
    /// The shading boil's clock (#214): beats, and seconds for the field's ramps.
    boil_clock: flint_wooze::BoilClock,
    boil_time: f64,
    last_boil_tick: Instant,
    /// This frame's boil drawing, for the frame's inputs.
    frame_boil: flint_wooze::BoilFrame,
    /// Shallow wooze's and woozy grass's clocks (#215, #252), on the same
    /// field clock as the boil, as the player's living subsystem keeps them.
    shallow_wooze: ShallowWooze,
    grass_wooze: GrassWooze,
    /// Seconds the last frame took, for styles whose history is a rate (#342).
    frame_dt: f32,
    /// The scene as the renderer binds it (#739): made when its user styles
    /// were registered, bound at load and at each hot reload.
    scene_binding: Option<SceneBinding>,
    /// `--watch`'s watcher, alive as long as the viewer
    _watcher: Option<Debouncer<RecommendedWatcher>>,
}

/// The style settings a scene authors: its `[post_process.styles]`, or
/// none (every style off) when it has no `[post_process]` block.
/// The folder a scene file sits in.
fn scene_dir_of(scene_path: &str) -> &Path {
    Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
}

/// `--watch`: the scene file, its folder (recursively) and the schema
/// folders, marking the scene for a reload on a change. A batch that is
/// only user styles' WGSL and descriptions marks nothing (#429): the
/// style's pass rebuilds its shader in place, and `check_reload`'s poll
/// of the descriptions reloads the scene only when one changed.
fn watch_scene(
    scene_path: &str,
    schema_dirs: &[PathBuf],
    state: &Arc<Mutex<ViewerState>>,
) -> Result<Debouncer<RecommendedWatcher>> {
    let state = Arc::clone(state);
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
    let scene_file = Path::new(scene_path);
    let scene_dir = scene_dir_of(scene_path);
    debouncer
        .watcher()
        .watch(scene_file, RecursiveMode::NonRecursive)?;
    debouncer
        .watcher()
        .watch(scene_dir, RecursiveMode::Recursive)?;
    for dir in schema_dirs {
        debouncer.watcher().watch(dir, RecursiveMode::Recursive)?;
    }
    let style_sources = StyleSources::for_scene_dir(scene_dir);

    std::thread::spawn(move || {
        for result in rx {
            match result {
                Ok(events) => {
                    let paths = events.iter().map(|e| e.path.as_path());
                    if !style_sources.wants_scene_reload(paths) {
                        tracing::debug!("style sources changed; left to the style's own reload");
                        continue;
                    }
                    if let Ok(mut state) = state.lock() {
                        state.needs_reload = true;
                    }
                }
                Err(e) => {
                    tracing::warn!("Watch error: {:?}", e);
                }
            }
        }
    });
    Ok(debouncer)
}

/// The renderer's share of the scene, at load and at each hot reload
/// (#739), as the player and `flint render` take it: the scene bound (its
/// project and folder, which glyph sets, palettes and papers are found
/// from, #265, ADR 0114; nothing of a last load; its `[environment]`
/// levers; its terrain, masked when styled, and its grass, a grass_ground's
/// over a terrain's, #332), then its models. The viewer loaded no terrain
/// and read no `[environment]` before.
fn stage_scene(
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    state: &mut ViewerState,
    binding: SceneBinding,
    config: &ModelLoadConfig,
) -> flint_render::BoundScene {
    let binding = binding
        .with_environment(state.environment.as_ref())
        .with_archetypes(&state.registry);
    let bound = renderer.bind_scene(device, queue, &state.world, &binding);
    model_loader::load_models_from_world(&mut state.world, renderer, device, queue, config);
    bound
}

/// The scene as the renderer binds it (#739), which registers the
/// user-authored styles (`styles/*.shader.toml`, ADR 0112) the scene can
/// see, before its `[post_process]` is read, so its tables for them aren't
/// dropped as unknown (#275).
fn scene_binding(scene_path: &str) -> SceneBinding {
    let binding = SceneBinding::for_scene(Path::new(scene_path));
    for message in binding.style_messages() {
        eprintln!("Warning: {message}");
    }
    binding
}

/// How long after Commit to scene writes the file the reload is taken for
/// its own (#516). The watcher reports a write after its half-second
/// debounce; the window only stops a commit it missed from keeping the F4
/// post over some later edit of the file.
const OWN_COMMIT_RELOAD: Duration = Duration::from_secs(3);

/// F4's Commit to scene just wrote the file through a document of its own
/// (#1046): append each `[post_process]` key it changed to the change list,
/// in a save's line shape, by comparing the file with the document the
/// viewer holds, and hold that document anew, so a Ctrl+S before the
/// watcher's reload doesn't write the old `[post_process]` back over it.
fn log_commit(state: &mut ViewerState) {
    let now_doc = match SceneDocument::from_file(&state.scene_path) {
        Ok(doc) => doc,
        Err(e) => {
            tracing::warn!("Commit not logged: {e}");
            return;
        }
    };
    let before = state
        .scene_doc
        .as_ref()
        .map(|d| d.to_string())
        .unwrap_or_default();
    let changes = crate::edits::post_process_changes(&before, &now_doc.to_string());
    if !changes.is_empty() {
        append_change_list(&state.scene_path, |scene, now| {
            crate::edits::commit_log_lines(&changes, scene, now)
        });
    }
    state.scene_doc = Some(now_doc);
}

/// Append lines to the project's change list, `.flint/viewer-edits.jsonl`
/// (#679): `lines` is given the scene as the log names it and the time.
fn append_change_list(scene_path: &str, lines: impl FnOnce(&str, u64) -> String) {
    let path = crate::edits::edit_log_path(Path::new(scene_path));
    #[allow(clippy::disallowed_methods)] // a wall-clock stamp for the log, native only
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let lines = lines(&crate::edits::scene_in_log(&path, scene_path), now);
    if let Err(e) = crate::edits::append_log(&path, &lines) {
        tracing::warn!("Couldn't append to {}: {e}", path.display());
    }
}

/// `config` with the `--style` words (#328) and `--motion-hold` (#871)
/// written over it.
fn with_flags(
    mut config: PostProcessConfig,
    specs: &[String],
    motion_hold: Option<f32>,
    styles: &flint_render::stylize::StyleRegistry,
) -> PostProcessConfig {
    for e in flint_render::stylize::apply_style_specs(&mut config, specs, styles) {
        eprintln!("Warning: {e}");
    }
    if let Some(rate) = motion_hold {
        config.motion_hold = rate;
    }
    config
}

/// A reloaded scene's `[post_process]` with its look laid under its keys
/// (#356): the look picked in F4 when there is one, else the scene's own.
fn with_look(
    def: Option<PostProcessDef>,
    scene_path: &str,
    picked: Option<&Option<String>>,
) -> Option<PostProcessDef> {
    let Some(picked) = picked else {
        return def.map(|d| flint_render::look::resolve_scene(&d, scene_path));
    };
    let base = def.or_else(|| PostProcessDef::from_authored(toml::Table::new()).ok())?;
    match flint_render::look::resolve(&base, picked.as_deref(), scene_path) {
        Ok(out) => Some(out),
        Err(e) => {
            eprintln!("look: {e}");
            Some(flint_render::look::resolve_scene(&base, scene_path))
        }
    }
}

fn authored_styles(
    def: Option<&PostProcessDef>,
    styles: &flint_render::stylize::StyleRegistry,
) -> flint_render::stylize::StyleSettings {
    def.map(|d| post_process_config_from_def(d, styles).styles)
        .unwrap_or_default()
}

/// Rendering & Effects menu interactions that need `&mut self` side effects
/// (authored-post swap, DoF follow enable/disable), collected inside the egui
/// closure and handled after the frame.
#[derive(Default)]
struct RenderMenuActions {
    authored_post_clicked: bool,
    dof_follow: Option<bool>,
    particles_show: Option<bool>,
    particles_pause: Option<bool>,
    particles_speed: Option<f32>,
    particles_restart: bool,
}

impl ViewerApp {
    fn new(state: Arc<Mutex<ViewerState>>, show_inspector: bool) -> Self {
        Self {
            state,
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            mouse_pressed: false,
            last_mouse_pos: None,
            right_mouse_pressed: false,
            middle_mouse_pressed: false,
            modifiers: ModifiersState::empty(),
            held_orbit_keys: std::collections::HashSet::new(),
            egui_ctx: egui::Context::default(),
            show_inspector,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            frame_stats: FrameStats::new(),
            show_stats: false,
            view_gizmo: ViewGizmo::new(),
            transform_gizmo: TransformGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            diagnostics: DiagnosticsPanel::new(),
            editor: None,
            undo_stack: UndoStack::new(),
            dirty: false,
            last_save_time: None,
            suppress_reload_until: None,
            own_commit_until: None,
            user_style_polls: 0,
            pending_edits: Vec::new(),
            gizmo_drag_ended: false,
            edits: FieldEdits::default(),
            save_problems: Vec::new(),
            status_message: None,
            scene_post_enabled: true,
            picked_look: None,
            default_post_config: None,
            dof_follow: false,
            dof_focus_entity: None,
            render_panel: flint_debug_ui::RenderDebugPanel::new(false),
            render_panel_open: false,
            auto_orbit: false,
            auto_orbit_speed: 0.5,
            style_specs: Vec::new(),
            style_time: None,
            motion_hold: None,
            motion_clock: Instant::now(),
            window_opts: ViewerWindow::default(),
            particles: flint_particles::ParticleSystem::new(),
            show_particles: true,
            particles_paused: false,
            particle_speed: 1.0,
            particle_alive: 0,
            last_particle_tick: Instant::now(),
            boil_clock: Default::default(),
            boil_time: 0.0,
            last_boil_tick: Instant::now(),
            frame_boil: Default::default(),
            shallow_wooze: Default::default(),
            grass_wooze: Default::default(),
            frame_dt: 1.0 / 60.0,
            scene_binding: None,
            _watcher: None,
        }
    }

    fn new_editor(state: Arc<Mutex<ViewerState>>, editor: SplineEditor) -> Self {
        Self {
            state,
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            mouse_pressed: false,
            last_mouse_pos: None,
            right_mouse_pressed: false,
            middle_mouse_pressed: false,
            modifiers: ModifiersState::empty(),
            held_orbit_keys: std::collections::HashSet::new(),
            egui_ctx: egui::Context::default(),
            show_inspector: true,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            frame_stats: FrameStats::new(),
            show_stats: false,
            view_gizmo: ViewGizmo::new(),
            transform_gizmo: TransformGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            diagnostics: DiagnosticsPanel::new(),
            editor: Some(editor),
            undo_stack: UndoStack::new(),
            dirty: false,
            last_save_time: None,
            suppress_reload_until: None,
            own_commit_until: None,
            user_style_polls: 0,
            pending_edits: Vec::new(),
            gizmo_drag_ended: false,
            edits: FieldEdits::default(),
            save_problems: Vec::new(),
            status_message: None,
            scene_post_enabled: true,
            picked_look: None,
            default_post_config: None,
            dof_follow: false,
            dof_focus_entity: None,
            render_panel: flint_debug_ui::RenderDebugPanel::new(false),
            render_panel_open: false,
            auto_orbit: false,
            auto_orbit_speed: 0.5,
            style_specs: Vec::new(),
            style_time: None,
            motion_hold: None,
            motion_clock: Instant::now(),
            window_opts: ViewerWindow::default(),
            particles: flint_particles::ParticleSystem::new(),
            show_particles: true,
            particles_paused: false,
            particle_speed: 1.0,
            particle_alive: 0,
            last_particle_tick: Instant::now(),
            boil_clock: Default::default(),
            boil_time: 0.0,
            last_boil_tick: Instant::now(),
            frame_boil: Default::default(),
            shallow_wooze: Default::default(),
            grass_wooze: Default::default(),
            frame_dt: 1.0 / 60.0,
            scene_binding: None,
            _watcher: None,
        }
    }

    /// Seed the orbit camera from the scene's authored `[camera]` block so
    /// the viewer opens on (and Space returns to) the authored framing.
    /// Orbit distance/yaw/pitch are derived from position/target the same
    /// way the player does, so `update_orbit()` stays consistent.
    fn seed_camera_from_scene(&mut self) {
        let scene_camera = self.state.lock().unwrap().scene_camera.clone();
        let Some(cam) = scene_camera else { return };
        if let Some(target) = cam.target {
            self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam.fov {
            self.camera.fov = fov;
        }
        if let Some(near) = cam.near {
            self.camera.near = near;
        }
        if let Some(far) = cam.far {
            self.camera.far = far;
        }
        if let Some(pos) = cam.position {
            let pos = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
            let dir = pos - self.camera.target;
            let dist = dir.length();
            if dist > 0.001 {
                self.camera.distance = dist;
                let n = dir * (1.0 / dist);
                self.camera.pitch = n.y.asin();
                self.camera.yaw = n.x.atan2(n.z);
            }
        }
    }

    /// The window's or the still's GPU is here (#712): the scene staged on
    /// it, the authored post-processing applied.
    fn initialize(&mut self, cx: InitCx) -> Result<()> {
        let render_context = cx.gpu.clone();
        self.window = cx.window;
        self.egui_ctx = cx.egui.clone();
        self.camera.aspect = render_context.aspect();

        // Standard viewer: open on the scene's authored [camera] framing
        if self.editor.is_none() {
            self.seed_camera_from_scene();
        }

        // In editor mode, start with a good birds-eye view
        if self.editor.is_some() {
            self.camera.distance = 150.0;
            self.camera.pitch = 1.0; // ~57 degrees, nearly top-down
            self.camera.yaw = 0.0;
            // Center on approximate track center
            if let Some(editor) = &self.editor {
                if !editor.control_points.is_empty() {
                    let n = editor.control_points.len() as f32;
                    let cx: f32 = editor
                        .control_points
                        .iter()
                        .map(|p| p.position[0])
                        .sum::<f32>()
                        / n;
                    let cz: f32 = editor
                        .control_points
                        .iter()
                        .map(|p| p.position[2])
                        .sum::<f32>()
                        / n;
                    self.camera.target = flint_core::Vec3::new(cx, 0.0, cz);
                }
            }
        }

        self.camera.update_orbit();

        let mut scene_renderer = render_context.scene_renderer(RendererConfig {
            show_grid: self.window_opts.grid,
            ..Default::default()
        });

        // Load models and update meshes from world
        {
            let mut state = self.state.lock().unwrap();
            let config = ModelLoadConfig::from_scene_path(&state.scene_path);
            stage_scene(
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &mut state,
                self.bound_scene(&config),
                &config,
            );

            // Particles (ADR 0068): effects next to the scene, then textures.
            let project = &config.project;
            flint_particles::load_particle_effects_from_world(
                project,
                &state.scene_path,
                &mut self.particles,
            );
            self.particles.sync.sync_from_world(&state.world);
            flint_render::load_scene_particle_textures(
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &self.particles.sync,
                project,
                scene_dir_of(&state.scene_path),
            );
        }

        // Apply the scene's authored [post_process] block (DoF, vignette,
        // fog, …) so the viewer shows the authored look. F4's "Authored post"
        // button toggles back to the viewer's default config (kept here for that).
        self.default_post_config = Some(scene_renderer.post_process_config().clone());
        {
            let state = self.state.lock().unwrap();
            if let Some(def) = &state.scene_post {
                scene_renderer.set_post_process_config(with_flags(
                    post_process_config_from_def(def, scene_renderer.styles()),
                    &self.style_specs,
                    self.motion_hold,
                    scene_renderer.styles(),
                ));
                eprintln!(
                    "Scene [post_process] applied (F4 > Authored post toggles scene/default)"
                );
            } else if !self.style_specs.is_empty() || self.motion_hold.is_some() {
                let config = scene_renderer.post_process_config().clone();
                scene_renderer.set_post_process_config(with_flags(
                    config,
                    &self.style_specs,
                    self.motion_hold,
                    scene_renderer.styles(),
                ));
            }
            if !self.style_specs.is_empty() {
                eprintln!("--style applied: {}", self.style_specs.join(" "));
            }
            if let Some(rate) = self.motion_hold {
                eprintln!("--motion-hold applied: the whole frame drawn {rate} times a second");
            }
            // The F4 style editor marks edits against these (#268)
            self.render_panel.set_authored_styles(Some(authored_styles(
                state.scene_post.as_ref(),
                scene_renderer.styles(),
            )));
            // ...and offers the scene's own palettes and glyph sets (#322)
            self.render_panel
                .set_scene_dir(Some(scene_dir_of(&state.scene_path).to_path_buf()));
            // ...and the looks the scene can reach (#356)
            self.render_panel.set_looks(
                &flint_render::look::list(&state.scene_path),
                state.scene_post.as_ref().and_then(|d| d.look.as_deref()),
            );
            if let Some(look) = state.scene_post.as_ref().and_then(|d| d.look.as_deref()) {
                eprintln!("Scene look: {look} (F4 -> Look switches it)");
            }
            // ...and writes a style back into the file (#364)
            self.render_panel.set_scene_file(
                state
                    .scene_doc
                    .is_some()
                    .then(|| std::path::PathBuf::from(&state.scene_path)),
            );
        }

        // Run initial constraint evaluation, and say what is wrong with the scene
        self.update_constraints();
        self.update_diagnostics();

        // Build initial scene tree
        {
            let state = self.state.lock().unwrap();
            self.scene_tree.update(&state.world);
        }

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);

        Ok(())
    }

    /// The diagnostics panel for the scene as the file now stands. A viewer
    /// given a world and no file (`run_with_world`) has nothing to diagnose.
    fn update_diagnostics(&mut self) {
        let state = self.state.lock().unwrap();
        let scene = Path::new(&state.scene_path);
        if flint_core::vfs::is_file(scene) {
            self.diagnostics.refresh(scene, &state.registry);
        }
    }

    fn update_constraints(&mut self) {
        let state = self.state.lock().unwrap();
        self.violation_messages =
            violation_lines(&state.world, &state.registry, &state.constraint_registry);
        self.violation_count = self.violation_messages.len();
    }

    /// The frame before it is drawn: the camera's snap, the panels' last
    /// edits, the boil, the particles, then the frame's inputs (#739).
    fn frame_update(&mut self) {
        if self.render_context.is_none() || self.scene_renderer.is_none() {
            return;
        }

        // Animate camera snap transitions
        self.animate_camera();

        // Track the last selected entity so DoF follow keeps its target
        // after a deselect, and drive the focus plane while F12 is on.
        if let Some(id) = self.scene_tree.selected_entity() {
            self.dof_focus_entity = Some(id);
        }
        if self.dof_follow {
            self.apply_dof_follow();
        }

        // Rendering & Effects panel: apply last frame's edits / mirror live
        // state (after DoF follow so the mirror shows the followed focus).
        self.sync_render_panel();

        // Shading boil: this frame's drawing, from the scene's field (#214)
        self.tick_boil();

        // Particles: step, pack, upload (ADR 0068)
        self.tick_particles();

        // The frame's inputs and the world (#739)
        self.prepare_frame();
    }

    /// The panels over the frame (the gizmo is always drawn), then what
    /// they asked for.
    fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let (gizmo_action, menu_actions) = self.render_egui(ui);

        // Rendering & Effects menu actions that need &mut self (deferred out
        // of the egui closure's disjoint field borrows)
        if menu_actions.authored_post_clicked {
            self.toggle_authored_post();
        }
        if let Some(on) = menu_actions.dof_follow {
            self.set_dof_follow(on);
        }
        if let Some(show) = menu_actions.particles_show {
            self.show_particles = show;
        }
        if let Some(paused) = menu_actions.particles_pause {
            self.particles_paused = paused;
        }
        if let Some(speed) = menu_actions.particles_speed {
            self.particle_speed = speed;
        }
        if menu_actions.particles_restart {
            self.particles.sync.restart_all();
        }

        // Process gizmo interaction
        if let Some(action) = gizmo_action {
            match action {
                GizmoAction::SnapToView { yaw, pitch } => {
                    self.camera_snap_target = Some((yaw, pitch));
                    // The turn eases in, but the projection switches at once
                    if !self.camera.orthographic {
                        self.reset_style_history();
                    }
                    self.camera.orthographic = true;
                }
                GizmoAction::OrbitDelta { dyaw, dpitch } => {
                    self.auto_orbit = false;
                    self.camera.orbit_horizontal(dyaw);
                    self.camera.orbit_vertical(dpitch);
                    self.camera_snap_target = None;
                    if self.camera.orthographic {
                        self.reset_style_history();
                    }
                    self.camera.orthographic = false;
                }
                GizmoAction::SwitchToPerspective => {
                    self.camera.orthographic = false;
                }
            }
        }

        // Apply any pending inspector edits (outside egui closure to avoid borrow conflicts)
        self.apply_pending_edits();

        // Handle gizmo drag end -> push undo
        if self.gizmo_drag_ended {
            self.gizmo_drag_ended = false;
            self.finalize_gizmo_undo();
        }
        // A gizmo drag's frames edit the world as it moves
        self.refresh_dirty();
    }

    /// The scene as the renderer binds it: the one made when its user styles
    /// were registered, or, for a world with no scene file, one for the
    /// models' project and folder.
    fn bound_scene(&self, config: &ModelLoadConfig) -> SceneBinding {
        self.scene_binding.clone().unwrap_or_else(|| {
            SceneBinding::new(flint_core::project::AssetScope::new(
                config.project.clone(),
                config.scene_dir.clone(),
            ))
        })
    }

    /// Hand the renderer this frame's inputs and the world (#739), as the
    /// player does once a frame: the clock motion on twos ticks by (#871),
    /// the viewer's own wall clock, which the grass's wind and the shallow
    /// wooze also run on; the post time held where it stands (0, or
    /// `--style-time`); the boil, shallow wooze and woozy grass from the
    /// field's clock; the particles; the window's scale. The world is read
    /// every frame, so an edit, a drag or a held entity's tick shows on the
    /// next frame without asking.
    fn prepare_frame(&mut self) {
        let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer)
        else {
            return;
        };
        renderer.set_selected_entity(self.scene_tree.selected_entity());
        let state = self.state.lock().unwrap();
        let woozy = state.wooze.is_some();
        let inputs = FrameInputs {
            time: self.motion_clock.elapsed().as_secs_f64(),
            // History styles decay by the frame's time (#342)
            dt: self.frame_dt,
            look_time: Some(self.style_time.map_or(0.0, f64::from)),
            grass_time: self.grass_wooze.sway_time(),
            grass_wooze: self.grass_wooze.comb(),
            shallow_wooze: if woozy {
                self.shallow_wooze.draws()
            } else {
                &[]
            },
            boil: self.frame_boil,
            // Styles sized in points (ASCII cells, #265) follow the window
            pixels_per_point: self
                .window
                .as_ref()
                .map_or(1.0, |w| w.scale_factor() as f32),
            particles: self.show_particles.then_some(&self.particles.sync),
            ..Default::default()
        };
        renderer.prepare(&context.device, &context.queue, &state.world, &inputs);
    }

    /// Advance the particle simulation by real time (scaled), then pack it
    /// for the frame's inputs. Runs every frame so inspector edits show
    /// live; paused keeps the last frame on screen.
    fn tick_particles(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_particle_tick).as_secs_f32().min(0.1);
        self.last_particle_tick = now;
        if self.scene_renderer.is_none() || !self.show_particles {
            return;
        }
        {
            let state = self.state.lock().unwrap();
            if self.particles_paused {
                // Still pick up component edits so a paused frame reflects them.
                self.particles.sync.sync_from_world(&state.world);
            } else {
                self.particles.step(&state.world, dt * self.particle_speed);
            }
        }
        self.particle_alive = self.particles.sync.total_alive();
        self.particles.pack(Some(self.camera.position_array()));
    }

    /// View depth of the DoF focus entity along the camera forward axis, as
    /// (entity name, view depth). Since ADR 0055 the projection and the
    /// composite linearizer agree, so the focus distance IS the view depth.
    fn dof_focus_readout(&self, world: &FlintWorld) -> Option<(String, f32)> {
        let id = self.dof_focus_entity?;
        let pos = world.get_world_position(id)?;
        let name = world.get_name(id).unwrap_or("<unnamed>").to_string();
        let cam = &self.camera;
        let fwd = flint_core::Vec3::new(
            cam.target.x - cam.position.x,
            cam.target.y - cam.position.y,
            cam.target.z - cam.position.z,
        )
        .normalized();
        let to_entity = flint_core::Vec3::new(
            pos.x - cam.position.x,
            pos.y - cam.position.y,
            pos.z - cam.position.z,
        );
        let z = to_entity.dot(&fwd).clamp(cam.near, cam.far);
        Some((name, z))
    }

    /// Drive the renderer's DoF params from the follow state (once per frame
    /// while F12 DoF follow is on). Only the three dof_* fields are touched;
    /// writes are diff-gated. A missing focus entity (deleted, or ids
    /// remapped by a scene reload) leaves the config untouched.
    fn apply_dof_follow(&mut self) {
        let focus = {
            let state = self.state.lock().unwrap();
            self.dof_focus_readout(&state.world)
        };
        let Some((_, focus_z)) = focus else {
            return;
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        // Strength/range are authored in the Rendering & Effects menu's DoF
        // section; follow owns only the focus distance.
        let dof_strength = self.render_panel.pp.dof_strength;
        let dof_range = self.render_panel.pp.dof_focus_range;
        let config = renderer.post_process_config();
        if (config.dof_strength - dof_strength).abs() < 1e-4
            && (config.dof_focus_distance - focus_z).abs() < 1e-4
            && (config.dof_focus_range - dof_range).abs() < 1e-4
        {
            return;
        }
        let mut config = config.clone();
        config.dof_strength = dof_strength;
        config.dof_focus_distance = focus_z;
        config.dof_focus_range = dof_range;
        renderer.set_post_process_config(config);
    }

    /// Advance the field's clocks: the shading boil's drawing (#214), and
    /// shallow wooze's and woozy grass's (#215, #252), which the viewer
    /// skipped before #739. The post passes' resources (styles, FXAA) follow
    /// the config by themselves: the renderer builds them each frame (#262,
    /// #297), however the viewer set it (load, scene/default toggle, hot
    /// reload, F4).
    fn tick_boil(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_boil_tick).as_secs_f64().min(0.1);
        self.last_boil_tick = now;
        let Some(renderer) = &self.scene_renderer else {
            return;
        };

        let eye = self.camera.position_array();
        let state = self.state.lock().unwrap();
        self.frame_boil = match &state.wooze {
            Some(field) => {
                self.boil_time += dt;
                let sample = field.sample(eye, self.boil_time);
                self.boil_clock.tick(dt, &sample);
                self.shallow_wooze
                    .advance(&state.world, field, self.boil_time, dt);
                self.grass_wooze
                    .advance(renderer.grass_config(), field, eye, self.boil_time, dt);
                self.boil_clock
                    .frame(&sample, renderer.post_process_config().boil_on)
            }
            None => Default::default(),
        };
    }

    /// Restore DoF params to the active base config (authored scene post if
    /// enabled, else the viewer default) when F12 follow is switched off.
    fn restore_base_dof(&mut self) {
        let scene_post = self.state.lock().unwrap().scene_post.clone();
        let Some(renderer) = self.scene_renderer.as_mut() else {
            return;
        };
        let base = match (&scene_post, self.scene_post_enabled) {
            (Some(def), true) => Some(post_process_config_from_def(def, renderer.styles())),
            _ => self.default_post_config.clone(),
        };
        let Some(base) = base else {
            return;
        };
        let mut config = renderer.post_process_config().clone();
        config.dof_strength = base.dof_strength;
        config.dof_focus_distance = base.dof_focus_distance;
        config.dof_focus_range = base.dof_focus_range;
        renderer.set_post_process_config(config);
    }

    /// Swap between the scene's authored `[post_process]` and the viewer
    /// default config (the old F11; now a button in the Rendering & Effects
    /// menu). No-op with a console note when the scene authors no block.
    fn toggle_authored_post(&mut self) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let scene_post = self.state.lock().unwrap().scene_post.clone();
        match scene_post {
            None => println!("Scene has no [post_process] block — nothing to toggle"),
            Some(def) if !self.scene_post_enabled => {
                renderer.set_post_process_config(with_flags(
                    post_process_config_from_def(&def, renderer.styles()),
                    &self.style_specs,
                    self.motion_hold,
                    renderer.styles(),
                ));
                self.scene_post_enabled = true;
                println!("Scene post-process: ON (authored)");
            }
            Some(_) => {
                if let Some(default) = &self.default_post_config {
                    renderer.set_post_process_config(with_flags(
                        default.clone(),
                        &self.style_specs,
                        self.motion_hold,
                        renderer.styles(),
                    ));
                }
                self.scene_post_enabled = false;
                println!("Scene post-process: OFF (viewer default)");
            }
        }
    }

    /// Enable/disable DoF follow (the old F12; now a checkbox in the
    /// Rendering & Effects menu). On enable, seeds the focus entity from the
    /// current selection and gives strength a visible floor; on disable,
    /// restores the active base config's DoF.
    fn set_dof_follow(&mut self, on: bool) {
        self.dof_follow = on;
        if on {
            if self.dof_focus_entity.is_none() {
                self.dof_focus_entity = self.scene_tree.selected_entity();
            }
            if self.render_panel.pp.dof_strength <= 0.0 {
                self.render_panel.pp.dof_strength = 0.5;
            }
            match self.dof_focus_entity {
                Some(_) => println!(
                    "DoF follow: ON — focus tracks the last selected entity \
                     (tune strength/range in the Rendering & Effects menu)"
                ),
                None => println!("DoF follow: ON — select an entity to set the focus target"),
            }
        } else {
            self.restore_base_dof();
            println!("DoF follow: OFF (base post-process DoF restored)");
        }
    }

    /// Rendering & Effects panel (ADR 0053), once per frame: apply edits the
    /// user made last frame (write-through), else mirror live renderer state
    /// into the widgets so the panel always displays the truth.
    fn sync_render_panel(&mut self) {
        use flint_debug_ui::DebugPanel as _;
        // Commit to scene (#364) writes the file, and the watcher reloads
        // it, which would drop entity edits not yet saved
        self.render_panel.set_commit_blocked(self.dirty.then(|| {
            "Save the scene first (Ctrl+S): committing reloads the file, \
             and the unsaved entity edits would be lost"
                .to_string()
        }));
        let Some(renderer) = self.scene_renderer.as_mut() else {
            return;
        };
        let rp = &mut self.render_panel;
        if rp.is_dirty() {
            let Some(context) = self.render_context.as_ref() else {
                return;
            };
            let flags = rp.take_flags();
            if flags.look_changed {
                // A look laid under the scene's keys (#356), kept over hot
                // reloads; the authored post goes back on to show it
                self.picked_look = Some(rp.look.clone());
                let mut state = self.state.lock().unwrap();
                let base = state.scene_post.clone().or_else(|| {
                    flint_scene::PostProcessDef::from_authored(toml::Table::new()).ok()
                });
                match base.map(|def| {
                    flint_render::look::resolve(&def, rp.look.as_deref(), &state.scene_path)
                }) {
                    Some(Ok(def)) => {
                        renderer.set_post_process_config(with_flags(
                            post_process_config_from_def(&def, renderer.styles()),
                            &self.style_specs,
                            self.motion_hold,
                            renderer.styles(),
                        ));
                        renderer.ensure_style_resources(&context.device);
                        renderer.ensure_fxaa_resources(&context.device);
                        rp.set_authored_styles(Some(authored_styles(
                            Some(&def),
                            renderer.styles(),
                        )));
                        println!("Look: {}", rp.look.as_deref().unwrap_or("none"));
                        state.scene_post = Some(def);
                        self.scene_post_enabled = true;
                    }
                    Some(Err(e)) => eprintln!("look: {e}"),
                    None => {}
                }
            }
            if flags.styles_committed {
                // The watcher is about to reload the file just written
                self.own_commit_until = Some(Instant::now() + OWN_COMMIT_RELOAD);
                log_commit(&mut self.state.lock().unwrap());
            }
            if flags.pp_changed {
                renderer.set_post_process_config(rp.pp.clone());
                if flags.style_needs_resources {
                    renderer.ensure_style_resources(&context.device);
                }
                if flags.fxaa_needs_resources {
                    renderer.ensure_fxaa_resources(&context.device);
                }
            }
            if flags.shadows_changed {
                renderer.set_shadows(rp.shadows_enabled);
            }
            if flags.shadow_res_changed {
                renderer.set_shadow_resolution(&context.device, rp.shadow_resolution);
            }
            if flags.lighting_reset {
                renderer.reset_ambient();
            } else if flags.lighting_changed {
                renderer.set_ambient(rp.lighting.ambient_sky, rp.lighting.ambient_ground);
                renderer.set_diffuse_wrap(rp.lighting.diffuse_wrap);
                renderer.set_oren_nayar(rp.lighting.oren_nayar);
                renderer.set_sheen(rp.lighting.sheen_color, rp.lighting.sheen_strength);
            }
            if flags.mode_changed {
                renderer.set_debug_mode(rp.debug_mode);
            }
            if flags.fov_changed {
                self.camera.fov = rp.fov_deg;
            }
            rp.clear_dirty();
        } else {
            rp.refresh(
                renderer.post_process_config(),
                renderer.debug_state().mode,
                renderer.shadows_enabled(),
                renderer.shadow_resolution(),
                renderer.lighting_levers(),
                self.camera.fov,
            );
        }
        rp.set_style_ids(renderer.style_ids());
    }

    fn animate_camera(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame_time).as_secs_f32().min(0.1);
        self.last_frame_time = now;
        // ASCII's smoothing is per 60 Hz frame of this (#342)
        self.frame_dt = dt;

        // Keyboard orbit: WASD to orbit, Q/E to zoom
        {
            let speed = 2.0f32;
            let zoom_speed = 2.0f32;
            let mut yaw_delta = 0.0f32;
            let mut pitch_delta = 0.0f32;
            let mut zoom_factor = 1.0f32;

            // Auto-orbit turntable: continuous yaw until manual orbit input.
            // While it runs, the orbit centre eases onto the selected entity
            // so the thing you picked stays framed instead of drifting out.
            if self.auto_orbit {
                yaw_delta += self.auto_orbit_speed * dt;
                if let Some(id) = self.scene_tree.selected_entity() {
                    let focus = self
                        .state
                        .lock()
                        .ok()
                        .and_then(|st| st.world.get_world_position(id));
                    if let Some(p) = focus {
                        let t = 1.0 - (-6.0 * dt).exp();
                        self.camera.target.x = lerp(self.camera.target.x, p.x, t);
                        self.camera.target.y = lerp(self.camera.target.y, p.y, t);
                        self.camera.target.z = lerp(self.camera.target.z, p.z, t);
                        self.camera.update_orbit();
                    }
                }
            }

            if self.held_orbit_keys.contains(&KeyCode::KeyA) {
                yaw_delta += speed * dt;
            }
            if self.held_orbit_keys.contains(&KeyCode::KeyD) {
                yaw_delta -= speed * dt;
            }
            if self.held_orbit_keys.contains(&KeyCode::KeyW) {
                pitch_delta += speed * dt;
            }
            if self.held_orbit_keys.contains(&KeyCode::KeyS) {
                pitch_delta -= speed * dt;
            }
            if self.held_orbit_keys.contains(&KeyCode::KeyQ) {
                zoom_factor *= 1.0 + zoom_speed * dt;
            }
            if self.held_orbit_keys.contains(&KeyCode::KeyE) {
                zoom_factor *= 1.0 - zoom_speed * dt;
            }

            if yaw_delta != 0.0 || pitch_delta != 0.0 || zoom_factor != 1.0 {
                self.camera.yaw += yaw_delta;
                self.camera.pitch = (self.camera.pitch + pitch_delta).clamp(-1.4, 1.4);
                self.camera.distance = (self.camera.distance * zoom_factor).max(0.1);
                self.camera.update_orbit();
                self.camera_snap_target = None;
            }
        }

        if let Some((target_yaw, target_pitch)) = self.camera_snap_target {
            let t = 1.0 - (-12.0 * dt).exp();
            self.camera.yaw = lerp_angle(self.camera.yaw, target_yaw, t);
            self.camera.pitch = lerp(self.camera.pitch, target_pitch, t);
            self.camera.update_orbit();

            let remaining = shortest_angle_diff(self.camera.yaw, target_yaw).abs()
                + (self.camera.pitch - target_pitch).abs();
            if remaining < 0.002 {
                self.camera.yaw = target_yaw;
                self.camera.pitch = target_pitch;
                self.camera.update_orbit();
                self.camera_snap_target = None;
            }
        }
    }

    fn render_egui(&mut self, ui: &mut egui::Ui) -> (Option<GizmoAction>, RenderMenuActions) {
        // DoF follow readout for the Rendering & Effects menu: computed up
        // front — it borrows all of self, which must end before the field
        // borrows below.
        let dof_follow = self.dof_follow;
        let dof_readout = if dof_follow || self.render_panel_open {
            let st = self.state.lock().unwrap();
            Some(self.dof_focus_readout(&st.world))
        } else {
            None
        };
        let has_scene_post = self.state.lock().unwrap().scene_post.is_some();
        let scene_post_enabled = self.scene_post_enabled;
        let show_particles = self.show_particles;
        let particles_paused = self.particles_paused;
        let particle_speed = self.particle_speed;
        let particle_alive = self.particle_alive;
        let particle_instances = self.particles.sync.instance_count();
        let particle_emitters = self.particles.sync.emitter_count();
        let particle_effects: Vec<(String, String)> = {
            let st = self.state.lock().unwrap();
            self.particles
                .sync
                .instances()
                .filter_map(|(key, inst)| match (&inst.source, key) {
                    (
                        flint_particles::EffectSource::Asset(fx),
                        flint_particles::InstanceKey::Entity(id),
                    ) => Some((
                        st.world.get_name(id).unwrap_or("?").to_string(),
                        fx.name.clone(),
                    )),
                    _ => None,
                })
                .collect()
        };

        // Snapshot camera state for gizmo (avoids holding &self.camera across the closure)
        let cam_view = CameraView::from_camera(&self.camera);

        // Build the UI — we need to collect data for the closure without borrowing self
        let scene_tree = &mut self.scene_tree;
        let entity_inspector = &mut self.entity_inspector;
        let frame_stats = &mut self.frame_stats;
        let violation_count = self.violation_count;
        let violation_messages = &self.violation_messages;
        let diagnostics = &self.diagnostics;
        let state = &self.state;
        let view_gizmo = &mut self.view_gizmo;
        let transform_gizmo = &mut self.transform_gizmo;
        let show_panels = self.show_inspector;
        let editor = &mut self.editor;
        let camera = &self.camera;
        let dirty = self.dirty;
        let save_problems = &self.save_problems;
        let undo_stack = &self.undo_stack;
        let status_message = &self.status_message;
        let render_panel = &mut self.render_panel;
        let render_panel_open = &mut self.render_panel_open;

        // Pre-compute render stats snapshot for the overlay (borrows scene_renderer before closure)
        let overlay_stats: Option<flint_render::RenderStats> = if self.show_stats {
            self.scene_renderer.as_ref().map(|r| {
                let mut s = r.collect_stats();
                (s.fps, s.frame_time_ms) = frame_stats.fps();
                if let Some(ctx) = &self.render_context {
                    s.resolution = [ctx.width, ctx.height];
                }
                s
            })
        } else {
            None
        };

        let gizmo_action;
        let mut menu_actions = RenderMenuActions::default();
        let mut panel_actions: Vec<SplinePanelAction> = Vec::new();
        let mut inspector_edits: Vec<EditAction> = Vec::new();
        let mut gizmo_edits: Vec<EditAction> = Vec::new();
        let mut gizmo_drag_just_ended = false;

        // User styles that don't compile (#275) or don't load (#425), and
        // built-in shaders read from FLINT_STYLE_SHADER_DIR that don't
        // compile (#418), shown until they do
        let styles = self
            .scene_renderer
            .as_ref()
            .map(|r| r.styles().clone())
            .unwrap_or_default();
        let mut style_errors = styles.errors();
        style_errors.extend(flint_render::stylize::dev_shader::errors());

        {
            let ctx = &ui.ctx().clone();
            flint_debug_ui::paint_style_errors(ctx, &style_errors, &styles, 0.0);
            if show_panels {
                // Left side panel: scene tree
                egui::Panel::left("scene_tree_panel")
                    .default_size(220.0)
                    .resizable(true)
                    .show(ui, |ui| {
                        scene_tree.ui(ui);
                    });

                // Right side panel: spline editor (in editor mode) or entity inspector
                egui::Panel::right("inspector_panel")
                    .default_size(300.0)
                    .resizable(true)
                    .show(ui, |ui| {
                        if let Some(ed) = editor.as_mut() {
                            panel_actions =
                                crate::panels::spline_panel::spline_editor_panel(ui, ed);
                        } else {
                            let selected = scene_tree.selected_entity();
                            if let Some(entity_id) = selected {
                                let st = state.lock().unwrap();
                                let edits = entity_inspector.edit_ui(
                                    ui,
                                    &st.world,
                                    &st.registry,
                                    entity_id,
                                );
                                inspector_edits.extend(edits);
                            } else {
                                ui.heading("Entity Inspector");
                                ui.label("Select an entity in the scene tree.");
                            }
                        }
                    });

                // Bottom panel: the scene's diagnostics + constraint
                // violations + mode indicator + undo info
                egui::Panel::bottom("status_panel")
                    .default_size(150.0)
                    .resizable(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            diagnostics.summary_ui(ui);
                            ui.separator();
                            if violation_count == 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 200, 100),
                                    "No violations",
                                );
                            } else {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 100, 100),
                                    format!("{} violation(s)", violation_count),
                                );
                            }

                            ui.separator();

                            // Dirty indicator
                            if dirty {
                                ui.colored_label(egui::Color32::from_rgb(255, 200, 80), "Modified");
                            }
                            // Edits the last save couldn't write (#679): shown
                            // until they're saved or put back
                            if let Some(first) = save_problems.first() {
                                ui.separator();
                                let more = match save_problems.len() {
                                    1 => String::new(),
                                    n => format!(" (+{} more)", n - 1),
                                };
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 100, 100),
                                    format!("Not saved: {first}{more}"),
                                )
                                .on_hover_text(save_problems.join(", "));
                            }

                            // Gizmo mode indicator (non-editor mode)
                            if editor.is_none() {
                                ui.separator();
                                let mode_label = match transform_gizmo.mode {
                                    GizmoMode::Translate => "W: Move",
                                    GizmoMode::Rotate => "E: Rotate",
                                    GizmoMode::Scale => "R: Scale",
                                };
                                ui.label(mode_label);
                            }

                            // Undo/redo info
                            if undo_stack.can_undo() {
                                ui.separator();
                                if let Some(desc) = undo_stack.undo_description() {
                                    ui.label(format!("Undo: {}", desc));
                                }
                            }

                            // Status message (e.g., "Saved!" or "Save not supported")
                            if let Some((msg, time)) = status_message {
                                let elapsed = time.elapsed().as_secs_f32();
                                if elapsed < 3.0 {
                                    ui.separator();
                                    let alpha = ((3.0 - elapsed) / 0.5).min(1.0);
                                    ui.colored_label(
                                        egui::Color32::from_rgba_unmultiplied(
                                            200,
                                            220,
                                            255,
                                            (alpha * 255.0) as u8,
                                        ),
                                        msg,
                                    );
                                }
                            }
                        });

                        if !diagnostics.diagnostics().is_empty() {
                            ui.separator();
                            if let Some(name) = diagnostics.ui(ui, 110.0) {
                                let id = state.lock().unwrap().world.get_id(&name);
                                scene_tree.select(id);
                            }
                        }

                        if !violation_messages.is_empty() {
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .max_height(60.0)
                                .show(ui, |ui| {
                                    for msg in violation_messages {
                                        ui.label(msg);
                                    }
                                });
                        }
                    });
            }

            // Rendering & Effects menu (F4, ADR 0053): the shared panel body
            // plus two viewer-only sections (authored-post toggle, DoF
            // follow). Side effects that need &mut self are deferred through
            // menu_actions and handled after the egui frame.
            if *render_panel_open {
                let mut open = true;
                egui::Window::new("Rendering & Effects")
                    .default_pos(egui::pos2(240.0, 60.0))
                    .default_width(340.0)
                    .vscroll(true)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        render_panel.ui_contents(ui);
                        ui.separator();
                        egui::CollapsingHeader::new("Authored post")
                            .default_open(false)
                            .show(ui, |ui| {
                                if has_scene_post {
                                    ui.label(if scene_post_enabled {
                                        "Active: scene [post_process]"
                                    } else {
                                        "Active: viewer default"
                                    });
                                    let label = if scene_post_enabled {
                                        "Switch to viewer default"
                                    } else {
                                        "Apply authored [post_process]"
                                    };
                                    if ui.button(label).clicked() {
                                        menu_actions.authored_post_clicked = true;
                                    }
                                } else {
                                    ui.label("Scene has no [post_process] block");
                                }
                            });
                        egui::CollapsingHeader::new("DoF follow")
                            .default_open(false)
                            .show(ui, |ui| {
                                let mut follow = dof_follow;
                                if ui
                                    .checkbox(&mut follow, "Focus tracks last selected entity")
                                    .changed()
                                {
                                    menu_actions.dof_follow = Some(follow);
                                }
                                match &dof_readout {
                                    Some(Some((name, z))) => {
                                        ui.label(format!("Focus: {}", name));
                                        ui.label(format!("Focus distance {:.2}", z));
                                    }
                                    _ => {
                                        ui.label("No focus target — select an entity.");
                                    }
                                }
                            });
                        // Particles (ADR 0068): the viewer simulates live.
                        egui::CollapsingHeader::new("Particles")
                            .default_open(false)
                            .show(ui, |ui| {
                                let mut show = show_particles;
                                if ui.checkbox(&mut show, "Simulate & draw").changed() {
                                    menu_actions.particles_show = Some(show);
                                }
                                let mut paused = particles_paused;
                                if ui.checkbox(&mut paused, "Pause").changed() {
                                    menu_actions.particles_pause = Some(paused);
                                }
                                let mut speed = particle_speed;
                                if ui
                                    .add(egui::Slider::new(&mut speed, 0.0..=4.0).text("Speed"))
                                    .changed()
                                {
                                    menu_actions.particles_speed = Some(speed);
                                }
                                if ui.button("Restart all").clicked() {
                                    menu_actions.particles_restart = true;
                                }
                                ui.label(format!(
                                    "{} alive · {} instance{} · {} emitter{}",
                                    particle_alive,
                                    particle_instances,
                                    if particle_instances == 1 { "" } else { "s" },
                                    particle_emitters,
                                    if particle_emitters == 1 { "" } else { "s" },
                                ));
                                for (entity, fx) in &particle_effects {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{entity} → {fx}"));
                                        ui.weak(format!(
                                            "flint edit particles/{fx}.particles.toml"
                                        ));
                                    });
                                }
                            });
                    });
                if !open {
                    *render_panel_open = false;
                }
            }

            // Transform gizmo for selected entity (non-editor mode only)
            if editor.is_none() {
                if let Some(entity_id) = scene_tree.selected_entity() {
                    let st = state.lock().unwrap();
                    let transform = st.world.get_transform(entity_id).unwrap_or_default();
                    let pos = [
                        transform.position.x,
                        transform.position.y,
                        transform.position.z,
                    ];
                    let rot = [
                        transform.rotation.x,
                        transform.rotation.y,
                        transform.rotation.z,
                    ];
                    let scl = [transform.scale.x, transform.scale.y, transform.scale.z];
                    // Use world position for gizmo placement (accounts for parent transforms)
                    let world_pos = st
                        .world
                        .get_world_position(entity_id)
                        .map(|p| [p.x, p.y, p.z])
                        .unwrap_or(pos);
                    drop(st); // Release lock before gizmo draw (it reads ctx.input)

                    let render_rect = ctx.content_rect();
                    let clip_rect = ui.available_rect_before_wrap();

                    if let Some(delta) = transform_gizmo.draw(
                        ctx,
                        camera,
                        entity_id,
                        world_pos,
                        rot,
                        scl,
                        render_rect,
                        clip_rect,
                    ) {
                        let edits = apply_gizmo_delta(&delta, pos, rot, scl);
                        gizmo_edits.extend(edits);
                    }

                    // Detect drag end for undo coalescing
                    if !transform_gizmo.is_dragging()
                        && transform_gizmo.drag_start_transform().is_some()
                    {
                        gizmo_drag_just_ended = true;
                    }
                }
            }

            // View orientation gizmo (always visible)
            gizmo_action = view_gizmo.draw(ctx, &cam_view, ui.available_rect_before_wrap());

            // Draw spline overlay on the central area (editor mode)
            if let Some(ed) = editor.as_ref() {
                let screen_rect = ctx.content_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("spline_overlay"),
                ));
                ed.draw_overlay(
                    &painter,
                    camera,
                    [screen_rect.width(), screen_rect.height()],
                );
            }

            // Render stats overlay (F2 toggle)
            if let Some(ref s) = overlay_stats {
                render_stats_overlay(ctx, s, frame_stats);
            }
        }

        // Process spline panel actions outside the egui closure
        for action in panel_actions {
            if let Some(ed) = &mut self.editor {
                match action {
                    SplinePanelAction::Save => {
                        if let Err(e) = ed.save() {
                            tracing::error!("Save failed: {}", e);
                        }
                    }
                    SplinePanelAction::Undo => {
                        ed.undo();
                    }
                    SplinePanelAction::InsertPoint(idx) => {
                        ed.insert_point(idx);
                    }
                    SplinePanelAction::DeletePoint(idx) => {
                        ed.delete_point(idx);
                    }
                    SplinePanelAction::Resample => {
                        ed.resample();
                    }
                }
            }
        }

        // Store inspector results for processing outside the closure
        self.pending_edits.extend(inspector_edits);

        // Apply gizmo edits immediately (for smooth visual feedback during drag)
        if !gizmo_edits.is_empty() {
            let mut st = self.state.lock().unwrap();
            for edit in &gizmo_edits {
                self.edits.set(
                    &mut st.world,
                    edit.entity_id,
                    &edit.component,
                    &edit.field,
                    edit.new_value.clone(),
                );
            }
            // `dirty` is refreshed once the frame is drawn
        }

        if gizmo_drag_just_ended {
            self.gizmo_drag_ended = true;
        }

        (gizmo_action, menu_actions)
    }

    /// Apply pending inspector edits to the world and push undo
    fn apply_pending_edits(&mut self) {
        if self.pending_edits.is_empty() {
            return;
        }

        let edits: Vec<EditAction> = self.pending_edits.drain(..).collect();

        {
            let mut st = self.state.lock().unwrap();
            for edit in &edits {
                self.edits.set(
                    &mut st.world,
                    edit.entity_id,
                    &edit.component,
                    &edit.field,
                    edit.new_value.clone(),
                );
            }
        }

        // Push undo
        let desc = if edits.len() == 1 {
            format!("Edit {}.{}", edits[0].component, edits[0].field)
        } else {
            format!("Edit {} fields", edits.len())
        };
        self.undo_stack.push(UndoCommand {
            actions: edits,
            description: desc,
        });
        self.refresh_dirty();
    }

    /// Finalize a gizmo drag into a single undo command
    fn finalize_gizmo_undo(&mut self) {
        let start = self.transform_gizmo.drag_start_transform();
        let entity_id = self.transform_gizmo.drag_entity();

        if let (Some((start_pos, start_rot, start_scl)), Some(eid)) = (start, entity_id) {
            let st = self.state.lock().unwrap();
            let current = st.world.get_transform(eid).unwrap_or_default();
            let cur_pos = [current.position.x, current.position.y, current.position.z];
            let cur_rot = [current.rotation.x, current.rotation.y, current.rotation.z];
            let cur_scl = [current.scale.x, current.scale.y, current.scale.z];
            drop(st);

            let mut actions = Vec::new();

            if (0..3).any(|i| (cur_pos[i] - start_pos[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: comp::TRANSFORM.to_string(),
                    field: "position".to_string(),
                    old_value: vec3_to_toml(start_pos),
                    new_value: vec3_to_toml(cur_pos),
                });
            }
            if (0..3).any(|i| (cur_rot[i] - start_rot[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: comp::TRANSFORM.to_string(),
                    field: "rotation".to_string(),
                    old_value: vec3_to_toml(start_rot),
                    new_value: vec3_to_toml(cur_rot),
                });
            }
            if (0..3).any(|i| (cur_scl[i] - start_scl[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: comp::TRANSFORM.to_string(),
                    field: "scale".to_string(),
                    old_value: vec3_to_toml(start_scl),
                    new_value: vec3_to_toml(cur_scl),
                });
            }

            if !actions.is_empty() {
                let mode_name = match self.transform_gizmo.mode {
                    GizmoMode::Translate => "Move",
                    GizmoMode::Rotate => "Rotate",
                    GizmoMode::Scale => "Scale",
                };
                self.undo_stack.push(UndoCommand {
                    actions,
                    description: format!("{} entity", mode_name),
                });
            }
        }

        self.transform_gizmo.clear_drag_start();
    }

    /// Undo the last edit. It goes through the edit path like any other
    /// (#679), so undo after a save leaves the scene dirty and the next save
    /// writes the value it put back.
    fn undo(&mut self) {
        if let Some(cmd) = self.undo_stack.undo() {
            let mut st = self.state.lock().unwrap();
            for action in &cmd.actions {
                self.edits.set(
                    &mut st.world,
                    action.entity_id,
                    &action.component,
                    &action.field,
                    action.old_value.clone(),
                );
            }
            drop(st);
            self.refresh_dirty();
            println!("Undo: {}", cmd.description);
        }
    }

    /// Redo the last undone edit
    fn redo(&mut self) {
        if let Some(cmd) = self.undo_stack.redo() {
            let mut st = self.state.lock().unwrap();
            for action in &cmd.actions {
                self.edits.set(
                    &mut st.world,
                    action.entity_id,
                    &action.component,
                    &action.field,
                    action.new_value.clone(),
                );
            }
            drop(st);
            self.refresh_dirty();
            println!("Redo: {}", cmd.description);
        }
    }

    /// Save the scene: write each field that differs from the file into it
    /// through its `SceneDocument`, leaving everything else as written
    /// (#677). An entity a prefab instance makes goes into the instance's
    /// overrides. A field that can't be written is named in the status bar
    /// and keeps the scene dirty; "Saved!" means every edit is in the file
    /// (#679). Each field written is a line appended to the project's
    /// `.flint/viewer-edits.jsonl`. With nothing changed this does nothing,
    /// so Ctrl+S on an untouched scene leaves the file byte for byte as it was.
    fn save(&mut self) {
        let mut guard = self.state.lock().unwrap();
        if !self.edits.is_dirty(&guard.world) {
            drop(guard);
            self.refresh_dirty();
            self.status_message = Some(("Nothing to save".to_string(), Instant::now()));
            return;
        }
        let st = &mut *guard;
        let scene_path = st.scene_path.clone();
        // No document means the file didn't parse when it was read. The world
        // alone would write a scene without its look, camera, wooze field and
        // prefabs, so the viewer doesn't write it at all.
        let Some(doc) = st.scene_doc.as_mut() else {
            drop(guard);
            tracing::error!("Not saving {scene_path}: it didn't parse as TOML when it was read");
            self.status_message = Some((
                "Save refused: the scene file didn't parse; reload it".to_string(),
                Instant::now(),
            ));
            return;
        };
        let plan = self.edits.plan(&st.world, doc);
        for (_, why) in &plan.failed {
            tracing::warn!("Not saved: {why}");
        }
        let written = if plan.written.is_empty() {
            Ok(())
        } else {
            doc.save(&scene_path)
        };
        drop(guard);
        match written {
            Ok(()) => {
                self.edits.saved(&plan);
                if !plan.written.is_empty() {
                    self.last_save_time = Some(Instant::now());
                    self.suppress_reload_until = Some(Instant::now() + Duration::from_millis(1500));
                    println!("Saved scene to {}", scene_path);
                    self.log_edits(&plan, &scene_path);
                }
                let message = match plan.failed.first() {
                    None => format!("Saved! ({} field(s))", plan.written.len()),
                    Some((_, why)) => format!(
                        "Not saved: {} of {} edit(s): {why}",
                        plan.failed.len(),
                        plan.failed.len() + plan.written.len()
                    ),
                };
                self.status_message = Some((message, Instant::now()));
            }
            Err(e) => {
                tracing::error!("Failed to save scene: {}", e);
                self.status_message = Some((format!("Save failed: {}", e), Instant::now()));
            }
        }
        self.refresh_dirty();
    }

    /// Append what a save wrote to the project's change list, for an agent
    /// to read what the human changed (#679).
    fn log_edits(&self, plan: &crate::edits::SavePlan, scene_path: &str) {
        append_change_list(scene_path, |scene, now| {
            crate::edits::log_lines(plan, scene, now)
        });
    }

    /// `dirty` is whether the world differs from the file: set it, and the
    /// title's marker and the status bar's unsaved-edit notes with it.
    fn refresh_dirty(&mut self) {
        let st = self.state.lock().unwrap();
        let dirty = self.edits.is_dirty(&st.world);
        self.save_problems = self.edits.problems(&st.world);
        drop(st);
        if dirty != self.dirty {
            self.dirty = dirty;
            self.update_window_title();
        }
    }

    /// Update the window title to reflect dirty state
    fn update_window_title(&self) {
        if let Some(window) = &self.window {
            let base = if self.editor.is_some() {
                "Flint Track Editor"
            } else {
                "Flint Viewer"
            };
            let title = if self.dirty {
                format!("{} *", base)
            } else {
                base.to_string()
            };
            window.set_title(&title);
        }
    }

    /// Refresh renderer from current world state
    /// A camera jump of the viewer's own (Space back to the authored
    /// framing, a switch between perspective and orthographic) or a hot
    /// reload: styles that remember earlier frames (ASCII's steady glyphs,
    /// #277) start over instead of carrying the old view into the new one
    /// (#474). The viewer knows each of its jumps, so it runs no detector:
    /// orbits, wheel zooms and view snaps ease, and keep their history.
    fn reset_style_history(&mut self) {
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.reset_style_history();
        }
    }

    /// Handle mouse click picking in the viewport
    fn try_pick_entity(&mut self, x: f64, y: f64) {
        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };

        // Check if pointer is over egui panels
        if self.egui_ctx.is_pointer_over_egui() {
            return;
        }

        // Don't pick while gizmo is active
        if self.transform_gizmo.is_dragging() || self.transform_gizmo.is_hovered() {
            return;
        }

        let st = self.state.lock().unwrap();
        let targets = build_pick_targets(&st.world);
        drop(st);

        let vw = context.width as f32;
        let vh = context.height as f32;

        if let Some((entity_id, _dist)) =
            pick_entity(x as f32, y as f32, vw, vh, &self.camera, &targets)
        {
            self.scene_tree.select(Some(entity_id));
        } else {
            self.scene_tree.select(None);
        }
    }

    fn check_reload(&mut self) {
        // Suppress reload if we just saved
        if let Some(until) = self.suppress_reload_until {
            if Instant::now() < until {
                // Clear the needs_reload flag silently
                if let Ok(mut state) = self.state.lock() {
                    state.needs_reload = false;
                }
                return;
            } else {
                self.suppress_reload_until = None;
            }
        }

        let (mut needs_reload, scene_path) = {
            let state = self.state.lock().unwrap();
            (state.needs_reload, state.scene_path.clone())
        };

        // User styles (#275): the watcher sees `<scene>/styles/`, but not
        // the parent's `styles/` a game keeps beside `levels/`, so poll the
        // descriptions a few times a second too. A changed one reloads the
        // scene, which reads its settings for the style again.
        self.user_style_polls = self.user_style_polls.wrapping_add(1);
        if needs_reload || self.user_style_polls % 15 == 0 {
            // From the project the renderer was given at load, not found anew
            // into the registry the renderer draws with (#742)
            let reload = match self.scene_renderer.as_mut() {
                Some(renderer) => {
                    let scope = renderer.asset_scope();
                    renderer.styles_mut().reload_changed_for(&scope)
                }
                None => Default::default(),
            };
            for message in &reload.messages {
                eprintln!("Warning: {message}");
            }
            needs_reload |= reload.changed();
        }

        if needs_reload {
            {
                let mut state = self.state.lock().unwrap();
                state.needs_reload = false;
            }
            // The first reload after a Commit to scene is its own (#516)
            let own_commit = self
                .own_commit_until
                .take()
                .is_some_and(|until| Instant::now() < until);

            let reload_result = {
                let state = self.state.lock().unwrap();
                // New ids, past the old world's (#688): the renderer's caches
                // keyed by entity must not take a reloaded entity for an old one
                load_scene_after(&scene_path, &state.registry, &state.world).and_then(
                    |(world, mut scene_file)| {
                        let path = std::path::Path::new(&scene_path);
                        flint_procgen::settle_scene(&mut scene_file, path)?;
                        Ok((world, scene_file))
                    },
                )
            };

            match reload_result {
                Ok((new_world, scene_file)) => {
                    println!("Reloaded scene: {}", scene_file.scene.name);
                    // Its entities are new: the field's clocks start over
                    self.shallow_wooze = Default::default();
                    self.grass_wooze = Default::default();

                    {
                        let mut state = self.state.lock().unwrap();
                        // A reload spawns every entity again under a new id:
                        // keep the selection (and its outline, #602) by name
                        let selected_name = self
                            .scene_tree
                            .selected_entity()
                            .and_then(|id| state.world.get_name(id))
                            .map(str::to_owned);
                        // The undo history follows the entities by name
                        // (#1046): an agent's edit to the file leaves the
                        // human's steps elsewhere to undo
                        let dropped = self.undo_stack.carry_over(&state.world, &new_world);
                        if dropped > 0 {
                            println!(
                                "Undo: {dropped} step(s) dropped, the reloaded file changed what they edited"
                            );
                        }
                        state.world = new_world;
                        self.scene_tree
                            .select(selected_name.and_then(|name| state.world.get_id(&name)));
                        state.scene_post = with_look(
                            scene_file.post_process.clone(),
                            &state.scene_path,
                            self.picked_look.as_ref(),
                        );
                        state.wooze = field_of(&scene_file);
                        state.environment = scene_file.environment.clone();
                        // Refresh authored camera too; applied on the next
                        // Space reset (the live camera is never yanked).
                        state.scene_camera = scene_file.camera.clone();
                    }

                    if let (Some(context), Some(renderer)) =
                        (&self.render_context, &mut self.scene_renderer)
                    {
                        let mut state = self.state.lock().unwrap();
                        let config = ModelLoadConfig::from_scene_path(&state.scene_path);
                        // Bound again (#739): no style history (the picture
                        // no longer follows on from the last one, #474), no
                        // bones or levers of the last load, and its terrain
                        // and grass, which may have changed or gone
                        // over the styles it draws with now, so a reload
                        // since the last load is kept and an unchanged
                        // style keeps its pass (#742)
                        let binding = match self.scene_binding.clone() {
                            Some(binding) => binding.restyled(renderer.styles()),
                            None => SceneBinding::over(
                                (*renderer.asset_scope()).clone(),
                                renderer.styles(),
                            ),
                        };
                        stage_scene(
                            renderer,
                            &context.device,
                            &context.queue,
                            &mut state,
                            binding,
                            &config,
                        );

                        // Particles: effects may have changed on disk too
                        // (the watcher covers the scene dir recursively).
                        self.particles.clear();
                        let project = &config.project;
                        flint_particles::load_particle_effects_from_world(
                            project,
                            &state.scene_path,
                            &mut self.particles,
                        );
                        self.particles.sync.sync_from_world(&state.world);
                        flint_render::load_scene_particle_textures(
                            renderer,
                            &context.device,
                            &context.queue,
                            &self.particles.sync,
                            project,
                            scene_dir_of(&state.scene_path),
                        );

                        // Re-derive the authored post-process so [post_process]
                        // edits hot-reload like the rest of the scene. Not
                        // after the viewer's own Commit to scene (#516): the
                        // config on screen already says what the file now
                        // says of the committed style, and taking the file's
                        // would throw away every F4 tweak never written
                        // anywhere (bloom, fog, SSAO, another style's).
                        if own_commit {
                            println!("Kept the F4 post-process: the reload was its own commit's");
                        } else if self.scene_post_enabled {
                            // --style keeps winning over the reloaded file (#328)
                            if let Some(def) = &state.scene_post {
                                renderer.set_post_process_config(with_flags(
                                    post_process_config_from_def(def, renderer.styles()),
                                    &self.style_specs,
                                    self.motion_hold,
                                    renderer.styles(),
                                ));
                            }
                        }
                        self.render_panel.set_authored_styles(Some(authored_styles(
                            state.scene_post.as_ref(),
                            renderer.styles(),
                        )));
                        self.render_panel
                            .set_scene_dir(Some(scene_dir_of(&state.scene_path).to_path_buf()));
                        // A look file may be new, changed or gone (#356)
                        self.render_panel.set_looks(
                            &flint_render::look::list(&state.scene_path),
                            state.scene_post.as_ref().and_then(|d| d.look.as_deref()),
                        );
                    }

                    {
                        let state = self.state.lock().unwrap();
                        self.scene_tree.update(&state.world);
                    }
                    self.update_constraints();
                    self.update_diagnostics();

                    // The world is the file's now: nothing is edited. The
                    // undo stack was carried over above, so an undo after
                    // this is an edit like any other, and dirty
                    self.edits.clear();
                    self.save_problems.clear();
                    self.dirty = false;
                    self.update_window_title();

                    // Re-parse scene document for patcher
                    {
                        let mut state = self.state.lock().unwrap();
                        state.scene_doc = SceneDocument::from_file(&scene_path).ok();
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to reload scene {}: {:?}",
                        flint_core::project::shown(std::path::Path::new(&scene_path)),
                        e
                    );
                    // The last world stays on screen; the panel says why
                    // the file no longer loads
                    self.update_diagnostics();
                }
            }
        }
    }

    /// Get screen size from render context.
    fn screen_size(&self) -> [f32; 2] {
        if let Some(ctx) = &self.render_context {
            [ctx.width as f32, ctx.height as f32]
        } else {
            [
                self.window_opts.width as f32,
                self.window_opts.height as f32,
            ]
        }
    }

    // --- Editor-specific input handlers ---

    fn handle_editor_mouse_press(&mut self, button: MouseButton) {
        match button {
            MouseButton::Left => {
                self.mouse_pressed = true;
                let screen = self.screen_size();
                let alt_held = self.modifiers.alt_key();
                // Try to pick a control point
                if let (Some(editor), Some((mx, my))) = (&mut self.editor, self.last_mouse_pos) {
                    if let Some(idx) = editor.pick(&self.camera, screen, mx as f32, my as f32) {
                        editor.selected = Some(idx);
                        editor.dragging = true;
                        editor.drag_start_pos = editor.control_points[idx].position;
                        editor.push_undo();
                        editor.drag_mode = if alt_held {
                            DragMode::VerticalY
                        } else {
                            DragMode::HorizontalXZ
                        };
                    } else {
                        // Click on empty space: deselect
                        editor.selected = None;
                    }
                }
            }
            MouseButton::Middle => {
                self.middle_mouse_pressed = true;
            }
            MouseButton::Right => {
                self.right_mouse_pressed = true;
            }
            _ => {}
        }
    }

    fn handle_editor_mouse_release(&mut self, button: MouseButton) {
        match button {
            MouseButton::Left => {
                self.mouse_pressed = false;
                if let Some(editor) = &mut self.editor {
                    editor.dragging = false;
                }
            }
            MouseButton::Middle => {
                self.middle_mouse_pressed = false;
            }
            MouseButton::Right => {
                self.right_mouse_pressed = false;
            }
            _ => {}
        }
    }

    fn handle_editor_cursor_moved(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        if let Some((last_x, last_y)) = self.last_mouse_pos {
            let dx = (position.x - last_x) as f32;
            let dy = (position.y - last_y) as f32;

            // Middle-drag = orbit (replaces left-drag in standard mode)
            if self.middle_mouse_pressed {
                self.camera.orbit_horizontal(-dx * 0.01);
                self.camera.orbit_vertical(-dy * 0.01);
                self.camera_snap_target = None;
                self.camera.orthographic = false;
            }

            // Right-drag = pan (same as standard mode)
            if self.right_mouse_pressed {
                self.camera.pan(-dx * 0.02, dy * 0.02);
                self.camera_snap_target = None;
            }

            // Left-drag with dragging = move control point
            if self.mouse_pressed {
                let screen = self.screen_size();
                if let Some(editor) = &mut self.editor {
                    if editor.dragging {
                        editor.handle_drag(
                            &self.camera,
                            screen,
                            position.x as f32,
                            position.y as f32,
                        );
                    }
                }
            }
        }

        self.last_mouse_pos = Some((position.x, position.y));

        // Update hover state
        let screen = self.screen_size();
        if let Some(editor) = &mut self.editor {
            editor.update_hover(&self.camera, screen, position.x as f32, position.y as f32);
        }
    }

    fn handle_editor_key(&mut self, key: KeyCode) -> bool {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        match key {
            KeyCode::KeyS if ctrl => {
                if let Some(editor) = &mut self.editor {
                    if let Err(e) = editor.save() {
                        tracing::error!("Save failed: {}", e);
                    }
                }
                true
            }
            KeyCode::KeyZ if ctrl => {
                if let Some(editor) = &mut self.editor {
                    editor.undo();
                }
                true
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if let Some(editor) = &mut self.editor {
                    if let Some(idx) = editor.selected {
                        editor.delete_point(idx);
                    }
                }
                true
            }
            KeyCode::KeyI => {
                if let Some(editor) = &mut self.editor {
                    if let Some(idx) = editor.selected {
                        editor.insert_point(idx);
                    }
                }
                true
            }
            KeyCode::Tab => {
                if let Some(editor) = &mut self.editor {
                    let n = editor.control_points.len();
                    if n > 0 {
                        editor.selected = Some(match editor.selected {
                            Some(idx) => {
                                if shift {
                                    if idx == 0 {
                                        n - 1
                                    } else {
                                        idx - 1
                                    }
                                } else {
                                    (idx + 1) % n
                                }
                            }
                            None => 0,
                        });
                    }
                }
                true
            }
            KeyCode::Escape => {
                if let Some(editor) = &mut self.editor {
                    if editor.dragging {
                        editor.cancel_drag();
                        return true;
                    }
                }
                false // Let standard handler process Escape (close window)
            }
            _ => false,
        }
    }
}

impl Editor for ViewerApp {
    fn title(&self) -> String {
        if self.editor.is_some() {
            "Flint Track Editor".into()
        } else {
            "Flint Viewer".into()
        }
    }

    fn init(&mut self, cx: InitCx) -> Result<()> {
        self.initialize(cx)
    }

    /// The viewer drives its own orbit camera (picking, the gizmo and the
    /// view cube share its mouse), so the host only sets and sizes it.
    fn camera(&mut self) -> Option<&mut Camera> {
        Some(&mut self.camera)
    }

    /// Shortcuts that win over a focused panel.
    fn early_event(&mut self, event: &WindowEvent) -> Flow {
        let is_editor = self.editor.is_some();

        // Track modifier keys
        if let WindowEvent::ModifiersChanged(mods) = event {
            self.modifiers = mods.state();
        }

        // Handle Tab and other global shortcuts before egui can consume them.
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = event
        {
            if key_event.state == ElementState::Pressed && !key_event.repeat {
                if is_editor {
                    // In editor mode, handle editor-specific keys first
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        if self.handle_editor_key(code) {
                            return Flow::Handled;
                        }
                    }
                } else {
                    // Non-editor viewer mode keyboard shortcuts
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        match code {
                            KeyCode::Tab if self.modifiers.is_empty() => {
                                self.show_inspector = !self.show_inspector;
                                println!(
                                    "Inspector: {}",
                                    if self.show_inspector { "ON" } else { "OFF" }
                                );
                                return Flow::Handled;
                            }
                            KeyCode::KeyZ
                                if self.modifiers.control_key() && self.modifiers.shift_key() =>
                            {
                                // Ctrl+Shift+Z = redo
                                self.redo();
                                return Flow::Handled;
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() => {
                                // Ctrl+Z = undo
                                self.undo();
                                return Flow::Handled;
                            }
                            KeyCode::KeyY if self.modifiers.control_key() => {
                                // Ctrl+Y = redo
                                self.redo();
                                return Flow::Handled;
                            }
                            KeyCode::KeyS if self.modifiers.control_key() => {
                                // Ctrl+S = save scene
                                self.save();
                                return Flow::Handled;
                            }
                            // W/E/R = gizmo mode shortcuts, but ONLY while an
                            // entity is selected (the gizmo doesn't exist
                            // otherwise). With no selection they fall through
                            // to the keyboard-orbit tracker — previously E
                            // (zoom in) was swallowed here on the initial
                            // press and only started zooming once OS key
                            // repeat kicked in.
                            KeyCode::KeyW
                                if self.modifiers.is_empty()
                                    && self.scene_tree.selected_entity().is_some() =>
                            {
                                self.transform_gizmo.mode = GizmoMode::Translate;
                                return Flow::Handled;
                            }
                            KeyCode::KeyE
                                if self.modifiers.is_empty()
                                    && self.scene_tree.selected_entity().is_some() =>
                            {
                                self.transform_gizmo.mode = GizmoMode::Rotate;
                                return Flow::Handled;
                            }
                            KeyCode::KeyR
                                if self.modifiers.is_empty()
                                    && self.scene_tree.selected_entity().is_some() =>
                            {
                                self.transform_gizmo.mode = GizmoMode::Scale;
                                return Flow::Handled;
                            }
                            // Ctrl+R = Reload
                            KeyCode::KeyR if self.modifiers.control_key() => {
                                if let Ok(mut state) = self.state.lock() {
                                    state.needs_reload = true;
                                }
                                return Flow::Handled;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Flow::Pass
    }

    fn event(&mut self, event: &WindowEvent) -> Flow {
        let is_editor = self.editor.is_some();
        // Handle gizmo input priority: if gizmo is hovered/dragging, suppress camera orbit
        let gizmo_active = self.transform_gizmo.is_dragging() || self.transform_gizmo.is_hovered();
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                // Track WASD/QE held state for keyboard orbit
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::KeyW
                        | KeyCode::KeyA
                        | KeyCode::KeyS
                        | KeyCode::KeyD
                        | KeyCode::KeyQ
                        | KeyCode::KeyE => {
                            if event.state == ElementState::Pressed {
                                // W/E double as gizmo-mode shortcuts while an
                                // entity is selected; keep key-repeat events
                                // from leaking them into orbit in that state.
                                let gizmo_key = matches!(code, KeyCode::KeyW | KeyCode::KeyE);
                                let gizmo_owns = gizmo_key
                                    && !is_editor
                                    && self.scene_tree.selected_entity().is_some();
                                if !gizmo_owns {
                                    self.held_orbit_keys.insert(code);
                                    self.auto_orbit = false;
                                }
                            } else {
                                self.held_orbit_keys.remove(&code);
                            }
                        }
                        _ => {}
                    }
                }

                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape)
                            if !self.transform_gizmo.is_dragging() =>
                        {
                            return Flow::Exit;
                        }

                        PhysicalKey::Code(KeyCode::Space) if !is_editor => {
                            self.camera = Camera::new();
                            if let Some(context) = &self.render_context {
                                self.camera.aspect = context.aspect();
                            }
                            // Back to the authored framing (viewer default if
                            // the scene has no [camera] block)
                            self.seed_camera_from_scene();
                            self.camera.update_orbit();
                            self.reset_style_history();
                        }
                        PhysicalKey::Code(KeyCode::KeyO) if !is_editor => {
                            self.auto_orbit = !self.auto_orbit;
                            println!("Auto-orbit: {}", if self.auto_orbit { "ON" } else { "OFF" });
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) if !is_editor => {
                            self.auto_orbit_speed = (self.auto_orbit_speed * 1.5).clamp(0.1, 5.0);
                            println!("Auto-orbit speed: {:.2} rad/s", self.auto_orbit_speed);
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) if !is_editor => {
                            self.auto_orbit_speed = (self.auto_orbit_speed / 1.5).clamp(0.1, 5.0);
                            println!("Auto-orbit speed: {:.2} rad/s", self.auto_orbit_speed);
                        }
                        PhysicalKey::Code(KeyCode::F2) => {
                            self.show_stats = !self.show_stats;
                            if self.show_stats {
                                self.frame_stats.reset();
                            }
                        }
                        PhysicalKey::Code(KeyCode::F3) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_normal_arrows();
                                println!("Normal arrows: {}", if on { "ON" } else { "OFF" });
                            }
                        }
                        // Rendering & Effects menu (ADR 0053): the one home
                        // for every render/post debug control the old
                        // F1/F4-F12 keys used to flip blindly, plus their
                        // non-binary parameters, the authored-post toggle,
                        // and DoF follow.
                        PhysicalKey::Code(KeyCode::F4) => {
                            self.render_panel_open = !self.render_panel_open;
                        }
                        _ => {}
                    }
                }
            }

            &WindowEvent::MouseInput { state, button, .. } => {
                if is_editor {
                    if state == ElementState::Pressed {
                        self.handle_editor_mouse_press(button);
                    } else {
                        self.handle_editor_mouse_release(button);
                    }
                } else {
                    match button {
                        MouseButton::Left => {
                            let was_pressed = self.mouse_pressed;
                            self.mouse_pressed = state == ElementState::Pressed;

                            // On click release (not drag), try picking
                            if state == ElementState::Released && was_pressed && !gizmo_active {
                                if let Some((x, y)) = self.last_mouse_pos {
                                    self.try_pick_entity(x, y);
                                }
                            }
                        }
                        MouseButton::Right => {
                            self.right_mouse_pressed = state == ElementState::Pressed;
                        }
                        _ => {}
                    }
                }
            }

            &WindowEvent::CursorMoved { position, .. } => {
                if is_editor {
                    self.handle_editor_cursor_moved(position);
                } else {
                    if let Some((last_x, last_y)) = self.last_mouse_pos {
                        let dx = (position.x - last_x) as f32;
                        let dy = (position.y - last_y) as f32;

                        // Only orbit if gizmo is not active
                        if self.mouse_pressed && !gizmo_active {
                            self.camera.orbit_horizontal(-dx * 0.01);
                            self.camera.orbit_vertical(-dy * 0.01);
                            self.camera_snap_target = None;
                            self.camera.orthographic = false;
                            self.auto_orbit = false;
                        }

                        if self.right_mouse_pressed {
                            self.camera.pan(-dx * 0.02, dy * 0.02);
                            self.camera_snap_target = None;
                        }
                    }

                    self.last_mouse_pos = Some((position.x, position.y));
                }
            }

            &WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 100.0,
                };
                self.camera.zoom(scroll);
            }

            _ => {}
        }
        Flow::Handled
    }

    fn resized(&mut self, gpu: &Gpu) {
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.resize_postprocess(&gpu.device, gpu.width, gpu.height);
        }
        self.render_context = Some(gpu.clone());
    }

    fn update(&mut self, _tick: Tick) {
        self.check_reload();
        self.frame_update();
    }

    /// The host times frames only while F2 shows the graph.
    fn times_frames(&self) -> bool {
        self.show_stats
    }

    fn frame_timed(&mut self, times: &host::FrameTimes) {
        self.frame_stats.record(times);
    }

    fn draw(&mut self, target: &Target) {
        if let Some(renderer) = &mut self.scene_renderer {
            target.render(renderer, &self.camera);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.frame_ui(ui);
    }
}

// --- Helpers ---

/// The wooze field a scene describes, naming what's wrong with it and going on
/// without that part, as the player does.
fn field_of(scene: &flint_scene::SceneFile) -> Option<flint_wooze::WoozeField> {
    let (field, issues) = flint_procgen::LivingTables::or_warn(scene).wooze?.field();
    for issue in issues {
        tracing::warn!("{issue}; going without it");
    }
    Some(field)
}

fn vec3_to_toml(v: [f32; 3]) -> toml::Value {
    toml::Value::Array(vec![
        toml::Value::Float(v[0] as f64),
        toml::Value::Float(v[1] as f64),
        toml::Value::Float(v[2] as f64),
    ])
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn shortest_angle_diff(from: f32, to: f32) -> f32 {
    use std::f32::consts::PI;
    let mut diff = to - from;
    while diff > PI {
        diff -= 2.0 * PI;
    }
    while diff < -PI {
        diff += 2.0 * PI;
    }
    diff
}

fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    from + shortest_angle_diff(from, to) * t
}

/// F2: the frame-time graph (#1110) above the rendering stats.
fn render_stats_overlay(
    ctx: &egui::Context,
    stats: &flint_render::RenderStats,
    frames: &mut FrameStats,
) {
    use flint_render::format_count;

    egui::Area::new(egui::Id::new("render_stats_overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .order(egui::Order::Foreground)
        // A readout: clicks go through to the inspector under it
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
                .stroke(egui::Stroke::new(
                    1.0f32,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
                ))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                    ui.set_min_width(180.0);

                    ui.colored_label(
                        egui::Color32::from_gray(136),
                        egui::RichText::new("RENDERING STATS").size(9.0),
                    );
                    ui.separator();

                    frames.ui(ui);
                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.label("FPS:");
                        ui.colored_label(
                            egui::Color32::from_rgb(74, 222, 128),
                            format!("{:.0}", stats.fps),
                        );
                        ui.colored_label(
                            egui::Color32::from_gray(102),
                            format!("({:.1}ms)", stats.frame_time_ms),
                        );
                    });
                    ui.label(format!("Draw Calls: {}", stats.draw_calls));
                    ui.label(format!("Triangles: {}", format_count(stats.triangles)));

                    ui.separator();

                    ui.label(format!("Entities: {}", stats.entity_draws));
                    if stats.skinned_draws > 0 {
                        ui.label(format!("Skinned: {}", stats.skinned_draws));
                    }
                    ui.label(format!(
                        "Terrain: {}/{} chunks",
                        stats.terrain_draws, stats.terrain_total_chunks
                    ));
                    if stats.transparent_draws > 0 {
                        ui.label(format!("Transparent: {}", stats.transparent_draws));
                    }
                    if stats.billboard_draws > 0 {
                        ui.label(format!("Billboards: {}", stats.billboard_draws));
                    }
                    if stats.particle_draws > 0 {
                        ui.label(format!(
                            "Particles: {} ({} inst)",
                            stats.particle_draws,
                            format_count(stats.particle_instances)
                        ));
                    }
                    if stats.sprite_batches > 0 {
                        ui.label(format!("Sprites: {}", stats.sprite_batches));
                    }
                    if stats.grass_instances > 0 {
                        ui.label(format!(
                            "Grass: {} inst",
                            format_count(stats.grass_instances)
                        ));
                    }

                    ui.separator();

                    ui.label(format!("Shadow Calls: {}", stats.shadow_draw_calls));
                    ui.label(format!(
                        "Shadow Tris: {}",
                        format_count(stats.shadow_triangles)
                    ));

                    ui.separator();

                    ui.colored_label(
                        egui::Color32::from_gray(102),
                        format!("{}x{}", stats.resolution[0], stats.resolution[1]),
                    );
                });
        });
}

#[cfg(test)]
#[path = "app_binding_tests.rs"]
mod binding_tests;

#[cfg(test)]
#[path = "app_commit_tests.rs"]
mod commit_tests;
