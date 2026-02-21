//! Main viewer application — combines wgpu scene rendering with egui panels.
//! Supports an optional spline editor mode for interactive track editing.

use anyhow::{Context, Result};
use crate::panels::{
    CameraView, EntityInspector, GizmoAction, GizmoMode, RenderStats, SceneTree,
    SplinePanelAction, TransformGizmo, ViewGizmo,
};
use crate::panels::transform_gizmo::apply_gizmo_delta;
use crate::picking::{build_pick_targets, pick_entity};
use crate::spline_editor::{DragMode, SplineEditor, SplineEditorConfig};
use crate::undo::{EditAction, UndoCommand, UndoStack};
use flint_constraint::{ConstraintEvaluator, ConstraintRegistry};
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_render::{Camera, RenderContext, RendererConfig, SceneRenderer};
use flint_scene::{load_scene, save_scene, SceneDocument};
use flint_schema::SchemaRegistry;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

struct ViewerState {
    world: FlintWorld,
    registry: SchemaRegistry,
    constraint_registry: ConstraintRegistry,
    scene_path: String,
    needs_reload: bool,
    scene_doc: Option<SceneDocument>,
}

/// Run the viewer application (standard viewer mode)
pub fn run(
    scene_path: &str,
    watch: bool,
    schemas_path: &str,
    inspector: bool,
) -> Result<()> {
    let registry = if Path::new(schemas_path).exists() {
        SchemaRegistry::load_from_directory(schemas_path)?
    } else {
        println!("Warning: Schemas directory not found: {}", schemas_path);
        SchemaRegistry::new()
    };

    let constraint_registry =
        ConstraintRegistry::load_from_directory(schemas_path).unwrap_or_default();

    let (world, scene_file) = load_scene(scene_path, &registry)?;

    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    // Parse the scene file as an editable document for structure-preserving saves
    let scene_doc = SceneDocument::from_file(scene_path).ok();

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
        scene_doc,
    }));

    let _watcher = if watch {
        let state_clone = Arc::clone(&state);
        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
        let scene_file = Path::new(scene_path);
        let scene_dir = scene_file.parent().unwrap_or_else(|| Path::new("."));

        debouncer
            .watcher()
            .watch(scene_file, RecursiveMode::NonRecursive)?;
        debouncer
            .watcher()
            .watch(scene_dir, RecursiveMode::Recursive)?;

        let schemas_dir = Path::new(schemas_path);
        if schemas_dir.exists() {
            debouncer
                .watcher()
                .watch(schemas_dir, RecursiveMode::Recursive)?;
        }

        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_events) => {
                        if let Ok(mut state) = state_clone.lock() {
                            state.needs_reload = true;
                        }
                    }
                    Err(e) => {
                        eprintln!("Watch error: {:?}", e);
                    }
                }
            }
        });

        println!("Watching for changes in scene and related assets...");
        Some(debouncer)
    } else {
        None
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewerApp::new(state, inspector);
    event_loop.run_app(&mut app)?;

    Ok(())
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

    println!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path_anchor.to_string(),
        needs_reload: false,
        scene_doc: None,
    }));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewerApp::new(state, inspector);
    event_loop.run_app(&mut app)?;

    Ok(())
}

/// Run the viewer in editor mode with a spline editor configuration.
pub fn run_editor(
    scene_path: &str,
    watch: bool,
    schemas_paths: &[&str],
    editor_config: SplineEditorConfig,
) -> Result<()> {
    let existing: Vec<&str> = schemas_paths
        .iter()
        .copied()
        .filter(|p| Path::new(p).exists())
        .collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing)?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    let constraint_registry = existing
        .first()
        .and_then(|p| ConstraintRegistry::load_from_directory(p).ok())
        .unwrap_or_default();

    let (world, scene_file) = load_scene(scene_path, &registry)?;

    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
        scene_doc: None,
    }));

    let _watcher = if watch {
        let state_clone = Arc::clone(&state);
        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
        let scene_file_path = Path::new(scene_path);
        let scene_dir = scene_file_path.parent().unwrap_or_else(|| Path::new("."));

        debouncer
            .watcher()
            .watch(scene_file_path, RecursiveMode::NonRecursive)?;
        debouncer
            .watcher()
            .watch(scene_dir, RecursiveMode::Recursive)?;

        for sp in schemas_paths {
            let schemas_dir = Path::new(sp);
            if schemas_dir.exists() {
                debouncer
                    .watcher()
                    .watch(schemas_dir, RecursiveMode::Recursive)?;
            }
        }

        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_events) => {
                        if let Ok(mut state) = state_clone.lock() {
                            state.needs_reload = true;
                        }
                    }
                    Err(e) => {
                        eprintln!("Watch error: {:?}", e);
                    }
                }
            }
        });

        println!("Watching for changes...");
        Some(debouncer)
    } else {
        None
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let editor = SplineEditor::from_config(editor_config);
    let mut app = ViewerApp::new_editor(state, editor);
    event_loop.run_app(&mut app)?;

    Ok(())
}

/// The main viewer application
pub struct ViewerApp {
    state: Arc<Mutex<ViewerState>>,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input state
    mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    right_mouse_pressed: bool,
    middle_mouse_pressed: bool,
    modifiers: ModifiersState,

    // egui state
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_inspector: bool,

    // Panel state
    scene_tree: SceneTree,
    entity_inspector: EntityInspector,
    render_stats: RenderStats,
    view_gizmo: ViewGizmo,
    transform_gizmo: TransformGizmo,

    // Camera snap animation
    camera_snap_target: Option<(f32, f32)>,
    last_frame_time: Instant,

    // Constraint violations cache
    violation_count: usize,
    violation_messages: Vec<String>,

    // Editor mode (None = standard viewer)
    editor: Option<SplineEditor>,

    // Undo/redo
    undo_stack: UndoStack,

    // Dirty tracking + save
    dirty: bool,
    last_save_time: Option<Instant>,
    suppress_reload_until: Option<Instant>,

    // Pending edit actions from inspector (collected inside egui closure, applied after)
    pending_edits: Vec<EditAction>,

    // Gizmo undo coalescing: when a drag ends, we push a single undo command
    // from the stored start transform to the current transform
    gizmo_drag_ended: bool,

    // Dirty field tracking for structure-preserving saves
    // Each entry is (entity_name, component, field)
    dirty_fields: std::collections::HashSet<(String, String, String)>,

    // Status message (e.g., "Saved!" in editor mode)
    status_message: Option<(String, Instant)>,
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
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            show_inspector,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            render_stats: RenderStats::new(),
            view_gizmo: ViewGizmo::new(),
            transform_gizmo: TransformGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            editor: None,
            undo_stack: UndoStack::new(),
            dirty: false,
            last_save_time: None,
            suppress_reload_until: None,
            pending_edits: Vec::new(),
            gizmo_drag_ended: false,
            dirty_fields: std::collections::HashSet::new(),
            status_message: None,
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
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            show_inspector: true,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            render_stats: RenderStats::new(),
            view_gizmo: ViewGizmo::new(),
            transform_gizmo: TransformGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            editor: Some(editor),
            undo_stack: UndoStack::new(),
            dirty: false,
            last_save_time: None,
            suppress_reload_until: None,
            pending_edits: Vec::new(),
            gizmo_drag_ended: false,
            dirty_fields: std::collections::HashSet::new(),
            status_message: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let title = if self.editor.is_some() {
            "Flint Track Editor"
        } else {
            "Flint Viewer"
        };

        let window_attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(PhysicalSize::new(1600, 900));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create viewer window")?,
        );
        self.window = Some(window.clone());

        let render_context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to initialize viewer render context")?;

        self.camera.aspect = render_context.aspect_ratio();

        // In editor mode, start with a good birds-eye view
        if self.editor.is_some() {
            self.camera.distance = 150.0;
            self.camera.pitch = 1.0; // ~57 degrees, nearly top-down
            self.camera.yaw = 0.0;
            // Center on approximate track center
            if let Some(editor) = &self.editor {
                if !editor.control_points.is_empty() {
                    let n = editor.control_points.len() as f32;
                    let cx: f32 = editor.control_points.iter().map(|p| p.position[0]).sum::<f32>() / n;
                    let cz: f32 = editor.control_points.iter().map(|p| p.position[2]).sum::<f32>() / n;
                    self.camera.target = flint_core::Vec3::new(cx, 0.0, cz);
                }
            }
        }

        self.camera.update_orbit();

        // Initialize egui
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        let egui_renderer = egui_wgpu::Renderer::new(
            &render_context.device,
            render_context.config.format,
            None,
            1,
            false,
        );

        let mut scene_renderer = SceneRenderer::new(&render_context, RendererConfig { show_grid: true });

        // Load models and update meshes from world
        {
            let state = self.state.lock().unwrap();
            load_models_from_world(
                &state.world,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &state.scene_path,
            );
            scene_renderer.update_from_world(&state.world, &render_context.device);
        }

        // Run initial constraint evaluation
        self.update_constraints();

        // Build initial scene tree
        {
            let state = self.state.lock().unwrap();
            self.scene_tree.update(&state.world);
        }

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        Ok(())
    }

    fn update_constraints(&mut self) {
        let state = self.state.lock().unwrap();
        let evaluator = ConstraintEvaluator::new(
            &state.world,
            &state.registry,
            &state.constraint_registry,
        );
        let report = evaluator.validate();
        self.violation_count = report.violations.len();
        self.violation_messages = report
            .violations
            .iter()
            .map(|v| format!("[{:?}] {}: {}", v.severity, v.entity_name, v.message))
            .collect();
    }

    fn render(&mut self) {
        if self.render_context.is_none()
            || self.scene_renderer.is_none()
            || self.window.is_none()
        {
            return;
        }

        // Animate camera snap transitions
        self.animate_camera();

        let output = match self
            .render_context
            .as_ref()
            .unwrap()
            .surface
            .get_current_texture()
        {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => return,
            Err(e) => {
                eprintln!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.render_stats.record_frame();

        // Render the 3D scene
        {
            let context = self.render_context.as_ref().unwrap();
            let renderer = self.scene_renderer.as_mut().unwrap();
            renderer.set_selected_entity(self.scene_tree.selected_entity());
            renderer.render(context, &self.camera, &view).ok();
        }

        // Always render egui overlay (gizmo is always visible, panels are conditional)
        let gizmo_action = self.render_egui(&view);

        // Process gizmo interaction
        if let Some(action) = gizmo_action {
            match action {
                GizmoAction::SnapToView { yaw, pitch } => {
                    self.camera_snap_target = Some((yaw, pitch));
                    self.camera.orthographic = true;
                }
                GizmoAction::OrbitDelta { dyaw, dpitch } => {
                    self.camera.orbit_horizontal(dyaw);
                    self.camera.orbit_vertical(dpitch);
                    self.camera_snap_target = None;
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

        output.present();
    }

    fn animate_camera(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame_time).as_secs_f32().min(0.1);
        self.last_frame_time = now;

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

    fn render_egui(&mut self, target_view: &wgpu::TextureView) -> Option<GizmoAction> {
        // Extract references to disjoint fields to satisfy the borrow checker
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return None,
        };
        let context = match &self.render_context {
            Some(c) => c,
            None => return None,
        };
        let egui_winit = match &mut self.egui_winit {
            Some(e) => e,
            None => return None,
        };

        let raw_input = egui_winit.take_egui_input(&window);

        // Snapshot camera state for gizmo (avoids holding &self.camera across the closure)
        let cam_view = CameraView::from_camera(&self.camera);

        // Build the UI — we need to collect data for the closure without borrowing self
        let scene_tree = &mut self.scene_tree;
        let entity_inspector = &mut self.entity_inspector;
        let render_stats = &self.render_stats;
        let violation_count = self.violation_count;
        let violation_messages = &self.violation_messages;
        let state = &self.state;
        let view_gizmo = &mut self.view_gizmo;
        let transform_gizmo = &mut self.transform_gizmo;
        let show_panels = self.show_inspector;
        let editor = &mut self.editor;
        let camera = &self.camera;
        let dirty = self.dirty;
        let undo_stack = &self.undo_stack;
        let status_message = &self.status_message;

        let mut gizmo_action = None;
        let mut panel_actions: Vec<SplinePanelAction> = Vec::new();
        let mut inspector_edits: Vec<EditAction> = Vec::new();
        let mut gizmo_edits: Vec<EditAction> = Vec::new();
        let mut gizmo_drag_just_ended = false;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if show_panels {
                // Left side panel: scene tree
                egui::SidePanel::left("scene_tree_panel")
                    .default_width(220.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        scene_tree.ui(ui);
                    });

                // Right side panel: spline editor (in editor mode) or entity inspector
                egui::SidePanel::right("inspector_panel")
                    .default_width(300.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        if let Some(ed) = editor.as_mut() {
                            panel_actions = crate::panels::spline_panel::spline_editor_panel(ui, ed);
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

                // Bottom panel: stats + constraint violations + mode indicator + undo info
                egui::TopBottomPanel::bottom("status_panel")
                    .default_height(100.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            render_stats.ui(ui);
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
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 200, 80),
                                    "Modified",
                                );
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
                                            200, 220, 255,
                                            (alpha * 255.0) as u8,
                                        ),
                                        msg,
                                    );
                                }
                            }
                        });

                        if !violation_messages.is_empty() {
                            ui.separator();
                            egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                                for msg in violation_messages {
                                    ui.label(msg);
                                }
                            });
                        }
                    });
            }

            // Transform gizmo for selected entity (non-editor mode only)
            if editor.is_none() {
                if let Some(entity_id) = scene_tree.selected_entity() {
                    let st = state.lock().unwrap();
                    let transform = st.world.get_transform(entity_id).unwrap_or_default();
                    let pos = [transform.position.x, transform.position.y, transform.position.z];
                    let rot = [transform.rotation.x, transform.rotation.y, transform.rotation.z];
                    let scl = [transform.scale.x, transform.scale.y, transform.scale.z];
                    // Use world position for gizmo placement (accounts for parent transforms)
                    let world_pos = st.world.get_world_position(entity_id)
                        .map(|p| [p.x, p.y, p.z])
                        .unwrap_or(pos);
                    drop(st); // Release lock before gizmo draw (it reads ctx.input)

                    let render_rect = ctx.screen_rect();
                    let clip_rect = ctx.available_rect();

                    if let Some(delta) = transform_gizmo.draw(
                        ctx,
                        camera,
                        entity_id,
                        world_pos, rot, scl,
                        render_rect,
                        clip_rect,
                    ) {
                        let edits = apply_gizmo_delta(&delta, pos, rot, scl);
                        gizmo_edits.extend(edits);
                    }

                    // Detect drag end for undo coalescing
                    if !transform_gizmo.is_dragging() && transform_gizmo.drag_start_transform().is_some() {
                        gizmo_drag_just_ended = true;
                    }
                }
            }

            // View orientation gizmo (always visible)
            gizmo_action = view_gizmo.draw(ctx, &cam_view);

            // Draw spline overlay on the central area (editor mode)
            if let Some(ed) = editor.as_ref() {
                let screen_rect = ctx.screen_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("spline_overlay"),
                ));
                ed.draw_overlay(&painter, camera, [screen_rect.width(), screen_rect.height()]);
            }
        });

        // Process spline panel actions outside the egui closure
        for action in panel_actions {
            if let Some(ed) = &mut self.editor {
                match action {
                    SplinePanelAction::Save => {
                        if let Err(e) = ed.save() {
                            eprintln!("Save failed: {}", e);
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
                if let Some(components) = st.world.get_components_mut(edit.entity_id) {
                    components.set_field(&edit.component, &edit.field, edit.new_value.clone());
                }
            }
            // Update renderer
            if let (Some(ctx), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
                renderer.update_from_world(&st.world, &ctx.device);
            }
            self.dirty = true;
        }

        if gizmo_drag_just_ended {
            self.gizmo_drag_ended = true;
        }

        egui_winit.handle_platform_output(&window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder =
            context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui Encoder"),
                });

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
        }

        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
        }

        context
            .queue
            .submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);

        gizmo_action
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
                // Track dirty fields for structure-preserving save
                if let Some(name) = st.world.get_name(edit.entity_id) {
                    self.dirty_fields.insert((
                        name.to_string(),
                        edit.component.clone(),
                        edit.field.clone(),
                    ));
                }
                if let Some(components) = st.world.get_components_mut(edit.entity_id) {
                    components.set_field(&edit.component, &edit.field, edit.new_value.clone());
                }
            }
        }

        // Update renderer
        {
            let st = self.state.lock().unwrap();
            if let (Some(ctx), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
                renderer.update_from_world(&st.world, &ctx.device);
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
        self.dirty = true;
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
            let entity_name = st.world.get_name(eid).map(|n| n.to_string());
            drop(st);

            let mut actions = Vec::new();

            if (0..3).any(|i| (cur_pos[i] - start_pos[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: "transform".to_string(),
                    field: "position".to_string(),
                    old_value: vec3_to_toml(start_pos),
                    new_value: vec3_to_toml(cur_pos),
                });
            }
            if (0..3).any(|i| (cur_rot[i] - start_rot[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: "transform".to_string(),
                    field: "rotation".to_string(),
                    old_value: vec3_to_toml(start_rot),
                    new_value: vec3_to_toml(cur_rot),
                });
            }
            if (0..3).any(|i| (cur_scl[i] - start_scl[i]).abs() > 1e-4) {
                actions.push(EditAction {
                    entity_id: eid,
                    component: "transform".to_string(),
                    field: "scale".to_string(),
                    old_value: vec3_to_toml(start_scl),
                    new_value: vec3_to_toml(cur_scl),
                });
            }

            if !actions.is_empty() {
                // Track dirty fields for patcher
                if let Some(name) = &entity_name {
                    for action in &actions {
                        self.dirty_fields.insert((
                            name.clone(),
                            action.component.clone(),
                            action.field.clone(),
                        ));
                    }
                }

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

    /// Undo the last edit
    fn undo(&mut self) {
        if let Some(cmd) = self.undo_stack.undo() {
            let mut st = self.state.lock().unwrap();
            for action in &cmd.actions {
                if let Some(components) = st.world.get_components_mut(action.entity_id) {
                    components.set_field(&action.component, &action.field, action.old_value.clone());
                }
            }
            drop(st);
            self.refresh_renderer();
            println!("Undo: {}", cmd.description);
        }
    }

    /// Redo the last undone edit
    fn redo(&mut self) {
        if let Some(cmd) = self.undo_stack.redo() {
            let mut st = self.state.lock().unwrap();
            for action in &cmd.actions {
                if let Some(components) = st.world.get_components_mut(action.entity_id) {
                    components.set_field(&action.component, &action.field, action.new_value.clone());
                }
            }
            drop(st);
            self.refresh_renderer();
            println!("Redo: {}", cmd.description);
        }
    }

    /// Save the scene to disk using structure-preserving patcher when possible
    fn save(&mut self) {
        let mut st = self.state.lock().unwrap();
        let scene_path = st.scene_path.clone();
        let has_doc = st.scene_doc.is_some();

        // Try structure-preserving save via patcher
        if has_doc && !self.dirty_fields.is_empty() {
            // Collect patch data from world first (immutable borrow of world)
            let patches: Vec<(String, String, String, toml::Value)> = self.dirty_fields.iter()
                .filter_map(|(entity_name, component, field)| {
                    let eid = st.world.get_id(entity_name)?;
                    let components = st.world.get_components(eid)?;
                    let value = components.get_field(component, field)?;
                    Some((entity_name.clone(), component.clone(), field.clone(), value.clone()))
                })
                .collect();

            // Now apply patches (mutable borrow of doc only)
            let doc = st.scene_doc.as_mut().unwrap();
            for (entity_name, component, field, value) in &patches {
                if let Err(e) = doc.patch_field(entity_name, component, field, value) {
                    eprintln!("Patch error: {}", e);
                }
            }

            match doc.save(&scene_path) {
                Ok(()) => {
                    drop(st);
                    self.dirty = false;
                    self.dirty_fields.clear();
                    self.last_save_time = Some(Instant::now());
                    self.suppress_reload_until = Some(Instant::now() + Duration::from_millis(1500));
                    self.update_window_title();
                    self.status_message = Some(("Saved!".to_string(), Instant::now()));
                    println!("Saved scene to {} (structure-preserving)", scene_path);
                    return;
                }
                Err(e) => {
                    eprintln!("Patcher save failed, falling back to full save: {}", e);
                }
            }
        }

        // Fallback: full serialize save
        let scene_name = Path::new(&scene_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scene")
            .to_string();

        match save_scene(&scene_path, &st.world, &scene_name) {
            Ok(()) => {
                // Re-parse the document after full save so future patches work
                st.scene_doc = SceneDocument::from_file(&scene_path).ok();
                drop(st);
                self.dirty = false;
                self.dirty_fields.clear();
                self.last_save_time = Some(Instant::now());
                self.suppress_reload_until = Some(Instant::now() + Duration::from_millis(1500));
                self.update_window_title();
                self.status_message = Some(("Saved!".to_string(), Instant::now()));
                println!("Saved scene to {}", scene_path);
            }
            Err(e) => {
                eprintln!("Failed to save scene: {:?}", e);
                drop(st);
                self.status_message = Some((format!("Save failed: {}", e), Instant::now()));
            }
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
    fn refresh_renderer(&mut self) {
        let st = self.state.lock().unwrap();
        if let (Some(ctx), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.update_from_world(&st.world, &ctx.device);
        }
    }

    /// Handle mouse click picking in the viewport
    fn try_pick_entity(&mut self, x: f64, y: f64) {
        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };

        // Check if pointer is over egui panels
        if self.egui_ctx.is_pointer_over_area() {
            return;
        }

        // Don't pick while gizmo is active
        if self.transform_gizmo.is_dragging() || self.transform_gizmo.is_hovered() {
            return;
        }

        let st = self.state.lock().unwrap();
        let targets = build_pick_targets(&st.world);
        drop(st);

        let vw = context.config.width as f32;
        let vh = context.config.height as f32;

        if let Some((entity_id, _dist)) = pick_entity(x as f32, y as f32, vw, vh, &self.camera, &targets) {
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

        let (needs_reload, scene_path) = {
            let state = self.state.lock().unwrap();
            (state.needs_reload, state.scene_path.clone())
        };

        if needs_reload {
            {
                let mut state = self.state.lock().unwrap();
                state.needs_reload = false;
            }

            let reload_result = {
                let state = self.state.lock().unwrap();
                load_scene(&scene_path, &state.registry)
            };

            match reload_result {
                Ok((new_world, scene_file)) => {
                    println!("Reloaded scene: {}", scene_file.scene.name);

                    {
                        let mut state = self.state.lock().unwrap();
                        state.world = new_world;
                    }

                    if let (Some(context), Some(renderer)) =
                        (&self.render_context, &mut self.scene_renderer)
                    {
                        let state = self.state.lock().unwrap();
                        load_models_from_world(
                            &state.world,
                            renderer,
                            &context.device,
                            &context.queue,
                            &state.scene_path,
                        );
                        renderer.update_from_world(&state.world, &context.device);
                    }

                    {
                        let state = self.state.lock().unwrap();
                        self.scene_tree.update(&state.world);
                    }
                    self.update_constraints();

                    // External reload clears undo and dirty state
                    self.undo_stack.clear();
                    self.dirty = false;
                    self.dirty_fields.clear();
                    self.update_window_title();

                    // Re-parse scene document for patcher
                    {
                        let mut state = self.state.lock().unwrap();
                        state.scene_doc = SceneDocument::from_file(&scene_path).ok();
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reload scene: {:?}", e);
                }
            }
        }
    }

    /// Get screen size from render context.
    fn screen_size(&self) -> [f32; 2] {
        if let Some(ctx) = &self.render_context {
            [ctx.config.width as f32, ctx.config.height as f32]
        } else {
            [1600.0, 900.0]
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
            editor.update_hover(
                &self.camera,
                screen,
                position.x as f32,
                position.y as f32,
            );
        }
    }

    fn handle_editor_key(&mut self, key: KeyCode) -> bool {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        match key {
            KeyCode::KeyS if ctrl => {
                if let Some(editor) = &mut self.editor {
                    if let Err(e) = editor.save() {
                        eprintln!("Save failed: {}", e);
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
                                    if idx == 0 { n - 1 } else { idx - 1 }
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

/// Scan the world for entities with model components and load the referenced glTF files
fn load_models_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let model_asset = world
            .get_components(entity.id)
            .and_then(|components| components.get("model").cloned())
            .and_then(|model| {
                model
                    .get("asset")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                continue;
            }

            // Search scene dir first, then parent (game root)
            let model_path = {
                let p = scene_dir.join("models").join(format!("{}.glb", asset_name));
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join("models").join(format!("{}.glb", asset_name))
                } else {
                    p
                }
            };

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        println!(
                            "Loaded model: {} ({} meshes, {} materials)",
                            asset_name,
                            import_result.meshes.len(),
                            import_result.materials.len()
                        );
                        renderer.load_model(device, queue, &asset_name, &import_result);
                    }
                    Err(e) => {
                        eprintln!("Failed to load model '{}': {:?}", asset_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Model file not found: {} (tried {})",
                    asset_name,
                    model_path.display()
                );
            }
        }
    }

    // Also load texture files referenced by material components
    load_textures_from_world(world, renderer, device, queue, scene_path);
}

/// Scan the world for entities with material.texture and pre-load the referenced image files
fn load_textures_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut loaded: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entity in world.all_entities() {
        let texture_name = world
            .get_components(entity.id)
            .and_then(|components| components.get("material").cloned())
            .and_then(|material| {
                material
                    .get("texture")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(tex_name) = texture_name {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            let tex_path = scene_dir.join(&tex_name);
            if tex_path.exists() {
                match renderer.load_texture_file(device, queue, &tex_name, &tex_path) {
                    Ok(true) => {
                        println!("Loaded texture: {}", tex_name);
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("Failed to load texture '{}': {}", tex_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Texture file not found: {} (tried {})",
                    tex_name,
                    tex_path.display()
                );
            }
        }
    }
}

impl ApplicationHandler for ViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.initialize(event_loop) {
                eprintln!("Failed to initialize viewer: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_editor = self.editor.is_some();

        // Track modifier keys
        if let WindowEvent::ModifiersChanged(mods) = &event {
            self.modifiers = mods.state();
        }

        // Handle gizmo input priority: if gizmo is hovered/dragging, suppress camera orbit
        // but still let egui process the event for gizmo interaction
        let gizmo_active = self.transform_gizmo.is_dragging() || self.transform_gizmo.is_hovered();

        // Handle Tab and other global shortcuts before egui can consume them.
        if let WindowEvent::KeyboardInput { event: ref key_event, .. } = event {
            if key_event.state == ElementState::Pressed && !key_event.repeat {
                if is_editor {
                    // In editor mode, handle editor-specific keys first
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        if self.handle_editor_key(code) {
                            return;
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
                                return;
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() && self.modifiers.shift_key() => {
                                // Ctrl+Shift+Z = redo
                                self.redo();
                                return;
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() => {
                                // Ctrl+Z = undo
                                self.undo();
                                return;
                            }
                            KeyCode::KeyY if self.modifiers.control_key() => {
                                // Ctrl+Y = redo
                                self.redo();
                                return;
                            }
                            KeyCode::KeyS if self.modifiers.control_key() => {
                                // Ctrl+S = save scene
                                self.save();
                                return;
                            }
                            // W = Translate gizmo mode
                            KeyCode::KeyW if self.modifiers.is_empty() => {
                                self.transform_gizmo.mode = GizmoMode::Translate;
                                return;
                            }
                            // E = Rotate gizmo mode
                            KeyCode::KeyE if self.modifiers.is_empty() => {
                                self.transform_gizmo.mode = GizmoMode::Rotate;
                                return;
                            }
                            // R = Scale gizmo mode
                            KeyCode::KeyR if self.modifiers.is_empty() => {
                                self.transform_gizmo.mode = GizmoMode::Scale;
                                return;
                            }
                            // Ctrl+R = Reload
                            KeyCode::KeyR if self.modifiers.control_key() => {
                                if let Ok(mut state) = self.state.lock() {
                                    state.needs_reload = true;
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Let egui handle the event first
        if let Some(egui_winit) = &mut self.egui_winit {
            if let Some(window) = &self.window {
                let response = egui_winit.on_window_event(window, &event);
                if response.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                if let Some(context) = &mut self.render_context {
                    context.resize(new_size);
                    self.camera.aspect = context.aspect_ratio();
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.resize_postprocess(
                            &context.device,
                            new_size.width,
                            new_size.height,
                        );
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) if !self.transform_gizmo.is_dragging() => {
                            event_loop.exit();
                        }

                        PhysicalKey::Code(KeyCode::Space) if !is_editor => {
                            self.camera = Camera::new();
                            self.camera.update_orbit();
                            if let Some(context) = &self.render_context {
                                self.camera.aspect = context.aspect_ratio();
                            }
                        }
                        PhysicalKey::Code(KeyCode::F1) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let next = renderer.debug_state().mode.next();
                                renderer.set_debug_mode(next);
                                println!("Debug mode: {}", next.label());

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F2) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_wireframe_overlay();
                                println!(
                                    "Wireframe overlay: {}",
                                    if on { "ON" } else { "OFF" }
                                );

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F3) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_normal_arrows();
                                println!(
                                    "Normal arrows: {}",
                                    if on { "ON" } else { "OFF" }
                                );

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F4) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_shadows();
                                println!("Shadows: {}", if on { "ON" } else { "OFF" });
                            }
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
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

            WindowEvent::CursorMoved { position, .. } => {
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
                        }

                        if self.right_mouse_pressed {
                            self.camera.pan(-dx * 0.02, dy * 0.02);
                            self.camera_snap_target = None;
                        }
                    }

                    self.last_mouse_pos = Some((position.x, position.y));
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 100.0,
                };
                self.camera.zoom(scroll);
            }

            WindowEvent::RedrawRequested => {
                self.check_reload();
                self.render();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

// --- Helpers ---

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
