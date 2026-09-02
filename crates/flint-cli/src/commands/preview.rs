//! Preview a 3D model file (GLB/glTF) with orbit camera
//!
//! Three modes:
//! 1. `flint preview model.glb` — interactive window with orbit camera
//! 2. `flint preview` — empty window, drag-and-drop a .glb/.gltf to load
//! 3. `flint preview model.glb --render out.png` — headless render to PNG

use anyhow::{Context, Result};
use flint_animation::node_clip::NodeClip;
use flint_animation::skeletal_clip::SkeletalClip;
use flint_animation::skeleton::Skeleton;
use flint_animation::{
    AnimLayer, AnimationSystem, LayerContribution, LayerMode, SequenceStep, WRITER_BASE,
    WRITER_REST,
};
use flint_core::components as comp;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_import::{import_gltf, ImportResult, MeshBounds};
use flint_render::model_loader::{self, ModelLoadConfig, ModelLoadResult};
use flint_render::{
    Camera, DebugMode, HeadlessContext, OrbitCameraController, RendererConfig, SceneRenderer,
};
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(clap::Args)]
pub struct PreviewArgs {
    /// Path to model file (.glb or .gltf). If omitted, opens an empty window for drag-and-drop.
    pub model: Option<String>,

    /// Render to a PNG file instead of opening a window
    #[arg(long)]
    pub render: Option<String>,

    /// Image width in pixels
    #[arg(long, default_value = "1280")]
    pub width: u32,

    /// Image height in pixels
    #[arg(long, default_value = "720")]
    pub height: u32,

    /// Camera orbit distance
    #[arg(long)]
    pub distance: Option<f32>,

    /// Camera horizontal angle in degrees
    #[arg(long)]
    pub yaw: Option<f32>,

    /// Camera vertical angle in degrees
    #[arg(long)]
    pub pitch: Option<f32>,

    /// Camera look-at point (comma-separated x,y,z)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees
    #[arg(long)]
    pub fov: Option<f32>,

    /// Disable ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Watch model file for changes and auto-reload
    #[arg(long)]
    pub watch: bool,

    /// Disable animation playback (animations play by default when present)
    #[arg(long)]
    pub no_animate: bool,

    /// Start with a specific animation clip by name
    #[arg(long)]
    pub clip: Option<String>,

    /// Animation playback speed multiplier (default: 1.0)
    #[arg(long, default_value = "1.0")]
    pub anim_speed: f32,

    /// Sample animation at a specific time in seconds (headless --render mode only)
    #[arg(long)]
    pub anim_time: Option<f32>,

    /// Add an animation layer: `clip[:weight[:mask[:mode]]]` (repeatable, in order)
    #[arg(long = "layer")]
    pub layers: Vec<String>,

    /// Play a `*.sequence.toml` of timestamped animator events (blend /
    /// layer / speed / cue). With --render, --anim-time samples the
    /// sequence by deterministic replay.
    #[arg(long)]
    pub sequence: Option<String>,

    /// Loop the --sequence regardless of its `loop` setting
    #[arg(long)]
    pub sequence_loop: bool,

    /// Start with auto-orbit enabled
    #[arg(long)]
    pub auto_orbit: bool,
}

pub fn run(args: PreviewArgs) -> Result<()> {
    if let Some(render_output) = &args.render {
        run_headless(&args, render_output)
    } else {
        run_interactive(args)
    }
}

// ---------------------------------------------------------------------------
// Animation info tracked alongside the preview
// ---------------------------------------------------------------------------

struct AnimationInfo {
    clip_names: Vec<String>,
    current_clip_index: usize,
    /// Skeletal clips only (the ones a layer can play)
    skeletal_clip_names: Vec<String>,
    /// Joint names of the first skeleton (mask targets, overlay labels)
    joint_names: Vec<String>,
}

/// One event marker on the sequence timeline
#[derive(Clone)]
struct SequenceMarker {
    time: f64,
    label: String,
    kind: &'static str,
    /// Layer index for `layer` events (colours the marker like the stack)
    layer: Option<usize>,
}

/// The sequence loaded by `--sequence`, plus what seeking needs
#[derive(Clone)]
struct SequenceUi {
    name: String,
    duration: f64,
    markers: Vec<SequenceMarker>,
    /// Animator table before the sequence wrote anything — restored
    /// before every replay so seeks are deterministic.
    initial_animator: toml::Value,
}

/// Load `--sequence`, register it, snapshot the animator and start it.
/// The snapshot is taken here, after `--clip`/`--layer` were written.
fn attach_sequence(
    path: &str,
    animation: &mut AnimationSystem,
    world: &FlintWorld,
    entity_id: EntityId,
    force_loop: bool,
) -> Option<SequenceUi> {
    let seq = match flint_animation::load_sequence_from_file(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to load sequence '{}': {:?}", path, e);
            return None;
        }
    };
    let known = animation.skeletal_clip_names();
    for ev in &seq.events {
        let clip = match &ev.step {
            SequenceStep::Blend { clip, .. } => Some(clip.as_str()),
            SequenceStep::Layer { clip, .. } => clip.as_deref(),
            _ => None,
        };
        if let Some(c) = clip {
            if !known.iter().any(|k| k == c) {
                tracing::warn!(
                    "Sequence '{}' references unknown clip '{}' at {:.2}s; available: {}",
                    seq.name,
                    c,
                    ev.time,
                    known.join(", ")
                );
            }
        }
    }
    let markers = seq
        .events
        .iter()
        .map(|ev| SequenceMarker {
            time: ev.time,
            label: ev.step.label(),
            kind: ev.step.kind(),
            layer: match &ev.step {
                SequenceStep::Layer { index, .. } => Some(*index),
                _ => None,
            },
        })
        .collect();
    let ui = SequenceUi {
        name: seq.name.clone(),
        duration: seq.resolved_duration(),
        markers,
        initial_animator: world
            .get_components(entity_id)
            .and_then(|c| c.get(comp::ANIMATOR))
            .cloned()
            .unwrap_or(toml::Value::Table(Default::default())),
    };
    println!(
        "Sequence '{}': {:.2}s, {} events",
        ui.name,
        ui.duration,
        ui.markers.len()
    );
    animation.add_sequence(seq);
    animation.play_sequence(entity_id, &ui.name);
    if force_loop {
        animation.set_sequence_loop_override(&entity_id, Some(true));
    }
    Some(ui)
}

/// Restore the pre-sequence animator and replay to `t`.
fn seek_sequence_in_world(
    seq: &SequenceUi,
    animation: &mut AnimationSystem,
    world: &mut FlintWorld,
    entity_id: EntityId,
    t: f64,
) -> usize {
    let _ = world.set_component(entity_id, comp::ANIMATOR, seq.initial_animator.clone());
    animation.seek_sequence(world, entity_id, t, 1.0 / 120.0)
}

/// Marker colour by event kind (layers reuse the stack palette)
fn marker_color(kind: &str, layer: Option<usize>) -> egui::Color32 {
    match kind {
        "blend" => egui::Color32::from_rgb(255, 160, 60),
        "layer" => to_color32(layer_color(layer.unwrap_or(0))),
        "speed" => egui::Color32::from_gray(170),
        _ => egui::Color32::from_rgb(80, 220, 230),
    }
}

/// Parse a `--layer clip[:weight[:mask[:mode]]]` spec.
fn parse_layer_spec(spec: &str) -> AnimLayer {
    let mut parts = spec.split(':');
    let mut layer = AnimLayer::new(parts.next().unwrap_or("").trim(), 1.0);
    if let Some(w) = parts.next().and_then(|w| w.trim().parse::<f32>().ok()) {
        layer.weight = w.clamp(0.0, 1.0);
    }
    if let Some(m) = parts.next() {
        layer.mask = m.trim().to_string();
    }
    if let Some(mode) = parts.next() {
        layer.mode = LayerMode::parse(mode);
    }
    layer
}

/// Write a layer list to the preview entity's animator component.
fn write_layers_to_world(world: &mut FlintWorld, entity_id: EntityId, layers: &[AnimLayer]) {
    if let Some(components) = world.get_components_mut(entity_id) {
        components.set_field(
            comp::ANIMATOR,
            "layers",
            toml::Value::Array(layers.iter().map(AnimLayer::to_toml).collect()),
        );
    }
}

// ---------------------------------------------------------------------------
// Layer visualisation
// ---------------------------------------------------------------------------

/// How the skeleton overlay / node tree colour joints by layer activity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum LayerViz {
    #[default]
    Off,
    /// Colour each bone by the last thing that wrote it (base / layer n / rest)
    LastWriter,
    /// Grey → layer colour by the weight layer `n` applied to the bone
    Weight(usize),
    /// Green inside layer `n`'s mask, grey outside
    Mask(usize),
    /// Cyan where layer `n`'s clip keys the bone
    Keyed(usize),
}

impl LayerViz {
    fn label(&self) -> String {
        match self {
            LayerViz::Off => "Plain".into(),
            LayerViz::LastWriter => "Last writer".into(),
            LayerViz::Weight(i) => format!("L{} weight", i + 1),
            LayerViz::Mask(i) => format!("L{} mask", i + 1),
            LayerViz::Keyed(i) => format!("L{} keyed joints", i + 1),
        }
    }
}

const BASE_COLOR: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const REST_COLOR: [f32; 4] = [0.4, 0.4, 0.4, 1.0];
const LAYER_PALETTE: [[f32; 4]; 6] = [
    [0.2, 0.8, 1.0, 1.0],
    [1.0, 0.4, 0.8, 1.0],
    [0.4, 1.0, 0.4, 1.0],
    [1.0, 0.6, 0.2, 1.0],
    [0.7, 0.5, 1.0, 1.0],
    [1.0, 1.0, 0.4, 1.0],
];

fn layer_color(index: usize) -> [f32; 4] {
    LAYER_PALETTE[index % LAYER_PALETTE.len()]
}

fn to_color32(c: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        1.0,
    ]
}

/// Per-joint overlay colour for a visualisation mode.
fn joint_viz_color(viz: LayerViz, joint: usize, contrib: Option<&LayerContribution>) -> [f32; 4] {
    let Some(c) = contrib else {
        return BASE_COLOR;
    };
    match viz {
        LayerViz::Off => BASE_COLOR,
        LayerViz::LastWriter => match c.last_writer.get(joint).copied() {
            Some(WRITER_REST) | None => REST_COLOR,
            Some(WRITER_BASE) => BASE_COLOR,
            Some(n) => layer_color(n as usize - 1),
        },
        LayerViz::Weight(li) => mix(REST_COLOR, layer_color(li), c.weight(li, joint)),
        LayerViz::Mask(li) => {
            if c.in_mask(li, joint) {
                [0.3, 1.0, 0.3, 1.0]
            } else {
                REST_COLOR
            }
        }
        LayerViz::Keyed(li) => {
            if c.is_keyed(li, joint) {
                [0.2, 1.0, 1.0, 1.0]
            } else {
                REST_COLOR
            }
        }
    }
}

/// Tooltip text describing what drives a joint.
fn joint_viz_tooltip(
    joint: usize,
    contrib: Option<&LayerContribution>,
    layers: &[AnimLayer],
) -> String {
    let Some(c) = contrib else {
        return String::new();
    };
    let mut lines = Vec::new();
    match c.last_writer.get(joint).copied() {
        Some(WRITER_REST) | None => lines.push("rest pose (nothing keys this joint)".to_string()),
        Some(WRITER_BASE) => lines.push("base clip".to_string()),
        Some(n) => lines.push(format!("last written by L{}", n)),
    }
    for (li, layer) in layers.iter().enumerate() {
        if !layer.is_active() {
            continue;
        }
        let keyed = c.is_keyed(li, joint);
        let masked = c.in_mask(li, joint);
        let w = c.weight(li, joint);
        lines.push(format!(
            "L{} {} ({}): {}{}w={:.2}",
            li + 1,
            layer.clip,
            layer.mode.as_str(),
            if keyed { "keyed, " } else { "not keyed, " },
            if masked { "" } else { "masked out, " },
            w
        ));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Model statistics for UI display
// ---------------------------------------------------------------------------

struct ModelStats {
    total_vertices: usize,
    total_triangles: usize,
    mesh_count: usize,
    material_count: usize,
    node_count: usize,
    skeleton_joint_count: usize,
    bounds: Option<MeshBounds>,
}

impl ModelStats {
    fn from_import(import: &ImportResult) -> Self {
        let total_vertices: usize = import.meshes.iter().map(|m| m.positions.len()).sum();
        let total_triangles: usize = import.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let skeleton_joint_count: usize = import.skeletons.iter().map(|s| s.joints.len()).sum();
        Self {
            total_vertices,
            total_triangles,
            mesh_count: import.meshes.len(),
            material_count: import.materials.len(),
            node_count: import.nodes.len(),
            skeleton_joint_count,
            bounds: import.bounds(),
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal ECS world for a single model
// ---------------------------------------------------------------------------

/// Create an ECS world with one entity at origin containing transform + model + animator components.
/// Returns (world, asset_name, entity_id).
fn create_model_world(model_path: &Path, anim_speed: f32) -> (FlintWorld, String, EntityId) {
    let asset_name = model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();

    let mut world = FlintWorld::new();
    let entity_id = world.spawn("preview_model").expect("spawn entity");

    // Transform at origin
    let transform = toml::Value::Table({
        let mut t = toml::map::Map::new();
        t.insert(
            "position".to_string(),
            toml::Value::Array(vec![
                toml::Value::Float(0.0),
                toml::Value::Float(0.0),
                toml::Value::Float(0.0),
            ]),
        );
        t
    });
    let _ = world.set_component(entity_id, comp::TRANSFORM, transform);

    // Model component pointing to asset name
    let model = toml::Value::Table({
        let mut m = toml::map::Map::new();
        m.insert("asset".to_string(), toml::Value::String(asset_name.clone()));
        m
    });
    let _ = world.set_component(entity_id, comp::MODEL, model);

    // Animator component — enables animated model expansion and skeletal sync discovery
    let animator = toml::Value::Table({
        let mut a = toml::map::Map::new();
        a.insert("clip".to_string(), toml::Value::String(String::new()));
        a.insert("playing".to_string(), toml::Value::Boolean(true));
        a.insert("loop".to_string(), toml::Value::Boolean(true));
        a.insert("speed".to_string(), toml::Value::Float(anim_speed as f64));
        a
    });
    let _ = world.set_component(entity_id, comp::ANIMATOR, animator);

    (world, asset_name, entity_id)
}

/// Build a ModelLoadConfig with an override so the asset name resolves to the exact file path.
fn model_load_config(model_path: &Path, asset_name: &str) -> ModelLoadConfig {
    let scene_dir = model_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut overrides = HashMap::new();
    overrides.insert(asset_name.to_string(), model_path.to_path_buf());
    ModelLoadConfig {
        scene_dir,
        overrides,
    }
}

// ---------------------------------------------------------------------------
// Animation registration from model load results
// ---------------------------------------------------------------------------

/// Register skeletal and node animation data from loaded models into the animation system.
/// Returns AnimationInfo if any clips were found, and the skeletal entity→asset map for bone upload.
fn register_animation_data(
    load_result: &ModelLoadResult,
    animation: &mut AnimationSystem,
    world: &mut FlintWorld,
    entity_id: EntityId,
    requested_clip: Option<&str>,
    anim_speed: f32,
    initial_layers: &[String],
) -> (Option<AnimationInfo>, HashMap<EntityId, String>) {
    let mut all_clip_names: Vec<String> = Vec::new();
    let mut skeletal_clip_names: Vec<String> = Vec::new();
    let mut joint_names: Vec<String> = Vec::new();
    let mut skeletal_entity_assets: HashMap<EntityId, String> = HashMap::new();

    for loaded in &load_result.models {
        // Skeletal animation
        if loaded.is_skinned {
            if let Some(ref import_result) = loaded.import_result {
                for imported_skel in &import_result.skeletons {
                    let skeleton = Skeleton::from_imported(imported_skel);
                    if joint_names.is_empty() {
                        joint_names = skeleton.joint_names.clone();
                    }
                    animation.add_skeleton(loaded.entity_id, skeleton);
                }
                for imported_clip in &import_result.skeletal_clips {
                    let clip = SkeletalClip::from_imported(imported_clip);
                    println!(
                        "  Skeletal clip: {} ({:.1}s, {} tracks)",
                        clip.name,
                        clip.duration,
                        clip.joint_tracks.len()
                    );

                    all_clip_names.push(clip.name.clone());
                    skeletal_clip_names.push(clip.name.clone());
                    animation.add_skeletal_clip(clip);
                }

                // Add skeleton component so SkeletalSync::sync_from_world discovers this entity
                let skeleton_comp = toml::Value::Table({
                    let mut s = toml::map::Map::new();
                    s.insert("skin".to_string(), toml::Value::String(String::new()));
                    s
                });
                let _ = world.set_component(loaded.entity_id, comp::SKELETON, skeleton_comp);

                skeletal_entity_assets.insert(loaded.entity_id, loaded.asset_name.clone());
            }
        }

        // Node animation
        if let Some(ref import_result) = loaded.import_result {
            for imported_clip in &import_result.node_clips {
                let clip = NodeClip::from_imported(imported_clip);
                println!(
                    "  Node clip: {} ({:.1}s, {} tracks)",
                    clip.name,
                    clip.duration,
                    clip.node_tracks.len()
                );

                all_clip_names.push(clip.name.clone());
                animation.add_node_clip(clip);
            }
        }
        if let Some(ref node_map) = loaded.node_map {
            animation.register_node_entity(loaded.entity_id, node_map.clone());
        }
    }

    // Deduplicate (skeletal and node clips could share names, though unlikely)
    all_clip_names.sort();
    all_clip_names.dedup();

    if all_clip_names.is_empty() {
        return (None, skeletal_entity_assets);
    }

    // Determine which clip to start with
    let current_clip_index = if let Some(requested) = requested_clip {
        all_clip_names
            .iter()
            .position(|n| n == requested)
            .unwrap_or_else(|| {
                tracing::warn!(
                    "Clip '{}' not found; available: {}",
                    requested,
                    all_clip_names.join(", ")
                );
                0
            })
    } else {
        0
    };

    let clip_name = &all_clip_names[current_clip_index];

    // Set the clip on the animator component so sync_from_world picks it up
    if let Some(components) = world.get_components_mut(entity_id) {
        components.set_field(
            comp::ANIMATOR,
            "clip",
            toml::Value::String(clip_name.clone()),
        );
        components.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
        components.set_field(
            comp::ANIMATOR,
            "speed",
            toml::Value::Float(anim_speed as f64),
        );
    }

    // Initial layers from --layer specs
    let layers: Vec<AnimLayer> = initial_layers.iter().map(|s| parse_layer_spec(s)).collect();
    if !layers.is_empty() {
        for l in &layers {
            if !skeletal_clip_names.contains(&l.clip) {
                tracing::warn!(
                    "Layer clip '{}' not found; available: {}",
                    l.clip,
                    skeletal_clip_names.join(", ")
                );
            }
        }
        write_layers_to_world(world, entity_id, &layers);
    }

    skeletal_clip_names.sort();
    skeletal_clip_names.dedup();

    let info = AnimationInfo {
        clip_names: all_clip_names,
        current_clip_index,
        skeletal_clip_names,
        joint_names,
    };

    (Some(info), skeletal_entity_assets)
}

// ---------------------------------------------------------------------------
// Auto-fit camera to model bounds
// ---------------------------------------------------------------------------

fn auto_fit_camera(bounds: &MeshBounds, camera: &mut Camera) {
    let center = [
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        (bounds.min[2] + bounds.max[2]) * 0.5,
    ];
    let size = bounds.size();
    let diagonal = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();

    camera.target = Vec3::new(center[0], center[1], center[2]);
    camera.distance = (diagonal * 1.2).max(2.0);
    camera.yaw = std::f32::consts::FRAC_PI_4; // 45 degrees
    camera.pitch = 0.5; // ~30 degrees
}

fn apply_cli_overrides(camera: &mut Camera, args: &PreviewArgs) {
    if let Some(d) = args.distance {
        camera.distance = d;
    }
    if let Some(y) = args.yaw {
        camera.yaw = y.to_radians();
    }
    if let Some(p) = args.pitch {
        camera.pitch = p.to_radians();
    }
    if let Some(t) = args.target {
        camera.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(f) = args.fov {
        camera.fov = f;
    }
}

// ---------------------------------------------------------------------------
// Headless render mode
// ---------------------------------------------------------------------------

fn run_headless(args: &PreviewArgs, output_path: &str) -> Result<()> {
    let model_str = args
        .model
        .as_deref()
        .context("A model path is required for headless rendering (--render)")?;
    let model_path = PathBuf::from(model_str);

    if !model_path.exists() {
        anyhow::bail!("Model file not found: {}", model_path.display());
    }

    // Import to get bounds for auto-fit
    let import_result =
        import_gltf(&model_path).context("Failed to import model for bounds computation")?;
    let bounds = import_result.bounds();

    // Headless context
    let ctx = pollster::block_on(HeadlessContext::new(args.width, args.height))
        .context("Failed to create headless render context")?;

    // Camera
    let mut camera = Camera::new();
    camera.aspect = ctx.aspect_ratio();
    if let Some(b) = &bounds {
        auto_fit_camera(b, &mut camera);
    }
    apply_cli_overrides(&mut camera, args);
    camera.update_orbit();

    // Renderer
    let mut renderer = SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        ctx.width,
        ctx.height,
        RendererConfig {
            show_grid: !args.no_grid,
            ..Default::default()
        },
    );

    // Build world and load model
    let (mut world, asset_name, entity_id) = create_model_world(&model_path, args.anim_speed);
    let config = model_load_config(&model_path, &asset_name);
    let load_result = model_loader::load_models_from_world(
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
        &config,
    );

    // Animation support for --anim-time / --layer / --sequence
    if let Some(anim_time) = args
        .anim_time
        .or_else(|| (!args.layers.is_empty() || args.sequence.is_some()).then_some(0.0))
    {
        if !args.no_animate {
            let mut animation = AnimationSystem::new();
            let (anim_info, skeletal_entity_assets) = register_animation_data(
                &load_result,
                &mut animation,
                &mut world,
                entity_id,
                args.clip.as_deref(),
                args.anim_speed,
                &args.layers,
            );

            if anim_info.is_some() {
                // Sync and advance to the requested time
                animation.sync_property_from_world(&world);
                animation.sync_skeletal_from_world(&world);
                animation.sync_node_from_world(&world);

                let sequence = args.sequence.as_deref().and_then(|p| {
                    attach_sequence(p, &mut animation, &world, entity_id, args.sequence_loop)
                });
                if let Some(seq) = &sequence {
                    let eid = entity_id;
                    // Deterministic replay: sequence + skeletal tiers stepped
                    // together, not one big dt.
                    let fired = seek_sequence_in_world(
                        seq,
                        &mut animation,
                        &mut world,
                        eid,
                        anim_time as f64,
                    );
                    println!(
                        "Sampled sequence '{}' at t={:.3}s ({} events fired)",
                        seq.name, anim_time, fired
                    );
                } else {
                    // Use update() which handles all three tiers
                    let _ = flint_runtime::RuntimeSystem::update(
                        &mut animation,
                        &mut world,
                        anim_time as f64,
                    );
                }

                // Upload bone matrices
                for (eid, asset) in &skeletal_entity_assets {
                    if let Some(matrices) = animation.bone_matrices(eid) {
                        renderer.update_bone_matrices(
                            &ctx.device,
                            &ctx.queue,
                            *eid,
                            asset,
                            matrices,
                        );
                    }
                }

                println!("Sampled animation at t={:.3}s", anim_time);
            }
        }
    }

    renderer.update_from_world(&world, &ctx.device);

    // Render
    renderer.render_to(
        &ctx.device,
        &ctx.queue,
        &ctx.depth_view,
        &camera,
        &ctx.color_view,
    );

    // Read pixels and save
    let pixels = pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
    let img = image::RgbaImage::from_raw(args.width, args.height, pixels)
        .context("Failed to create image from pixel data")?;
    img.save(output_path)
        .context(format!("Failed to save image to {}", output_path))?;

    println!(
        "Rendered {}x{} preview to {}",
        args.width, args.height, output_path
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive windowed mode
// ---------------------------------------------------------------------------

struct PreviewState {
    world: FlintWorld,
    asset_name: String,
    model_path: Option<PathBuf>,
    has_model: bool,
    needs_reload: bool,
    entity_id: Option<EntityId>,
    import_result: Option<ImportResult>,
    model_stats: Option<ModelStats>,
}

fn run_interactive(args: PreviewArgs) -> Result<()> {
    let mut state = PreviewState {
        world: FlintWorld::new(),
        asset_name: String::new(),
        model_path: None,
        has_model: false,
        needs_reload: false,
        entity_id: None,
        import_result: None,
        model_stats: None,
    };

    // If a model path is provided, create the initial world
    let initial_bounds;
    if let Some(model_str) = &args.model {
        let model_path = PathBuf::from(model_str);
        if !model_path.exists() {
            anyhow::bail!("Model file not found: {}", model_path.display());
        }
        let (world, asset_name, entity_id) = create_model_world(&model_path, args.anim_speed);
        // Import to get bounds and cache the result for UI display
        let import = import_gltf(&model_path).ok();
        initial_bounds = import.as_ref().and_then(|r| r.bounds());
        if let Some(ref ir) = import {
            state.model_stats = Some(ModelStats::from_import(ir));
        }
        state.import_result = import;
        state.world = world;
        state.asset_name = asset_name;
        state.model_path = Some(model_path);
        state.has_model = true;
        state.entity_id = Some(entity_id);
    } else {
        initial_bounds = None;
    }

    let state = Arc::new(Mutex::new(state));

    // File watcher for --watch mode
    let _watcher = if args.watch {
        if let Some(model_str) = &args.model {
            let state_clone = Arc::clone(&state);
            let (tx, rx) = std::sync::mpsc::channel();
            let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
            let model_file = Path::new(model_str.as_str());

            debouncer
                .watcher()
                .watch(model_file, RecursiveMode::NonRecursive)?;

            std::thread::spawn(move || {
                for result in rx {
                    match result {
                        Ok(_events) => {
                            if let Ok(mut s) = state_clone.lock() {
                                s.needs_reload = true;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Watch error: {:?}", e);
                        }
                    }
                }
            });

            println!("Watching for changes...");
            Some(debouncer)
        } else {
            None
        }
    } else {
        None
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let auto_orbit = args.auto_orbit;

    let mut app = PreviewApp {
        state,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),
        initial_bounds,
        args,

        orbit: OrbitCameraController::new(),
        last_frame_time: Instant::now(),

        // Animation
        animation: AnimationSystem::new(),
        skeletal_entity_assets: HashMap::new(),
        anim_info: None,
        anim_paused: false,
        anim_time_accumulator: 0.0,

        // egui overlay
        egui_ctx: egui::Context::default(),
        egui_winit: None,
        egui_renderer: None,
        show_ui: true,

        // FPS tracking
        frame_times: VecDeque::new(),
        fps: 0.0,
        last_fps_update: Instant::now(),
        layer_viz: LayerViz::Off,
        solo_layer: None,
        muted_layers: Vec::new(),
        sequence: None,
    };
    app.orbit.auto_orbit = auto_orbit;

    event_loop.run_app(&mut app)?;

    Ok(())
}

struct PreviewApp {
    state: Arc<Mutex<PreviewState>>,
    window: Option<Arc<Window>>,
    render_context: Option<flint_render::RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,
    initial_bounds: Option<MeshBounds>,
    args: PreviewArgs,

    // Input
    orbit: OrbitCameraController,
    last_frame_time: Instant,

    // Animation
    animation: AnimationSystem,
    skeletal_entity_assets: HashMap<EntityId, String>,
    anim_info: Option<AnimationInfo>,
    anim_paused: bool,
    /// Accumulated playback time for window title display
    anim_time_accumulator: f64,
    /// How the skeleton overlay / node tree colour joints by layer
    layer_viz: LayerViz,
    /// Previewer solo: every other layer is muted at runtime
    solo_layer: Option<usize>,
    /// Per-layer mute toggles (runtime only, never written to the animator)
    muted_layers: Vec<bool>,
    /// `--sequence` playback (timeline, seek snapshot)
    sequence: Option<SequenceUi>,

    // egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_ui: bool,

    // FPS tracking
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_fps_update: Instant,
}

impl PreviewApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let has_model = self.state.lock().map(|s| s.has_model).unwrap_or(false);
        let model_name = self
            .state
            .lock()
            .ok()
            .and_then(|s| {
                s.model_path
                    .as_ref()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            })
            .unwrap_or_default();

        let title = if has_model {
            format!("Flint Preview \u{2014} {}", model_name)
        } else {
            "Flint Preview \u{2014} Drop a .glb/.gltf file".to_string()
        };

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&title)
                        .with_inner_size(PhysicalSize::new(self.args.width, self.args.height)),
                )
                .expect("Failed to create window"),
        );

        let context = pollster::block_on(flint_render::RenderContext::new(window.clone()))
            .expect("Failed to create render context");

        self.camera.aspect = context.aspect_ratio();

        let mut renderer = SceneRenderer::new(
            &context,
            RendererConfig {
                show_grid: !self.args.no_grid,
                ..Default::default()
            },
        );

        // Load model if we have one
        if has_model {
            let state = self.state.lock().unwrap();
            let maybe_path = state.model_path.clone();
            let asset_name = state.asset_name.clone();
            let entity_id = state.entity_id;
            drop(state);

            if let Some(model_path) = maybe_path {
                let config = model_load_config(&model_path, &asset_name);
                let mut state = self.state.lock().unwrap();
                let load_result = model_loader::load_models_from_world(
                    &mut state.world,
                    &mut renderer,
                    &context.device,
                    &context.queue,
                    &config,
                );

                // Register animation data
                if !self.args.no_animate {
                    if let Some(eid) = entity_id {
                        let (info, skel_assets) = register_animation_data(
                            &load_result,
                            &mut self.animation,
                            &mut state.world,
                            eid,
                            self.args.clip.as_deref(),
                            self.args.anim_speed,
                            &self.args.layers,
                        );
                        self.anim_info = info;
                        self.skeletal_entity_assets = skel_assets;
                        if let Some(p) = self.args.sequence.clone() {
                            self.sequence = attach_sequence(
                                &p,
                                &mut self.animation,
                                &state.world,
                                eid,
                                self.args.sequence_loop,
                            );
                        }
                    }
                }
            }

            // Auto-fit camera
            if let Some(b) = &self.initial_bounds {
                auto_fit_camera(b, &mut self.camera);
            }
            apply_cli_overrides(&mut self.camera, &self.args);
            self.camera.update_orbit();
        }

        // Initialize egui
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&context.device, context.config.format, None, 1, false);
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        self.window = Some(window);
        self.render_context = Some(context);
        self.scene_renderer = Some(renderer);

        if let Some(info) = &self.anim_info {
            println!(
                "Animation: {} clip(s) found — {}",
                info.clip_names.len(),
                info.clip_names.join(", ")
            );
            println!("  Tab=toggle UI  P=play/pause  ,/.=prev/next clip  +/-=speed  0=reset speed");
            if self.args.sequence.is_some() {
                println!("  R=restart sequence  Home=seek to 0  (Loop checkbox / --sequence-loop)");
            }
        }
    }

    fn setup_animation_after_load(&mut self, load_result: &ModelLoadResult) {
        self.animation.clear();
        self.skeletal_entity_assets.clear();
        self.anim_info = None;
        self.anim_paused = false;
        self.anim_time_accumulator = 0.0;
        self.solo_layer = None;
        self.muted_layers.clear();
        self.sequence = None;

        if self.args.no_animate {
            return;
        }

        let mut state = self.state.lock().unwrap();
        if let Some(eid) = state.entity_id {
            let (info, skel_assets) = register_animation_data(
                load_result,
                &mut self.animation,
                &mut state.world,
                eid,
                None, // No clip preference on reload/drop
                self.args.anim_speed,
                &self.args.layers,
            );
            self.anim_info = info;
            self.skeletal_entity_assets = skel_assets;
            self.sequence = self.args.sequence.clone().and_then(|p| {
                attach_sequence(
                    &p,
                    &mut self.animation,
                    &state.world,
                    eid,
                    self.args.sequence_loop,
                )
            });
        }
    }

    fn load_model_file(&mut self, path: PathBuf) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext != "glb" && ext != "gltf" {
            tracing::error!("Unsupported file type: .{} (expected .glb or .gltf)", ext);
            return;
        }

        // Import to get bounds and cache for UI
        let import = match import_gltf(&path) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to import model: {:?}", e);
                return;
            }
        };
        let bounds = import.bounds();
        let stats = ModelStats::from_import(&import);

        let (world, asset_name, entity_id) = create_model_world(&path, self.args.anim_speed);

        // Clear and reload
        let load_result;
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.clear_model_data();

            if let Some(context) = &self.render_context {
                let config = model_load_config(&path, &asset_name);
                let mut state = self.state.lock().unwrap();
                state.world = world;
                state.asset_name = asset_name;
                state.has_model = true;
                state.model_path = Some(path.clone());
                state.entity_id = Some(entity_id);
                state.model_stats = Some(stats);
                state.import_result = Some(import);

                load_result = Some(model_loader::load_models_from_world(
                    &mut state.world,
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                ));
            } else {
                load_result = None;
            }
        } else {
            load_result = None;
        }

        // Register animation data
        if let Some(lr) = &load_result {
            self.setup_animation_after_load(lr);
        }

        // Auto-fit camera
        if let Some(b) = &bounds {
            auto_fit_camera(b, &mut self.camera);
        }
        self.camera.update_orbit();

        // Update window title
        if let Some(window) = &self.window {
            let file_name = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            window.set_title(&format!("Flint Preview \u{2014} {}", file_name));
        }

        println!("Loaded: {}", path.display());
        if let Some(info) = &self.anim_info {
            println!(
                "Animation: {} clip(s) — {}",
                info.clip_names.len(),
                info.clip_names.join(", ")
            );
        }
    }

    fn check_reload(&mut self) {
        let needs_reload = self.state.lock().map(|s| s.needs_reload).unwrap_or(false);

        if !needs_reload {
            return;
        }

        // Phase 1: Import model, create world, load into renderer (holds state lock)
        let (load_result, bounds, model_display) = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.needs_reload = false;
            let Some(model_path) = state.model_path.clone() else {
                return;
            };

            let import = match import_gltf(&model_path) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Reload failed: {:?}", e);
                    return;
                }
            };
            let bounds = import.bounds();
            state.model_stats = Some(ModelStats::from_import(&import));
            state.import_result = Some(import);

            let (world, asset_name, entity_id) =
                create_model_world(&model_path, self.args.anim_speed);
            state.world = world;
            state.asset_name = asset_name.clone();
            state.entity_id = Some(entity_id);

            let load_result = if let (Some(renderer), Some(context)) =
                (&mut self.scene_renderer, &self.render_context)
            {
                renderer.clear_model_data();
                let config = model_load_config(&model_path, &asset_name);
                Some(model_loader::load_models_from_world(
                    &mut state.world,
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                ))
            } else {
                None
            };

            let display = model_path.display().to_string();
            (load_result, bounds, display)
        };
        // State lock is now dropped

        // Phase 2: Register animation data (acquires state lock internally)
        if let Some(lr) = &load_result {
            self.setup_animation_after_load(lr);
        }

        if let Some(b) = &bounds {
            auto_fit_camera(b, &mut self.camera);
        }
        self.camera.update_orbit();

        println!("Reloaded: {}", model_display);
    }

    /// Switch to a different animation clip by index
    fn switch_clip(&mut self, new_index: usize) {
        let info = match &mut self.anim_info {
            Some(i) => i,
            None => return,
        };
        if new_index >= info.clip_names.len() {
            return;
        }

        info.current_clip_index = new_index;
        let clip_name = info.clip_names[new_index].clone();

        // Reset accumulated time
        self.anim_time_accumulator = 0.0;

        // Update animator component in ECS so sync_from_world picks up the change
        if let Ok(mut state) = self.state.lock() {
            if let Some(eid) = state.entity_id {
                if let Some(components) = state.world.get_components_mut(eid) {
                    components.set_field(
                        comp::ANIMATOR,
                        "clip",
                        toml::Value::String(clip_name.clone()),
                    );
                    components.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
                }

                // Reset skeletal playback state so it re-syncs with the new clip
                self.animation.reset_skeletal_state(&eid);
            }
        }

        println!("Switched to clip: {}", clip_name);
    }

    /// Update the window title with animation playback info
    fn update_title(&self) {
        let window = match &self.window {
            Some(w) => w,
            None => return,
        };

        let model_name = self
            .state
            .lock()
            .ok()
            .and_then(|s| {
                s.model_path
                    .as_ref()
                    .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            })
            .unwrap_or_default();

        if let Some(info) = &self.anim_info {
            let clip_name = &info.clip_names[info.current_clip_index];
            let status = if self.anim_paused {
                "\u{23f8}"
            } else {
                "\u{25b6}"
            };
            let speed = self
                .state
                .lock()
                .ok()
                .and_then(|s| {
                    s.entity_id.and_then(|eid| {
                        s.world.get_components(eid).and_then(|c| {
                            c.get(comp::ANIMATOR).and_then(|a| {
                                a.get("speed").and_then(|v| {
                                    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
                                })
                            })
                        })
                    })
                })
                .unwrap_or(1.0);

            let mut title = format!(
                "Flint Preview \u{2014} {} | {} {} ({}/{}) [{:.1}x]",
                model_name,
                status,
                clip_name,
                info.current_clip_index + 1,
                info.clip_names.len(),
                speed,
            );
            if let Some(seq) = &self.sequence {
                let t = self
                    .state
                    .lock()
                    .ok()
                    .and_then(|s| s.entity_id)
                    .and_then(|eid| self.animation.sequence_state(&eid))
                    .map(|rt| rt.time)
                    .unwrap_or(0.0);
                title.push_str(&format!(
                    " | seq {} {:.1}/{:.1}s",
                    seq.name, t, seq.duration
                ));
            }
            window.set_title(&title);
        } else {
            let title = if model_name.is_empty() {
                "Flint Preview \u{2014} Drop a .glb/.gltf file".to_string()
            } else {
                format!("Flint Preview \u{2014} {}", model_name)
            };
            window.set_title(&title);
        }
    }

    /// Seek the `--sequence` to `t` by replay, then re-pose.
    fn seek_sequence_to(&mut self, t: f64) {
        let Some(seq) = self.sequence.clone() else {
            return;
        };
        if let Ok(mut state) = self.state.lock() {
            if let Some(eid) = state.entity_id {
                seek_sequence_in_world(&seq, &mut self.animation, &mut state.world, eid, t);
            }
        }
        self.recompute_pose_now();
    }

    /// Push solo/mute flags into the animation system as runtime mutes.
    fn apply_layer_mutes(&mut self) {
        let Some(eid) = self.state.lock().ok().and_then(|s| s.entity_id) else {
            return;
        };
        let layer_count = self
            .animation
            .skeletal_layers(&eid)
            .map(|l| l.len())
            .unwrap_or(0)
            .max(self.muted_layers.len());
        self.animation.clear_skeletal_layer_mutes(&eid);
        for li in 0..layer_count {
            let muted = self.muted_layers.get(li).copied().unwrap_or(false)
                || self.solo_layer.is_some_and(|s| s != li);
            if muted {
                self.animation.set_skeletal_layer_mute(eid, li, true);
            }
        }
    }

    /// Re-pose the skeleton at the current time (dt = 0), upload bone
    /// matrices and rebuild the overlay. Lets layer dials and scrubbing
    /// take effect immediately while playback is paused.
    fn recompute_pose_now(&mut self) {
        if let Ok(state) = self.state.lock() {
            self.animation.sync_skeletal_from_world(&state.world);
        }
        self.apply_layer_mutes();
        self.animation.advance_skeletal(0.0);
        if let Ok(mut state) = self.state.lock() {
            self.animation.write_back_skeletal(&mut state.world);
        }
        self.upload_bone_matrices();
        self.update_skeleton_overlay_from_model();
    }

    /// Upload every skinned entity's bone matrices to the renderer.
    fn upload_bone_matrices(&mut self) {
        if let (Some(renderer), Some(ctx)) = (&mut self.scene_renderer, &self.render_context) {
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.animation.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(
                        &ctx.device,
                        &ctx.queue,
                        *entity_id,
                        asset_name,
                        matrices,
                    );
                }
            }
        }
    }

    /// Update skeleton overlay for imported glTF models.
    ///
    /// Extracts rest-pose bone positions from inverse bind matrices.
    fn update_skeleton_overlay_from_model(&mut self) {
        let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer)
        else {
            return;
        };

        if !renderer.debug_state().show_skeleton {
            renderer.clear_skeleton_overlay();
            return;
        }

        // Prefer the live animated pose when the animation system owns a skeleton
        // for this entity; fall back to the rest pose baked into the import.
        let entity_id = self.state.lock().ok().and_then(|s| s.entity_id);
        if let Some(eid) = entity_id {
            if let Some(skel) = self.animation.skeleton(&eid) {
                let contrib = self.animation.skeletal_layer_contribution(&eid);
                let mesh = skeleton_overlay_mesh(skel, self.layer_viz, contrib);
                renderer.set_skeleton_overlay(&context.device, &mesh);
                return;
            }
        }

        let state_guard = self.state.lock().ok();
        let skeleton = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref().and_then(|ir| ir.skeletons.first()));

        let Some(skeleton) = skeleton else {
            renderer.clear_skeleton_overlay();
            return;
        };

        // Extract rest-pose world positions from inverse bind matrices.
        // world_pos = inverse(inverse_bind_matrix) translation column.
        let positions: Vec<[f32; 3]> = skeleton
            .joints
            .iter()
            .map(|j| {
                let ibm = &j.inverse_bind_matrix;
                // Invert the 4x4 matrix to get world transform, extract translation
                let world = invert_4x4(ibm);
                [world[3][0], world[3][1], world[3][2]]
            })
            .collect();

        let parents: Vec<Option<usize>> = skeleton.joints.iter().map(|j| j.parent).collect();
        let mesh = flint_render::generate_skeleton_lines(&positions, &parents);
        renderer.set_skeleton_overlay(&context.device, &mesh);
    }

    // -----------------------------------------------------------------------
    // egui overlay rendering
    // -----------------------------------------------------------------------

    fn render_egui(&mut self, target_view: &wgpu::TextureView) {
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return,
        };
        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };
        let egui_winit = match &mut self.egui_winit {
            Some(e) => e,
            None => return,
        };

        let raw_input = egui_winit.take_egui_input(&window);

        // Snapshot renderer state before closure (avoid borrow conflicts)
        let current_debug_mode = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().mode)
            .unwrap_or_default();
        let current_wireframe = current_debug_mode == DebugMode::WireframeOverlay;
        let current_normals = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().show_normals)
            .unwrap_or(false);
        let current_skeleton = self
            .scene_renderer
            .as_ref()
            .map(|r| r.debug_state().show_skeleton)
            .unwrap_or(false);
        let current_grid = self
            .scene_renderer
            .as_ref()
            .map(|r| r.show_grid())
            .unwrap_or(false);

        // Snapshot camera info
        let cam_distance = self.camera.distance;
        let cam_yaw_deg = self.camera.yaw.to_degrees();
        let cam_pitch_deg = self.camera.pitch.to_degrees();
        let fps = self.fps;

        // Snapshot animation state
        let anim_paused = self.anim_paused;
        let anim_clip_names: Vec<String> = self
            .anim_info
            .as_ref()
            .map(|i| i.clip_names.clone())
            .unwrap_or_default();
        let anim_clip_index = self
            .anim_info
            .as_ref()
            .map(|i| i.current_clip_index)
            .unwrap_or(0);
        let has_anim = self.anim_info.is_some();

        // Snapshot layer state
        let skeletal_clip_names: Vec<String> = self
            .anim_info
            .as_ref()
            .map(|i| i.skeletal_clip_names.clone())
            .unwrap_or_default();
        let joint_names: Vec<String> = self
            .anim_info
            .as_ref()
            .map(|i| i.joint_names.clone())
            .unwrap_or_default();
        let preview_eid = self.state.lock().ok().and_then(|s| s.entity_id);
        let layers: Vec<AnimLayer> = preview_eid
            .and_then(|eid| self.animation.skeletal_layers(&eid))
            .map(|l| l.to_vec())
            .unwrap_or_default();
        let contrib: Option<LayerContribution> = preview_eid
            .and_then(|eid| self.animation.skeletal_layer_contribution(&eid))
            .cloned();
        let layer_viz = self.layer_viz;
        let solo_layer = self.solo_layer;
        let muted_layers = self.muted_layers.clone();
        let has_skeleton = !joint_names.is_empty();
        // In-flight weight ramps per layer: (target, seconds left)
        let layer_fades: Vec<Option<(f32, f32)>> = (0..layers.len())
            .map(|li| preview_eid.and_then(|eid| self.animation.skeletal_layer_fade(&eid, li)))
            .collect();
        // Sequence snapshot: (ui, time, playing, looping, fired flags)
        let seq_snapshot: Option<(SequenceUi, f64, bool, bool, Vec<bool>)> =
            self.sequence.as_ref().and_then(|seq| {
                let rt = preview_eid.and_then(|eid| self.animation.sequence_state(&eid))?;
                let fired = (0..seq.markers.len()).map(|i| rt.fired(i)).collect();
                Some((seq.clone(), rt.time, rt.playing, rt.looping(), fired))
            });
        // Joint name -> (colour, tooltip) for the node tree
        let joint_annot: HashMap<String, (egui::Color32, String)> = if layer_viz != LayerViz::Off {
            joint_names
                .iter()
                .enumerate()
                .map(|(j, name)| {
                    (
                        name.clone(),
                        (
                            to_color32(joint_viz_color(layer_viz, j, contrib.as_ref())),
                            joint_viz_tooltip(j, contrib.as_ref(), &layers),
                        ),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Get playback time and clip duration from animation system
        let (anim_time, anim_duration, anim_speed) = if has_anim {
            let state_guard = self.state.lock().ok();
            let entity_id = state_guard.as_ref().and_then(|s| s.entity_id);

            let mut time = 0.0f64;
            let mut duration = 0.0f64;
            let mut speed = 1.0f64;

            if let Some(eid) = entity_id {
                // Try skeletal sync first
                if let Some(ps) = self.animation.skeletal_playback_state(&eid) {
                    time = ps.time;
                    speed = ps.speed;
                }
                // Try node sync
                if let Some(ps) = self.animation.node_playback_state(&eid) {
                    time = ps.time;
                    speed = ps.speed;
                }
                // Get duration from current clip name
                if !anim_clip_names.is_empty() {
                    let clip_name = &anim_clip_names[anim_clip_index];
                    if let Some(d) = self.animation.skeletal_clip_duration(clip_name) {
                        duration = d;
                    }
                    if let Some(d) = self.animation.node_clip_duration(clip_name) {
                        duration = d;
                    }
                }
            }
            (time, duration, speed)
        } else {
            (0.0, 0.0, 1.0)
        };

        // Read model stats and node/material info from state
        let state_guard = self.state.lock().ok();
        let model_stats_snapshot = state_guard
            .as_ref()
            .and_then(|s| s.model_stats.as_ref())
            .map(|ms| {
                (
                    ms.total_vertices,
                    ms.total_triangles,
                    ms.mesh_count,
                    ms.material_count,
                    ms.node_count,
                    ms.skeleton_joint_count,
                    ms.bounds,
                )
            });
        // Snapshot node tree data
        let node_data: Vec<(String, Vec<usize>, bool)> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| {
                ir.nodes
                    .iter()
                    .map(|n| {
                        (
                            n.name.clone(),
                            n.children.clone(),
                            !n.mesh_primitive_indices.is_empty(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let root_nodes: Vec<usize> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| ir.root_nodes.clone())
            .unwrap_or_default();
        // Snapshot material data
        let material_data: Vec<(String, [f32; 4], f32, f32)> = state_guard
            .as_ref()
            .and_then(|s| s.import_result.as_ref())
            .map(|ir| {
                ir.materials
                    .iter()
                    .map(|m| (m.name.clone(), m.base_color, m.metallic, m.roughness))
                    .collect()
            })
            .unwrap_or_default();
        let has_model = state_guard.as_ref().map(|s| s.has_model).unwrap_or(false);
        drop(state_guard);

        // Mutation requests collected from UI interactions
        let mut new_debug_mode: Option<DebugMode> = None;
        let mut new_wireframe: Option<bool> = None;
        let mut new_normals: Option<bool> = None;
        let mut new_skeleton: Option<bool> = None;
        let mut new_grid: Option<bool> = None;
        let mut new_anim_paused: Option<bool> = None;
        let mut new_clip_index: Option<usize> = None;
        let mut new_speed: Option<f64> = None;
        let mut scrub_time: Option<f64> = None;
        let mut new_layers: Option<Vec<AnimLayer>> = None;
        let mut new_solo: Option<Option<usize>> = None;
        let mut new_mutes: Option<Vec<bool>> = None;
        let mut new_layer_viz: Option<LayerViz> = None;
        let mut seq_seek: Option<f64> = None;
        let mut seq_restart = false;
        let mut seq_loop: Option<bool> = None;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // ── Left panel: model info ──
            egui::SidePanel::left("preview_info_panel")
                .default_width(220.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading("Model Inspector");
                    ui.separator();

                    if !has_model {
                        ui.label("No model loaded");
                        ui.label("Drop a .glb/.gltf file to preview");
                        return;
                    }

                    // ── Statistics ──
                    egui::CollapsingHeader::new("Statistics")
                        .default_open(true)
                        .show(ui, |ui| {
                            if let Some((verts, tris, meshes, mats, nodes, joints, bounds)) =
                                model_stats_snapshot
                            {
                                egui::Grid::new("stats_grid")
                                    .num_columns(2)
                                    .spacing([8.0, 2.0])
                                    .show(ui, |ui| {
                                        ui.label("Meshes:");
                                        ui.label(format!("{}", meshes));
                                        ui.end_row();
                                        ui.label("Vertices:");
                                        ui.label(format!("{}", verts));
                                        ui.end_row();
                                        ui.label("Triangles:");
                                        ui.label(format!("{}", tris));
                                        ui.end_row();
                                        ui.label("Materials:");
                                        ui.label(format!("{}", mats));
                                        ui.end_row();
                                        ui.label("Nodes:");
                                        ui.label(format!("{}", nodes));
                                        ui.end_row();
                                        if joints > 0 {
                                            ui.label("Joints:");
                                            ui.label(format!("{}", joints));
                                            ui.end_row();
                                        }
                                    });

                                if let Some(b) = bounds {
                                    let size = b.size();
                                    ui.add_space(4.0);
                                    ui.label(format!(
                                        "Size: {:.2} x {:.2} x {:.2}",
                                        size[0], size[1], size[2]
                                    ));
                                }
                            }
                        });

                    ui.add_space(4.0);

                    // ── Node Hierarchy ──
                    if !node_data.is_empty() {
                        egui::CollapsingHeader::new("Node Hierarchy")
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::ScrollArea::both()
                                    .max_height(200.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        for &root_idx in &root_nodes {
                                            render_node_tree(ui, &node_data, root_idx, 0, &joint_annot);
                                        }
                                    });
                            });

                        ui.add_space(4.0);
                    }

                    // ── Materials ──
                    if !material_data.is_empty() {
                        egui::CollapsingHeader::new("Materials")
                            .default_open(false)
                            .show(ui, |ui| {
                                for (i, (name, color, metallic, roughness)) in
                                    material_data.iter().enumerate()
                                {
                                    let display_name = if name.is_empty() {
                                        format!("Material {}", i)
                                    } else {
                                        name.clone()
                                    };
                                    egui::CollapsingHeader::new(&display_name)
                                        .id_salt(format!("mat_{}", i))
                                        .default_open(false)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let swatch = egui::color_picker::show_color(
                                                    ui,
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        (color[0] * 255.0) as u8,
                                                        (color[1] * 255.0) as u8,
                                                        (color[2] * 255.0) as u8,
                                                        (color[3] * 255.0) as u8,
                                                    ),
                                                    egui::vec2(16.0, 16.0),
                                                );
                                                let _ = swatch;
                                                ui.label(format!(
                                                    "RGBA: ({:.2}, {:.2}, {:.2}, {:.2})",
                                                    color[0], color[1], color[2], color[3]
                                                ));
                                            });
                                            ui.label(format!("Metallic: {:.2}", metallic));
                                            ui.label(format!("Roughness: {:.2}", roughness));
                                        });
                                }
                            });

                        ui.add_space(4.0);
                    }

                    // ── View Controls ──
                    egui::CollapsingHeader::new("View")
                        .default_open(true)
                        .show(ui, |ui| {
                            // Debug mode combo
                            let modes = [
                                DebugMode::Pbr,
                                DebugMode::WireframeOverlay,
                                DebugMode::WireframeOnly,
                                DebugMode::Normals,
                                DebugMode::Depth,
                                DebugMode::UvChecker,
                                DebugMode::Unlit,
                                DebugMode::MetallicRoughness,
                            ];
                            let mut selected = current_debug_mode;
                            egui::ComboBox::from_label("Shading")
                                .selected_text(selected.label())
                                .show_ui(ui, |ui| {
                                    for &mode in &modes {
                                        ui.selectable_value(&mut selected, mode, mode.label());
                                    }
                                });
                            if selected != current_debug_mode {
                                new_debug_mode = Some(selected);
                            }

                            // Wireframe overlay
                            let mut wf = current_wireframe;
                            ui.checkbox(&mut wf, "Wireframe overlay");
                            if wf != current_wireframe {
                                new_wireframe = Some(wf);
                            }

                            // Normal arrows
                            let mut na = current_normals;
                            ui.checkbox(&mut na, "Show normals");
                            if na != current_normals {
                                new_normals = Some(na);
                            }

                            // Skeleton overlay
                            let mut sk = current_skeleton;
                            ui.checkbox(&mut sk, "Show skeleton");
                            if sk != current_skeleton {
                                new_skeleton = Some(sk);
                            }

                            // Skeleton colouring by layer activity
                            if has_skeleton {
                                let mut viz_modes = vec![LayerViz::Off, LayerViz::LastWriter];
                                for li in 0..layers.len() {
                                    viz_modes.push(LayerViz::Weight(li));
                                    viz_modes.push(LayerViz::Mask(li));
                                    viz_modes.push(LayerViz::Keyed(li));
                                }
                                let mut sel = layer_viz;
                                egui::ComboBox::from_label("Skeleton colour")
                                    .selected_text(sel.label())
                                    .show_ui(ui, |ui| {
                                        for &m in &viz_modes {
                                            ui.selectable_value(&mut sel, m, m.label());
                                        }
                                    });
                                if sel != layer_viz {
                                    new_layer_viz = Some(sel);
                                }
                                if layer_viz != LayerViz::Off {
                                    ui.horizontal_wrapped(|ui| {
                                        let swatch = |ui: &mut egui::Ui, c: [f32; 4], label: &str| {
                                            ui.colored_label(to_color32(c), "\u{25a0}");
                                            ui.label(label);
                                        };
                                        match layer_viz {
                                            LayerViz::LastWriter => {
                                                swatch(ui, BASE_COLOR, "base");
                                                for (li, l) in layers.iter().enumerate() {
                                                    if l.is_active() {
                                                        swatch(ui, layer_color(li), &format!("L{}", li + 1));
                                                    }
                                                }
                                                swatch(ui, REST_COLOR, "rest");
                                            }
                                            LayerViz::Weight(li) => {
                                                swatch(ui, REST_COLOR, "0");
                                                swatch(ui, layer_color(li), "1");
                                            }
                                            LayerViz::Mask(_) => {
                                                swatch(ui, [0.3, 1.0, 0.3, 1.0], "in mask");
                                                swatch(ui, REST_COLOR, "outside");
                                            }
                                            LayerViz::Keyed(_) => {
                                                swatch(ui, [0.2, 1.0, 1.0, 1.0], "keyed");
                                                swatch(ui, REST_COLOR, "not keyed");
                                            }
                                            LayerViz::Off => {}
                                        }
                                    });
                                }
                            }

                            // Grid
                            let mut gr = current_grid;
                            ui.checkbox(&mut gr, "Show grid");
                            if gr != current_grid {
                                new_grid = Some(gr);
                            }
                        });
                });

            // ── Bottom panel: animation timeline ──
            if has_anim {
                egui::TopBottomPanel::bottom("animation_panel")
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.add_space(4.0);

                        // Top row: play/pause, clip selector, speed
                        ui.horizontal(|ui| {
                            // Play/pause button
                            let btn_text = if anim_paused { "\u{25b6}" } else { "\u{23f8}" };
                            if ui.button(btn_text).clicked() {
                                new_anim_paused = Some(!anim_paused);
                            }

                            // Clip selector
                            if anim_clip_names.len() > 1 {
                                let current_name = &anim_clip_names[anim_clip_index];
                                let mut sel_idx = anim_clip_index;
                                egui::ComboBox::from_id_salt("clip_selector")
                                    .selected_text(current_name)
                                    .show_ui(ui, |ui| {
                                        for (i, name) in anim_clip_names.iter().enumerate() {
                                            ui.selectable_value(&mut sel_idx, i, name);
                                        }
                                    });
                                if sel_idx != anim_clip_index {
                                    new_clip_index = Some(sel_idx);
                                }
                            } else if !anim_clip_names.is_empty() {
                                ui.label(&anim_clip_names[0]);
                            }

                            ui.separator();

                            // Speed control
                            let mut spd = anim_speed;
                            ui.label("Speed:");
                            let drag = egui::DragValue::new(&mut spd)
                                .range(0.1..=10.0)
                                .speed(0.05)
                                .suffix("x");
                            if ui.add(drag).changed() {
                                new_speed = Some(spd);
                            }
                            if ui.small_button("1x").clicked() {
                                new_speed = Some(1.0);
                            }
                        });

                        // Timeline slider
                        if anim_duration > 0.0 {
                            ui.horizontal(|ui| {
                                ui.monospace(format!("{:.2}s", anim_time));

                                let mut t = anim_time;
                                let slider = egui::Slider::new(&mut t, 0.0..=anim_duration)
                                    .show_value(false)
                                    .trailing_fill(true);
                                let response = ui.add(slider);
                                if response.dragged() || response.changed() {
                                    scrub_time = Some(t);
                                    if !anim_paused {
                                        new_anim_paused = Some(true);
                                    }
                                }

                                ui.monospace(format!("{:.2}s", anim_duration));
                            });

                            // Frame counter
                            let frame = (anim_time * 30.0) as u32;
                            let total_frames = (anim_duration * 30.0) as u32;
                            ui.label(format!("Frame: {} / {}", frame, total_frames));
                        }

                        // ── Sequence ──
                        if let Some((seq, seq_time, seq_playing, seq_looping, fired)) =
                            &seq_snapshot
                        {
                            let header = format!("Sequence: {}", seq.name);
                            egui::CollapsingHeader::new(header)
                                .id_salt("anim_sequence")
                                .default_open(true)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let btn = if anim_paused || !seq_playing {
                                            "\u{25b6}"
                                        } else {
                                            "\u{23f8}"
                                        };
                                        if ui.button(btn).on_hover_text("Play / pause (P)").clicked() {
                                            if !seq_playing && anim_paused {
                                                // Finished: restart and play
                                                seq_restart = true;
                                            }
                                            new_anim_paused = Some(!anim_paused);
                                        }
                                        if ui.button("\u{23ee} Restart").on_hover_text("R").clicked() {
                                            seq_restart = true;
                                        }
                                        ui.monospace(format!("{:6.2}s / {:.2}s", seq_time, seq.duration));
                                        let mut lp = *seq_looping;
                                        if ui.checkbox(&mut lp, "Loop").changed() {
                                            seq_loop = Some(lp);
                                        }
                                        if !seq_playing {
                                            ui.weak("(finished)");
                                        }
                                    });

                                    // Seek slider — seeking replays from 0, so pause
                                    if seq.duration > 0.0 {
                                        let mut t = *seq_time;
                                        let slider = egui::Slider::new(&mut t, 0.0..=seq.duration)
                                            .show_value(false)
                                            .trailing_fill(true);
                                        let resp = ui.add_sized(
                                            [ui.available_width(), 18.0],
                                            slider,
                                        );
                                        if resp.dragged() || resp.changed() {
                                            seq_seek = Some(t);
                                            if !anim_paused {
                                                new_anim_paused = Some(true);
                                            }
                                        }

                                        // Event markers under the slider
                                        let strip_h = 30.0;
                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), strip_h),
                                            egui::Sense::click(),
                                        );
                                        let painter = ui.painter_at(rect);
                                        let pad = 8.0;
                                        let x_of = |t: f64| {
                                            rect.left()
                                                + pad
                                                + (t / seq.duration) as f32 * (rect.width() - 2.0 * pad)
                                        };
                                        painter.line_segment(
                                            [
                                                egui::pos2(rect.left() + pad, rect.top() + 1.0),
                                                egui::pos2(rect.right() - pad, rect.top() + 1.0),
                                            ],
                                            egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
                                        );
                                        let hover = resp.hover_pos();
                                        let mut hover_label: Option<String> = None;
                                        let mut hover_best = f32::MAX;
                                        for (i, m) in seq.markers.iter().enumerate() {
                                            let x = x_of(m.time);
                                            let mut col = marker_color(m.kind, m.layer);
                                            if !fired.get(i).copied().unwrap_or(false) {
                                                col = col.gamma_multiply(0.45);
                                            }
                                            painter.line_segment(
                                                [
                                                    egui::pos2(x, rect.top()),
                                                    egui::pos2(x, rect.top() + 10.0),
                                                ],
                                                egui::Stroke::new(2.0, col),
                                            );
                                            // Alternate label rows so neighbours don't collide
                                            let row = (i % 2) as f32;
                                            let short: String = m.label.chars().take(14).collect();
                                            painter.text(
                                                egui::pos2(x + 2.0, rect.top() + 10.0 + row * 9.0),
                                                egui::Align2::LEFT_TOP,
                                                short,
                                                egui::FontId::monospace(8.5),
                                                col,
                                            );
                                            if let Some(h) = hover {
                                                let d = (h.x - x).abs();
                                                if d < 8.0 && d < hover_best {
                                                    hover_best = d;
                                                    hover_label = Some(format!("{:.2}s  {}", m.time, m.label));
                                                }
                                            }
                                        }
                                        // Playhead
                                        let px = x_of(*seq_time);
                                        painter.line_segment(
                                            [
                                                egui::pos2(px, rect.top() - 2.0),
                                                egui::pos2(px, rect.bottom()),
                                            ],
                                            egui::Stroke::new(1.0, egui::Color32::WHITE),
                                        );
                                        if let Some(label) = hover_label {
                                            resp.clone().on_hover_text(label);
                                        }
                                        if resp.clicked() {
                                            if let Some(h) = hover {
                                                let frac = ((h.x - rect.left() - pad)
                                                    / (rect.width() - 2.0 * pad))
                                                    .clamp(0.0, 1.0);
                                                seq_seek = Some(frac as f64 * seq.duration);
                                                if !anim_paused {
                                                    new_anim_paused = Some(true);
                                                }
                                            }
                                        }
                                    }

                                    // Now / next readout
                                    let last = fired.iter().rposition(|f| *f);
                                    let next = fired.iter().position(|f| !*f);
                                    ui.horizontal(|ui| {
                                        match last.and_then(|i| seq.markers.get(i)) {
                                            Some(m) => ui.label(format!("Now: {}", m.label)),
                                            None => ui.weak("Now: (start)"),
                                        };
                                        ui.separator();
                                        match next.and_then(|i| seq.markers.get(i)) {
                                            Some(m) => ui.label(format!(
                                                "Next: {} in {:.2}s",
                                                m.label,
                                                (m.time - seq_time).max(0.0)
                                            )),
                                            None => ui.weak("Next: (end)"),
                                        };
                                    });
                                });
                        }

                        // ── Layer stack ──
                        if has_skeleton {
                            let active = layers.iter().filter(|l| l.is_active()).count();
                            let header = if active > 0 {
                                format!("Layers ({} active)", active)
                            } else {
                                "Layers".to_string()
                            };
                            egui::CollapsingHeader::new(header)
                                .id_salt("anim_layers")
                                .default_open(!layers.is_empty())
                                .show(ui, |ui| {
                                    let mut edited = layers.clone();
                                    let mut mutes = muted_layers.clone();
                                    mutes.resize(edited.len(), false);
                                    let mut solo = solo_layer;
                                    let mut changed = false;
                                    let mut remove: Option<usize> = None;

                                    egui::Grid::new("layer_grid")
                                        .num_columns(8)
                                        .spacing([6.0, 4.0])
                                        .show(ui, |ui| {
                                            for (li, layer) in edited.iter_mut().enumerate() {
                                                // Swatch + index
                                                ui.horizontal(|ui| {
                                                    ui.colored_label(to_color32(layer_color(li)), "\u{25a0}");
                                                    ui.label(format!("L{}", li + 1));
                                                });

                                                // Clip
                                                let shown = if layer.clip.is_empty() {
                                                    "(none)".to_string()
                                                } else {
                                                    layer.clip.clone()
                                                };
                                                egui::ComboBox::from_id_salt(("layer_clip", li))
                                                    .selected_text(shown)
                                                    .width(140.0)
                                                    .show_ui(ui, |ui| {
                                                        if ui
                                                            .selectable_label(layer.clip.is_empty(), "(none)")
                                                            .clicked()
                                                        {
                                                            layer.clip.clear();
                                                            changed = true;
                                                        }
                                                        for name in &skeletal_clip_names {
                                                            if ui
                                                                .selectable_label(&layer.clip == name, name)
                                                                .clicked()
                                                            {
                                                                layer.clip = name.clone();
                                                                changed = true;
                                                            }
                                                        }
                                                    });

                                                // Weight
                                                let mut w = layer.weight;
                                                if ui
                                                    .add(
                                                        egui::Slider::new(&mut w, 0.0..=1.0)
                                                            .show_value(true)
                                                            .fixed_decimals(2),
                                                    )
                                                    .changed()
                                                {
                                                    layer.weight = w;
                                                    changed = true;
                                                }
                                                if let Some(Some((target, left))) = layer_fades.get(li) {
                                                    ui.weak(format!("\u{2192} {:.2} ({:.1}s)", target, left))
                                                        .on_hover_text("Weight ramp in flight (fade_target / seconds left)");
                                                }

                                                // Mode
                                                let mut mode = layer.mode;
                                                ui.horizontal(|ui| {
                                                    ui.selectable_value(&mut mode, LayerMode::Additive, "Add")
                                                        .on_hover_text("Additive: delta from rest pose × weight");
                                                    ui.selectable_value(&mut mode, LayerMode::Override, "Over")
                                                        .on_hover_text("Override: blend keyed joints toward the clip by weight");
                                                });
                                                if mode != layer.mode {
                                                    layer.mode = mode;
                                                    changed = true;
                                                }

                                                // Mask
                                                let mask_shown = if layer.mask.is_empty() {
                                                    "(all keyed)".to_string()
                                                } else {
                                                    layer.mask.clone()
                                                };
                                                egui::ComboBox::from_id_salt(("layer_mask", li))
                                                    .selected_text(mask_shown)
                                                    .width(120.0)
                                                    .show_ui(ui, |ui| {
                                                        if ui
                                                            .selectable_label(layer.mask.is_empty(), "(all keyed)")
                                                            .clicked()
                                                        {
                                                            layer.mask.clear();
                                                            changed = true;
                                                        }
                                                        for name in &joint_names {
                                                            if ui
                                                                .selectable_label(&layer.mask == name, name)
                                                                .clicked()
                                                            {
                                                                layer.mask = name.clone();
                                                                changed = true;
                                                            }
                                                        }
                                                    })
                                                    .response
                                                    .on_hover_text("Limit this layer to a joint subtree");

                                                // Solo / mute
                                                ui.horizontal(|ui| {
                                                    let is_solo = solo == Some(li);
                                                    if ui
                                                        .selectable_label(is_solo, "S")
                                                        .on_hover_text("Solo: hear only this layer")
                                                        .clicked()
                                                    {
                                                        solo = if is_solo { None } else { Some(li) };
                                                    }
                                                    let mut m = mutes[li];
                                                    if ui
                                                        .selectable_label(m, "M")
                                                        .on_hover_text("Mute this layer (weight is kept)")
                                                        .clicked()
                                                    {
                                                        m = !m;
                                                        mutes[li] = m;
                                                    }
                                                });

                                                // Time readout
                                                ui.monospace(format!("{:.2}s", layer.time));

                                                // Remove
                                                if ui.small_button("\u{2715}").on_hover_text("Remove layer").clicked() {
                                                    remove = Some(li);
                                                }
                                                ui.end_row();
                                            }
                                        });

                                    if let Some(i) = remove {
                                        edited.remove(i);
                                        mutes.remove(i);
                                        solo = match solo {
                                            Some(s) if s == i => None,
                                            Some(s) if s > i => Some(s - 1),
                                            other => other,
                                        };
                                        changed = true;
                                    }

                                    ui.horizontal(|ui| {
                                        if ui.button("+ Add layer").clicked() {
                                            let mut l = AnimLayer::default();
                                            // Default to the first clip that isn't the base clip
                                            let base = anim_clip_names.get(anim_clip_index).cloned().unwrap_or_default();
                                            l.clip = skeletal_clip_names
                                                .iter()
                                                .find(|n| **n != base)
                                                .cloned()
                                                .unwrap_or_default();
                                            edited.push(l);
                                            mutes.push(false);
                                            changed = true;
                                        }
                                        if !edited.is_empty() && ui.button("Clear all").clicked() {
                                            edited.clear();
                                            mutes.clear();
                                            solo = None;
                                            changed = true;
                                        }
                                        ui.label("Composed in order after the base clip.");
                                    });

                                    if changed {
                                        new_layers = Some(edited);
                                    }
                                    if solo != solo_layer {
                                        new_solo = Some(solo);
                                    }
                                    if mutes != muted_layers {
                                        new_mutes = Some(mutes);
                                    }
                                });
                        }

                        ui.add_space(2.0);
                    });
            }

            // ── Top-right overlay: FPS, camera info ──
            egui::Area::new(egui::Id::new("preview_overlay"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_black_alpha(160))
                        .rounding(4.0)
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(180, 220, 180),
                                format!("FPS: {:.0}", fps),
                            );
                            ui.label(format!("Mode: {}", current_debug_mode.label()));
                            ui.label(format!(
                                "Cam: d={:.1} y={:.0}\u{b0} p={:.0}\u{b0}",
                                cam_distance, cam_yaw_deg, cam_pitch_deg
                            ));
                        });
                });
        });

        // Apply mutations
        egui_winit.handle_platform_output(&window, full_output.platform_output);

        // Tessellate and render
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder = context
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

        context.queue.submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);

        // Apply collected mutations to renderer/animation state
        if let Some(mode) = new_debug_mode {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.set_debug_mode(mode);
            }
        }
        if let Some(wf) = new_wireframe {
            if let Some(renderer) = &mut self.scene_renderer {
                let mode = if wf {
                    DebugMode::WireframeOverlay
                } else {
                    DebugMode::Pbr
                };
                renderer.set_debug_mode(mode);
            }
        }
        if let Some(na) = new_normals {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.debug_state_mut().show_normals = na;
            }
        }
        if let Some(sk) = new_skeleton {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.debug_state_mut().show_skeleton = sk;
            }
            // Generate skeleton overlay from imported model's inverse bind matrices
            self.update_skeleton_overlay_from_model();
        }
        if let Some(gr) = new_grid {
            if let (Some(renderer), Some(ctx)) = (&mut self.scene_renderer, &self.render_context) {
                renderer.set_show_grid(&ctx.device, gr);
            }
        }
        if let Some(paused) = new_anim_paused {
            self.anim_paused = paused;
            if let Ok(mut state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    if let Some(components) = state.world.get_components_mut(eid) {
                        components.set_field(
                            comp::ANIMATOR,
                            "playing",
                            toml::Value::Boolean(!paused),
                        );
                    }
                }
            }
        }
        if let Some(idx) = new_clip_index {
            self.switch_clip(idx);
        }
        if let Some(spd) = new_speed {
            if let Ok(mut state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    if let Some(components) = state.world.get_components_mut(eid) {
                        components.set_field(comp::ANIMATOR, "speed", toml::Value::Float(spd));
                    }
                }
            }
        }
        let mut repose = false;
        if let Some(t) = scrub_time {
            if let Ok(state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    self.animation.set_skeletal_playback_time(&eid, t);
                    self.animation.set_node_playback_time(&eid, t);
                }
            }
            repose = true;
        }
        if let Some(layers) = new_layers {
            if let Ok(mut state) = self.state.lock() {
                if let Some(eid) = state.entity_id {
                    write_layers_to_world(&mut state.world, eid, &layers);
                }
            }
            self.muted_layers.resize(layers.len(), false);
            repose = true;
        }
        if let Some(solo) = new_solo {
            self.solo_layer = solo;
            repose = true;
        }
        if let Some(mutes) = new_mutes {
            self.muted_layers = mutes;
            repose = true;
        }
        if let Some(viz) = new_layer_viz {
            self.layer_viz = viz;
            if viz != LayerViz::Off {
                if let Some(renderer) = &mut self.scene_renderer {
                    renderer.debug_state_mut().show_skeleton = true;
                }
            }
            self.update_skeleton_overlay_from_model();
        }
        if let Some(lp) = seq_loop {
            if let Some(eid) = self.state.lock().ok().and_then(|s| s.entity_id) {
                self.animation.set_sequence_loop_override(&eid, Some(lp));
                // Turning loop on after the end: pick up from the top
                if lp
                    && self
                        .animation
                        .sequence_state(&eid)
                        .is_some_and(|rt| !rt.playing)
                {
                    seq_restart = true;
                    if self.anim_paused {
                        self.anim_paused = false;
                        if let Ok(mut state) = self.state.lock() {
                            if let Some(c) = state.world.get_components_mut(eid) {
                                c.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
                            }
                        }
                    }
                }
            }
            self.update_skeleton_overlay_from_model();
        }
        if seq_restart {
            self.seek_sequence_to(0.0);
            repose = false;
        } else if let Some(t) = seq_seek {
            self.seek_sequence_to(t);
            repose = false;
        }
        if repose {
            // dt = 0 re-pose so dials and scrubs show immediately, even paused
            self.recompute_pose_now();
        }
    }
}

/// Recursively render a node tree in the UI
/// Build the armature overlay from a skeleton's current model-space joint globals.
fn skeleton_overlay_mesh(
    skel: &Skeleton,
    viz: LayerViz,
    contrib: Option<&LayerContribution>,
) -> flint_render::Mesh {
    let positions: Vec<[f32; 3]> = skel
        .global_matrices
        .iter()
        .map(|g| [g[3][0], g[3][1], g[3][2]])
        .collect();
    if viz == LayerViz::Off {
        return flint_render::generate_skeleton_lines(&positions, &skel.parents);
    }
    let colors: Vec<[f32; 4]> = (0..positions.len())
        .map(|j| joint_viz_color(viz, j, contrib))
        .collect();
    flint_render::generate_skeleton_lines_colored(&positions, &skel.parents, &colors)
}

/// Invert a row-major 4x4 matrix (for extracting world transforms from inverse bind matrices).
fn invert_4x4(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Use cofactor expansion for a general 4x4 inverse
    let s = [
        m[0][0] * m[1][1] - m[1][0] * m[0][1],
        m[0][0] * m[1][2] - m[1][0] * m[0][2],
        m[0][0] * m[1][3] - m[1][0] * m[0][3],
        m[0][1] * m[1][2] - m[1][1] * m[0][2],
        m[0][1] * m[1][3] - m[1][1] * m[0][3],
        m[0][2] * m[1][3] - m[1][2] * m[0][3],
    ];
    let c = [
        m[2][0] * m[3][1] - m[3][0] * m[2][1],
        m[2][0] * m[3][2] - m[3][0] * m[2][2],
        m[2][0] * m[3][3] - m[3][0] * m[2][3],
        m[2][1] * m[3][2] - m[3][1] * m[2][2],
        m[2][1] * m[3][3] - m[3][1] * m[2][3],
        m[2][2] * m[3][3] - m[3][2] * m[2][3],
    ];

    let det = s[0] * c[5] - s[1] * c[4] + s[2] * c[3] + s[3] * c[2] - s[4] * c[1] + s[5] * c[0];
    if det.abs() < 1e-12 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let inv_det = 1.0 / det;

    [
        [
            (m[1][1] * c[5] - m[1][2] * c[4] + m[1][3] * c[3]) * inv_det,
            (-m[0][1] * c[5] + m[0][2] * c[4] - m[0][3] * c[3]) * inv_det,
            (m[3][1] * s[5] - m[3][2] * s[4] + m[3][3] * s[3]) * inv_det,
            (-m[2][1] * s[5] + m[2][2] * s[4] - m[2][3] * s[3]) * inv_det,
        ],
        [
            (-m[1][0] * c[5] + m[1][2] * c[2] - m[1][3] * c[1]) * inv_det,
            (m[0][0] * c[5] - m[0][2] * c[2] + m[0][3] * c[1]) * inv_det,
            (-m[3][0] * s[5] + m[3][2] * s[2] - m[3][3] * s[1]) * inv_det,
            (m[2][0] * s[5] - m[2][2] * s[2] + m[2][3] * s[1]) * inv_det,
        ],
        [
            (m[1][0] * c[4] - m[1][1] * c[2] + m[1][3] * c[0]) * inv_det,
            (-m[0][0] * c[4] + m[0][1] * c[2] - m[0][3] * c[0]) * inv_det,
            (m[3][0] * s[4] - m[3][1] * s[2] + m[3][3] * s[0]) * inv_det,
            (-m[2][0] * s[4] + m[2][1] * s[2] - m[2][3] * s[0]) * inv_det,
        ],
        [
            (-m[1][0] * c[3] + m[1][1] * c[1] - m[1][2] * c[0]) * inv_det,
            (m[0][0] * c[3] - m[0][1] * c[1] + m[0][2] * c[0]) * inv_det,
            (-m[3][0] * s[3] + m[3][1] * s[1] - m[3][2] * s[0]) * inv_det,
            (m[2][0] * s[3] - m[2][1] * s[1] + m[2][2] * s[0]) * inv_det,
        ],
    ]
}

fn render_node_tree(
    ui: &mut egui::Ui,
    nodes: &[(String, Vec<usize>, bool)],
    node_idx: usize,
    depth: usize,
    joint_annot: &HashMap<String, (egui::Color32, String)>,
) {
    if node_idx >= nodes.len() {
        return;
    }
    let (name, children, has_mesh) = &nodes[node_idx];
    let display_name = if name.is_empty() {
        format!("Node {}", node_idx)
    } else {
        name.clone()
    };
    let icon = if *has_mesh { "\u{25a0} " } else { "\u{25cb} " };
    let annot = joint_annot.get(name);
    let label = match annot {
        Some((color, _)) => egui::RichText::new(format!("{}{}", icon, display_name)).color(*color),
        None => egui::RichText::new(format!("{}{}", icon, display_name)),
    };

    if children.is_empty() {
        ui.indent(format!("node_{}", node_idx), |ui| {
            let r = ui.label(label);
            if let Some((_, tip)) = annot {
                r.on_hover_text(tip);
            }
        });
    } else {
        let r = egui::CollapsingHeader::new(label)
            .id_salt(format!("node_{}", node_idx))
            .default_open(depth < 2)
            .show(ui, |ui| {
                for &child_idx in children {
                    render_node_tree(ui, nodes, child_idx, depth + 1, joint_annot);
                }
            });
        if let Some((_, tip)) = annot {
            r.header_response.on_hover_text(tip);
        }
    }
}

impl ApplicationHandler for PreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.initialize(event_loop);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Forward events to egui first
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
            WindowEvent::KeyboardInput { event, .. } => {
                // Let orbit controller track WASD/QE held state
                self.orbit.handle_key_event(&event);

                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::Tab) => {
                            self.show_ui = !self.show_ui;
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            // Reset camera to auto-fit
                            if let Ok(state) = self.state.lock() {
                                if let Some(model_path) = &state.model_path {
                                    if let Ok(r) = import_gltf(model_path) {
                                        if let Some(b) = r.bounds() {
                                            auto_fit_camera(&b, &mut self.camera);
                                            self.camera.update_orbit();
                                        }
                                    }
                                }
                            }
                        }
                        // Animation controls
                        PhysicalKey::Code(KeyCode::KeyP) => {
                            if self.anim_info.is_some() {
                                self.anim_paused = !self.anim_paused;
                                // Update playing state in ECS
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            components.set_field(
                                                comp::ANIMATOR,
                                                "playing",
                                                toml::Value::Boolean(!self.anim_paused),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            if self.sequence.is_some() {
                                self.seek_sequence_to(0.0);
                                println!("Sequence restarted");
                            }
                        }
                        PhysicalKey::Code(KeyCode::Home) => {
                            if self.sequence.is_some() {
                                self.seek_sequence_to(0.0);
                            }
                        }
                        PhysicalKey::Code(KeyCode::Period) => {
                            // Next clip
                            if let Some(info) = &self.anim_info {
                                let next = (info.current_clip_index + 1) % info.clip_names.len();
                                self.switch_clip(next);
                            }
                        }
                        PhysicalKey::Code(KeyCode::Comma) => {
                            // Previous clip
                            if let Some(info) = &self.anim_info {
                                let prev = if info.current_clip_index == 0 {
                                    info.clip_names.len() - 1
                                } else {
                                    info.current_clip_index - 1
                                };
                                self.switch_clip(prev);
                            }
                        }
                        PhysicalKey::Code(KeyCode::KeyO) => {
                            self.orbit.auto_orbit = !self.orbit.auto_orbit;
                            println!(
                                "Auto-orbit: {}",
                                if self.orbit.auto_orbit { "ON" } else { "OFF" }
                            );
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            self.orbit.adjust_auto_orbit_speed(1.5);
                            println!("Auto-orbit speed: {:.2} rad/s", self.orbit.auto_orbit_speed);
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) => {
                            self.orbit.adjust_auto_orbit_speed(1.0 / 1.5);
                            println!("Auto-orbit speed: {:.2} rad/s", self.orbit.auto_orbit_speed);
                        }
                        PhysicalKey::Code(KeyCode::Equal) => {
                            // Increase speed (×1.5)
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            let current = components
                                                .get(comp::ANIMATOR)
                                                .and_then(|a| {
                                                    a.get("speed").and_then(|v| {
                                                        v.as_float().or_else(|| {
                                                            v.as_integer().map(|i| i as f64)
                                                        })
                                                    })
                                                })
                                                .unwrap_or(1.0);
                                            let new_speed = (current * 1.5).min(10.0);
                                            components.set_field(
                                                comp::ANIMATOR,
                                                "speed",
                                                toml::Value::Float(new_speed),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::Minus) => {
                            // Decrease speed (÷1.5)
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            let current = components
                                                .get(comp::ANIMATOR)
                                                .and_then(|a| {
                                                    a.get("speed").and_then(|v| {
                                                        v.as_float().or_else(|| {
                                                            v.as_integer().map(|i| i as f64)
                                                        })
                                                    })
                                                })
                                                .unwrap_or(1.0);
                                            let new_speed = (current / 1.5).max(0.1);
                                            components.set_field(
                                                comp::ANIMATOR,
                                                "speed",
                                                toml::Value::Float(new_speed),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::Digit0) => {
                            // Reset speed to 1.0
                            if self.anim_info.is_some() {
                                if let Ok(mut state) = self.state.lock() {
                                    if let Some(eid) = state.entity_id {
                                        if let Some(components) =
                                            state.world.get_components_mut(eid)
                                        {
                                            components.set_field(
                                                comp::ANIMATOR,
                                                "speed",
                                                toml::Value::Float(1.0),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width > 0 && new_size.height > 0 {
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
            }
            WindowEvent::DroppedFile(path) => {
                self.load_model_file(path);
            }
            ref ev @ (WindowEvent::MouseInput { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseWheel { .. }) => {
                self.orbit
                    .handle_event(ev, &mut self.camera, self.egui_ctx.is_pointer_over_area());
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt_secs = self.last_frame_time.elapsed().as_secs_f64();
                self.last_frame_time = now;

                // Apply held keyboard orbit/zoom
                self.orbit.update(&mut self.camera, dt_secs as f32);

                // FPS tracking
                self.frame_times.push_back(now);
                while self
                    .frame_times
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
                {
                    self.frame_times.pop_front();
                }
                if now.duration_since(self.last_fps_update) > Duration::from_millis(250) {
                    self.fps = self.frame_times.len() as f32;
                    self.last_fps_update = now;
                }

                // Check for file-watch reload
                self.check_reload();

                // Update window title with animation status
                if self.anim_info.is_some() {
                    self.update_title();
                }

                // Animation update
                if self.anim_info.is_some() && !self.anim_paused {
                    if self.sequence.is_some() {
                        // Sequence writes animator fields; run it before the
                        // skeletal sync so they land this frame.
                        if let Ok(mut state) = self.state.lock() {
                            self.animation.sync_sequences_from_world(&state.world);
                            self.animation.advance_sequences(&mut state.world, dt_secs);
                        }
                        for cue in self.animation.drain_sequence_cues() {
                            println!("[sequence] cue '{}' at {:.2}s", cue.cue, cue.time);
                        }
                    }
                    if let Ok(state) = self.state.lock() {
                        // Sync from world picks up component changes (clip switches, speed, layers, etc.)
                        self.animation.sync_property_from_world(&state.world);
                        self.animation.sync_skeletal_from_world(&state.world);
                        self.animation.sync_node_from_world(&state.world);
                    }
                    self.apply_layer_mutes();
                    if let Ok(mut state) = self.state.lock() {
                        // Advance all animation tiers
                        self.animation
                            .advance_property_and_write(&mut state.world, dt_secs);
                        self.animation.advance_skeletal(dt_secs);
                        // Retire finished blends/fades or they re-arm next frame
                        self.animation.write_back_skeletal(&mut state.world);
                        self.animation
                            .advance_node_and_apply(&mut state.world, dt_secs);

                        self.anim_time_accumulator += dt_secs;
                    }
                }

                let context = match &self.render_context {
                    Some(c) => c,
                    None => return,
                };
                let renderer = match &mut self.scene_renderer {
                    Some(r) => r,
                    None => return,
                };

                // Bone upload + overlay run every frame (cheap) so a paused
                // pose still reflects layer dials after a dt=0 re-pose.
                if self.anim_info.is_some() {
                    for (entity_id, asset_name) in &self.skeletal_entity_assets {
                        if let Some(matrices) = self.animation.bone_matrices(entity_id) {
                            renderer.update_bone_matrices(
                                &context.device,
                                &context.queue,
                                *entity_id,
                                asset_name,
                                matrices,
                            );
                        }
                    }

                    // Keep the armature overlay in step with the animated pose
                    if renderer.debug_state().show_skeleton {
                        if let Some(eid) = self.skeletal_entity_assets.keys().next() {
                            if let Some(skel) = self.animation.skeleton(eid) {
                                let contrib = self.animation.skeletal_layer_contribution(eid);
                                let mesh = skeleton_overlay_mesh(skel, self.layer_viz, contrib);
                                renderer.set_skeleton_overlay(&context.device, &mesh);
                            }
                        }
                    }
                }

                if let Ok(state) = self.state.lock() {
                    renderer.update_from_world(&state.world, &context.device);
                }

                let output = match context.surface.get_current_texture() {
                    Ok(o) => o,
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let size = context.size;
                        if let Some(ctx) = &mut self.render_context {
                            ctx.resize(size);
                        }
                        return;
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("Out of GPU memory");
                        event_loop.exit();
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("Surface error: {:?}", e);
                        return;
                    }
                };

                let view = output
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());

                let _ = renderer.render(context, &self.camera, &view);

                // egui overlay
                if self.show_ui {
                    self.render_egui(&view);
                }

                output.present();
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
