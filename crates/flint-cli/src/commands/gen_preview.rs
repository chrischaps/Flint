//! `flint gen-preview` — Interactive previewer for procedural generation specs.
//!
//! Opens a window with a 3D viewport (for mesh generators) or texture tabs
//! (for texture generators), plus an auto-generated parameter editor panel.
//! Parameters are derived from each generator's `param_schema()` JSON Schema.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use flint_core::components as comp;
use flint_core::Vec3;
use flint_ecs::FlintWorld;
use flint_procgen::{
    export_glb, parse_param_schema, GeneratorOutput, GeneratorRegistry, MeshData, ParamFieldSpec,
    ProcGenSpec, SeedConfig, SeedMode,
};
use flint_render::{
    Camera, DebugMode, OrbitCameraController, RendererConfig, SceneRenderer, Vertex as RenderVertex,
};
use flint_viewer::host::{
    self, Editor, Flow, Gpu, HostOptions, InitCx, StillOptions, Target, Tick,
};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

// ─── CLI args ───────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct GenPreviewArgs {
    /// Path to a .procgen.toml spec file
    pub spec: String,

    /// Override the spec's seed with a fixed value
    #[arg(long)]
    pub seed: Option<u64>,

    /// Initial window width
    #[arg(long, default_value = "1440")]
    pub width: u32,

    /// Initial window height
    #[arg(long, default_value = "900")]
    pub height: u32,

    /// Disable the ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Start with auto-orbit enabled
    #[arg(long)]
    pub auto_orbit: bool,
}

/// `flint gen-preview <spec>`: the previewer in a window.
pub fn run(args: GenPreviewArgs) -> Result<()> {
    let options = HostOptions {
        width: args.width,
        height: args.height,
        camera: Default::default(),
        auto_orbit: args.auto_orbit,
    };
    host::run(editor(args)?, options)
}

/// Whether the spec at `path` makes images (a texture generator) rather
/// than a mesh: `flint edit` takes no camera flags for one (#712).
pub fn draws_images(path: &Path) -> Result<bool> {
    let spec = ProcGenSpec::from_file(path)
        .with_context(|| format!("failed to load spec from {}", path.display()))?;
    let mut registry = GeneratorRegistry::new();
    flint_procgen::register_built_in_generators(&mut registry);
    let generator = registry
        .get(&spec.generator)
        .with_context(|| format!("unknown generator '{}'", spec.generator))?;
    Ok(generator.output_kind() == flint_procgen::OutputKind::Image)
}

/// The procgen previewer for `args.spec`, for [`flint_viewer::host`] to
/// open in a window or draw as a `--render` still.
pub fn editor(args: GenPreviewArgs) -> Result<GenPreviewApp> {
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

    // Build registry and resolve generator
    let registry = GeneratorRegistry::with_built_ins();

    let generator = registry
        .get(&spec.generator)
        .with_context(|| format!("unknown generator '{}'", spec.generator))?;
    let schema = generator.param_schema();
    let field_specs = parse_param_schema(&schema);
    let output_kind = generator.output_kind();

    // Build initial param map from spec (or empty table)
    let initial_params = match &spec.params {
        toml::Value::Table(t) => t.clone(),
        _ => toml::map::Map::new(),
    };

    Ok(GenPreviewApp {
        spec,
        spec_path,
        registry,
        field_specs,
        param_schema: schema,
        output_kind,

        // Param editor
        params: initial_params,
        dirty: true, // generate on first frame
        last_change: Instant::now() - Duration::from_secs(1),
        debounce_ms: 200,

        // Seed
        current_seed: args.seed.unwrap_or(42),

        // Generation output
        last_skeleton: None,
        last_mesh: None,
        last_images: Vec::new(),
        selected_image_tab: 0,
        texture_zoom: 1.0,
        texture_offset: [0.0, 0.0],
        texture_tiled: false,
        texture_tile_count: 3,
        gen_time_ms: 0.0,
        last_world: None,

        // Window / GPU
        args,
        window: None,
        render_context: None,
        scene_renderer: None,
        camera: Camera::new(),
        orbit: OrbitCameraController::new(),

        // egui
        egui_ctx: egui::Context::default(),
        egui_textures: Vec::new(),
        show_ui: true,
        fps: 0.0,

        // LOD
        lod_count: 0,
        selected_lod: 0,

        // Save state
        watcher_paused_until: None,

        // Status message
        status_message: None,
    })
}

// ─── App state ──────────────────────────────────────────────────────────────

pub struct GenPreviewApp {
    spec: ProcGenSpec,
    spec_path: PathBuf,
    registry: GeneratorRegistry,
    field_specs: Vec<ParamFieldSpec>,
    /// The generator's param schema; decides which fields apply (`x-when`).
    param_schema: serde_json::Value,
    #[allow(dead_code)]
    output_kind: flint_procgen::OutputKind,

    // Param editor
    params: toml::map::Map<String, toml::Value>,
    dirty: bool,
    last_change: Instant,
    debounce_ms: u64,

    // Seed
    current_seed: u64,

    // Generation output
    last_skeleton: Option<flint_procgen::SkeletonDef>,
    last_mesh: Option<MeshData>,
    last_images: Vec<flint_procgen::ImageData>,
    selected_image_tab: usize,
    texture_zoom: f32,
    texture_offset: [f32; 2],
    texture_tiled: bool,
    texture_tile_count: usize,
    gen_time_ms: f64,
    last_world: Option<FlintWorld>,

    // Window / GPU (the host's, #712)
    args: GenPreviewArgs,
    window: Option<Arc<Window>>,
    render_context: Option<Gpu>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,
    orbit: OrbitCameraController,

    // egui
    egui_ctx: egui::Context,
    egui_textures: Vec<(String, egui::TextureHandle)>,
    show_ui: bool,
    /// The host's frames-a-second readout
    fps: f32,

    // LOD
    lod_count: usize,
    selected_lod: usize,

    // Save state: temporarily ignore file watcher events after saving
    watcher_paused_until: Option<Instant>,

    // Status message shown briefly in the UI after actions
    status_message: Option<(String, Instant)>,
}

// ─── Initialization ─────────────────────────────────────────────────────────

impl GenPreviewApp {
    // ─── Generation ─────────────────────────────────────────────────────

    fn regenerate(&mut self) {
        // Update spec params and seed
        self.spec.params = toml::Value::Table(self.params.clone());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };

        let start = Instant::now();
        let result = self.registry.generate_from_spec(&self.spec);
        self.gen_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(output) => match output {
                GeneratorOutput::Mesh(mesh) => {
                    self.upload_mesh(&mesh, 0);
                    self.last_mesh = Some(mesh);
                    self.last_skeleton = None;
                    self.last_images.clear();
                    self.lod_count = 1;
                    self.selected_lod = 0;
                }
                GeneratorOutput::MeshWithLods(lods) => {
                    self.last_skeleton = None;
                    self.lod_count = lods.len();
                    if self.selected_lod >= self.lod_count {
                        self.selected_lod = 0;
                    }
                    if let Some(mesh) = lods.get(self.selected_lod) {
                        self.upload_mesh(mesh, self.selected_lod);
                        self.last_mesh = Some(mesh.clone());
                    }
                    self.last_images.clear();
                }
                GeneratorOutput::Image(img) => {
                    self.last_mesh = None;
                    self.last_images = vec![img];
                    self.update_egui_textures();
                }
                GeneratorOutput::ImageSet(images) => {
                    self.last_mesh = None;
                    self.last_images = images;
                    if self.selected_image_tab >= self.last_images.len() {
                        self.selected_image_tab = 0;
                    }
                    self.update_egui_textures();
                }
                GeneratorOutput::SkinnedMesh(skinned) => {
                    // Store skeleton for visualization
                    self.last_skeleton = Some(skinned.skeleton.clone());

                    // Convert skinned mesh to regular MeshData for preview
                    // (renders the rest-pose geometry without skinning)
                    let mesh = skinned.rest_mesh();
                    self.upload_mesh(&mesh, 0);
                    self.last_mesh = Some(mesh);
                    self.last_images.clear();
                    self.lod_count = 1;
                    self.selected_lod = 0;
                }
                GeneratorOutput::Sound(_) => {
                    tracing::warn!("Sound output not supported in gen-preview");
                }
            },
            Err(e) => {
                tracing::error!("Generation failed: {}", e);
            }
        }

        self.dirty = false;

        // Refresh skeleton overlay if enabled
        self.update_skeleton_overlay();
    }

    /// Convert procgen MeshData to render vertices and upload to the GPU.
    fn upload_mesh(&mut self, mesh: &MeshData, _lod: usize) {
        if !mesh.submeshes.is_empty() {
            self.upload_submeshes(mesh);
            return;
        }

        // Single-material path (no submeshes)
        let base_color = mesh
            .materials
            .first()
            .map(|m| m.base_color)
            .unwrap_or([0.6, 0.6, 0.6, 1.0]);
        let metallic = mesh.materials.first().map(|m| m.metallic).unwrap_or(0.0);
        let roughness = mesh.materials.first().map(|m| m.roughness).unwrap_or(0.5);

        let render_vertices: Vec<RenderVertex> = mesh
            .vertices
            .iter()
            .map(|v| RenderVertex {
                position: v.position,
                normal: v.normal,
                color: base_color,
                uv: v.uv,
            })
            .collect();

        let material = flint_import::ImportedMaterial {
            name: "procgen_material".to_string(),
            base_color,
            metallic,
            roughness,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: false,
            alpha_mode: flint_import::AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            ..Default::default()
        };

        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.load_procedural_mesh(
                &context.device,
                "procgen_preview",
                &render_vertices,
                &mesh.indices,
                material,
            );
        }

        let world = build_mesh_world(1);
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.prepare(
                &context.device,
                &context.queue,
                &world,
                &flint_render::FrameInputs::default(),
            );
        }
        self.last_world = Some(world);

        self.auto_fit_to_mesh(mesh);
    }

    /// Upload one procedural mesh per submesh, each with the correct material color.
    fn upload_submeshes(&mut self, mesh: &MeshData) {
        for (i, sub) in mesh.submeshes.iter().enumerate() {
            let mat = mesh
                .materials
                .get(sub.material_index)
                .cloned()
                .unwrap_or_default();

            // Collect the referenced indices and remap to a compact vertex set
            let start = sub.index_start as usize;
            let end = start + sub.index_count as usize;
            let sub_indices = &mesh.indices[start..end];

            // Find unique vertices and build a remapping
            let mut vertex_map: std::collections::HashMap<u32, u32> =
                std::collections::HashMap::new();
            let mut compact_verts: Vec<RenderVertex> = Vec::new();
            let mut compact_indices: Vec<u32> = Vec::new();

            for &orig_idx in sub_indices {
                let new_idx = *vertex_map.entry(orig_idx).or_insert_with(|| {
                    let idx = compact_verts.len() as u32;
                    let v = &mesh.vertices[orig_idx as usize];
                    compact_verts.push(RenderVertex {
                        position: v.position,
                        normal: v.normal,
                        color: mat.base_color,
                        uv: v.uv,
                    });
                    idx
                });
                compact_indices.push(new_idx);
            }

            let mesh_name = format!("procgen_preview_{i}");
            let material = flint_import::ImportedMaterial {
                name: format!("procgen_material_{i}"),
                base_color: mat.base_color,
                metallic: mat.metallic,
                roughness: mat.roughness,
                base_color_texture: None,
                normal_texture: None,
                metallic_roughness_texture: None,
                use_vertex_color: false,
                alpha_mode: flint_import::AlphaMode::Opaque,
                alpha_cutoff: 0.5,
                ..Default::default()
            };

            if let (Some(context), Some(renderer)) =
                (&self.render_context, &mut self.scene_renderer)
            {
                renderer.load_procedural_mesh(
                    &context.device,
                    &mesh_name,
                    &compact_verts,
                    &compact_indices,
                    material,
                );
            }
        }

        let world = build_mesh_world(mesh.submeshes.len());
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.prepare(
                &context.device,
                &context.queue,
                &world,
                &flint_render::FrameInputs::default(),
            );
        }
        self.last_world = Some(world);

        self.auto_fit_to_mesh(mesh);
    }

    // mesh_world is now build_mesh_world() free function below

    fn auto_fit_to_mesh(&mut self, mesh: &MeshData) {
        let bb = &mesh.bounding_box;
        let center = bb.center();
        let size = bb.size();
        let diag = (size.x * size.x + size.y * size.y + size.z * size.z).sqrt();

        self.camera.target = Vec3::new(center.x, center.y, center.z);
        self.camera.distance = (diag * 1.2).max(2.0);
        self.camera.yaw = std::f32::consts::FRAC_PI_4;
        self.camera.pitch = 0.5;
        self.camera.update_orbit();
    }

    /// Update skeleton overlay from stored skeleton data.
    fn update_skeleton_overlay(&mut self) {
        let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer)
        else {
            return;
        };

        if !renderer.debug_state().show_skeleton {
            renderer.clear_skeleton_overlay();
            return;
        }

        let Some(skeleton) = &self.last_skeleton else {
            renderer.clear_skeleton_overlay();
            return;
        };

        let world_transforms = skeleton.compute_world_transforms();
        let positions: Vec<[f32; 3]> = world_transforms
            .iter()
            .map(|m| {
                // Extract translation from column-major 4x4 matrix (columns 12,13,14)
                [m[12], m[13], m[14]]
            })
            .collect();
        let parents: Vec<Option<usize>> = skeleton.bones.iter().map(|b| b.parent).collect();

        let mesh = flint_render::generate_skeleton_lines(&positions, &parents);
        renderer.set_skeleton_overlay(&context.device, &mesh);
    }

    /// Convert ImageData to egui texture handles for display.
    fn update_egui_textures(&mut self) {
        self.egui_textures.clear();
        for img in &self.last_images {
            let label = format!("{:?}", img.channel_semantics);
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.pixels,
            );
            let handle =
                self.egui_ctx
                    .load_texture(&label, color_image, egui::TextureOptions::LINEAR);
            self.egui_textures.push((label, handle));
        }
    }

    // ─── File watching ──────────────────────────────────────────────────

    /// The spec changed on disk (the host's watch): read it again, unless
    /// the change was this editor's own save.
    fn check_file_reload(&mut self) {
        // Skip if watcher is paused (after save)
        if let Some(until) = self.watcher_paused_until {
            if Instant::now() < until {
                return;
            }
            self.watcher_paused_until = None;
        }

        {
            match ProcGenSpec::from_file(&self.spec_path) {
                Ok(new_spec) => {
                    // Update params from disk
                    if let toml::Value::Table(t) = &new_spec.params {
                        self.params = t.clone();
                    }
                    self.spec = new_spec;
                    self.mark_dirty();
                }
                Err(e) => {
                    tracing::warn!("Failed to reload spec: {}", e);
                }
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    fn should_regenerate(&self) -> bool {
        self.dirty && self.last_change.elapsed() > Duration::from_millis(self.debounce_ms)
    }

    // ─── Save spec ──────────────────────────────────────────────────────

    /// The edited params minus any the generator would ignore under the
    /// current settings (a knob set while another pattern was selected).
    fn params_to_save(&self) -> toml::map::Map<String, toml::Value> {
        params_to_save(&self.param_schema, &self.params)
    }

    fn spec_text_for_save(&mut self) -> Result<String, String> {
        self.spec.params = toml::Value::Table(self.params_to_save());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };
        spec_text_for_save(&self.spec_path, &self.spec)
    }

    fn save_spec(&mut self) {
        match self.spec_text_for_save() {
            Ok(content) => {
                if let Err(e) = std::fs::write(&self.spec_path, &content) {
                    self.set_status(&format!("Failed to save: {e}"));
                } else {
                    // Pause watcher to avoid reload loop
                    self.watcher_paused_until = Some(Instant::now() + Duration::from_millis(1000));
                    self.set_status(&format!("Saved {}", self.spec_path.display()));
                }
            }
            Err(e) => {
                self.set_status(&format!("Serialize failed: {e}"));
            }
        }
    }

    // ─── Export output ────────────────────────────────────────────────────

    fn export_output(&mut self) {
        // Determine default filename and extension from the spec name
        let base_name = self
            .spec
            .meta
            .name
            .replace(' ', "_")
            .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");

        let has_mesh = self.last_mesh.is_some();
        let has_images = !self.last_images.is_empty();

        if !has_mesh && !has_images {
            self.set_status("Nothing to export");
            return;
        }

        if has_mesh {
            let dialog = crate::file_dialog::FileDialog::new()
                .set_title("Export GLB")
                .add_filter("glTF Binary", &["glb"])
                .set_file_name(format!("{base_name}.glb"));
            let dialog = if let Some(parent) = self.spec_path.parent() {
                dialog.set_directory(parent)
            } else {
                dialog
            };

            if let Some(path) = dialog.save_file() {
                self.export_mesh_to_path(&path);
            }
        } else if has_images {
            if self.last_images.len() == 1 {
                let dialog = crate::file_dialog::FileDialog::new()
                    .set_title("Export PNG")
                    .add_filter("PNG Image", &["png"])
                    .set_file_name(format!("{base_name}.png"));
                let dialog = if let Some(parent) = self.spec_path.parent() {
                    dialog.set_directory(parent)
                } else {
                    dialog
                };

                if let Some(path) = dialog.save_file() {
                    self.export_image_to_path(&self.last_images[0].clone(), &path);
                }
            } else {
                // Multiple images: pick a directory, save with suffixed names
                let dialog = crate::file_dialog::FileDialog::new()
                    .set_title("Export PNGs (select folder)")
                    .set_file_name(format!("{base_name}.png"));
                let dialog = if let Some(parent) = self.spec_path.parent() {
                    dialog.set_directory(parent)
                } else {
                    dialog
                };

                if let Some(base_path) = dialog.save_file() {
                    let stem = base_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("output");
                    let parent = base_path.parent().unwrap_or(Path::new("."));
                    let images = self.last_images.clone();
                    let mut count = 0;
                    for img in &images {
                        let suffix = format!("{:?}", img.channel_semantics).to_lowercase();
                        let img_path = parent.join(format!("{stem}_{suffix}.png"));
                        if let Err(e) = img.save_png(&img_path) {
                            tracing::error!("Failed to save {}: {}", img_path.display(), e);
                        } else {
                            count += 1;
                        }
                    }
                    self.set_status(&format!("Exported {count} PNGs to {}", parent.display()));
                }
            }
        }
    }

    fn export_mesh_to_path(&mut self, path: &Path) {
        // Re-generate to get the current output (need the full GeneratorOutput for LODs/skinned)
        self.spec.params = toml::Value::Table(self.params.clone());
        self.spec.seed = SeedConfig {
            mode: SeedMode::Fixed,
            value: Some(self.current_seed),
            derive_from: None,
        };

        let result = self.registry.generate_from_spec(&self.spec);
        match result {
            Ok(GeneratorOutput::Mesh(mesh)) => match export_glb::mesh_to_glb(&mesh) {
                Ok(glb) => match std::fs::write(path, &glb) {
                    Ok(()) => self.set_status(&format!("Exported {}", path.display())),
                    Err(e) => self.set_status(&format!("Write failed: {e}")),
                },
                Err(e) => self.set_status(&format!("GLB export failed: {e}")),
            },
            Ok(GeneratorOutput::MeshWithLods(lods)) => match export_glb::mesh_lods_to_glb(&lods) {
                Ok(glb) => match std::fs::write(path, &glb) {
                    Ok(()) => self.set_status(&format!(
                        "Exported {} ({} LODs)",
                        path.display(),
                        lods.len()
                    )),
                    Err(e) => self.set_status(&format!("Write failed: {e}")),
                },
                Err(e) => self.set_status(&format!("GLB export failed: {e}")),
            },
            Ok(GeneratorOutput::SkinnedMesh(mesh)) => {
                match export_glb::skinned_mesh_to_glb(&mesh) {
                    Ok(glb) => match std::fs::write(path, &glb) {
                        Ok(()) => self.set_status(&format!("Exported {}", path.display())),
                        Err(e) => self.set_status(&format!("Write failed: {e}")),
                    },
                    Err(e) => self.set_status(&format!("GLB export failed: {e}")),
                }
            }
            Ok(_) => self.set_status("No mesh to export"),
            Err(e) => self.set_status(&format!("Generation failed: {e}")),
        }
    }

    fn export_image_to_path(&mut self, img: &flint_procgen::ImageData, path: &Path) {
        match img.save_png(path) {
            Ok(()) => self.set_status(&format!("Exported {}", path.display())),
            Err(e) => self.set_status(&format!("Save failed: {e}")),
        }
    }

    // ─── Save spec as ────────────────────────────────────────────────────

    fn save_spec_as(&mut self) {
        let dialog = crate::file_dialog::FileDialog::new()
            .set_title("Save Spec As")
            .add_filter("ProcGen Spec", &["toml"])
            .set_file_name(
                self.spec_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("spec.procgen.toml"),
            );
        let dialog = if let Some(parent) = self.spec_path.parent() {
            dialog.set_directory(parent)
        } else {
            dialog
        };

        if let Some(path) = dialog.save_file() {
            match self.spec_text_for_save() {
                Ok(content) => {
                    if let Err(e) = std::fs::write(&path, &content) {
                        self.set_status(&format!("Failed to save: {e}"));
                    } else {
                        // Update the spec path to the new location
                        self.spec_path = path.clone();
                        self.watcher_paused_until =
                            Some(Instant::now() + Duration::from_millis(1000));
                        self.set_status(&format!("Saved to {}", path.display()));
                    }
                }
                Err(e) => {
                    self.set_status(&format!("Serialize failed: {e}"));
                }
            }
        }
    }

    fn set_status(&mut self, msg: &str) {
        tracing::info!("{}", msg);
        self.status_message = Some((msg.to_string(), Instant::now()));
    }

    // ─── egui parameter panel ───────────────────────────────────────────

    fn render_egui(&mut self, ui: &mut egui::Ui) {
        // Snapshot state for the closure
        let mut params = self.params.clone();
        let field_specs = self.field_specs.clone();
        let param_schema = &self.param_schema;
        let mut current_seed = self.current_seed;
        let gen_time_ms = self.gen_time_ms;
        let last_mesh = self.last_mesh.as_ref();
        let lod_count = self.lod_count;
        let mut selected_lod = self.selected_lod;
        let fps = self.fps;
        let spec_name = self.spec.meta.name.clone();
        let generator_name = self.spec.generator.clone();
        let is_texture = !self.last_images.is_empty();
        let mut selected_tab = self.selected_image_tab;
        let mut texture_zoom = self.texture_zoom;
        let mut texture_offset = self.texture_offset;
        let mut texture_tiled = self.texture_tiled;
        let mut texture_tile_count = self.texture_tile_count;
        let egui_textures = &self.egui_textures;
        let status_msg = self.status_message.clone();

        let mut dirty = false;
        let mut want_save = false;
        let mut want_save_as = false;
        let mut want_export = false;
        let mut want_reset = false;
        let mut want_randomize = false;

        {
            // Left side panel — parameter editor
            egui::Panel::left("param_panel")
                .default_size(320.0)
                .resizable(true)
                .show(ui, |ui| {
                    ui.heading(&spec_name);
                    ui.label(format!("Generator: {}", generator_name));
                    ui.separator();

                    // Seed control
                    ui.horizontal(|ui| {
                        ui.label("Seed:");
                        let mut seed_i64 = current_seed as i64;
                        if ui
                            .add(egui::DragValue::new(&mut seed_i64).speed(1))
                            .changed()
                        {
                            current_seed = seed_i64.max(0) as u64;
                            dirty = true;
                        }
                        if ui.button("Randomize").clicked() {
                            want_randomize = true;
                        }
                    });
                    ui.separator();

                    // Bottom section (stats, buttons, status) — always visible
                    egui::Panel::bottom("param_panel_bottom").show(ui, |ui| {
                        // LOD selector
                        if lod_count > 1 {
                            ui.horizontal(|ui| {
                                ui.label("LOD:");
                                for i in 0..lod_count {
                                    if ui
                                        .selectable_label(selected_lod == i, format!("{}", i))
                                        .clicked()
                                    {
                                        selected_lod = i;
                                        dirty = true;
                                    }
                                }
                            });
                            ui.separator();
                        }

                        // Stats
                        if let Some(mesh) = last_mesh {
                            ui.label(format!(
                                "Vertices: {}  Triangles: {}",
                                format_count(mesh.vertex_count() as u64),
                                format_count(mesh.triangle_count() as u64),
                            ));
                        }
                        ui.label(format!("Gen time: {:.1} ms", gen_time_ms));

                        ui.separator();

                        // Action buttons
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                want_save = true;
                            }
                            if ui.button("Save As").clicked() {
                                want_save_as = true;
                            }
                            if ui.button("Export").clicked() {
                                want_export = true;
                            }
                            if ui.button("Reset").clicked() {
                                want_reset = true;
                            }
                        });

                        // Status message (auto-fades after 4 seconds)
                        if let Some((msg, when)) = &status_msg {
                            let age = when.elapsed().as_secs_f32();
                            if age < 4.0 {
                                let alpha = if age > 3.0 { 1.0 - (age - 3.0) } else { 1.0 };
                                ui.label(egui::RichText::new(msg).small().color(
                                    egui::Color32::from_rgba_unmultiplied(
                                        180,
                                        220,
                                        180,
                                        (alpha * 255.0) as u8,
                                    ),
                                ));
                            }
                        }
                    });

                    // Parameter groups — fills remaining space with scroll
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // Only the knobs the generator reads for the
                            // current settings (e.g. the texture's pattern).
                            let shown: Vec<ParamFieldSpec> = field_specs
                                .iter()
                                .filter(|f| f.applies(param_schema, &params))
                                .cloned()
                                .collect();
                            let groups = group_fields(&shown);
                            for (group_name, fields) in &groups {
                                egui::CollapsingHeader::new(group_name)
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        for field in fields {
                                            if render_param_field(ui, field, &mut params) {
                                                dirty = true;
                                            }
                                        }
                                    });
                            }
                        });
                });

            // Texture display in central panel (only for image output)
            if is_texture && !egui_textures.is_empty() {
                egui::CentralPanel::default().show(ui, |ui| {
                    // Toolbar: channel tabs + view controls
                    ui.horizontal(|ui| {
                        for (i, (label, _)) in egui_textures.iter().enumerate() {
                            if ui.selectable_label(selected_tab == i, label).clicked() {
                                selected_tab = i;
                            }
                        }
                        ui.separator();
                        ui.checkbox(&mut texture_tiled, "Tiled");
                        if texture_tiled {
                            for n in [2usize, 3, 4] {
                                if ui
                                    .selectable_label(texture_tile_count == n, format!("{n}x{n}"))
                                    .clicked()
                                {
                                    texture_tile_count = n;
                                }
                            }
                        }
                        ui.separator();
                        if ui.button("\u{2212}").clicked() {
                            texture_zoom = (texture_zoom / 1.25).max(0.1);
                        }
                        ui.label(format!("{:.0}%", texture_zoom * 100.0));
                        if ui.button("+").clicked() {
                            texture_zoom = (texture_zoom * 1.25).min(20.0);
                        }
                        if ui.button("Fit").clicked() {
                            texture_zoom = 1.0;
                            texture_offset = [0.0, 0.0];
                        }
                    });
                    ui.separator();

                    // Display selected texture with zoom/pan/tiling
                    if let Some((_, handle)) = egui_textures.get(selected_tab) {
                        let tex_size = handle.size_vec2();
                        let available = ui.available_size();

                        // Fit scale: zoom 1.0 = content fits in viewport
                        let tiles_f = if texture_tiled {
                            texture_tile_count as f32
                        } else {
                            1.0
                        };
                        let fit_scale = (available.x / (tex_size.x * tiles_f))
                            .min(available.y / (tex_size.y * tiles_f));

                        // Interactive area for zoom/pan
                        let (response, painter) =
                            ui.allocate_painter(available, egui::Sense::click_and_drag());
                        let rect = response.rect;

                        // Scroll-to-zoom (centered on cursor)
                        if response.hovered() {
                            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                            if scroll != 0.0 {
                                let old_zoom = texture_zoom;
                                texture_zoom =
                                    (texture_zoom * (1.0 + scroll * 0.003)).clamp(0.1, 20.0);
                                if let Some(mouse_pos) = response.hover_pos() {
                                    let factor = texture_zoom / old_zoom;
                                    let rel = mouse_pos - rect.center();
                                    texture_offset[0] =
                                        texture_offset[0] * factor + rel.x * (1.0 - factor);
                                    texture_offset[1] =
                                        texture_offset[1] * factor + rel.y * (1.0 - factor);
                                }
                            }
                        }

                        // Drag-to-pan
                        if response.dragged() {
                            let delta = response.drag_delta();
                            texture_offset[0] += delta.x;
                            texture_offset[1] += delta.y;
                        }

                        // Double-click to reset
                        if response.double_clicked() {
                            texture_zoom = 1.0;
                            texture_offset = [0.0, 0.0];
                        }

                        // Calculate tile size and draw
                        let tile_px = egui::vec2(
                            tex_size.x * fit_scale * texture_zoom,
                            tex_size.y * fit_scale * texture_zoom,
                        );
                        let total = egui::vec2(tile_px.x * tiles_f, tile_px.y * tiles_f);
                        let origin = rect.center()
                            + egui::vec2(texture_offset[0], texture_offset[1])
                            - total * 0.5;

                        let uv =
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        let tiles_n = if texture_tiled { texture_tile_count } else { 1 };
                        for ty in 0..tiles_n {
                            for tx in 0..tiles_n {
                                let pos = origin
                                    + egui::vec2(tx as f32 * tile_px.x, ty as f32 * tile_px.y);
                                let tile_rect = egui::Rect::from_min_size(pos, tile_px);
                                painter.image(handle.id(), tile_rect, uv, egui::Color32::WHITE);
                            }
                        }
                    }
                });
            }

            // Top-right FPS overlay (only when in 3D viewport mode)
            if !is_texture {
                egui::Area::new(egui::Id::new("stats_overlay"))
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 10.0))
                    .show(ui, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(format!("FPS: {:.0}", fps));
                        });
                    });
            }
        }

        // Apply deferred mutations (after egui_winit borrow is released)
        if dirty {
            self.params = params;
            self.selected_lod = selected_lod;
            self.current_seed = current_seed;
            self.mark_dirty();
        }
        if selected_tab != self.selected_image_tab {
            self.selected_image_tab = selected_tab;
        }
        self.texture_zoom = texture_zoom;
        self.texture_offset = texture_offset;
        self.texture_tiled = texture_tiled;
        self.texture_tile_count = texture_tile_count;
        if want_randomize {
            self.current_seed = random_seed();
            self.mark_dirty();
        }
        if want_save {
            self.save_spec();
        }
        if want_save_as {
            self.save_spec_as();
        }
        if want_export {
            self.export_output();
        }
        if want_reset {
            self.reload_from_disk();
        }
    }

    fn reload_from_disk(&mut self) {
        match ProcGenSpec::from_file(&self.spec_path) {
            Ok(new_spec) => {
                if let toml::Value::Table(t) = &new_spec.params {
                    self.params = t.clone();
                }
                self.spec = new_spec;
                self.mark_dirty();
            }
            Err(e) => {
                tracing::warn!("Failed to reload spec from disk: {}", e);
            }
        }
    }
}

// ─── The host's editor (#712) ───────────────────────────────────────────────

impl Editor for GenPreviewApp {
    fn title(&self) -> String {
        format!("Flint Gen Preview \u{2014} {}", self.spec.meta.name)
    }

    fn init(&mut self, cx: InitCx) -> Result<()> {
        self.camera.aspect = cx.gpu.aspect();
        self.scene_renderer = Some(cx.gpu.scene_renderer(RendererConfig {
            show_grid: !self.args.no_grid,
            ..Default::default()
        }));
        self.egui_ctx = cx.egui.clone();
        self.window = cx.window;
        self.render_context = Some(cx.gpu.clone());
        // The first generation frames the camera, under the camera flags
        self.regenerate();
        Ok(())
    }

    fn orbit(&mut self) -> Option<(&mut Camera, &mut OrbitCameraController)> {
        Some((&mut self.camera, &mut self.orbit))
    }

    fn watched_files(&self) -> Vec<PathBuf> {
        vec![self.spec_path.clone()]
    }

    fn on_file_changed(&mut self) {
        self.check_file_reload();
    }

    fn resized(&mut self, gpu: &Gpu) {
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.resize_postprocess(&gpu.device, gpu.width, gpu.height);
        }
        self.render_context = Some(gpu.clone());
    }

    fn event(&mut self, event: &WindowEvent) -> Flow {
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return Flow::Pass;
        };
        if event.state != ElementState::Pressed {
            return Flow::Pass;
        }
        match event.physical_key {
            PhysicalKey::Code(KeyCode::Escape) => return Flow::Exit,
            PhysicalKey::Code(KeyCode::Tab) => {
                self.show_ui = !self.show_ui;
            }
            PhysicalKey::Code(KeyCode::KeyR) => {
                self.current_seed = random_seed();
                self.mark_dirty();
            }
            PhysicalKey::Code(KeyCode::KeyS)
                if !event.repeat && event.state == ElementState::Pressed =>
            {
                let mods = self.egui_ctx.input(|i| i.modifiers);
                if mods.command && mods.shift {
                    // Save As (Ctrl+Shift+S)
                    self.save_spec_as();
                } else if mods.command {
                    // Save spec (Ctrl+S)
                    self.save_spec();
                }
            }
            PhysicalKey::Code(KeyCode::KeyE)
                if !event.repeat && event.state == ElementState::Pressed =>
            {
                // Export output (Ctrl+E)
                if self.egui_ctx.input(|i| i.modifiers.command) {
                    self.export_output();
                }
            }
            PhysicalKey::Code(KeyCode::Space) => {
                // Reset camera / texture view
                if let Some(mesh) = &self.last_mesh {
                    let mesh_clone = mesh.clone();
                    self.auto_fit_to_mesh(&mesh_clone);
                }
                self.texture_zoom = 1.0;
                self.texture_offset = [0.0, 0.0];
            }
            PhysicalKey::Code(KeyCode::KeyT) => {
                if !self.last_images.is_empty() {
                    self.texture_tiled = !self.texture_tiled;
                }
            }
            PhysicalKey::Code(KeyCode::F1) => {
                if let Some(renderer) = &mut self.scene_renderer {
                    let next = renderer.debug_state().mode.next();
                    renderer.set_debug_mode(next);
                    println!("Debug mode: {}", next.label());
                    if let (Some(world), Some(context)) = (&self.last_world, &self.render_context) {
                        renderer.prepare(
                            &context.device,
                            &context.queue,
                            world,
                            &flint_render::FrameInputs::default(),
                        );
                    }
                }
            }
            PhysicalKey::Code(KeyCode::F2) => {
                if let Some(renderer) = &mut self.scene_renderer {
                    let mode = if renderer.debug_state().mode == DebugMode::WireframeOverlay {
                        DebugMode::Pbr
                    } else {
                        DebugMode::WireframeOverlay
                    };
                    renderer.set_debug_mode(mode);
                    println!("Debug mode: {}", mode.label());
                    if let (Some(world), Some(context)) = (&self.last_world, &self.render_context) {
                        renderer.prepare(
                            &context.device,
                            &context.queue,
                            world,
                            &flint_render::FrameInputs::default(),
                        );
                    }
                }
            }
            PhysicalKey::Code(KeyCode::F3) => {
                if let Some(renderer) = &mut self.scene_renderer {
                    let on = renderer.toggle_normal_arrows();
                    println!("Normal arrows: {}", if on { "ON" } else { "OFF" });
                    if let (Some(world), Some(context)) = (&self.last_world, &self.render_context) {
                        renderer.prepare(
                            &context.device,
                            &context.queue,
                            world,
                            &flint_render::FrameInputs::default(),
                        );
                    }
                }
            }
            PhysicalKey::Code(KeyCode::F4) => {
                if let Some(renderer) = &mut self.scene_renderer {
                    let on = renderer.toggle_skeleton_overlay();
                    println!("Skeleton overlay: {}", if on { "ON" } else { "OFF" });
                }
                self.update_skeleton_overlay();
            }
            _ => return Flow::Pass,
        }
        Flow::Handled
    }

    fn update(&mut self, tick: Tick) {
        self.fps = tick.fps;
        // Texture zoom via Q/E keys (same keys as 3D zoom)
        if !self.last_images.is_empty() {
            let zoom_speed = 2.0;
            if self.orbit.is_key_held(KeyCode::KeyE) {
                self.texture_zoom = (self.texture_zoom * (1.0 + zoom_speed * tick.dt)).min(20.0);
            }
            if self.orbit.is_key_held(KeyCode::KeyQ) {
                self.texture_zoom = (self.texture_zoom * (1.0 - zoom_speed * tick.dt)).max(0.1);
            }
        }
        // Debounced regeneration
        if self.should_regenerate() {
            self.regenerate();
        }
    }

    fn draw(&mut self, target: &Target) {
        match (&self.last_mesh, &mut self.scene_renderer) {
            (Some(_), Some(renderer)) => target.render(renderer, &self.camera),
            // Texture-only mode: dark grey behind the texture tabs
            _ => target.clear([0.12, 0.12, 0.14]),
        }
    }

    fn show_ui(&self) -> bool {
        self.show_ui
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.render_egui(ui);
    }

    /// A mesh is drawn as the window shows it; a texture generator's
    /// images are the still, side by side in the order the tabs list them.
    fn still(&mut self, _options: &StillOptions) -> Result<Option<image::RgbaImage>> {
        if self.last_mesh.is_none() && self.last_images.is_empty() {
            anyhow::bail!("'{}' generated nothing to draw", self.spec_path.display());
        }
        if self.last_images.is_empty() {
            return Ok(None);
        }
        let (width, height) = self
            .render_context
            .as_ref()
            .map_or((1, 1), |gpu| (gpu.width, gpu.height));
        let images: Vec<image::RgbaImage> = self
            .last_images
            .iter()
            .filter_map(|img| image::RgbaImage::from_raw(img.width, img.height, img.pixels.clone()))
            .collect();
        Ok(Some(host::image_sheet(
            &images,
            width,
            height,
            [31, 31, 36],
        )))
    }

    fn still_note(&self) -> Option<String> {
        (!self.last_images.is_empty()).then(|| {
            let names: Vec<String> = self
                .last_images
                .iter()
                .map(|img| format!("{:?}", img.channel_semantics))
                .collect();
            format!("images, left to right: {}", names.join(", "))
        })
    }
}

// ─── ECS helper ─────────────────────────────────────────────────────────────

/// Build a minimal ECS world with one entity referencing the "procgen_preview" mesh.
/// The asset a one-mesh [`build_mesh_world`] world draws.
pub(super) const PREVIEW_ASSET: &str = "procgen_preview";

pub(super) fn build_mesh_world(submesh_count: usize) -> FlintWorld {
    let mut world = FlintWorld::new();

    let count = submesh_count.max(1);
    for i in 0..count {
        let asset_name = if count == 1 {
            PREVIEW_ASSET.to_string()
        } else {
            format!("procgen_preview_{i}")
        };
        let entity_name = if count == 1 {
            PREVIEW_ASSET.to_string()
        } else {
            format!("procgen_preview_{i}")
        };

        let eid = world.spawn(&entity_name).expect("spawn entity");

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
        let _ = world.set_component(eid, comp::TRANSFORM, transform);

        let model = toml::Value::Table({
            let mut m = toml::map::Map::new();
            m.insert("asset".to_string(), toml::Value::String(asset_name));
            m
        });
        let _ = world.set_component(eid, comp::MODEL, model);
    }

    world
}

// ─── Parameter UI helpers (delegated to gen_preview_common) ─────────────────

use super::gen_preview_common::{format_count, group_fields, random_seed, render_param_field};

/// `params` minus any the generator would ignore under the current settings.
/// Unknown keys stay: the editor didn't write them.
fn params_to_save(
    schema: &serde_json::Value,
    params: &toml::map::Map<String, toml::Value>,
) -> toml::map::Map<String, toml::Value> {
    let mut params = params.clone();
    let table = toml::Value::Table(params.clone());
    for issue in flint_procgen::validate_params(schema, &table) {
        if let flint_procgen::ParamIssue::Inapplicable { key, .. } = issue {
            params.remove(&key);
        }
    }
    params
}

/// The text Ctrl+S writes: the file at `path` with only `[params]` and
/// `[seed]` patched to `spec`'s, so its comments, key order and formatting
/// survive (#194). A missing or broken file is written fresh from the spec.
fn spec_text_for_save(path: &Path, spec: &ProcGenSpec) -> Result<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let Ok(mut doc) = text.parse::<toml::Table>() else {
        return toml::to_string_pretty(spec).map_err(|e| e.to_string());
    };
    doc.insert("params".into(), spec.params.clone());
    let seed = toml::Value::try_from(&spec.seed).map_err(|e| e.to_string())?;
    doc.insert("seed".into(), seed);
    super::spec_save::patch_document(&text, &toml::Value::Table(doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #194's done-when, through the code Ctrl+S runs: open the commented
    /// example oak, move one slider, save; one line of the file changes.
    #[test]
    fn saving_one_slider_change_rewrites_one_line() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = root.join("specs/examples/oak_tree.procgen.toml");
        let dir = std::env::temp_dir().join(format!("flint-194-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("oak_tree.procgen.toml");
        std::fs::copy(&src, &path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let mut spec = ProcGenSpec::from_file(&path).unwrap();
        let registry = GeneratorRegistry::with_built_ins();
        let schema = registry.get(&spec.generator).unwrap().param_schema();
        let mut params = spec.params.as_table().unwrap().clone();

        // Saving untouched writes the file back byte for byte.
        spec.params = toml::Value::Table(params_to_save(&schema, &params));
        assert_eq!(spec_text_for_save(&path, &spec).unwrap(), before);

        params.insert("crown_radius".into(), toml::Value::Float(4.4));
        spec.params = toml::Value::Table(params_to_save(&schema, &params));
        let after = spec_text_for_save(&path, &spec).unwrap();
        let changed: Vec<_> = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(before.lines().count(), after.lines().count());
        assert_eq!(changed.len(), 1, "{changed:?}");
        assert_eq!(
            changed[0].1,
            "crown_radius = 4.4                       # how far the limbs reach from the trunk"
        );
        assert_eq!(ProcGenSpec::parse(&after).unwrap().params, spec.params);

        // The old save's output (toml::to_string_pretty), for comparison.
        if let Some(out) = flint_core::env::SAVE_194_DIR.path() {
            let out = out.as_path();
            std::fs::write(out.join("before.toml"), &before).unwrap();
            std::fs::write(out.join("after_new.toml"), &after).unwrap();
            std::fs::write(
                out.join("after_old.toml"),
                toml::to_string_pretty(&spec).unwrap(),
            )
            .unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
