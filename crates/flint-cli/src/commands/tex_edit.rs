//! `flint tex-edit` — Visual node editor for texture pipeline specs.
//!
//! Opens an egui window with a three-panel layout:
//! - Left: node properties (global params + selected node params)
//! - Center: egui-snarl node graph canvas
//! - Right: output texture preview with channel browser

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use egui_snarl::ui::{PinInfo, SnarlStyle, SnarlViewer};
use egui_snarl::{InPinId, NodeId, OutPinId, Snarl};
use flint_procgen::generators::texture::node_graph::{self, TextureNode};
use flint_procgen::generators::texture::pipeline::{self, op_param_schema, ALL_OP_TYPES};
use flint_procgen::{
    parse_param_schema, ImageData, ProcGenSpec, SeededRng, SeedConfig, SeedMode, TextureField,
    TextureMapParams,
};
use flint_render::RenderContext;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use super::gen_preview_common::{self, random_seed};

// ─── CLI args ───────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct TexEditArgs {
    /// Path to a .procgen.toml spec file (must use pattern = "pipeline")
    pub spec: String,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Window width
    #[arg(long, default_value = "1600")]
    pub width: u32,

    /// Window height
    #[arg(long, default_value = "1000")]
    pub height: u32,
}

pub fn run(args: TexEditArgs) -> Result<()> {
    let spec_path = PathBuf::from(&args.spec);
    let mut spec = ProcGenSpec::from_file(&spec_path)
        .with_context(|| format!("failed to load spec from {}", spec_path.display()))?;

    if let Some(seed_val) = args.seed {
        spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(seed_val),
            derive_from: None,
        };
    }

    // Verify this is a pipeline pattern spec
    let map_params = TextureMapParams::from_toml(&spec.params)
        .with_context(|| "failed to parse texture map params")?;
    if map_params.pattern != "pipeline" {
        anyhow::bail!(
            "tex-edit only works with pattern = \"pipeline\" specs (got \"{}\")",
            map_params.pattern
        );
    }

    // Parse ops into snarl graph
    let ops_array = spec
        .params
        .as_table()
        .and_then(|t| t.get("ops"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let (nodes, connections) =
        node_graph::ops_to_graph(&ops_array, &map_params.output_maps)
            .with_context(|| "failed to parse pipeline ops into graph")?;

    // Build the snarl graph
    let mut snarl = Snarl::new();
    let mut node_ids: Vec<NodeId> = Vec::new();
    for (i, node) in nodes.into_iter().enumerate() {
        let pos = egui::pos2(250.0 + i as f32 * 200.0, 100.0 + i as f32 * 120.0);
        node_ids.push(snarl.insert_node(pos, node));
    }

    // Wire up connections
    for conn in &connections {
        if conn.from_node < node_ids.len() && conn.to_node < node_ids.len() {
            snarl.connect(
                OutPinId {
                    node: node_ids[conn.from_node],
                    output: conn.from_pin,
                },
                InPinId {
                    node: node_ids[conn.to_node],
                    input: conn.to_pin,
                },
            );
        }
    }

    // File watcher
    let reload_flag = Arc::new(Mutex::new(false));
    let _watcher = {
        let flag = Arc::clone(&reload_flag);
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
        debouncer
            .watcher()
            .watch(spec_path.as_ref(), RecursiveMode::NonRecursive)?;
        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_) => {
                        if let Ok(mut f) = flag.lock() {
                            *f = true;
                        }
                    }
                    Err(e) => tracing::warn!("Watch error: {:?}", e),
                }
            }
        });
        Some(debouncer)
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let current_seed = spec
        .seed
        .value
        .unwrap_or(42);

    let mut app = TexEditApp {
        spec,
        spec_path,
        map_params,
        reload_flag,
        _watcher,

        snarl,
        selected_node: None,

        // Pipeline execution
        dirty: true,
        last_change: Instant::now() - Duration::from_secs(1),
        debounce_ms: 200,
        current_seed,
        last_images: Vec::new(),
        snapshots: Vec::new(),
        gen_time_ms: 0.0,

        // Node preview thumbnails
        node_preview_textures: HashMap::new(),

        // Preview
        selected_image_tab: 0,
        selected_channel: None,
        selected_snapshot_node: None,
        texture_zoom: 1.0,
        texture_offset: [0.0, 0.0],
        texture_tiled: false,

        // Window / GPU
        args,
        window: None,
        render_context: None,

        // egui
        egui_ctx: egui::Context::default(),
        egui_winit: None,
        egui_renderer: None,
        egui_textures: Vec::new(),
        show_ui: true,

        // FPS
        frame_times: VecDeque::new(),
        fps: 0.0,
        last_fps_update: Instant::now(),

        // Undo
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),

        // Save state
        watcher_paused_until: None,
        status_message: None,
    };

    // Auto-assign IDs to any nodes that don't have them (upgrades legacy specs)
    app.ensure_all_ids();

    event_loop.run_app(&mut app)?;
    Ok(())
}

// ─── App state ──────────────────────────────────────────────────────────────

struct TexEditApp {
    spec: ProcGenSpec,
    spec_path: PathBuf,
    map_params: TextureMapParams,
    reload_flag: Arc<Mutex<bool>>,
    #[allow(dead_code)]
    _watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>,

    // Node graph
    snarl: Snarl<TextureNode>,
    selected_node: Option<NodeId>,

    // Pipeline execution
    dirty: bool,
    last_change: Instant,
    debounce_ms: u64,
    current_seed: u64,
    last_images: Vec<ImageData>,
    snapshots: Vec<TextureField>,
    gen_time_ms: f64,

    // Node preview thumbnails
    node_preview_textures: HashMap<NodeId, egui::TextureHandle>,

    // Preview
    selected_image_tab: usize,
    selected_channel: Option<String>,
    selected_snapshot_node: Option<usize>,
    texture_zoom: f32,
    #[allow(dead_code)]
    texture_offset: [f32; 2],
    texture_tiled: bool,

    // Window / GPU
    args: TexEditArgs,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,

    // egui
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    egui_textures: Vec<(String, egui::TextureHandle)>,
    show_ui: bool,

    // FPS
    frame_times: VecDeque<Instant>,
    fps: f32,
    last_fps_update: Instant,

    // Undo
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,

    // Save state
    watcher_paused_until: Option<Instant>,
    status_message: Option<(String, Instant)>,
}

#[derive(Clone)]
struct UndoSnapshot {
    ops_toml: Vec<toml::Value>,
}

// ─── Initialization ─────────────────────────────────────────────────────────

impl TexEditApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let title = format!(
            "Flint Tex Edit \u{2014} {}",
            self.spec.meta.name
        );

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(&title)
                        .with_inner_size(PhysicalSize::new(self.args.width, self.args.height)),
                )
                .expect("Failed to create window"),
        );

        let context = pollster::block_on(RenderContext::new(window.clone()))
            .expect("Failed to create render context");

        // egui
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
        self.render_context = Some(context);
        self.window = Some(window);
    }

    /// Generate a unique node ID for an op type, given a set of existing IDs.
    ///
    /// Returns the op_type itself if unused, otherwise appends `_2`, `_3`, etc.
    fn auto_assign_id(
        existing: &std::collections::HashSet<String>,
        op_type: &str,
    ) -> String {
        let base = op_type.to_string();
        if !existing.contains(&base) {
            return base;
        }

        let mut counter = 2;
        loop {
            let candidate = format!("{base}_{counter}");
            if !existing.contains(&candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    /// Ensure all Op nodes in the snarl have IDs assigned.
    fn ensure_all_ids(&mut self) {
        // Collect existing IDs and nodes needing assignment
        let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut needs_id: Vec<(NodeId, String)> = Vec::new();

        for (nid, node) in self.snarl.node_ids() {
            match node {
                TextureNode::Op {
                    id: Some(id),
                    ..
                } => {
                    existing.insert(id.clone());
                }
                TextureNode::Op {
                    id: None, op_type, ..
                } => {
                    needs_id.push((nid, op_type.clone()));
                }
                _ => {}
            }
        }

        for (nid, op_type) in needs_id {
            let new_id = Self::auto_assign_id(&existing, &op_type);
            existing.insert(new_id.clone());
            if let Some(TextureNode::Op { id, .. }) = self.snarl.get_node_mut(nid) {
                *id = Some(new_id);
            }
        }
    }

    fn push_undo(&mut self) {
        let ops = self.collect_ops_toml();
        self.undo_stack.push(UndoSnapshot { ops_toml: ops });
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo_stack.pop() {
            let current = self.collect_ops_toml();
            self.redo_stack.push(UndoSnapshot { ops_toml: current });
            self.restore_from_ops(&snapshot.ops_toml);
        }
    }

    fn redo(&mut self) {
        if let Some(snapshot) = self.redo_stack.pop() {
            let current = self.collect_ops_toml();
            self.undo_stack.push(UndoSnapshot { ops_toml: current });
            self.restore_from_ops(&snapshot.ops_toml);
        }
    }

    fn collect_ops_toml(&self) -> Vec<toml::Value> {
        // Collect ordered op nodes (by Y position), building inputs from wires
        let ordered = self.ordered_op_nodes();

        let mut result = Vec::with_capacity(ordered.len());
        for (nid, node) in &ordered {
            if let TextureNode::Op {
                params, id, ..
            } = node
            {
                let mut table = params.clone();

                // Serialize id
                if let Some(node_id) = id {
                    table.insert("id".to_string(), toml::Value::String(node_id.clone()));
                }

                // Build inputs from snarl wire connections
                let in_channels = node.input_channels();
                let mut inputs_table = toml::map::Map::new();

                for (pin_idx, channel) in in_channels.iter().enumerate() {
                    let in_pin_id = InPinId {
                        node: *nid,
                        input: pin_idx,
                    };
                    // Check if this input pin has a remote connection
                    let in_pin = self.snarl.in_pin(in_pin_id);
                    if let Some(remote) = in_pin.remotes.first() {
                        // Find the source node's id and output channel
                        let src_node = &self.snarl[remote.node];
                        if let TextureNode::Op {
                            id: Some(src_id), ..
                        } = src_node
                        {
                            let src_out_channels = src_node.output_channels();
                            if let Some(src_channel) =
                                src_out_channels.get(remote.output)
                            {
                                inputs_table.insert(
                                    channel.to_string(),
                                    toml::Value::String(format!(
                                        "{src_id}.{src_channel}"
                                    )),
                                );
                            }
                        }
                    }
                }

                if !inputs_table.is_empty() {
                    table.insert(
                        "inputs".to_string(),
                        toml::Value::Table(inputs_table),
                    );
                }

                result.push(toml::Value::Table(table));
            }
        }

        result
    }

    fn restore_from_ops(&mut self, ops: &[toml::Value]) {
        let output_maps = self.map_params.output_maps.clone();
        if let Ok((nodes, connections)) = node_graph::ops_to_graph(ops, &output_maps) {
            self.snarl = Snarl::new();
            let mut node_ids = Vec::new();
            for (i, node) in nodes.into_iter().enumerate() {
                let pos = egui::pos2(250.0 + i as f32 * 200.0, 100.0 + i as f32 * 120.0);
                node_ids.push(self.snarl.insert_node(pos, node));
            }
            for conn in &connections {
                if conn.from_node < node_ids.len() && conn.to_node < node_ids.len() {
                    self.snarl.connect(
                        OutPinId {
                            node: node_ids[conn.from_node],
                            output: conn.from_pin,
                        },
                        InPinId {
                            node: node_ids[conn.to_node],
                            input: conn.to_pin,
                        },
                    );
                }
            }
            self.ensure_all_ids();
            self.mark_dirty();
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn regenerate(&mut self) {
        let ops_toml = self.collect_ops_toml();

        // Rebuild the params table with updated ops
        let mut params_table = match &self.spec.params {
            toml::Value::Table(t) => t.clone(),
            _ => toml::map::Map::new(),
        };
        let ops_array: Vec<toml::Value> = ops_toml;
        params_table.insert("ops".to_string(), toml::Value::Array(ops_array));
        let params = toml::Value::Table(params_table);

        let mut rng = SeededRng::new(self.current_seed);

        let start = Instant::now();
        match pipeline::run_pipeline_with_snapshots(&params, &self.map_params, &mut rng) {
            Ok((images, snapshots)) => {
                self.last_images = images;
                self.snapshots = snapshots;
                self.gen_time_ms = start.elapsed().as_secs_f64() * 1000.0;

                // Update egui textures
                self.update_textures();
                self.update_node_previews();
            }
            Err(e) => {
                self.status_message =
                    Some((format!("Generation error: {e}"), Instant::now()));
            }
        }
    }

    fn update_node_previews(&mut self) {
        self.node_preview_textures.clear();

        // Collect (node_id, cloned_node) pairs to avoid borrowing self.snarl across the loop
        let op_entries: Vec<(NodeId, TextureNode)> = self
            .ordered_op_nodes()
            .into_iter()
            .map(|(nid, node)| (nid, node.clone()))
            .collect();

        for (snap_idx, (node_id, node)) in op_entries.iter().enumerate() {
            if snap_idx >= self.snapshots.len() {
                continue;
            }
            let snapshot = &self.snapshots[snap_idx];
            let img = best_preview_image(node, snapshot);
            let size = [img.width as usize, img.height as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &img.pixels);
            let handle = self.egui_ctx.load_texture(
                format!("node_preview_{}", node_id.0),
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.node_preview_textures.insert(*node_id, handle);
        }

        // Preview for Output node
        let output_node_id: Option<NodeId> = self
            .snarl
            .node_ids()
            .find(|(_, node)| matches!(node, TextureNode::Output { .. }))
            .map(|(nid, _)| nid);

        if let Some(nid) = output_node_id {
            if let Some(img) = self.last_images.first() {
                let size = [img.width as usize, img.height as usize];
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied(size, &img.pixels);
                let handle = self.egui_ctx.load_texture(
                    "node_preview_output",
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                self.node_preview_textures.insert(nid, handle);
            }
        }
    }

    fn update_textures(&mut self) {
        self.egui_textures.clear();
        let names = ["Albedo", "Normal", "Roughness"];
        for (i, img) in self.last_images.iter().enumerate() {
            let name = names.get(i).copied().unwrap_or("Unknown");
            let size = [img.width as usize, img.height as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &img.pixels);
            let handle = self.egui_ctx.load_texture(
                name,
                color_image,
                egui::TextureOptions::LINEAR,
            );
            self.egui_textures.push((name.to_string(), handle));
        }
    }

    fn save_spec(&mut self) {
        let ops_toml = self.collect_ops_toml();

        // Re-read spec file as raw TOML text, update ops + seed, write back
        let raw = match std::fs::read_to_string(&self.spec_path) {
            Ok(s) => s,
            Err(e) => {
                self.status_message =
                    Some((format!("Read error: {e}"), Instant::now()));
                return;
            }
        };

        let mut doc: toml::Value = match raw.parse() {
            Ok(v) => v,
            Err(e) => {
                self.status_message =
                    Some((format!("Parse error: {e}"), Instant::now()));
                return;
            }
        };

        if let Some(root) = doc.as_table_mut() {
            // Update seed value
            if let Some(seed_table) = root.get_mut("seed").and_then(|v| v.as_table_mut()) {
                seed_table.insert(
                    "value".to_string(),
                    toml::Value::Integer(self.current_seed as i64),
                );
            }
            // Update ops array
            if let Some(params) = root.get_mut("params").and_then(|v| v.as_table_mut()) {
                params.insert("ops".to_string(), toml::Value::Array(ops_toml));
                // Update width/height
                params.insert(
                    "width".to_string(),
                    toml::Value::Integer(self.map_params.width as i64),
                );
                params.insert(
                    "height".to_string(),
                    toml::Value::Integer(self.map_params.height as i64),
                );
            }
        }

        // Pause file watcher
        self.watcher_paused_until = Some(Instant::now() + Duration::from_secs(2));

        let output = toml::to_string_pretty(&doc).unwrap_or_default();
        match std::fs::write(&self.spec_path, output) {
            Ok(()) => {
                self.status_message =
                    Some((format!("Saved to {}", self.spec_path.display()), Instant::now()));
            }
            Err(e) => {
                self.status_message =
                    Some((format!("Save error: {e}"), Instant::now()));
            }
        }
    }

    fn export_textures(&mut self) {
        let stem = self.spec_path.file_stem().and_then(|s| s.to_str()).unwrap_or("texture");
        // Strip .procgen if present
        let stem = stem.strip_suffix(".procgen").unwrap_or(stem);
        let dir = self.spec_path.parent().unwrap_or(std::path::Path::new("."));

        let suffixes = ["color", "normal", "roughness"];
        for (i, img) in self.last_images.iter().enumerate() {
            let suffix = suffixes.get(i).copied().unwrap_or("unknown");
            let path = dir.join(format!("{}_{}.png", stem, suffix));
            if let Err(e) = img.save_png(&path) {
                self.status_message =
                    Some((format!("Export error: {e}"), Instant::now()));
                return;
            }
        }
        self.status_message = Some((
            format!("Exported {} textures to {}", self.last_images.len(), dir.display()),
            Instant::now(),
        ));
    }

    fn reload_from_disk(&mut self) {
        if let Ok(spec) = ProcGenSpec::from_file(&self.spec_path) {
            if let Ok(map_params) = TextureMapParams::from_toml(&spec.params) {
                if map_params.pattern == "pipeline" {
                    let ops_array = spec
                        .params
                        .as_table()
                        .and_then(|t| t.get("ops"))
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    self.spec = spec;
                    self.map_params = map_params;
                    self.restore_from_ops(&ops_array);
                    self.status_message =
                        Some(("Reloaded from disk".to_string(), Instant::now()));
                }
            }
        }
    }

    /// Get the ordered list of op nodes (excluding Output) for pipeline execution.
    fn ordered_op_nodes(&self) -> Vec<(NodeId, &TextureNode)> {
        let mut ops: Vec<(NodeId, &TextureNode)> = self
            .snarl
            .node_ids()
            .filter(|(_, node)| matches!(node, TextureNode::Op { .. }))
            .collect();
        // Sort by Y position (top to bottom = pipeline order)
        ops.sort_by(|(id_a, _), (id_b, _)| {
            let pos_a = self.snarl.get_node_info(*id_a).map(|n| n.pos.y).unwrap_or(0.0);
            let pos_b = self.snarl.get_node_info(*id_b).map(|n| n.pos.y).unwrap_or(0.0);
            pos_a.partial_cmp(&pos_b).unwrap_or(std::cmp::Ordering::Equal)
        });
        ops
    }
}

// ─── SnarlViewer implementation ─────────────────────────────────────────────

struct TexEditViewer<'a> {
    app: &'a mut TexEditApp,
}

/// Pick the best visualization for a node's preview thumbnail based on its port info.
fn best_preview_image(node: &TextureNode, snapshot: &TextureField) -> ImageData {
    match node {
        TextureNode::Op { port_info, params, .. } => {
            let all_channels: Vec<&str> = port_info
                .writes
                .iter()
                .chain(port_info.modifies.iter())
                .copied()
                .collect();

            if all_channels.iter().any(|c| matches!(*c, "r" | "g" | "b")) {
                snapshot.to_albedo_image()
            } else if all_channels
                .iter()
                .any(|c| matches!(*c, "normal_x" | "normal_y" | "normal_z"))
            {
                snapshot.to_normal_image()
            } else if all_channels.contains(&"roughness") {
                snapshot.to_roughness_image()
            } else if all_channels.contains(&"height") {
                snapshot.to_channel_grayscale("height")
            } else if all_channels.contains(&"mask") {
                // Structure ops (e.g. brick_grid): mask shows the grid pattern
                snapshot.to_channel_grayscale("mask")
            } else if let Some(first) = all_channels.first() {
                // Resolve dynamic placeholders like <target>, <source>, <output>
                let ch = if first.starts_with('<') && first.ends_with('>') {
                    let param_name = &first[1..first.len() - 1];
                    params
                        .get(param_name)
                        .and_then(|v| v.as_str())
                } else {
                    Some(*first)
                };
                if let Some(name) = ch.filter(|n| snapshot.has_channel(n)) {
                    snapshot.to_channel_grayscale(name)
                } else {
                    // Fallback: pick the first available channel in the snapshot
                    let names = snapshot.channel_names();
                    if let Some(name) = names.first() {
                        snapshot.to_channel_grayscale(name)
                    } else {
                        snapshot.to_albedo_image()
                    }
                }
            } else {
                snapshot.to_albedo_image()
            }
        }
        TextureNode::Output { .. } => snapshot.to_albedo_image(),
    }
}

/// Resolve a dynamic placeholder like `<target>` to its actual channel name from the node's params.
fn resolve_channel_label(node: &TextureNode, channel: &str) -> String {
    if channel.starts_with('<') && channel.ends_with('>') {
        if let TextureNode::Op { params, .. } = node {
            let param_name = &channel[1..channel.len() - 1];
            if let Some(resolved) = params.get(param_name).and_then(|v| v.as_str()) {
                return resolved.to_string();
            }
        }
    }
    channel.to_string()
}

/// Collect the set of param names that correspond to dynamic channel references
/// (e.g. `<target>` → "target", `<source>` → "source", `<output>` → "output").
fn dynamic_channel_params(port_info: &flint_procgen::generators::texture::ops::OpPortInfo) -> Vec<String> {
    port_info
        .reads
        .iter()
        .chain(port_info.writes.iter())
        .chain(port_info.modifies.iter())
        .filter(|ch| ch.starts_with('<') && ch.ends_with('>'))
        .map(|ch| ch[1..ch.len() - 1].to_string())
        .collect()
}

/// Get a pin color based on channel name.
fn channel_color(channel: &str) -> egui::Color32 {
    match channel {
        "cell_id" | "edge_dist" | "mask" => egui::Color32::from_rgb(80, 140, 220), // Blue: structure
        "height" => egui::Color32::from_rgb(160, 160, 160),                // Gray: height
        "r" | "g" | "b" | "a" => egui::Color32::from_rgb(220, 150, 50),   // Orange: color
        "normal_x" | "normal_y" | "normal_z" => egui::Color32::from_rgb(160, 80, 200), // Purple
        "roughness" => egui::Color32::from_rgb(80, 200, 100),              // Green
        _ => egui::Color32::from_rgb(180, 180, 180),                       // Default gray
    }
}

impl<'a> SnarlViewer<TextureNode> for TexEditViewer<'a> {
    fn has_body(&mut self, _node: &TextureNode) -> bool {
        true
    }

    fn title(&mut self, node: &TextureNode) -> String {
        node.label().to_string()
    }

    fn inputs(&mut self, node: &TextureNode) -> usize {
        node.input_channels().len()
    }

    fn outputs(&mut self, node: &TextureNode) -> usize {
        node.output_channels().len()
    }

    fn show_input(
        &mut self,
        pin: &egui_snarl::InPin,
        ui: &mut egui::Ui,
        _scale: f32,
        snarl: &mut Snarl<TextureNode>,
    ) -> PinInfo {
        let node = &snarl[pin.id.node];
        let channels = node.input_channels();
        let channel = channels.get(pin.id.input).copied().unwrap_or("?");
        let label = resolve_channel_label(node, channel);
        ui.label(&label);
        PinInfo::circle().with_fill(channel_color(&label))
    }

    fn show_output(
        &mut self,
        pin: &egui_snarl::OutPin,
        ui: &mut egui::Ui,
        _scale: f32,
        snarl: &mut Snarl<TextureNode>,
    ) -> PinInfo {
        let node = &snarl[pin.id.node];
        let channels = node.output_channels();
        let channel = channels.get(pin.id.output).copied().unwrap_or("?");
        let label = resolve_channel_label(node, channel);
        ui.label(&label);
        PinInfo::circle().with_fill(channel_color(&label))
    }

    fn show_body(
        &mut self,
        node: NodeId,
        _inputs: &[egui_snarl::InPin],
        _outputs: &[egui_snarl::OutPin],
        ui: &mut egui::Ui,
        scale: f32,
        snarl: &mut Snarl<TextureNode>,
    ) {
        ui.vertical(|ui| {
            // Show preview thumbnail if available
            if let Some(handle) = self.app.node_preview_textures.get(&node) {
                let preview_size = 96.0 * scale;
                ui.image(egui::load::SizedTexture::new(
                    handle.id(),
                    egui::Vec2::splat(preview_size),
                ));
            }

            // Gather op info (cloned to release snarl borrow)
            let op_info = match &snarl[node] {
                TextureNode::Op { op_type, port_info, .. } => {
                    let schema = op_param_schema(op_type);
                    let field_specs = parse_param_schema(&schema);
                    let hidden = dynamic_channel_params(port_info);
                    Some((op_type.clone(), field_specs, hidden))
                }
                TextureNode::Output { output_maps, .. } => {
                    ui.label(
                        egui::RichText::new(output_maps.join(", "))
                            .small()
                            .color(egui::Color32::from_gray(140)),
                    );
                    None
                }
            };

            // Render inline parameter editors for Op nodes
            if let Some((op_type, field_specs, hidden_params)) = op_info {
                if let Some(TextureNode::Op { params, .. }) = snarl.get_node_mut(node) {
                    let mut changed = false;
                    for field in &field_specs {
                        // Skip "type" and channel-reference params (shown as pin labels)
                        if field.name == "type"
                            || hidden_params.iter().any(|h| h.as_str() == field.name)
                        {
                            continue;
                        }
                        if gen_preview_common::render_param_field_full(ui, field, params, false) {
                            changed = true;
                        }
                    }
                    if changed {
                        self.app.push_undo();
                        self.app.mark_dirty();
                    }
                }

                ui.label(
                    egui::RichText::new(&op_type)
                        .small()
                        .color(egui::Color32::from_gray(140)),
                );
            }
        });
    }

    fn connect(
        &mut self,
        from: &egui_snarl::OutPin,
        to: &egui_snarl::InPin,
        snarl: &mut Snarl<TextureNode>,
    ) {
        // Allow connection — just wire it up
        snarl.connect(from.id, to.id);
        self.app.push_undo();
        self.app.mark_dirty();
    }

    fn disconnect(
        &mut self,
        from: &egui_snarl::OutPin,
        to: &egui_snarl::InPin,
        snarl: &mut Snarl<TextureNode>,
    ) {
        self.app.push_undo();
        snarl.disconnect(from.id, to.id);
        self.app.mark_dirty();
    }

    fn show_graph_menu(
        &mut self,
        pos: egui::Pos2,
        ui: &mut egui::Ui,
        _scale: f32,
        snarl: &mut Snarl<TextureNode>,
    ) {
        ui.label("Add Op");
        ui.separator();
        for op_type in ALL_OP_TYPES {
            if ui.button(*op_type).clicked() {
                if let Ok(mut node) = TextureNode::new_op(op_type) {
                    // Auto-assign a unique ID
                    let existing: std::collections::HashSet<String> = snarl
                        .node_ids()
                        .filter_map(|(_, n)| match n {
                            TextureNode::Op { id: Some(id), .. } => Some(id.clone()),
                            _ => None,
                        })
                        .collect();
                    let new_id = TexEditApp::auto_assign_id(&existing, op_type);
                    if let TextureNode::Op { ref mut id, .. } = node {
                        *id = Some(new_id);
                    }

                    self.app.push_undo();
                    snarl.insert_node(pos, node);
                    self.app.mark_dirty();
                    ui.close_menu();
                }
            }
        }
    }

    fn show_node_menu(
        &mut self,
        node: NodeId,
        _inputs: &[egui_snarl::InPin],
        _outputs: &[egui_snarl::OutPin],
        ui: &mut egui::Ui,
        _scale: f32,
        snarl: &mut Snarl<TextureNode>,
    ) {
        if matches!(&snarl[node], TextureNode::Op { .. }) {
            if ui.button("Delete").clicked() {
                self.app.push_undo();
                snarl.remove_node(node);
                if self.app.selected_node == Some(node) {
                    self.app.selected_node = None;
                }
                self.app.mark_dirty();
                ui.close_menu();
            }
        }
    }
}

// ─── ApplicationHandler ────────────────────────────────────────────────────

impl ApplicationHandler for TexEditApp {
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
        // Pass events to egui
        if let Some(egui_winit) = &mut self.egui_winit {
            if let Some(window) = &self.window {
                let resp = egui_winit.on_window_event(window.as_ref(), &event);
                if resp.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(new_size) => {
                if let Some(ctx) = &mut self.render_context {
                    if new_size.width > 0 && new_size.height > 0 {
                        ctx.config.width = new_size.width;
                        ctx.config.height = new_size.height;
                        ctx.surface.configure(&ctx.device, &ctx.config);
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Tab) => {
                            self.show_ui = !self.show_ui;
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            self.current_seed = random_seed();
                            self.mark_dirty();
                        }
                        PhysicalKey::Code(KeyCode::KeyS)
                            if self.egui_ctx.input(|i| i.modifiers.ctrl) =>
                        {
                            self.save_spec();
                        }
                        PhysicalKey::Code(KeyCode::KeyE)
                            if self.egui_ctx.input(|i| i.modifiers.ctrl) =>
                        {
                            self.export_textures();
                        }
                        PhysicalKey::Code(KeyCode::KeyZ)
                            if self.egui_ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift) =>
                        {
                            self.undo();
                        }
                        PhysicalKey::Code(KeyCode::KeyZ)
                            if self.egui_ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift) =>
                        {
                            self.redo();
                        }
                        PhysicalKey::Code(KeyCode::Delete) => {
                            if let Some(node_id) = self.selected_node {
                                if matches!(self.snarl.get_node(node_id), Some(TextureNode::Op { .. }))
                                {
                                    self.push_undo();
                                    self.snarl.remove_node(node_id);
                                    self.selected_node = None;
                                    self.mark_dirty();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.frame_times.push_back(Instant::now());
                while self
                    .frame_times
                    .front()
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(1))
                {
                    self.frame_times.pop_front();
                }
                if self.last_fps_update.elapsed() > Duration::from_millis(500) {
                    self.fps = self.frame_times.len() as f32;
                    self.last_fps_update = Instant::now();
                }

                // Check file watcher
                let should_reload = if let Some(until) = self.watcher_paused_until {
                    if Instant::now() > until {
                        self.watcher_paused_until = None;
                        false
                    } else {
                        false
                    }
                } else if let Ok(mut flag) = self.reload_flag.lock() {
                    if *flag {
                        *flag = false;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if should_reload {
                    self.reload_from_disk();
                }

                // Debounced regeneration
                if self.dirty
                    && self.last_change.elapsed()
                        >= Duration::from_millis(self.debounce_ms)
                {
                    self.dirty = false;
                    self.regenerate();
                }

                self.render_frame();
            }
            _ => {}
        }

        // Request continuous redraw
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

impl TexEditApp {
    fn render_frame(&mut self) {
        if self.render_context.is_none() || self.window.is_none() {
            return;
        }

        // Take egui input before draw_ui so we don't hold borrows across it
        let window = self.window.clone().unwrap();
        let raw_input = self
            .egui_winit
            .as_mut()
            .unwrap()
            .take_egui_input(window.as_ref());
        self.egui_ctx.begin_pass(raw_input);
        self.draw_ui();
        let full_output = self.egui_ctx.end_pass();

        self.egui_winit
            .as_mut()
            .unwrap()
            .handle_platform_output(window.as_ref(), full_output.platform_output);

        let paint_jobs = self.egui_ctx.tessellate(
            full_output.shapes,
            self.egui_ctx.pixels_per_point(),
        );

        let ctx = self.render_context.as_ref().unwrap();

        let surface_texture = match ctx.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                ctx.surface.configure(&ctx.device, &ctx.config);
                return;
            }
            Err(e) => {
                tracing::warn!("Surface error: {e}");
                return;
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [ctx.config.width, ctx.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        // Take renderer out to avoid borrow conflicts (same pattern as gen_preview)
        let mut egui_renderer = self.egui_renderer.take().unwrap();

        for (id, delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&ctx.device, &ctx.queue, *id, delta);
        }

        let mut encoder = ctx.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("tex_edit_encoder"),
            },
        );

        egui_renderer.update_buffers(
            &ctx.device,
            &ctx.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tex_edit_egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.12,
                            g: 0.12,
                            b: 0.14,
                            a: 1.0,
                        }),
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

        ctx.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);
    }

    fn draw_ui(&mut self) {
        if !self.show_ui {
            return;
        }

        let ctx = self.egui_ctx.clone();

        // ── Left panel: Node Properties ──
        egui::SidePanel::left("properties_panel")
            .default_width(280.0)
            .show(&ctx, |ui| {
                self.draw_properties_panel(ui);
            });

        // ── Right panel: Output Preview ──
        egui::SidePanel::right("preview_panel")
            .default_width(320.0)
            .show(&ctx, |ui| {
                self.draw_preview_panel(ui);
            });

        // ── Center: Node Graph Canvas ──
        egui::CentralPanel::default().show(&ctx, |ui| {
            self.draw_graph_panel(ui);
        });
    }

    fn draw_properties_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Texture Pipeline");
        ui.separator();

        // Global params
        egui::CollapsingHeader::new("Global")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Width:");
                    let mut w = self.map_params.width as i32;
                    if ui.add(egui::DragValue::new(&mut w).speed(1).range(1..=4096)).changed() {
                        self.map_params.width = w.max(1) as u32;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Height:");
                    let mut h = self.map_params.height as i32;
                    if ui.add(egui::DragValue::new(&mut h).speed(1).range(1..=4096)).changed() {
                        self.map_params.height = h.max(1) as u32;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Seed:");
                    let mut s = self.current_seed as i64;
                    if ui
                        .add(egui::DragValue::new(&mut s).speed(1).range(0..=999999))
                        .changed()
                    {
                        self.current_seed = s.max(0) as u64;
                        self.mark_dirty();
                    }
                    if ui.button("\u{1f3b2}").on_hover_text("Randomize (R)").clicked() {
                        self.current_seed = random_seed();
                        self.mark_dirty();
                    }
                });
            });

        ui.separator();

        // Selected node params
        if let Some(node_id) = self.selected_node {
            if let Some(node) = self.snarl.get_node(node_id) {
                let label = node.label().to_string();
                let op_type = node.op_type().map(|s| s.to_string());
                let current_id = match node {
                    TextureNode::Op { id, .. } => id.clone().unwrap_or_default(),
                    _ => String::new(),
                };

                ui.heading(&label);

                if let Some(op_type) = &op_type {
                    // Editable node ID
                    let mut id_buf = current_id;
                    ui.horizontal(|ui| {
                        ui.label("ID:");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut id_buf)
                                    .desired_width(160.0)
                                    .hint_text("node_id"),
                            )
                            .changed()
                        {
                            if let Some(TextureNode::Op { id, .. }) =
                                self.snarl.get_node_mut(node_id)
                            {
                                *id = if id_buf.is_empty() {
                                    None
                                } else {
                                    Some(id_buf.clone())
                                };
                            }
                            self.push_undo();
                            self.mark_dirty();
                        }
                    });

                    ui.separator();

                    let schema = op_param_schema(op_type);
                    let field_specs = parse_param_schema(&schema);

                    if let Some(TextureNode::Op { params, .. }) =
                        self.snarl.get_node_mut(node_id)
                    {
                        let mut changed = false;
                        for field in &field_specs {
                            // Skip the "type" field
                            if field.name == "type" {
                                continue;
                            }
                            if gen_preview_common::render_param_field_full(ui, field, params, false) {
                                changed = true;
                            }
                        }
                        if changed {
                            self.push_undo();
                            self.mark_dirty();
                        }
                    }
                }
            }
        } else {
            ui.label(
                egui::RichText::new("Click a node to edit its parameters")
                    .italics()
                    .color(egui::Color32::from_gray(120)),
            );
        }

        ui.separator();

        // Action buttons
        ui.horizontal(|ui| {
            if ui.button("Save (Ctrl+S)").clicked() {
                self.save_spec();
            }
            if ui.button("Export (Ctrl+E)").clicked() {
                self.export_textures();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Undo (Ctrl+Z)").clicked() {
                self.undo();
            }
            if ui.button("Redo (Ctrl+Shift+Z)").clicked() {
                self.redo();
            }
        });

        // Status message
        if let Some((msg, time)) = &self.status_message {
            if time.elapsed() < Duration::from_secs(3) {
                ui.separator();
                ui.label(egui::RichText::new(msg).color(egui::Color32::from_rgb(100, 200, 100)));
            }
        }

        // Stats
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "{:.1} fps | gen: {:.1}ms",
                self.fps, self.gen_time_ms
            ))
            .small()
            .color(egui::Color32::from_gray(100)),
        );
    }

    fn draw_preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Preview");
        ui.separator();

        // Output map tabs
        if !self.egui_textures.is_empty() {
            ui.horizontal(|ui| {
                for (i, (name, _)) in self.egui_textures.iter().enumerate() {
                    if ui
                        .selectable_label(
                            self.selected_image_tab == i && self.selected_channel.is_none(),
                            name,
                        )
                        .clicked()
                    {
                        self.selected_image_tab = i;
                        self.selected_channel = None;
                    }
                }
            });

            ui.separator();

            // Display selected texture
            let tex_handle = if let Some(channel) = &self.selected_channel {
                // Render channel from selected snapshot
                let snap_idx = self.selected_snapshot_node.unwrap_or(0);
                if snap_idx < self.snapshots.len() {
                    let img = self.snapshots[snap_idx].to_channel_grayscale(channel);
                    let size = [img.width as usize, img.height as usize];
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied(size, &img.pixels);
                    Some(self.egui_ctx.load_texture(
                        format!("chan_{channel}"),
                        color_image,
                        egui::TextureOptions::LINEAR,
                    ))
                } else {
                    None
                }
            } else {
                self.egui_textures
                    .get(self.selected_image_tab)
                    .map(|(_, h)| h.clone())
            };

            if let Some(handle) = &tex_handle {
                let avail = ui.available_size();
                let tex_size = handle.size_vec2();
                let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0)
                    * self.texture_zoom;
                let sized = egui::Vec2::new(tex_size.x * scale, tex_size.y * scale);

                // Tiled or single
                if self.texture_tiled {
                    let tiles = 3;
                    let tile_size = egui::Vec2::new(
                        sized.x / tiles as f32 * tiles as f32,
                        sized.y / tiles as f32 * tiles as f32,
                    );
                    let _ = tile_size; // TODO: tiling with UV
                    ui.image(egui::load::SizedTexture::new(handle.id(), sized));
                } else {
                    ui.image(egui::load::SizedTexture::new(handle.id(), sized));
                }
            }

            ui.separator();

            // Zoom controls
            ui.horizontal(|ui| {
                ui.label("Zoom:");
                ui.add(
                    egui::Slider::new(&mut self.texture_zoom, 0.25..=4.0)
                        .logarithmic(true),
                );
            });
            ui.checkbox(&mut self.texture_tiled, "Tile preview");
        } else {
            ui.label("No textures generated yet");
        }

        // ── Channel Browser ──
        ui.separator();
        ui.heading("Channel Browser");

        // Collect op node labels by pipeline order (sorted by Y position)
        let op_labels: Vec<(usize, String)> = {
            let op_nodes = self.ordered_op_nodes();
            op_nodes
                .iter()
                .enumerate()
                .map(|(i, (_, node))| (i, node.label().to_string()))
                .collect()
        };

        let mut new_channel: Option<(String, usize)> = None;

        for (snap_idx, label) in &op_labels {
            egui::CollapsingHeader::new(format!("{}. {}", snap_idx + 1, label))
                .default_open(false)
                .id_salt(format!("snap_{snap_idx}"))
                .show(ui, |ui| {
                    if *snap_idx < self.snapshots.len() {
                        let channel_names = self.snapshots[*snap_idx].channel_names();
                        for ch in &channel_names {
                            let is_ch_selected =
                                self.selected_channel.as_deref() == Some(ch.as_str())
                                    && self.selected_snapshot_node == Some(*snap_idx);
                            if ui.selectable_label(is_ch_selected, ch).clicked() {
                                new_channel = Some((ch.clone(), *snap_idx));
                            }
                        }
                    }
                });
        }

        if let Some((ch, idx)) = new_channel {
            self.selected_channel = Some(ch);
            self.selected_snapshot_node = Some(idx);
        }
    }

    fn draw_graph_panel(&mut self, ui: &mut egui::Ui) {
        // Check for node selection via click
        let selected = Snarl::<TextureNode>::get_selected_nodes("tex_edit_snarl", ui);
        if let Some(&node_id) = selected.first() {
            self.selected_node = Some(node_id);
        }

        let style = SnarlStyle::default();
        let mut viewer = TexEditViewer { app: self };

        // We need to split the borrow: viewer borrows app, but snarl is part of app.
        // Workaround: use a temporary reference to the snarl.
        // Actually, egui-snarl's show() takes &mut Snarl and &mut Viewer separately,
        // so we need to avoid the double borrow.

        // The solution: take snarl out temporarily
        let mut snarl = std::mem::replace(&mut viewer.app.snarl, Snarl::new());
        snarl.show(&mut viewer, &style, "tex_edit_snarl", ui);
        viewer.app.snarl = snarl;
    }
}
