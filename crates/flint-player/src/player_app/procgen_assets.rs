//! Procgen in the player's asset resolution (#32): **catalog > procgen > file**.
//!
//! At scene load every `model.asset` and `material`/`sprite` `texture` name
//! is resolved in that order. A catalog entry (`ModelLoadConfig::overrides`)
//! wins outright. A name with a matching `*.procgen.toml` (`meta.name`) is
//! generated through the [`ProcGenResolver`]'s cache and uploaded under that
//! name, *before* the file loader runs, so the file loader finds it cached
//! and leaves it alone. Anything else is the file loader's, as before.
//!
//! When procgen can't make an asset (an unregistered generator, a generator
//! error, an output the renderer can't draw) it warns and falls through to
//! the file. If there is no file either, a placeholder is uploaded under the
//! name and stays: a magenta box for a model, a 1x1 magenta texel for a
//! texture, so a missing asset is seen rather than silently absent.
//!
//! With [`ProcGenMode::Deferred`] nothing is generated at load: each asset
//! gets its placeholder at once and is queued on the resolver (Task 6.3's
//! frame-budgeted queue); [`apply_completed`] swaps the real asset in under
//! the same name as it finishes, and [`apply_failures`] falls back to the
//! file for the ones that fail.
//!
//! Everything here speaks to the renderer through [`AssetSink`], so the
//! resolution order is tested without a GPU; [`GpuSink`] is the real one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use flint_core::components as comp;
use flint_core::toml_util::toml_vec3;
use flint_core::vfs;
use flint_ecs::FlintWorld;
use flint_procgen::{
    AssetKind, ChannelSemantics, CompletedAsset, FailedAsset, GeneratorOutput, ImageData, MeshData,
    ProcGenResolver,
};
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::SceneRenderer;

/// When the player generates a scene's procgen assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcGenMode {
    /// Generate during scene load; the first frame shows the real assets.
    #[default]
    Immediate,
    /// Show placeholders at once and generate on the frame-budgeted queue.
    Deferred,
}

/// The magenta of "this asset is missing or still coming", in linear RGB.
pub const PLACEHOLDER_RGBA: [f32; 4] = [0.85, 0.05, 0.7, 1.0];
/// The 1x1 placeholder texel (sRGB bytes).
pub const PLACEHOLDER_TEXEL: [u8; 4] = [255, 0, 255, 255];

/// One asset name the scene asks for, with somewhere to stand for queue priority.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetRequest {
    pub name: String,
    pub kind: AssetKind,
    pub position: [f32; 3],
}

/// What became of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Generated now (a cache miss).
    Generated,
    /// Served from the resolver's cache, no generation.
    CacheHit,
    /// Placeholder shown; queued for generation.
    Deferred,
    /// Procgen couldn't make it (the reason); the file loader takes it.
    FileFallback(String),
    /// Procgen couldn't make it (the reason) and there is no file: placeholder for good.
    Placeholder(String),
}

/// Where the resolved assets go. The player's is [`GpuSink`].
pub trait AssetSink {
    /// A mesh is already uploaded under `name` (a previous scene, say).
    fn has_mesh(&self, name: &str) -> bool;
    /// The file loader would find a file for `name`.
    fn has_file(&self, name: &str, kind: AssetKind) -> bool;
    /// Upload (or replace) the mesh `name`.
    fn upload_mesh(&mut self, name: &str, mesh: &MeshData);
    /// Upload (or replace) the texture `name`.
    fn upload_image(&mut self, name: &str, image: &ImageData);
    /// Upload (or replace) the placeholder for `name`.
    fn upload_placeholder(&mut self, name: &str, kind: AssetKind);
    /// Drop what is under `name` and load its file instead. Returns whether it loaded.
    fn load_file(&mut self, name: &str, kind: AssetKind) -> bool;
}

/// The directories searched for `*.procgen.toml`, least specific first: the
/// resolver lets later directories override earlier ones, so a spec beside
/// the scene beats a game-wide one of the same name.
pub fn spec_dirs(scene_path: &str) -> Vec<PathBuf> {
    let scene_dir = Path::new(scene_path).parent().unwrap_or(Path::new("."));
    let mut dirs = Vec::new();
    if let Some(parent) = scene_dir.parent() {
        dirs.push(parent.join("specs"));
    }
    dirs.push(scene_dir.join("specs"));
    dirs.push(scene_dir.join("models"));
    dirs.into_iter().filter(|d| vfs::is_dir(d)).collect()
}

/// Index the scene's specs into `resolver`.
pub fn discover_specs(resolver: &mut ProcGenResolver, scene_path: &str) {
    let dirs = spec_dirs(scene_path);
    let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    resolver.discover_and_index(&refs);
}

/// The names in `world` that procgen should provide: those with a spec, not
/// in the catalog (`config.overrides`), and (for models) not already uploaded.
/// Each (name, kind) once, at the first entity that asks.
pub fn collect_requests(
    world: &FlintWorld,
    config: &ModelLoadConfig,
    resolver: &ProcGenResolver,
    sink: &dyn AssetSink,
) -> Vec<AssetRequest> {
    let mut seen: HashSet<(String, bool)> = HashSet::new();
    let mut requests = Vec::new();
    let mut want = |name: &str, kind: AssetKind, position: [f32; 3]| {
        if name.is_empty() || config.overrides.contains_key(name) || !resolver.has_spec(name) {
            return; // catalog wins; no spec is the file loader's
        }
        if kind == AssetKind::Model && sink.has_mesh(name) {
            return;
        }
        if seen.insert((name.to_string(), kind == AssetKind::Model)) {
            requests.push(AssetRequest {
                name: name.to_string(),
                kind,
                position,
            });
        }
    };

    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let position = comps
            .get(comp::TRANSFORM)
            .and_then(|t| t.get("position").and_then(toml_vec3))
            .unwrap_or([0.0; 3]);
        if let Some(model) = comps.get(comp::MODEL) {
            if !model_loader::is_runtime_model(model) {
                if let Some(name) = model.get("asset").and_then(|v| v.as_str()) {
                    want(name, AssetKind::Model, position);
                }
            }
        }
        for comp_name in [comp::MATERIAL, comp::SPRITE] {
            if let Some(tex) = comps
                .get(comp_name)
                .and_then(|c| c.get("texture"))
                .and_then(|v| v.as_str())
            {
                want(tex, AssetKind::Texture, position);
            }
        }
    }
    requests
}

/// Resolve each request through procgen, falling through to the file and
/// then to a placeholder. Returns each request's outcome, in order.
pub fn resolve(
    requests: &[AssetRequest],
    resolver: &mut ProcGenResolver,
    sink: &mut dyn AssetSink,
    mode: ProcGenMode,
) -> Vec<(AssetRequest, Outcome)> {
    let mut outcomes = Vec::with_capacity(requests.len());
    for req in requests {
        let outcome = resolve_one(req, resolver, sink, mode);
        log_outcome(req, &outcome);
        outcomes.push((req.clone(), outcome));
    }
    outcomes
}

fn resolve_one(
    req: &AssetRequest,
    resolver: &mut ProcGenResolver,
    sink: &mut dyn AssetSink,
    mode: ProcGenMode,
) -> Outcome {
    // A spec naming a generator that isn't registered can never succeed:
    // say so now rather than queue it.
    if let Err(reason) = resolver.check_generator(&req.name) {
        return fall_through(req, reason, sink);
    }

    let cached = resolver.is_cached(&req.name);
    if mode == ProcGenMode::Deferred && !cached {
        sink.upload_placeholder(&req.name, req.kind);
        resolver.enqueue(&req.name, req.position, req.kind);
        return Outcome::Deferred;
    }

    match resolver.generate(&req.name) {
        Ok(output) => match upload_output(&req.name, req.kind, &output, sink) {
            Ok(()) if cached => Outcome::CacheHit,
            Ok(()) => Outcome::Generated,
            Err(reason) => fall_through(req, reason, sink),
        },
        Err(e) => fall_through(req, e.to_string(), sink),
    }
}

/// Procgen gave up on `req`: the file loader takes it if it can, otherwise a
/// placeholder stays under its name.
fn fall_through(req: &AssetRequest, reason: String, sink: &mut dyn AssetSink) -> Outcome {
    if sink.has_file(&req.name, req.kind) {
        Outcome::FileFallback(reason)
    } else {
        sink.upload_placeholder(&req.name, req.kind);
        Outcome::Placeholder(reason)
    }
}

fn log_outcome(req: &AssetRequest, outcome: &Outcome) {
    let what = match req.kind {
        AssetKind::Model => "model",
        AssetKind::Texture => "texture",
    };
    match outcome {
        Outcome::Generated => tracing::info!("Procgen generated {what} '{}'", req.name),
        Outcome::CacheHit => tracing::info!("Procgen {what} '{}' from cache", req.name),
        Outcome::Deferred => tracing::info!("Procgen {what} '{}' queued (placeholder)", req.name),
        Outcome::FileFallback(reason) => tracing::warn!(
            "Procgen couldn't make {what} '{}' ({reason}); loading its file",
            req.name
        ),
        Outcome::Placeholder(reason) => tracing::warn!(
            "Procgen couldn't make {what} '{}' ({reason}) and there is no file; showing a placeholder",
            req.name
        ),
    }
}

/// The texture name each map of an image set goes under: the first (the
/// colour) under the spec's own name, the rest as `<name>_<map>`, which is
/// how the material path finds companion maps.
pub fn image_set_name(name: &str, index: usize, semantics: ChannelSemantics) -> String {
    if index == 0 {
        return name.to_string();
    }
    let suffix = match semantics {
        ChannelSemantics::Color => "color",
        ChannelSemantics::Normal => "normal",
        ChannelSemantics::Roughness => "roughness",
        ChannelSemantics::Metallic => "metallic",
        ChannelSemantics::Height => "height",
        ChannelSemantics::Mask => "mask",
    };
    format!("{name}_{suffix}")
}

/// Upload one generator output under `name`, or say why it can't be drawn.
pub fn upload_output(
    name: &str,
    kind: AssetKind,
    output: &GeneratorOutput,
    sink: &mut dyn AssetSink,
) -> Result<(), String> {
    match (output, kind) {
        (GeneratorOutput::Mesh(mesh), AssetKind::Model) => sink.upload_mesh(name, mesh),
        (GeneratorOutput::MeshWithLods(lods), AssetKind::Model) => match lods.first() {
            Some(mesh) => sink.upload_mesh(name, mesh),
            None => return Err("it produced no LODs".into()),
        },
        (GeneratorOutput::Image(image), _) => sink.upload_image(name, image),
        (GeneratorOutput::ImageSet(images), _) if !images.is_empty() => {
            for (i, image) in images.iter().enumerate() {
                sink.upload_image(&image_set_name(name, i, image.channel_semantics), image);
            }
        }
        (other, kind) => {
            return Err(format!(
                "a {} can't be drawn as a {}",
                output_label(other),
                match kind {
                    AssetKind::Model => "model",
                    AssetKind::Texture => "texture",
                }
            ))
        }
    }
    Ok(())
}

fn output_label(output: &GeneratorOutput) -> &'static str {
    match output {
        GeneratorOutput::Mesh(_) => "mesh",
        GeneratorOutput::MeshWithLods(_) => "LOD mesh",
        GeneratorOutput::Image(_) => "image",
        GeneratorOutput::ImageSet(_) => "image set",
        GeneratorOutput::SkinnedMesh(_) => "skinned mesh",
        GeneratorOutput::Sound(_) => "sound",
        #[allow(unreachable_patterns)]
        _ => "output",
    }
}

/// Swap queued assets in over their placeholders as they finish.
pub fn apply_completed(completed: &[CompletedAsset], sink: &mut dyn AssetSink) {
    for asset in completed {
        match upload_output(&asset.spec_name, asset.asset_kind, &asset.output, sink) {
            Ok(()) => tracing::info!("Procgen swapped in '{}'", asset.spec_name),
            Err(reason) => fail_queued(&asset.spec_name, asset.asset_kind, &reason, sink),
        }
    }
}

/// Queued assets that failed: load the file in place of the placeholder,
/// or keep the placeholder when there is none.
pub fn apply_failures(failures: &[FailedAsset], sink: &mut dyn AssetSink) {
    for failed in failures {
        fail_queued(&failed.spec_name, failed.asset_kind, &failed.error, sink);
    }
}

fn fail_queued(name: &str, kind: AssetKind, reason: &str, sink: &mut dyn AssetSink) {
    if sink.has_file(name, kind) && sink.load_file(name, kind) {
        tracing::warn!("Procgen couldn't make '{name}' ({reason}); loaded its file");
    } else {
        tracing::warn!("Procgen couldn't make '{name}' ({reason}); keeping the placeholder");
    }
}

// ── The renderer's sink ───────────────────────────────────────────────────

/// [`AssetSink`] over the scene renderer.
pub struct GpuSink<'a> {
    pub renderer: &'a mut SceneRenderer,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub config: &'a ModelLoadConfig,
}

impl GpuSink<'_> {
    fn material(mat: &flint_procgen::MaterialData) -> flint_import::ImportedMaterial {
        flint_import::ImportedMaterial {
            name: mat.name.clone(),
            base_color: mat.base_color,
            metallic: mat.metallic,
            roughness: mat.roughness,
            use_vertex_color: true,
            alpha_mode: flint_import::AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            ..Default::default()
        }
    }
}

impl AssetSink for GpuSink<'_> {
    fn has_mesh(&self, name: &str) -> bool {
        self.renderer.mesh_cache().contains(name)
    }

    fn has_file(&self, name: &str, kind: AssetKind) -> bool {
        match kind {
            AssetKind::Model => model_loader::resolve_model_path(self.config, name).is_some(),
            AssetKind::Texture => model_loader::resolve_texture_path(self.config, name).is_some(),
        }
    }

    fn upload_mesh(&mut self, name: &str, mesh: &MeshData) {
        let default_material = flint_procgen::MaterialData::default();
        // One part per sub-mesh (one for the whole mesh when it has none),
        // all under `name`: a tree's bark and leaves draw as one model.
        let whole = [flint_procgen::SubMesh {
            index_start: 0,
            index_count: mesh.indices.len() as u32,
            material_index: 0,
        }];
        let subs: &[flint_procgen::SubMesh] = if mesh.submeshes.is_empty() {
            &whole
        } else {
            &mesh.submeshes
        };

        let mut parts = Vec::with_capacity(subs.len());
        for (i, sub) in subs.iter().enumerate() {
            let mat = mesh
                .materials
                .get(sub.material_index)
                .unwrap_or(&default_material);
            let start = sub.index_start as usize;
            let end = (start + sub.index_count as usize).min(mesh.indices.len());
            let indices = &mesh.indices[start.min(end)..end];
            if indices.is_empty() {
                continue;
            }
            // Compact the vertex range this part touches.
            let lo = *indices.iter().min().unwrap() as usize;
            let hi = *indices.iter().max().unwrap() as usize;
            let vertices: Vec<flint_render::Vertex> = mesh.vertices[lo..=hi]
                .iter()
                .map(|v| flint_render::Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: mat.base_color,
                    uv: v.uv,
                })
                .collect();
            let indices: Vec<u32> = indices.iter().map(|&ix| ix - lo as u32).collect();

            let mut material = Self::material(mat);
            if let Some(normal) = &mat.normal_map {
                let tex = format!("{name}_part{i}_normal");
                self.upload_image(&tex, normal);
                material.normal_texture = Some(tex);
            }
            parts.push((vertices, indices, material));
        }
        self.renderer
            .load_procedural_mesh_parts(self.device, name, parts);
    }

    fn upload_image(&mut self, name: &str, image: &ImageData) {
        let is_normal = matches!(image.channel_semantics, ChannelSemantics::Normal);
        if let Err(e) = self.renderer.replace_texture_rgba(
            self.device,
            self.queue,
            name,
            image.width,
            image.height,
            &image.pixels,
            is_normal,
        ) {
            tracing::warn!("Failed to upload procgen texture '{name}': {e}");
        }
    }

    fn upload_placeholder(&mut self, name: &str, kind: AssetKind) {
        match kind {
            AssetKind::Model => {
                // A unit box standing on the entity's origin, as a model's base does.
                let mut box_mesh = flint_render::create_box_mesh(1.0, 1.0, 1.0, PLACEHOLDER_RGBA);
                for v in &mut box_mesh.vertices {
                    v.position[1] += 0.5;
                }
                let material = flint_import::ImportedMaterial {
                    name: format!("{name} (placeholder)"),
                    base_color: PLACEHOLDER_RGBA,
                    roughness: 0.6,
                    use_vertex_color: true,
                    ..Default::default()
                };
                self.renderer.load_procedural_mesh_parts(
                    self.device,
                    name,
                    vec![(box_mesh.vertices, box_mesh.indices, material)],
                );
            }
            AssetKind::Texture => {
                if let Err(e) = self.renderer.replace_texture_rgba(
                    self.device,
                    self.queue,
                    name,
                    1,
                    1,
                    &PLACEHOLDER_TEXEL,
                    false,
                ) {
                    tracing::warn!("Failed to upload placeholder texture '{name}': {e}");
                }
            }
        }
    }

    fn load_file(&mut self, name: &str, kind: AssetKind) -> bool {
        match kind {
            AssetKind::Model => {
                let Some(path) = model_loader::resolve_model_path(self.config, name) else {
                    return false;
                };
                match flint_import::import_gltf(&path) {
                    Ok(import) => {
                        self.renderer.remove_mesh(name);
                        self.renderer.mesh_cache_mut().upload_imported(
                            self.device,
                            name,
                            &import,
                            [0.7, 0.7, 0.7, 1.0],
                        );
                        true
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load '{}': {e}", path.display());
                        false
                    }
                }
            }
            AssetKind::Texture => {
                let Some(path) = model_loader::resolve_texture_path(self.config, name) else {
                    return false;
                };
                self.renderer.remove_texture(name);
                self.renderer
                    .load_texture_file(self.device, self.queue, name, &path)
                    .is_ok()
            }
        }
    }
}

/// Resolve the scene's procgen assets into the renderer: discover specs,
/// then catalog > procgen > file > placeholder for each name. Runs before
/// the model loader, which then skips every name this uploaded.
pub fn resolve_scene(
    scene_path: &str,
    world: &FlintWorld,
    resolver: &mut ProcGenResolver,
    mode: ProcGenMode,
    mut sink: GpuSink<'_>,
) {
    discover_specs(resolver, scene_path);
    let requests = collect_requests(world, sink.config, resolver, &sink);
    if requests.is_empty() {
        return;
    }
    resolve(&requests, resolver, &mut sink, mode);
    let stats = resolver.cache_stats();
    tracing::debug!(
        "Procgen cache: {} entries, {} of {} bytes, {} hits / {} misses, {} evicted",
        stats.entry_count,
        stats.total_bytes,
        stats.max_memory_bytes,
        stats.hits,
        stats.misses,
        stats.evictions,
    );
}

#[cfg(test)]
#[path = "procgen_assets_tests.rs"]
mod tests;
