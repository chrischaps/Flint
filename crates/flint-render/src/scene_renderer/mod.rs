//! Scene renderer - converts FlintWorld entities to GPU meshes

mod extract;
mod grass_ground;
mod helpers;
mod particles;
mod render_passes;
mod selection;

pub use grass_ground::{load_grass_ground_in, load_scene_grass_ground, LoadedGrassGround};
pub use particles::load_scene_particle_textures;
use render_passes::RenderPhase;

use helpers::{identity_matrix, mat4_inv_transpose};

use crate::billboard_pipeline::BillboardPipeline;
use crate::bind_cache::{ArenaSlot, TransformArena};
use crate::bitmap_font::BitmapFont;
use crate::camera::Camera;
use crate::context::RenderContext;
use crate::debug::{DebugMode, DebugState};
use crate::gpu_mesh::MeshCache;
use crate::grass_pipeline::{
    check_grass_mask_patch, GrassComputeUniforms, GrassCountReadback, GrassDrawArgs,
    GrassEntityPosition, GrassGround, GrassHeight, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, MAX_GRASS_ENTITIES,
};
use crate::particle_pipeline::{ParticleDrawCall, ParticlePipeline};
use crate::pipeline::{
    BlendMode, DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline,
    SpotLight, TransformUniforms, MAX_DIRECTIONAL_LIGHTS, MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS,
};
use crate::postprocess::{
    PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT,
};
use crate::primitives::create_grid_mesh;
use crate::shadow::{ShadowPass, DEFAULT_SHADOW_RESOLUTION};
use crate::skinned_pipeline::SkinnedPipeline;
use crate::skybox_pipeline::{SkyboxPipeline, SkyboxUniforms};
use crate::sprite2d_pipeline::{Sprite2dInstanceGpu, Sprite2dPipeline};
use crate::terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
use crate::texture_cache::{open_image, TextureCache};
use crate::wooze_pipeline::{
    grow_bounds, ShallowWoozeDraw, ShallowWoozeUniforms, WoozePipelineSet,
};
use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::vfs;
use flint_core::{Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_import::ImportResult;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// A scope and the `scene_dir` it was made for.
type AssetScopeFor = (
    Option<std::path::PathBuf>,
    Arc<flint_core::project::AssetScope>,
);

/// Default main-pass clear colour (linear RGBA), matching the historical
/// dark slate background.
pub const DEFAULT_CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.15, 1.0];

/// Visual representation for an archetype
#[derive(Clone)]
pub struct ArchetypeVisual {
    pub color: [f32; 4],
    pub wireframe: bool,
    pub default_size: [f32; 3],
}

impl Default for ArchetypeVisual {
    fn default() -> Self {
        Self {
            color: [0.5, 0.5, 0.5, 1.0],
            wireframe: false,
            default_size: [1.0, 1.0, 1.0],
        }
    }
}

/// A single draw call with its own GPU resources
#[allow(dead_code)]
pub(super) struct DrawCall {
    // GPU handles are `Arc`s so the per-entity caches below can hand the
    // same buffers and bind groups to every frame's draw list (wgpu 23
    // resources are not `Clone`).
    vertex_buffer: Arc<wgpu::Buffer>,
    index_buffer: Arc<wgpu::Buffer>,
    index_count: u32,
    is_wireframe: bool,
    /// Whether the draw goes into the shadow cascades: the entity's
    /// `material.cast_shadows`, true unless it says otherwise (ADR 0100).
    casts_shadow: bool,
    transform: TransformTarget,
    transform_bind_group: Arc<wgpu::BindGroup>,
    material_buffer: Arc<MaterialBuffer>, // kept alive for bind group
    material_bind_group: Arc<wgpu::BindGroup>,
    model: [[f32; 4]; 4],
    model_inv_transpose: [[f32; 4]; 4],
    entity_id: Option<flint_core::EntityId>,
    blend_mode: BlendMode,
    sort_depth: f32,
    /// World-space bounds for frustum culling. `None` draws are never culled.
    world_aabb: Option<([f32; 3], [f32; 3])>,
    /// World-space bounds of a draw that isn't culled by them (a
    /// procedural box), for the per-entity style scissor (#385): it reads
    /// `world_aabb`, else these.
    style_aabb: Option<([f32; 3], [f32; 3])>,
    /// The camera distances this draw is drawn at: its mesh's level of detail
    /// and the entity's `lod.draw_distance`. `None` draws at every distance.
    lod: Option<DrawLod>,
    /// Its drift this frame, if its entity has shallow wooze (#215).
    wooze: Option<ShallowWoozeUniforms>,
    /// Drawn with no face culled, in the scene and shadow passes: its
    /// material is glTF `doubleSided` (#623).
    double_sided: bool,
}

/// A draw's level of detail in world space: the band of camera distances it
/// draws in, measured to the closest point of `ref_min..ref_max` (the box
/// every level of its group shares, so the levels hand over cleanly).
#[derive(Clone, Copy, Debug)]
pub(super) struct DrawLod {
    pub band: flint_import::LodBand,
    pub ref_min: [f32; 3],
    pub ref_max: [f32; 3],
}

impl DrawLod {
    /// Whether a camera at `eye` draws it, under the renderer's scales.
    pub fn visible_from(&self, eye: [f32; 3], lod_scale: f32, draw_scale: f32) -> bool {
        let d = distance_to_box(eye, self.ref_min, self.ref_max);
        self.band.visible_at(d, lod_scale, draw_scale)
    }
}

/// Distance from `p` to the closest point of the box (0 inside it).
pub(super) fn distance_to_box(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    let mut d2 = 0.0;
    for i in 0..3 {
        let v = (min[i] - p[i]).max(0.0).max(p[i] - max[i]);
        d2 += v * v;
    }
    d2.sqrt()
}

/// The renderer's distance levers (#56). `lod_scale` multiplies every level
/// switch distance (below 1 a model drops to its simpler levels sooner);
/// `draw_distance_scale` multiplies every draw distance (below 1 far things
/// stop drawing sooner). Both start at 1, or at `FLINT_LOD_SCALE` /
/// `FLINT_DRAW_DISTANCE_SCALE`, and quality settings change them at runtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodSettings {
    pub lod_scale: f32,
    pub draw_distance_scale: f32,
}

impl Default for LodSettings {
    fn default() -> Self {
        Self {
            lod_scale: 1.0,
            draw_distance_scale: 1.0,
        }
    }
}

impl LodSettings {
    /// The smallest scale accepted: zero would draw only the last level and
    /// nothing with a draw distance.
    pub const MIN_SCALE: f32 = 0.05;

    /// The defaults, overridden by `FLINT_LOD_SCALE` and
    /// `FLINT_DRAW_DISTANCE_SCALE` when they hold a number.
    pub fn from_env() -> Self {
        let read = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .map(|v| v.max(Self::MIN_SCALE))
        };
        let mut s = Self::default();
        if let Some(v) = read("FLINT_LOD_SCALE") {
            s.lod_scale = v;
        }
        if let Some(v) = read("FLINT_DRAW_DISTANCE_SCALE") {
            s.draw_distance_scale = v;
        }
        s
    }
}

/// Transform + material GPU resources of one draw of one entity, kept
/// across frames. `update_from_world` used to rebuild every draw call from
/// scratch each frame (buffers, bind groups, even a private copy of the
/// vertex data), which cost more CPU than the draws themselves once a scene
/// had a few hundred entities. The transform buffer is rewritten every frame
/// by `update_per_frame_uniforms` anyway; the material side is rebuilt only
/// when `material_key` (uniforms + texture identity) changes.
pub(super) struct CachedDrawBinds {
    /// Its slot in the cache's transform arena (#139).
    pub transform: TransformTarget,
    pub transform_bind_group: Arc<wgpu::BindGroup>,
    pub material_buffer: Arc<MaterialBuffer>,
    pub material_bind_group: Arc<wgpu::BindGroup>,
    pub material_key: u64,
    /// `extract_frame` of the last frame that used this entry; stale entries
    /// (despawned entities, swapped models) are evicted after extraction.
    pub touched: u64,
}

/// One material's uniform buffer and bind group, shared by every draw whose
/// `material_key` matches. A city is thousands of entities but only a few
/// hundred looks. Sharing began as the fix for wgpu 23's DX12 sampler heap
/// (2048 slots, every bind group's samplers copied in), which one bind group
/// per entity ran dry at about 512 entities (Capitol #525). Since #577 a
/// material bind group holds no samplers at all (they share the light
/// group's `material_sampler`), so the number of looks has no DX12 cap;
/// sharing still saves a buffer and a bind group per entity. The buffer's
/// per-frame fields (debug mode, tonemapping) are the same for every draw,
/// so sharing it is safe.
pub(super) struct SharedMaterial {
    pub buffer: Arc<MaterialBuffer>,
    pub bind_group: Arc<wgpu::BindGroup>,
    pub touched: u64,
}

/// The per-draw binds and the shared materials they point at.
#[derive(Default)]
pub(super) struct DrawBindCache {
    pub draws: HashMap<(flint_core::EntityId, u32), CachedDrawBinds>,
    pub materials: HashMap<u64, SharedMaterial>,
    /// The draws' transform uniforms, a slot each (#139).
    pub arena: TransformArena,
}

impl DrawBindCache {
    pub fn clear(&mut self) {
        for (_, entry) in self.draws.drain() {
            entry.transform.free_in(&mut self.arena);
        }
        self.materials.clear();
    }

    /// Drop the entries and materials no draw used in `frame`.
    pub fn retain_frame(&mut self, frame: u64) {
        let arena = &mut self.arena;
        self.draws.retain(|_, c| {
            let keep = c.touched == frame;
            if !keep {
                c.transform.free_in(arena);
            }
            keep
        });
        // Kept a little past their last draw: a living prop's leaves flick
        // between a few shared tints (#212), which shouldn't remake a buffer and
        // a bind group each time one goes unused for a frame.
        self.materials
            .retain(|_, m| frame.saturating_sub(m.touched) < MATERIAL_GRACE_FRAMES);
    }
}

/// Where a draw's transform uniforms are written each frame.
#[derive(Clone)]
pub(super) enum TransformTarget {
    /// A buffer of the draw's own: draws made fresh each frame (the grid,
    /// debug overlays).
    Own(Arc<wgpu::Buffer>),
    /// A slot in the draw bind cache's arena, uploaded a page at a time
    /// instead of a write per draw (#139).
    Slot(ArenaSlot),
}

impl TransformTarget {
    /// Write `uniforms` for this frame: at once into a buffer of the draw's
    /// own, or staged in `arena` for its page's one upload.
    fn write(&self, queue: &wgpu::Queue, arena: &mut TransformArena, uniforms: &TransformUniforms) {
        match self {
            TransformTarget::Own(buffer) => {
                queue.write_buffer(buffer, 0, bytemuck::bytes_of(uniforms));
            }
            TransformTarget::Slot(slot) => arena.stage(*slot, bytemuck::bytes_of(uniforms)),
        }
    }

    /// Put shallow wooze's drift in the rest of the slot (#215). Only cached draws
    /// have the room, and only they can have drift.
    fn write_wooze(&self, arena: &mut TransformArena, wooze: &ShallowWoozeUniforms) {
        if let TransformTarget::Slot(slot) = self {
            arena.stage_at(
                *slot,
                std::mem::size_of::<TransformUniforms>(),
                bytemuck::bytes_of(wooze),
            );
        }
    }

    fn free_in(&self, arena: &mut TransformArena) {
        if let TransformTarget::Slot(slot) = self {
            arena.free(*slot);
        }
    }
}

/// A material's uniform buffer, and the per-frame flags (debug mode,
/// tonemapping) last written into it. They change only when a debug view
/// or the HDR chain does, so they're written then and not every frame
/// (#139).
pub(super) struct MaterialBuffer {
    buffer: wgpu::Buffer,
    /// `debug_mode << 32 | enable_tonemapping` as last written; `u64::MAX`
    /// before the first frame.
    flags: std::sync::atomic::AtomicU64,
}

impl MaterialBuffer {
    fn new(buffer: wgpu::Buffer) -> Self {
        Self {
            buffer,
            flags: std::sync::atomic::AtomicU64::new(u64::MAX),
        }
    }

    /// Write `debug_mode` and `enable_tonemapping` (byte offsets 28 and 32:
    /// base_color(16) + metallic(4) + roughness(4) + use_vertex_color(4))
    /// when they differ from what's there.
    fn write_flags(&self, queue: &wgpu::Queue, debug_mode: u32, tonemapping: u32) {
        let want = (debug_mode as u64) << 32 | tonemapping as u64;
        if self.flags.swap(want, std::sync::atomic::Ordering::Relaxed) != want {
            queue.write_buffer(
                &self.buffer,
                28,
                bytemuck::cast_slice(&[debug_mode, tonemapping]),
            );
        }
    }
}

impl std::ops::Deref for MaterialBuffer {
    type Target = wgpu::Buffer;

    fn deref(&self) -> &wgpu::Buffer {
        &self.buffer
    }
}

/// One uploaded procedural box, shared by every `bounds` entity with the
/// same size/colour/wireframe (a tiled park is mostly identical boxes).
pub(super) struct CachedBoxMesh {
    pub vertex_buffer: Arc<wgpu::Buffer>,
    pub index_buffer: Arc<wgpu::Buffer>,
    pub index_count: u32,
}

/// Whether `draw` may be inside `frustum`. Draws without bounds always are.
pub(super) fn draw_visible(draw: &DrawCall, frustum: &crate::frustum::Frustum) -> bool {
    draw.world_aabb
        .map_or(true, |(min, max)| frustum.aabb_visible(min, max))
}

/// The bounds of a skinned mesh whose bind-pose box is `min..max` once
/// posed by `bones` (#385). A skinned vertex is a weighted blend of its
/// position under up to four bone matrices; with weights summing to one
/// the blend lies in the box around every bone's image of the bind-pose
/// box. The bind-pose box itself (identity bones) and the origin (a bone
/// slot never written reads as zero) are in too, so the bound holds
/// whichever slots a vertex names.
pub(super) fn posed_aabb(
    bones: &[[[f32; 4]; 4]],
    min: [f32; 3],
    max: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for i in 0..3 {
        lo[i] = min[i].min(0.0);
        hi[i] = max[i].max(0.0);
    }
    for bone in bones {
        let (a, b) = transform_aabb(bone, min, max);
        for i in 0..3 {
            lo[i] = lo[i].min(a[i]);
            hi[i] = hi[i].max(b[i]);
        }
    }
    (lo, hi)
}

/// The world-space AABB enclosing a local AABB under `model` (column-major),
/// by Arvo's method: exact for the transformed box, never too small.
pub(super) fn transform_aabb(
    model: &[[f32; 4]; 4],
    min: [f32; 3],
    max: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let mut out_min = [model[3][0], model[3][1], model[3][2]];
    let mut out_max = out_min;
    for row in 0..3 {
        for col in 0..3 {
            let a = model[col][row] * min[col];
            let b = model[col][row] * max[col];
            out_min[row] += a.min(b);
            out_max[row] += a.max(b);
        }
    }
    (out_min, out_max)
}

/// FNV-1a over a few byte slices: a cheap, stable key for the draw caches.
pub(super) fn hash_parts(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.iter() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// One skinned draw's bone bind group, kept across frames like
/// [`CachedDrawBinds`]. It points at the entity's own bone buffer once
/// `update_bone_matrices` has made one, and at the asset's shared buffer
/// before that; `from_entity` records which, so the switch rebuilds it.
pub(super) struct CachedBoneBind {
    pub from_entity: bool,
    pub bind_group: Arc<wgpu::BindGroup>,
    pub touched: u64,
}

/// Write `matrices` into a bone buffer from bone `first`, stopping at its end.
fn write_bones(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    first: usize,
    matrices: &[[[f32; 4]; 4]],
) {
    let room = crate::skinned_pipeline::MAX_BONES.saturating_sub(first);
    let matrices = &matrices[..matrices.len().min(room)];
    if !matrices.is_empty() {
        queue.write_buffer(
            buffer,
            (first * std::mem::size_of::<[[f32; 4]; 4]>()) as u64,
            bytemuck::cast_slice(matrices),
        );
    }
}

/// How long a material nothing draws with stays in the cache: about 2 s.
const MATERIAL_GRACE_FRAMES: u64 = 120;

/// The `skinned_bone_binds` key bit for a draw's shadow bones, when it has its
/// own apart from the bones it's drawn with.
pub(super) const SHADOW_BONE_KEY: u32 = 1 << 30;

/// The `draw_bind_cache` key bit that keeps a skinned draw's binds apart
/// from a static draw's of the same entity and sub-mesh index.
pub(super) const SKINNED_DRAW_KEY: u32 = 1 << 31;

/// A draw call for a skinned mesh (has bone bind group). Everything in it
/// is shared or cached: until #121 each frame made each skinned sub-mesh
/// four buffers (two of them copies of the mesh) and three bind groups,
/// which Firefox's WebGPU paid for in garbage collection.
#[allow(dead_code)]
struct SkinnedDrawCall {
    vertex_buffer: Arc<wgpu::Buffer>,
    index_buffer: Arc<wgpu::Buffer>,
    index_count: u32,
    transform: TransformTarget,
    transform_bind_group: Arc<wgpu::BindGroup>,
    material_buffer: Arc<MaterialBuffer>,
    material_bind_group: Arc<wgpu::BindGroup>,
    bone_bind_group: Arc<wgpu::BindGroup>,
    /// What the shadow pass binds: `bone_bind_group`, unless the entity has
    /// shadow bones of its own (`write_entity_shadow_bones`).
    shadow_bone_bind_group: Arc<wgpu::BindGroup>,
    /// Unique-edge index buffer for wireframe debug modes (shares the
    /// skinned vertex buffer, so the lines follow the animated pose).
    wire_index_buffer: Option<wgpu::Buffer>,
    wire_index_count: u32,
    model: [[f32; 4]; 4],
    model_inv_transpose: [[f32; 4]; 4],
    entity_id: Option<flint_core::EntityId>,
    blend_mode: BlendMode,
    sort_depth: f32,
    /// As [`DrawCall::casts_shadow`].
    casts_shadow: bool,
    /// World-space bounds of the posed mesh (#385): its bind-pose box
    /// under every bone matrix, which holds any blend of them. `None`
    /// when the bones aren't known on the CPU.
    world_aabb: Option<([f32; 3], [f32; 3])>,
    /// As [`DrawCall::wooze`].
    wooze: Option<ShallowWoozeUniforms>,
    /// As [`DrawCall::double_sided`].
    double_sided: bool,
    /// Translucent only for its soft edge (#582): it still casts a whole
    /// shadow, since softness is how the outline looks to the eye, not how
    /// much light the body stops.
    soft_edge_only: bool,
}

impl SceneRenderer {
    /// The skinned draws that go into the shadow map, in the order every
    /// shadow loop walks them: the opaque ones that cast, then the translucent
    /// ones that are translucent only for their softness (#582).
    /// Over the two lists, not `self`, so the shadow pass can hold its own
    /// borrow of the renderer's shadow state.
    fn skinned_shadow_draws<'a>(
        opaque: &'a [SkinnedDrawCall],
        translucent: &'a [SkinnedDrawCall],
    ) -> impl Iterator<Item = &'a SkinnedDrawCall> + Clone {
        let opaque = opaque.iter().filter(|d| d.casts_shadow);
        let soft = (translucent.iter()).filter(|d| d.casts_shadow && d.soft_edge_only);
        opaque.chain(soft)
    }
}

/// One faint copy of a skinned entity (#228): bones of its own and how opaque
/// it draws. The rest it takes from the entity's own draw at extraction.
struct EntityEcho {
    bones: wgpu::Buffer,
    bind_group: Arc<wgpu::BindGroup>,
    opacity: f32,
}

/// A draw of one part of one echo. It reads the entity's transform slot rather
/// than having one of its own, and a material shared by content.
struct EchoDrawCall {
    vertex_buffer: Arc<wgpu::Buffer>,
    index_buffer: Arc<wgpu::Buffer>,
    index_count: u32,
    transform_bind_group: Arc<wgpu::BindGroup>,
    material_buffer: Arc<MaterialBuffer>,
    material_bind_group: Arc<wgpu::BindGroup>,
    bone_bind_group: Arc<wgpu::BindGroup>,
    /// Whether it may only add light: its entity's `[living] echo_blend`.
    lighten: bool,
    /// Where the entity stands, since all its echoes sort as one.
    origin: [f32; 3],
    /// Which echo: they draw in this order, so farthest from now first.
    echo: usize,
    sort_depth: f32,
}

/// Configuration for creating a SceneRenderer
pub struct RendererConfig {
    /// Show the ground-plane grid (useful for debug/inspection modes)
    pub show_grid: bool,
    /// MSAA sample count for the scene passes: 1 (off, the default) or 4
    /// (ADR 0058). Anything else is clamped to 1 with a warning. Post/
    /// shadow/blit passes stay single-sample; depth consumers read a
    /// sample-0 resolve.
    pub sample_count: u32,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            show_grid: false,
            sample_count: 1,
        }
    }
}

/// Snapshot of the runtime lighting levers (ambient hemisphere, diffuse
/// wrap, Oren-Nayar blend, sheen). Values are the active overrides when set,
/// else the built-in neutral defaults — suitable for seeding UI controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightingLevers {
    /// Hemisphere ambient, sky half (linear RGB)
    pub ambient_sky: [f32; 3],
    /// Hemisphere ambient, ground half (linear RGB)
    pub ambient_ground: [f32; 3],
    /// Diffuse terminator wrap (0 = physically sharp / legacy)
    pub diffuse_wrap: f32,
    /// Lambert → Oren-Nayar blend (0 = exact legacy shading)
    pub oren_nayar: f32,
    /// Charlie-sheen rim tint (linear RGB)
    pub sheen_color: [f32; 3],
    /// Sheen strength, 0..~0.3 (0 = exact legacy shading)
    pub sheen_strength: f32,
}

/// Renders a FlintWorld to the screen
pub struct SceneRenderer {
    pipeline: RenderPipeline,
    skinned_pipeline: Option<SkinnedPipeline>,
    /// Shallow wooze (#215): each entity's drift this frame, from the caller, and
    /// the pipelines its draws use, built when the first one needs them.
    shallow_wooze: HashMap<flint_core::EntityId, ShallowWoozeDraw>,
    wooze_standard: Option<WoozePipelineSet>,
    wooze_skinned: Option<WoozePipelineSet>,
    billboard_pipeline: Option<BillboardPipeline>,
    archetype_visuals: HashMap<String, ArchetypeVisual>,
    mesh_cache: MeshCache,
    /// Per-(entity, sub-mesh) transform/material resources reused across
    /// frames; see `CachedDrawBinds`.
    draw_bind_cache: DrawBindCache,
    /// Uploaded procedural boxes keyed by size/colour/wireframe.
    box_mesh_cache: HashMap<u64, CachedBoxMesh>,
    /// Bumped once per `update_from_world`; stamps cache entries.
    extract_frame: u64,
    /// Runtime meshes uploaded as ray targets, by name (ADR 0105).
    ray_meshes: HashMap<String, Arc<flint_core::ray_mesh::RayMesh>>,
    /// Where the ray targets stood at the last `update_from_world`.
    ray_scene: Arc<flint_core::ray_mesh::RayScene>,
    grid_draw: Option<DrawCall>,
    entity_draws: Vec<DrawCall>,
    skinned_entity_draws: Vec<SkinnedDrawCall>,
    transparent_draws: Vec<DrawCall>,
    transparent_skinned_draws: Vec<SkinnedDrawCall>,
    echo_draws: Vec<EchoDrawCall>,
    /// Draws outside the camera frustum in the last `render_to`, set aside
    /// from `entity_draws` / `transparent_draws` after the shadow pass.
    culled_entity_draws: Vec<DrawCall>,
    culled_transparent_draws: Vec<DrawCall>,
    billboard_draws: Vec<crate::billboard_pipeline::BillboardDrawCall>,
    debug_state: DebugState,
    wireframe_overlay_draws: Vec<DrawCall>,
    normal_arrow_draws: Vec<DrawCall>,
    skeleton_overlay_draws: Vec<DrawCall>,
    tonemapping_enabled: bool,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    light_uniforms: LightUniforms,
    /// Scene-authored hemisphere ambient override (sky, ground). None keeps
    /// the built-in default; stored so the zero-light fallback in
    /// `extract_lights_from_world` cannot stomp it.
    ambient_override: Option<([f32; 3], [f32; 3])>,
    /// The scene's `[environment] shadows` (#627): false keeps the shadow
    /// pass from drawing whatever the quality tier or F4 say. True unless
    /// a scene says otherwise.
    scene_shadows: bool,
    /// The scene's `[environment] default_lights` (#627): false lights a
    /// scene with no light entities by its ambient alone, instead of the
    /// built-in key and fill. True unless a scene says otherwise.
    default_lights: bool,
    /// Scene-authored diffuse terminator wrap. Rides ambient_sky.w encoded as
    /// (1 + wrap) so legacy writes of 1.0 decode to wrap = 0 in the shader.
    diffuse_wrap_override: Option<f32>,
    oren_nayar_override: Option<f32>,
    sheen_override: Option<([f32; 3], f32)>,
    texture_cache: Option<TextureCache>,
    shadow_pass: Option<ShadowPass>,
    selected_entity: Option<flint_core::EntityId>,
    /// Each entity's children, as the last `update_from_world` saw them,
    /// so the selection outline covers a selected entity's subtree (#602)
    entity_children: HashMap<flint_core::EntityId, Vec<flint_core::EntityId>>,
    /// The selection outline's pipelines and mask, made the first frame
    /// something is selected (#602)
    selection_outline: Option<crate::selection_outline::SelectionOutline>,
    // Skybox
    skybox_pipeline: Option<SkyboxPipeline>,
    skybox_uniform_buffer: Option<wgpu::Buffer>,
    skybox_uniform_bind_group: Option<wgpu::BindGroup>,
    skybox_texture_bind_group: Option<wgpu::BindGroup>,
    // Terrain
    terrain_pipeline: Option<TerrainPipeline>,
    terrain_draws: Vec<TerrainDrawCall>,
    terrain_material_bind_group: Option<wgpu::BindGroup>,
    terrain_material_buffer: Option<wgpu::Buffer>,
    terrain_total_chunks: u32,
    terrain_visible_chunks: u32,
    /// The entity the loaded terrain belongs to, so a `style_mask` on it
    /// masks the chunks (#310). Set by [`SceneRenderer::set_terrain_entity`].
    terrain_entity: Option<flint_core::EntityId>,
    camera_frustum: Option<crate::frustum::Frustum>,
    /// Level-of-detail and draw-distance scales (#56)
    lod_settings: LodSettings,
    /// Draws the last `render_to` set aside by level of detail
    lod_culled: u32,
    // Grass
    grass_pipeline: Option<GrassPipeline>,
    grass_instance_buffer: Option<wgpu::Buffer>,
    /// Blades drawn, for stats: read back without waiting, so a frame or
    /// more behind (ADR 0092). The draws themselves read the GPU's count.
    grass_instance_count: u32,
    /// The grass compute ran for this render_to, so the draws have arguments
    grass_drawing: bool,
    grass_max_instances: u32,
    grass_draw_args_buffer: Option<wgpu::Buffer>,
    grass_count_readback: Option<GrassCountReadback>,
    /// The frame being encoded dispatched the grass: read its count back
    /// once it is submitted (`frame_submitted`).
    grass_readback_due: bool,
    /// The grass's view of each shadow cascade it casts into: a buffer and
    /// its bind group, kept across frames (#139).
    grass_shadow_binds: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Times the frame's phases on the GPU, while timing is on (ADR 0108)
    gpu_timer: Option<crate::gpu_timer::GpuTimer>,
    /// The entity style mask's pass (#263); `None` until a frame needs it.
    style_mask_pass: Option<crate::style_mask::StyleMaskPass>,
    /// Style names entities ask for, numbered for the mask.
    style_ids: crate::style_mask::StyleIds,
    /// Styled entities -> their mask id, rebuilt each `update_from_world`
    /// while the mask is wanted (empty otherwise).
    style_by_entity: HashMap<flint_core::EntityId, u16>,
    /// Whether per-entity styles scissor their passes to their entities'
    /// screen bounds (#385); `FLINT_STYLE_SCISSOR=0` starts it off.
    style_scissor: bool,
    /// The blend of each styled entity drawn at less than 1 (#278).
    style_blend_by_entity: HashMap<flint_core::EntityId, f32>,
    /// Whether the last style mask drew a veil over its entities (#290):
    /// something unstyled in front, so the mask's blend is below 1 there
    /// even with every entity fully styled (#396 cross-fades by it).
    style_mask_veiled: bool,
    /// The `style` debug view switched `mask_view` on, so it switches it off.
    mask_view_forced: bool,
    /// Whether the `edges` view switched ASCII on itself (#349).
    ascii_forced: bool,
    grass_compute_uniform_buffer: Option<wgpu::Buffer>,
    grass_compute_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_compute_texture_bind_group: Option<wgpu::BindGroup>,
    grass_compute_storage_bind_group: Option<wgpu::BindGroup>,
    /// Each placement workgroup's blade count, then its first slot, and how
    /// many workgroups it holds (#341). Grown by the dispatch that needs more.
    grass_group_slots: Option<(wgpu::Buffer, u32)>,
    grass_render_uniform_buffer: Option<wgpu::Buffer>,
    grass_render_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_render_instance_bind_group: Option<wgpu::BindGroup>,
    grass_entity_buffer: Option<wgpu::Buffer>,
    grass_entity_count: u32,
    grass_config: Option<flint_terrain::GrassConfig>,
    grass_terrain_offset: [f32; 3],
    grass_terrain_width: f32,
    grass_terrain_depth: f32,
    grass_terrain_height_scale: f32,
    /// Grass grows on a game's ground (ADR 0089), not a terrain's splat map.
    grass_on_ground: bool,
    /// A mask weight thins the field in proportion (ground only).
    grass_scale_by_density: bool,
    /// The ground's mask and its size in texels, kept so part of it can be
    /// repainted in place (ADR 0093). `None` for a terrain's grass.
    grass_mask: Option<(wgpu::Texture, u32, u32)>,
    /// The config grew past the instance buffer; reallocate before the next dispatch.
    grass_needs_realloc: bool,
    /// The grass draw's own view uniforms, so it needs no terrain to borrow them from.
    grass_transform_buffer: Option<wgpu::Buffer>,
    grass_transform_bind_group: Option<wgpu::BindGroup>,
    // Procedural sky (replaces the texture skybox when a `sky` component exists)
    sky_pipeline: Option<crate::sky_pipeline::SkyPipeline>,
    sky_uniform_buffer: Option<wgpu::Buffer>,
    sky_uniform_bind_group: Option<wgpu::BindGroup>,
    sky_params: crate::sky_pipeline::SkyParams,
    sky_active: bool,
    // Ocean
    ocean_pipeline: Option<crate::ocean_pipeline::OceanPipeline>,
    ocean_uniform_buffer: Option<wgpu::Buffer>,
    ocean_uniform_bind_group: Option<wgpu::BindGroup>,
    ocean_transform_buffer: Option<wgpu::Buffer>,
    ocean_transform_bind_group: Option<wgpu::BindGroup>,
    /// The sea's own body block (group 1, binding 1): its colours, full
    /// waves, no flow (ADR 0094).
    ocean_sea_body_buffer: Option<wgpu::Buffer>,
    /// Water bodies a game set at run time, by its id (ADR 0094).
    water_bodies: std::collections::BTreeMap<String, crate::water_body::GpuWaterBody>,
    /// Water bodies from `water_body` components, by entity name, with the
    /// component they were built from (rebuilt only when it changes). `None`
    /// when the component didn't describe a mesh; the warning said why.
    scene_water_bodies:
        std::collections::BTreeMap<String, (toml::Value, Option<crate::water_body::GpuWaterBody>)>,
    ocean_spectrum: Option<flint_core::ocean::WaveSpectrum>,
    ocean_visuals: crate::ocean_pipeline::OceanVisuals,
    ocean_active: bool,
    /// Contact-foam hull: ([x, z, cos_yaw, sin_yaw], [half_x, half_z]).
    /// None when the scene has no `ocean_contact` entity.
    ocean_contact: Option<([f32; 4], [f32; 2])>,
    /// (ocean_time, hull position) of the previous frame, for velocity.
    ocean_contact_prev: Option<(f64, [f32; 3])>,
    /// Smoothed hull velocity fed to the contact-foam churn term.
    ocean_contact_vel: [f32; 3],
    // Grab pass (refraction): opaque scene color+depth snapshots
    ocean_grab_color: Option<(wgpu::Texture, wgpu::TextureView)>,
    ocean_grab_depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    ocean_grab_size: (u32, u32),
    ocean_grab_bind_group: Option<wgpu::BindGroup>,
    ocean_grab_dummy_bind_group: Option<wgpu::BindGroup>,
    ocean_grab_this_frame: bool,
    ocean_camera_near_far: (f32, f32),
    /// Time in seconds driving wave phases. Set by the player each frame from
    /// the game clock — the same clock scripts see via total_time(), which is
    /// what keeps script-side ocean_height() queries in sync with the GPU.
    pub ocean_time: f64,
    /// Seconds of game time this frame stepped, for styles whose history
    /// is a rate (ASCII's smoothing, #342): the player sets it from its
    /// clock's `delta_time` (so it follows `--frame-step`), the viewer from
    /// its frame time. Hosts that don't set it get 1/60, which is what a
    /// per-frame weight always meant.
    pub style_dt: f32,
    // Particles (ADR 0068)
    particle_pipeline: Option<ParticlePipeline>,
    particle_draws: Vec<ParticleDrawCall>,
    /// Texture bind groups keyed by texture-cache name ("" = white).
    particle_texture_bind_groups: HashMap<String, std::sync::Arc<wgpu::BindGroup>>,
    /// Reused CPU staging for the per-frame instance upload.
    particle_upload_scratch: Vec<flint_particles::ParticleInstance>,
    /// Editor/gizmo line overlay, drawn on top regardless of debug mode.
    debug_overlay_draws: Vec<DrawCall>,
    /// Depth-tested script lines for the next frame (ADR 0083), and the
    /// logical-point scale their widths are given in.
    world_lines: Vec<flint_core::world_line::WorldLine>,
    world_line_px_per_point: f32,
    /// Built on the first frame that has lines.
    world_line_pipeline: Option<crate::world_line_pipeline::WorldLinePipeline>,
    /// Main-pass clear colour (linear RGBA).
    clear_color: [f32; 4],
    // 2D sprites
    sprite2d_pipeline: Option<Sprite2dPipeline>,
    sprite2d_batches: Vec<crate::sprite2d_pipeline::Sprite2dBatch>,
    // MSAA (ADR 0058): 1 = off (default), 4 = on. Scene passes render into
    // the MSAA color/depth targets and resolve color into the HDR buffer;
    // depth resolves (sample 0) into the caller's depth view so every
    // depth consumer stays single-sample and unchanged.
    sample_count: u32,
    msaa_color: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    msaa_depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// A new id each time `msaa_depth` is made, so the style mask can keep
    /// a bind group over it (#306).
    msaa_depth_id: u64,
    depth_resolve_pipeline: Option<wgpu::RenderPipeline>,
    depth_resolve_layout: Option<wgpu::BindGroupLayout>,
    // Post-processing
    postprocess_pipeline: Option<PostProcessPipeline>,
    postprocess_resources: Option<PostProcessResources>,
    /// What's drawn: `authored_postprocess` under the quality caps.
    postprocess_config: PostProcessConfig,
    /// What callers set (`set_post_process_config`): the scene's look, kept
    /// whole so lifting a quality cap brings its effect back.
    authored_postprocess: PostProcessConfig,
    /// A fresh [`next_post_config_generation`] whenever the drawn
    /// configuration may have changed, so a host mirroring it (scripts'
    /// style reads, #372) copies it only then. Unique in the process, so a
    /// rebuilt renderer never repeats an old one's.
    post_config_generation: u64,
    /// The quality settings in force (FlintKart#55, `set_quality`).
    quality: crate::quality::QualitySettings,
    /// Render scale (#122): the HDR chain is this fraction of the surface on
    /// each side, 1 = the surface. `surface_size` is what the last resize
    /// gave; the chain's own size is `postprocess_resources`'.
    render_scale: f32,
    surface_size: (u32, u32),
    /// Below scale 1, the scene's depth at the chain's size, and the pass
    /// that copies it into the caller's surface-sized depth view.
    scaled_depth: Option<std::sync::Arc<crate::render_scale::ScaledDepth>>,
    depth_upscale: Option<crate::render_scale::DepthUpscale>,
    /// The ground fog's map, set by the game (ADR 0106).
    ground_fog: Option<crate::ground_fog::GroundFogGpu>,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
    // Parallax: camera offset in world units + orthographic viewport height
    pub camera_offset: [f32; 2],
    pub ortho_height: f32,
    pub aspect_ratio: f32,
    /// `ui_text` fonts by what the component named, each uploaded once
    /// (`load_ui_font`); `None` for one that failed, warned about once.
    bitmap_font_cache: HashMap<String, Option<BitmapFont>>,
    /// The scope `bitmap_font_cache` was filled from. A name means another
    /// font in another scene's folder (#312), so a load from a different
    /// scope forgets them all first.
    bitmap_font_scope: Option<flint_core::project::AssetScope>,
    /// Scene directory for resolving relative font/texture paths. A host
    /// that holds the scene's project sets it through
    /// [`set_asset_scope`](Self::set_asset_scope).
    pub scene_dir: Option<std::path::PathBuf>,
    /// The project and folder styles search from, with the `scene_dir` it
    /// was made for: found once a scene, not once a lookup.
    asset_scope: std::sync::Mutex<Option<AssetScopeFor>>,
    /// Physical pixels per logical point of the window it draws into: the
    /// OS scale factor. Styles sized in points (ASCII cells, #265) scale by
    /// it. Hosts with a window set it; 1 headless.
    pub pixels_per_point: f32,
    /// Frames drawn through the post chain, handed to styles as
    /// [`StyleFrame::frame`](crate::stylize::StyleFrame::frame) (#277).
    style_frame_serial: std::sync::atomic::AtomicU64,
    /// Bumped by [`reset_style_history`](Self::reset_style_history).
    style_history_epoch: u64,
    /// Time in seconds, used for grass wind animation. Set by the player before render().
    pub grass_time: f32,
    /// The comb of woozy grass this frame (#252), from the clock the player set
    /// `grass_time` on. `None`, the default, combs nothing.
    pub grass_wooze: Option<ShallowWoozeDraw>,
    /// Set when GPU device is lost (e.g. driver crash); skips all rendering
    device_lost: bool,
    /// Per-entity bone matrix buffers. The `GpuSkinnedMesh` bone buffer is
    /// shared by every entity that instances the asset, so entities playing
    /// different clips need their own storage or the last upload wins.
    entity_bone_buffers: HashMap<flint_core::EntityId, wgpu::Buffer>,
    /// The bone matrices last written for each entity, and for each
    /// asset's shared buffer, kept on the CPU to bound the posed meshes
    /// for the per-entity style scissor (#385).
    entity_bone_matrices: HashMap<flint_core::EntityId, Vec<[[f32; 4]; 4]>>,
    asset_bone_matrices: HashMap<String, Vec<[[f32; 4]; 4]>>,
    /// Entities whose shadow follows other bones than they're drawn with.
    entity_shadow_bone_buffers: HashMap<flint_core::EntityId, wgpu::Buffer>,
    /// Per entity, what each skinned part's vertex colour is multiplied by.
    entity_part_tints: HashMap<flint_core::EntityId, Vec<[f32; 3]>>,
    /// Per entity, how soft each skinned part's silhouette is (#582); a part
    /// missing or at 0 is a hard edge.
    entity_part_softness: HashMap<flint_core::EntityId, Vec<f32>>,
    /// Living props' echoes, in the order they draw. Empty costs nothing.
    entity_echoes: HashMap<flint_core::EntityId, Vec<EntityEcho>>,
    /// The entities whose echoes may only add light.
    entity_echo_lighten: std::collections::HashSet<flint_core::EntityId>,
    /// Bone bind groups of skinned draws, by entity and sub-mesh.
    skinned_bone_binds: HashMap<(flint_core::EntityId, u32), CachedBoneBind>,
}

/// A number no other call in this process returns, for
/// [`SceneRenderer::post_config_generation`].
fn next_post_config_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl SceneRenderer {
    pub fn new(context: &RenderContext, config: RendererConfig) -> Self {
        let surface_format = context.view_format;
        // Scene geometry renders to HDR; the composite pass tonemaps to the surface.
        let scene_format = HDR_FORMAT;
        let sample_count = Self::resolve_sample_count(config.sample_count);

        let pipeline = RenderPipeline::new(&context.device, scene_format, sample_count);
        let archetype_visuals = Self::default_archetype_visuals();
        let texture_cache = TextureCache::new(&context.device, &context.queue);

        // Create grid draw call (only for debug/inspection modes)
        let grid_draw = if config.show_grid {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            Some(Self::create_draw_call(
                &context.device,
                &pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                &texture_cache,
            ))
        } else {
            None
        };

        let shadow_pass = ShadowPass::new(&context.device, DEFAULT_SHADOW_RESOLUTION);

        let light_uniforms = LightUniforms::default_scene_lights();
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(&context.device, &pipeline, &light_uniforms, &shadow_pass);

        let skinned_pipeline = SkinnedPipeline::new(
            &context.device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.material_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );

        let billboard_pipeline =
            BillboardPipeline::new(&context.device, scene_format, sample_count);
        let terrain_pipeline = TerrainPipeline::new(
            &context.device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );
        let ocean_resources = Self::create_ocean_resources(
            &context.device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );
        let sky_resources = Self::create_sky_resources(&context.device, scene_format, sample_count);

        // Graceful degradation: wrap in catch_unwind like the Kuwahara pipeline.
        // If compute shaders aren't supported, grass is silently disabled.
        let grass_pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GrassPipeline::new(
                &context.device,
                scene_format,
                &pipeline.transform_bind_group_layout,
                &pipeline.light_bind_group_layout,
                sample_count,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Grass pipeline creation failed — grass disabled");
            None
        });

        let particle_pipeline = ParticlePipeline::new(&context.device, scene_format, sample_count);
        let sprite2d_pipeline = Sprite2dPipeline::new(&context.device, scene_format, sample_count);
        let skybox_pipeline = SkyboxPipeline::new(&context.device, scene_format, sample_count);

        // Create post-processing pipeline and resources
        let postprocess_config = PostProcessConfig::default();
        let postprocess_pipeline =
            PostProcessPipeline::new(&context.device, &context.queue, surface_format);
        let postprocess_resources =
            PostProcessResources::new(&context.device, context.config.width, context.config.height);

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            shallow_wooze: HashMap::new(),
            wooze_standard: None,
            wooze_skinned: None,
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            draw_bind_cache: DrawBindCache::default(),
            box_mesh_cache: HashMap::new(),
            extract_frame: 0,
            ray_meshes: HashMap::new(),
            ray_scene: Arc::default(),
            entity_bone_buffers: HashMap::new(),
            entity_bone_matrices: HashMap::new(),
            asset_bone_matrices: HashMap::new(),
            entity_shadow_bone_buffers: HashMap::new(),
            entity_part_tints: HashMap::new(),
            entity_part_softness: HashMap::new(),
            entity_echoes: HashMap::new(),
            entity_echo_lighten: std::collections::HashSet::new(),
            skinned_bone_binds: HashMap::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            transparent_draws: Vec::new(),
            transparent_skinned_draws: Vec::new(),
            echo_draws: Vec::new(),
            culled_entity_draws: Vec::new(),
            culled_transparent_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
            skeleton_overlay_draws: Vec::new(),
            tonemapping_enabled: true,
            light_buffer,
            light_bind_group,
            light_uniforms,
            ambient_override: None,
            scene_shadows: true,
            default_lights: true,
            diffuse_wrap_override: None,
            oren_nayar_override: None,
            sheen_override: None,
            texture_cache: Some(texture_cache),
            shadow_pass: Some(shadow_pass),
            selected_entity: None,
            entity_children: HashMap::new(),
            selection_outline: None,
            skybox_pipeline: Some(skybox_pipeline),
            skybox_uniform_buffer: None,
            skybox_uniform_bind_group: None,
            skybox_texture_bind_group: None,
            terrain_pipeline: Some(terrain_pipeline),
            terrain_draws: Vec::new(),
            terrain_material_bind_group: None,
            terrain_material_buffer: None,
            terrain_total_chunks: 0,
            terrain_visible_chunks: 0,
            terrain_entity: None,
            camera_frustum: None,
            lod_settings: LodSettings::from_env(),
            lod_culled: 0,
            grass_pipeline,
            grass_instance_buffer: None,
            grass_instance_count: 0,
            grass_drawing: false,
            grass_max_instances: 0,
            grass_draw_args_buffer: None,
            grass_count_readback: None,
            grass_readback_due: false,
            grass_shadow_binds: Vec::new(),
            gpu_timer: None,
            style_mask_pass: None,
            style_ids: Default::default(),
            style_by_entity: HashMap::new(),
            style_scissor: crate::stylize::scissor_enabled(),
            style_blend_by_entity: HashMap::new(),
            style_mask_veiled: false,
            mask_view_forced: false,
            ascii_forced: false,
            grass_compute_uniform_buffer: None,
            grass_compute_uniform_bind_group: None,
            grass_compute_texture_bind_group: None,
            grass_compute_storage_bind_group: None,
            grass_group_slots: None,
            grass_render_uniform_buffer: None,
            grass_render_uniform_bind_group: None,
            grass_render_instance_bind_group: None,
            grass_entity_buffer: None,
            grass_entity_count: 0,
            grass_config: None,
            grass_terrain_offset: [0.0; 3],
            grass_terrain_width: 0.0,
            grass_terrain_depth: 0.0,
            grass_terrain_height_scale: 0.0,
            grass_on_ground: false,
            grass_scale_by_density: false,
            grass_mask: None,
            grass_needs_realloc: false,
            grass_transform_buffer: None,
            grass_transform_bind_group: None,
            sky_pipeline: sky_resources.0,
            sky_uniform_buffer: sky_resources.1,
            sky_uniform_bind_group: sky_resources.2,
            sky_params: crate::sky_pipeline::SkyParams::default(),
            sky_active: false,
            ocean_pipeline: ocean_resources.0,
            ocean_uniform_buffer: ocean_resources.1,
            ocean_uniform_bind_group: ocean_resources.2,
            ocean_transform_buffer: ocean_resources.3,
            ocean_transform_bind_group: ocean_resources.4,
            ocean_sea_body_buffer: ocean_resources.5,
            water_bodies: std::collections::BTreeMap::new(),
            scene_water_bodies: std::collections::BTreeMap::new(),
            ocean_spectrum: None,
            ocean_visuals: crate::ocean_pipeline::OceanVisuals::default(),
            ocean_active: false,
            ocean_contact: None,
            ocean_contact_prev: None,
            ocean_contact_vel: [0.0; 3],
            ocean_grab_color: None,
            ocean_grab_depth: None,
            ocean_grab_size: (0, 0),
            ocean_grab_bind_group: None,
            ocean_grab_dummy_bind_group: None,
            ocean_grab_this_frame: false,
            ocean_camera_near_far: (0.1, 1000.0),
            ocean_time: 0.0,
            style_dt: 1.0 / 60.0,
            particle_pipeline: Some(particle_pipeline),
            particle_draws: Vec::new(),
            particle_texture_bind_groups: HashMap::new(),
            particle_upload_scratch: Vec::new(),
            debug_overlay_draws: Vec::new(),
            world_lines: Vec::new(),
            world_line_px_per_point: 1.0,
            world_line_pipeline: None,
            clear_color: DEFAULT_CLEAR_COLOR,
            sprite2d_pipeline: Some(sprite2d_pipeline),
            sprite2d_batches: Vec::new(),
            sample_count,
            msaa_color: None,
            msaa_depth: None,
            msaa_depth_id: 0,
            depth_resolve_pipeline: None,
            depth_resolve_layout: None,
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            authored_postprocess: postprocess_config.clone(),
            postprocess_config,
            post_config_generation: next_post_config_generation(),
            quality: crate::quality::QualitySettings {
                msaa: sample_count,
                ..Default::default()
            },
            render_scale: 1.0,
            surface_size: (context.config.width, context.config.height),
            scaled_depth: None,
            depth_upscale: None,
            ground_fog: None,
            surface_format,
            camera_offset: [0.0, 0.0],
            ortho_height: 10.0,
            aspect_ratio: 16.0 / 9.0,
            bitmap_font_cache: HashMap::new(),
            bitmap_font_scope: None,
            scene_dir: None,
            asset_scope: std::sync::Mutex::new(None),
            pixels_per_point: 1.0,
            style_frame_serial: std::sync::atomic::AtomicU64::new(0),
            style_history_epoch: 0,
            grass_time: 0.0,
            grass_wooze: None,
            device_lost: false,
        }
    }

    /// Create the procedural sky pipeline + uniform buffer/bind group.
    /// catch_unwind so shader failures degrade to "texture skybox only".
    fn create_sky_resources(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> (
        Option<crate::sky_pipeline::SkyPipeline>,
        Option<wgpu::Buffer>,
        Option<wgpu::BindGroup>,
    ) {
        let pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::sky_pipeline::SkyPipeline::new(device, scene_format, sample_count)
        }));
        let pipeline = match pipeline {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("Sky pipeline creation failed — procedural sky disabled");
                return (None, None, None);
            }
        };
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sky Uniform Buffer"),
            size: std::mem::size_of::<crate::sky_pipeline::SkyUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Sky Uniform Bind Group"),
        });
        (Some(pipeline), Some(buffer), Some(bind_group))
    }

    /// Create the ocean pipeline plus its uniform/transform buffers and bind
    /// groups. Wrapped in catch_unwind like grass so shader-compilation
    /// failures degrade to "no ocean" instead of crashing the renderer.
    #[allow(clippy::type_complexity)]
    fn create_ocean_resources(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        transform_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
        sample_count: u32,
    ) -> (
        Option<crate::ocean_pipeline::OceanPipeline>,
        Option<wgpu::Buffer>,
        Option<wgpu::BindGroup>,
        Option<wgpu::Buffer>,
        Option<wgpu::BindGroup>,
        Option<wgpu::Buffer>,
    ) {
        let pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::ocean_pipeline::OceanPipeline::new(
                device,
                scene_format,
                transform_layout,
                light_layout,
                sample_count,
            )
        }));
        let pipeline = match pipeline {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("Ocean pipeline creation failed — ocean disabled");
                return (None, None, None, None, None, None);
            }
        };

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Uniform Buffer"),
            size: std::mem::size_of::<crate::ocean_pipeline::OceanUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sea_body_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Sea Body Buffer"),
            size: std::mem::size_of::<crate::water_body::WaterBodyUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.ocean_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sea_body_buffer.as_entire_binding(),
                },
            ],
            label: Some("Ocean Uniform Bind Group"),
        });

        let transform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Transform Buffer"),
            size: std::mem::size_of::<TransformUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: transform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: transform_buffer.as_entire_binding(),
            }],
            label: Some("Ocean Transform Bind Group"),
        });

        (
            Some(pipeline),
            Some(uniform_buffer),
            Some(uniform_bind_group),
            Some(transform_buffer),
            Some(transform_bind_group),
            Some(sea_body_buffer),
        )
    }

    /// Create the placeholder group-3 bind group (1x1 textures) so the ocean
    /// can always bind group 3 even when the grab pass didn't run.
    fn ensure_ocean_grab_dummy(&mut self, device: &wgpu::Device) {
        if self.ocean_grab_dummy_bind_group.is_some() {
            return;
        }
        let Some(op) = &self.ocean_pipeline else {
            return;
        };
        let dummy_color = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            HDR_FORMAT,
            "Ocean Grab Dummy Color",
        );
        let dummy_depth = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            wgpu::TextureFormat::R32Float,
            "Ocean Grab Dummy Depth",
        );
        let cv = dummy_color.create_view(&Default::default());
        let dv = dummy_depth.create_view(&Default::default());
        self.ocean_grab_dummy_bind_group =
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &op.grab_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&cv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&dv),
                    },
                ],
                label: Some("Ocean Grab Dummy Bind Group"),
            }));
    }

    /// Ensure grab-pass textures + bind groups exist at the given size.
    /// Called only on the ocean+postprocess path; cheap when size unchanged.
    fn ensure_ocean_grab_resources(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.ensure_ocean_grab_dummy(device);
        let Some(op) = &self.ocean_pipeline else {
            return;
        };

        if self.ocean_grab_size == (width, height) && self.ocean_grab_bind_group.is_some() {
            return;
        }

        let make = |format: wgpu::TextureFormat, label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let color = make(HDR_FORMAT, "Ocean Grab Color");
        let depth = make(wgpu::TextureFormat::R32Float, "Ocean Grab Depth");

        self.ocean_grab_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &op.grab_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color.1),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth.1),
                },
            ],
            label: Some("Ocean Grab Bind Group"),
        }));
        self.ocean_grab_color = Some(color);
        self.ocean_grab_depth = Some(depth);
        self.ocean_grab_size = (width, height);
    }

    /// Load terrain chunks and create GPU resources for terrain rendering.
    pub fn load_terrain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat_path: &str,
        layer_paths: &[String; 4],
        scene_dir: &Path,
    ) {
        // The splat map and the layers are paths the scene names: found by
        // the project's one search (`AssetKind::Files`, ADR 0114), and a
        // missing one is a warning listing every path tried.
        let scope = self.asset_scope();
        let find = |what: &str, name: &str| -> Option<std::path::PathBuf> {
            scope
                .project
                .resolve(flint_core::project::AssetKind::Files, name, scene_dir)
                .map_err(|not_found| tracing::warn!("{what}: {not_found}"))
                .ok()
        };

        let tp = match &self.terrain_pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match &mut self.texture_cache {
            Some(tc) => tc,
            None => return,
        };

        // Load splat map
        if !splat_path.is_empty() {
            if let Some(path) = find("terrain splat map", splat_path) {
                let _ = tc.load_file(device, queue, "terrain_splat", &path);
            }
        }

        // Load layer textures
        for (i, layer_path) in layer_paths.iter().enumerate() {
            if !layer_path.is_empty() {
                if let Some(path) = find("terrain layer texture", layer_path) {
                    let name = format!("terrain_layer{}", i);
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = self.texture_cache.as_ref().unwrap();

        let splat_tex = tc.get("terrain_splat").unwrap_or(&tc.default_white);
        let layer0_tex = tc.get("terrain_layer0").unwrap_or(&tc.default_white);
        let layer1_tex = tc.get("terrain_layer1").unwrap_or(&tc.default_white);
        let layer2_tex = tc.get("terrain_layer2").unwrap_or(&tc.default_white);
        let layer3_tex = tc.get("terrain_layer3").unwrap_or(&tc.default_white);

        // Create terrain uniform buffer
        let terrain_uniforms = TerrainUniforms {
            texture_tile,
            metallic,
            roughness,
            enable_tonemapping: 0, // Will be updated per-frame
        };

        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Terrain Uniform Buffer"),
            contents: bytemuck::cast_slice(&[terrain_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create material bind group (shared across all terrain chunks)
        let material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &tp.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&splat_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&splat_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&layer0_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&layer0_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&layer1_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&layer1_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&layer2_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&layer2_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&layer3_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&layer3_tex.sampler),
                },
            ],
            label: Some("Terrain Material Bind Group"),
        });

        self.terrain_material_buffer = Some(material_buffer);
        self.terrain_material_bind_group = Some(material_bind_group);

        // Create per-chunk draw calls
        self.terrain_draws.clear();
        self.terrain_total_chunks = chunks.len() as u32;

        let model = transform.to_matrix();
        let model_inv_transpose = mat4_inv_transpose(&model);

        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];

        for chunk in chunks {
            // Build Vertex array from chunk data
            let vertices: Vec<crate::primitives::Vertex> = (0..chunk.positions.len())
                .map(|i| crate::primitives::Vertex {
                    position: chunk.positions[i],
                    normal: chunk.normals[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    uv: chunk.uvs[i],
                })
                .collect();

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Index Buffer"),
                contents: bytemuck::cast_slice(&chunk.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4], // Updated per frame
                model,
                model_inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Transform Buffer"),
                contents: bytemuck::cast_slice(&[transform_uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

            let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: transform_buffer.as_entire_binding(),
                }],
                label: Some("Terrain Chunk Transform Bind Group"),
            });

            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [
                    chunk.aabb_min[0] + tx,
                    chunk.aabb_min[1] + ty,
                    chunk.aabb_min[2] + tz,
                ],
                aabb_max: [
                    chunk.aabb_max[0] + tx,
                    chunk.aabb_max[1] + ty,
                    chunk.aabb_max[2] + tz,
                ],
            });
        }

        tracing::info!(
            "Loaded {} terrain chunks ({} draw calls)",
            chunks.len(),
            self.terrain_draws.len()
        );
    }

    /// Reload only the terrain geometry (vertex/index buffers) without rebuilding
    /// the material bind group. Used for brush sculpting where only the mesh changes.
    pub fn reload_terrain_geometry(
        &mut self,
        device: &wgpu::Device,
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
    ) {
        // Keep existing material bind group and buffer
        self.terrain_draws.clear();

        let model = transform.to_matrix();
        let model_inv_transpose = mat4_inv_transpose(&model);

        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];

        for chunk in chunks {
            let vertices: Vec<crate::primitives::Vertex> = (0..chunk.positions.len())
                .map(|i| crate::primitives::Vertex {
                    position: chunk.positions[i],
                    normal: chunk.normals[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    uv: chunk.uvs[i],
                })
                .collect();

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Index Buffer"),
                contents: bytemuck::cast_slice(&chunk.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4],
                model,
                model_inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Transform Buffer"),
                contents: bytemuck::cast_slice(&[transform_uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

            let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: transform_buffer.as_entire_binding(),
                }],
                label: Some("Terrain Chunk Transform Bind Group"),
            });

            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [
                    chunk.aabb_min[0] + tx,
                    chunk.aabb_min[1] + ty,
                    chunk.aabb_min[2] + tz,
                ],
                aabb_max: [
                    chunk.aabb_max[0] + tx,
                    chunk.aabb_max[1] + ty,
                    chunk.aabb_max[2] + tz,
                ],
            });
        }
    }

    /// Load terrain from raw data (heightmap-generated chunks + raw splat RGBA).
    /// Like `load_terrain()` but accepts raw splat data instead of a file path.
    pub fn load_terrain_from_data(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat_data: &[u8],
        splat_res: u32,
        layer_paths: &[String; 4],
        spec_dir: &Path,
    ) {
        let tp = match &self.terrain_pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match &mut self.texture_cache {
            Some(tc) => tc,
            None => return,
        };

        // Upload splat map from raw data
        tc.remove_texture("terrain_splat");
        let _ = tc.upload_rgba(
            device,
            queue,
            "terrain_splat",
            splat_res,
            splat_res,
            splat_data,
            false,
        );

        // Load layer textures
        for (i, layer_path) in layer_paths.iter().enumerate() {
            if !layer_path.is_empty() {
                let name = format!("terrain_layer{}", i);
                tc.remove_texture(&name);
                let path = spec_dir.join(layer_path);
                if vfs::exists(&path) {
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = self.texture_cache.as_ref().unwrap();

        let splat_tex = tc.get("terrain_splat").unwrap_or(&tc.default_white);
        let layer0_tex = tc.get("terrain_layer0").unwrap_or(&tc.default_white);
        let layer1_tex = tc.get("terrain_layer1").unwrap_or(&tc.default_white);
        let layer2_tex = tc.get("terrain_layer2").unwrap_or(&tc.default_white);
        let layer3_tex = tc.get("terrain_layer3").unwrap_or(&tc.default_white);

        // Create terrain uniform buffer
        let terrain_uniforms = TerrainUniforms {
            texture_tile,
            metallic,
            roughness,
            enable_tonemapping: 0,
        };

        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Terrain Uniform Buffer"),
            contents: bytemuck::cast_slice(&[terrain_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &tp.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&splat_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&splat_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&layer0_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&layer0_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&layer1_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&layer1_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&layer2_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&layer2_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&layer3_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&layer3_tex.sampler),
                },
            ],
            label: Some("Terrain Material Bind Group"),
        });

        self.terrain_material_buffer = Some(material_buffer);
        self.terrain_material_bind_group = Some(material_bind_group);

        // Build chunk draw calls
        self.terrain_total_chunks = chunks.len() as u32;
        self.reload_terrain_geometry(device, chunks, transform);
    }

    /// Name the entity the loaded terrain belongs to (#310): a
    /// `style_mask` (or a style's own component) on it, or on an ancestor,
    /// then masks the terrain's chunks like any styled mesh. Hosts call it
    /// beside [`load_terrain`](Self::load_terrain); `None` leaves the
    /// terrain unmasked, as it was before.
    pub fn set_terrain_entity(&mut self, entity: Option<flint_core::EntityId>) {
        self.terrain_entity = entity;
    }

    /// Clear terrain draw calls (for scene transitions)
    pub fn clear_terrain(&mut self) {
        self.terrain_draws.clear();
        self.terrain_entity = None;
        self.terrain_material_bind_group = None;
        self.terrain_material_buffer = None;
    }

    /// Initialize grass GPU resources for the loaded terrain.
    /// Call after load_terrain() when grass is enabled.
    #[allow(clippy::too_many_arguments)]
    pub fn load_grass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        heightmap_data: &[f32],
        heightmap_width: u32,
        heightmap_depth: u32,
        splat_data: &[u8],
        splat_width: u32,
        splat_height: u32,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
    ) {
        if self.grass_pipeline.is_none() {
            return;
        }
        let (_, hm_view) = upload_grass_texture(
            device,
            queue,
            "Grass Heightmap Texture",
            wgpu::TextureFormat::R32Float,
            bytemuck::cast_slice(heightmap_data),
            4,
            heightmap_width,
            heightmap_depth,
        );
        let (_, splat_view) = upload_grass_texture(
            device,
            queue,
            "Grass Splat Texture",
            wgpu::TextureFormat::Rgba8Unorm,
            splat_data,
            4,
            splat_width,
            splat_height,
        );
        self.install_grass(
            device,
            config,
            &hm_view,
            &splat_view,
            terrain_offset,
            terrain_width,
            terrain_depth,
            height_scale,
            None,
        );
    }

    /// Grow grass on a game's own ground instead of a Flint terrain
    /// (ADR 0089): a density mask and a height over a world rectangle.
    ///
    /// Replaces whatever grass is loaded, a terrain's included, and can be
    /// called again whenever the ground changes. `config` is used as for a
    /// terrain, except that `density_layer` and `density_source` don't
    /// apply: the mask is the density. Does nothing, and returns `Ok`, when
    /// the device has no compute grass (see `has_grass_pipeline`).
    pub fn set_grass_ground(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        ground: &GrassGround,
    ) -> Result<(), String> {
        ground.validate()?;
        if self.grass_pipeline.is_none() {
            return Ok(());
        }
        let flat;
        let (heights, hm_w, hm_d) = match ground.height {
            GrassHeight::Flat(y) => {
                flat = [y];
                (&flat[..], 1, 1)
            }
            GrassHeight::Map {
                heights,
                width,
                depth,
            } => (heights, width, depth),
        };
        let (_, hm_view) = upload_grass_texture(
            device,
            queue,
            "Grass Ground Height Texture",
            wgpu::TextureFormat::R32Float,
            bytemuck::cast_slice(heights),
            4,
            hm_w,
            hm_d,
        );
        let (mask, mask_view) = upload_grass_texture(
            device,
            queue,
            "Grass Ground Mask Texture",
            wgpu::TextureFormat::R8Unorm,
            ground.mask,
            1,
            ground.mask_width,
            ground.mask_height,
        );
        // The heights are world y: offset 0, scale 1.
        self.install_grass(
            device,
            config,
            &hm_view,
            &mask_view,
            [ground.origin[0], 0.0, ground.origin[1]],
            ground.size[0],
            ground.size[1],
            1.0,
            Some(ground.scale_by_mask),
        );
        self.grass_mask = Some((mask, ground.mask_width, ground.mask_height));
        Ok(())
    }

    /// Repaint part of the grass ground's mask in place (ADR 0093): the
    /// `width × height` texels at (`x`, `y`), from `bytes` (one a texel,
    /// row by row toward +Z). The buffers and bind groups stay as they
    /// are, so a small clearing costs a small upload, not a reinstall.
    ///
    /// Errs when no ground was set with `set_grass_ground` (a terrain's
    /// grass has no mask to repaint), or when the rectangle runs off the
    /// mask or `bytes` is the wrong length. On a device with no compute
    /// grass it does nothing, as `set_grass_ground` does.
    pub fn update_grass_mask(
        &mut self,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.grass_pipeline.is_none() {
            return Ok(());
        }
        let Some((mask, mask_width, mask_height)) = &self.grass_mask else {
            return Err("no grass ground to repaint: set one with set_grass_ground".into());
        };
        check_grass_mask_patch(*mask_width, *mask_height, x, y, width, height, bytes.len())?;
        if width == 0 || height == 0 {
            return Ok(());
        }
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: mask,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Whether this device runs the compute grass at all.
    pub fn has_grass_pipeline(&self) -> bool {
        self.grass_pipeline.is_some()
    }

    /// Build the grass buffers and bind groups over ground textures already
    /// uploaded. `ground` is `None` for a terrain (its splat layer is the
    /// density), or `Some(scale_by_mask)` for a game's R8 mask.
    #[allow(clippy::too_many_arguments)]
    fn install_grass(
        &mut self,
        device: &wgpu::Device,
        config: &flint_terrain::GrassConfig,
        hm_view: &wgpu::TextureView,
        splat_view: &wgpu::TextureView,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
        ground: Option<bool>,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.instance_capacity(terrain_width, terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Instance storage buffer (compute writes, render reads)
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Indirect draw arguments; the compute pass counts blades into them (ADR 0092)
        let counter_buffer = create_grass_draw_args_buffer(device);

        // Compute uniform buffer
        let compute_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Compute Uniform Buffer"),
            size: std::mem::size_of::<GrassComputeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: compute_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Compute Uniform Bind Group"),
        });

        // Heightmap uses nearest (non-filtering) sampler since R32Float is not filterable
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Nearest Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let compute_texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(hm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&nearest_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(splat_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
            ],
            label: Some("Grass Compute Texture Bind Group"),
        });

        // Render uniform buffer
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Render Uniform Buffer"),
            size: std::mem::size_of::<GrassRenderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_grass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: render_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Render Uniform Bind Group"),
        });

        // Entity positions buffer
        let entity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Entity Buffer"),
            size: (MAX_GRASS_ENTITIES * std::mem::size_of::<GrassEntityPosition>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: entity_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Store everything
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_draw_args_buffer = Some(counter_buffer);
        self.grass_count_readback = Some(GrassCountReadback::new(device));
        self.grass_compute_uniform_buffer = Some(compute_uniform_buffer);
        self.grass_compute_uniform_bind_group = Some(compute_uniform_bind_group);
        self.grass_compute_texture_bind_group = Some(compute_texture_bind_group);
        // Made by the first dispatch, which knows how many workgroups it runs
        self.grass_compute_storage_bind_group = None;
        self.grass_group_slots = None;
        self.grass_render_uniform_buffer = Some(render_uniform_buffer);
        self.grass_render_uniform_bind_group = Some(render_uniform_bind_group);
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_entity_buffer = Some(entity_buffer);
        self.grass_config = Some(config.clone());
        self.grass_terrain_offset = terrain_offset;
        self.grass_terrain_width = terrain_width;
        self.grass_terrain_depth = terrain_depth;
        self.grass_terrain_height_scale = height_scale;
        self.grass_on_ground = ground.is_some();
        self.grass_scale_by_density = ground.unwrap_or(false);
        self.grass_mask = None;
        self.grass_needs_realloc = false;
        if self.grass_transform_bind_group.is_none() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Transform Buffer"),
                size: std::mem::size_of::<TransformUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
                label: Some("Grass Transform Bind Group"),
            });
            self.grass_transform_buffer = Some(buffer);
            self.grass_transform_bind_group = Some(bind_group);
        }

        tracing::info!(
            "Grass loaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Clear all grass GPU resources.
    pub fn unload_grass(&mut self) {
        self.grass_instance_buffer = None;
        self.grass_instance_count = 0;
        self.grass_max_instances = 0;
        self.grass_draw_args_buffer = None;
        self.grass_count_readback = None;
        self.grass_compute_uniform_buffer = None;
        self.grass_compute_uniform_bind_group = None;
        self.grass_compute_texture_bind_group = None;
        self.grass_compute_storage_bind_group = None;
        self.grass_group_slots = None;
        self.grass_render_uniform_buffer = None;
        self.grass_render_uniform_bind_group = None;
        self.grass_render_instance_bind_group = None;
        self.grass_entity_buffer = None;
        self.grass_entity_count = 0;
        self.grass_config = None;
        self.grass_on_ground = false;
        self.grass_scale_by_density = false;
        self.grass_mask = None;
        self.grass_needs_realloc = false;
    }

    /// The blades the grass's last compute pass kept, read back from the
    /// GPU (ADR 0098). It waits for the GPU, so it's for tests and tools,
    /// never for a frame. Empty with no grass loaded.
    pub fn read_grass_blades(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<GrassInstanceGpu> {
        let (Some(instances), Some(args)) =
            (&self.grass_instance_buffer, &self.grass_draw_args_buffer)
        else {
            return Vec::new();
        };
        let read = |buffer: &wgpu::Buffer, offset: u64, size: u64| -> Vec<u8> {
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Blade Read-back"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(buffer, offset, &staging, 0, size);
            queue.submit([encoder.finish()]);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::Maintain::Wait);
            let bytes = staging.slice(..).get_mapped_range().to_vec();
            staging.unmap();
            bytes
        };
        let count = read(args, GrassDrawArgs::INSTANCE_COUNT_OFFSET, 4);
        let count = u32::from_le_bytes([count[0], count[1], count[2], count[3]])
            .min(self.grass_max_instances);
        if count == 0 {
            return Vec::new();
        }
        let stride = std::mem::size_of::<GrassInstanceGpu>();
        read(instances, 0, count as u64 * stride as u64)
            .chunks_exact(stride)
            .map(bytemuck::pod_read_unaligned)
            .collect()
    }

    /// Update grass config without buffer reallocation.
    /// Compute and render passes read `grass_config` fresh each frame to build
    /// uniforms, so changes take effect on the next `render()` call.
    /// If the new config needs more instances than the buffer holds (a
    /// higher density or a longer `max_distance`), the buffer is
    /// reallocated before the next dispatch.
    pub fn set_grass_config(&mut self, config: flint_terrain::GrassConfig) {
        let capacity = config.instance_capacity(self.grass_terrain_width, self.grass_terrain_depth);
        if self.grass_instance_buffer.is_some() && capacity > self.grass_max_instances {
            self.grass_needs_realloc = true;
        }
        self.grass_config = Some(config);
    }

    /// Reallocate the grass instance buffer for a new density value,
    /// reusing existing heightmap/splat GPU textures.
    /// Call this when `GrassConfig.density` changes (affects buffer capacity).
    pub fn reload_grass_config(
        &mut self,
        device: &wgpu::Device,
        config: flint_terrain::GrassConfig,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances =
            config.instance_capacity(self.grass_terrain_width, self.grass_terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Reallocate instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Indirect draw arguments; the compute pass counts blades into them (ADR 0092)
        let counter_buffer = create_grass_draw_args_buffer(device);

        // Recreate render instance bind group (binds instance buffer at binding 0)
        let entity_buffer = self
            .grass_entity_buffer
            .as_ref()
            .expect("grass entity buffer");
        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: entity_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Update stored state
        // Note: grass_compute_texture_bind_group, compute_uniform_bind_group,
        // render_uniform_bind_group, and entity_buffer are intentionally NOT
        // recreated — they don't reference the instance buffer.
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_draw_args_buffer = Some(counter_buffer);
        self.grass_count_readback = Some(GrassCountReadback::new(device));
        // The next dispatch binds the new buffers
        self.grass_compute_storage_bind_group = None;
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_config = Some(config);
        self.grass_needs_realloc = false;

        tracing::info!(
            "Grass reloaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Make sure the grass compute's storage bind group exists and its
    /// per-workgroup slots hold `groups` workgroups (#341). The slots grow
    /// to the next power of two, so a window that changes size as the
    /// camera nears the ground's edge rebuilds them a few times at most.
    fn ensure_grass_group_slots(&mut self, device: &wgpu::Device, groups: u32) {
        let groups = groups.max(1);
        let fits = self
            .grass_group_slots
            .as_ref()
            .is_some_and(|(_, capacity)| *capacity >= groups);
        if fits && self.grass_compute_storage_bind_group.is_some() {
            return;
        }
        let (Some(grass_pipeline), Some(instances), Some(draw_args)) = (
            &self.grass_pipeline,
            &self.grass_instance_buffer,
            &self.grass_draw_args_buffer,
        ) else {
            return;
        };
        if !fits {
            let capacity = groups.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Group Slots"),
                size: capacity as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            self.grass_group_slots = Some((buffer, capacity));
        }
        let (slots, _) = self.grass_group_slots.as_ref().expect("grass group slots");
        self.grass_compute_storage_bind_group =
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.compute_storage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: draw_args.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: slots.as_entire_binding(),
                    },
                ],
                label: Some("Grass Compute Storage Bind Group"),
            }));
    }

    /// Read-only access to the current grass config (if loaded).
    pub fn grass_config(&self) -> Option<&flint_terrain::GrassConfig> {
        self.grass_config.as_ref()
    }

    /// Update entity positions for grass bend-on-contact.
    /// Also updates entity_count in the render uniform buffer.
    pub fn update_grass_entities(
        &mut self,
        queue: &wgpu::Queue,
        positions: &[GrassEntityPosition],
    ) {
        let count = positions.len().min(MAX_GRASS_ENTITIES);
        self.grass_entity_count = count as u32;
        if let Some(buf) = &self.grass_entity_buffer {
            if count > 0 {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&positions[..count]));
            }
        }
        // Note: entity_count is written to the render uniform buffer in dispatch_grass_compute,
        // which uses self.grass_entity_count to preserve the value set here.
    }

    /// Get an immutable reference to the texture cache.
    pub fn texture_cache(&self) -> Option<&TextureCache> {
        self.texture_cache.as_ref()
    }

    /// Clear all model/mesh data so a new model can be loaded cleanly.
    /// Used by the preview command when drag-and-dropping a replacement model.
    pub fn clear_model_data(&mut self) {
        self.mesh_cache.clear();
        self.draw_bind_cache.clear();
        self.skinned_bone_binds.clear();
        self.box_mesh_cache.clear();
        if let Some(tc) = &mut self.texture_cache {
            tc.clear_user_textures();
        }
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.transparent_draws.clear();
        self.transparent_skinned_draws.clear();
        self.echo_draws.clear();
        self.culled_entity_draws.clear();
        self.culled_transparent_draws.clear();
        self.billboard_draws.clear();
        self.sprite2d_batches.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();
        self.skeleton_overlay_draws.clear();
        self.debug_overlay_draws.clear();
        self.particle_draws.clear();
        self.particle_texture_bind_groups.clear();
        self.bitmap_font_cache.clear();
        self.bitmap_font_scope = None;
    }

    /// Create a renderer for headless (offscreen) use with explicit device and format
    pub fn new_headless(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        config: RendererConfig,
    ) -> Self {
        let surface_format = format;
        // Scene geometry renders to HDR; composite tonemaps to the readback surface.
        let scene_format = HDR_FORMAT;
        let sample_count = Self::resolve_sample_count(config.sample_count);

        let pipeline = RenderPipeline::new(device, scene_format, sample_count);
        let archetype_visuals = Self::default_archetype_visuals();
        let texture_cache = TextureCache::new(device, queue);

        // Create grid draw call (only for debug/inspection modes)
        let grid_draw = if config.show_grid {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            Some(Self::create_draw_call(
                device,
                &pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                &texture_cache,
            ))
        } else {
            None
        };

        let shadow_pass = ShadowPass::new(device, DEFAULT_SHADOW_RESOLUTION);

        let light_uniforms = LightUniforms::default_scene_lights();
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(device, &pipeline, &light_uniforms, &shadow_pass);

        let skinned_pipeline = SkinnedPipeline::new(
            device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.material_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );

        let terrain_pipeline = TerrainPipeline::new(
            device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );

        let grass_pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GrassPipeline::new(
                device,
                scene_format,
                &pipeline.transform_bind_group_layout,
                &pipeline.light_bind_group_layout,
                sample_count,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Grass pipeline creation failed — grass disabled");
            None
        });

        let billboard_pipeline = BillboardPipeline::new(device, scene_format, sample_count);
        let sprite2d_pipeline = Sprite2dPipeline::new(device, scene_format, sample_count);
        let particle_pipeline = ParticlePipeline::new(device, scene_format, sample_count);
        let skybox_pipeline = SkyboxPipeline::new(device, scene_format, sample_count);

        let ocean_resources = Self::create_ocean_resources(
            device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
            sample_count,
        );
        let sky_resources = Self::create_sky_resources(device, scene_format, sample_count);

        // Create post-processing pipeline and resources for headless
        let postprocess_config = PostProcessConfig::default();
        let postprocess_pipeline = PostProcessPipeline::new(device, queue, surface_format);
        let postprocess_resources = PostProcessResources::new(device, width, height);

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            shallow_wooze: HashMap::new(),
            wooze_standard: None,
            wooze_skinned: None,
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            draw_bind_cache: DrawBindCache::default(),
            box_mesh_cache: HashMap::new(),
            extract_frame: 0,
            ray_meshes: HashMap::new(),
            ray_scene: Arc::default(),
            entity_bone_buffers: HashMap::new(),
            entity_bone_matrices: HashMap::new(),
            asset_bone_matrices: HashMap::new(),
            entity_shadow_bone_buffers: HashMap::new(),
            entity_part_tints: HashMap::new(),
            entity_part_softness: HashMap::new(),
            entity_echoes: HashMap::new(),
            entity_echo_lighten: std::collections::HashSet::new(),
            skinned_bone_binds: HashMap::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            transparent_draws: Vec::new(),
            transparent_skinned_draws: Vec::new(),
            echo_draws: Vec::new(),
            culled_entity_draws: Vec::new(),
            culled_transparent_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
            skeleton_overlay_draws: Vec::new(),
            tonemapping_enabled: true,
            light_buffer,
            light_bind_group,
            light_uniforms,
            ambient_override: None,
            scene_shadows: true,
            default_lights: true,
            diffuse_wrap_override: None,
            oren_nayar_override: None,
            sheen_override: None,
            texture_cache: Some(texture_cache),
            shadow_pass: Some(shadow_pass),
            selected_entity: None,
            entity_children: HashMap::new(),
            selection_outline: None,
            skybox_pipeline: Some(skybox_pipeline),
            skybox_uniform_buffer: None,
            skybox_uniform_bind_group: None,
            skybox_texture_bind_group: None,
            terrain_pipeline: Some(terrain_pipeline),
            terrain_draws: Vec::new(),
            terrain_material_bind_group: None,
            terrain_material_buffer: None,
            terrain_total_chunks: 0,
            terrain_visible_chunks: 0,
            terrain_entity: None,
            camera_frustum: None,
            lod_settings: LodSettings::from_env(),
            lod_culled: 0,
            grass_pipeline,
            grass_instance_buffer: None,
            grass_instance_count: 0,
            grass_drawing: false,
            grass_max_instances: 0,
            grass_draw_args_buffer: None,
            grass_count_readback: None,
            grass_readback_due: false,
            grass_shadow_binds: Vec::new(),
            gpu_timer: None,
            style_mask_pass: None,
            style_ids: Default::default(),
            style_by_entity: HashMap::new(),
            style_scissor: crate::stylize::scissor_enabled(),
            style_blend_by_entity: HashMap::new(),
            style_mask_veiled: false,
            mask_view_forced: false,
            ascii_forced: false,
            grass_compute_uniform_buffer: None,
            grass_compute_uniform_bind_group: None,
            grass_compute_texture_bind_group: None,
            grass_compute_storage_bind_group: None,
            grass_group_slots: None,
            grass_render_uniform_buffer: None,
            grass_render_uniform_bind_group: None,
            grass_render_instance_bind_group: None,
            grass_entity_buffer: None,
            grass_entity_count: 0,
            grass_config: None,
            grass_terrain_offset: [0.0; 3],
            grass_terrain_width: 0.0,
            grass_terrain_depth: 0.0,
            grass_terrain_height_scale: 0.0,
            grass_on_ground: false,
            grass_scale_by_density: false,
            grass_mask: None,
            grass_needs_realloc: false,
            grass_transform_buffer: None,
            grass_transform_bind_group: None,
            sky_pipeline: sky_resources.0,
            sky_uniform_buffer: sky_resources.1,
            sky_uniform_bind_group: sky_resources.2,
            sky_params: crate::sky_pipeline::SkyParams::default(),
            sky_active: false,
            ocean_pipeline: ocean_resources.0,
            ocean_uniform_buffer: ocean_resources.1,
            ocean_uniform_bind_group: ocean_resources.2,
            ocean_transform_buffer: ocean_resources.3,
            ocean_transform_bind_group: ocean_resources.4,
            ocean_sea_body_buffer: ocean_resources.5,
            water_bodies: std::collections::BTreeMap::new(),
            scene_water_bodies: std::collections::BTreeMap::new(),
            ocean_spectrum: None,
            ocean_visuals: crate::ocean_pipeline::OceanVisuals::default(),
            ocean_active: false,
            ocean_contact: None,
            ocean_contact_prev: None,
            ocean_contact_vel: [0.0; 3],
            ocean_grab_color: None,
            ocean_grab_depth: None,
            ocean_grab_size: (0, 0),
            ocean_grab_bind_group: None,
            ocean_grab_dummy_bind_group: None,
            ocean_grab_this_frame: false,
            ocean_camera_near_far: (0.1, 1000.0),
            ocean_time: 0.0,
            style_dt: 1.0 / 60.0,
            // Headless particles (ADR 0068): zero instances until a caller
            // steps a ParticleSystem, so existing pixel gates are unchanged.
            particle_pipeline: Some(particle_pipeline),
            particle_draws: Vec::new(),
            particle_texture_bind_groups: HashMap::new(),
            particle_upload_scratch: Vec::new(),
            debug_overlay_draws: Vec::new(),
            world_lines: Vec::new(),
            world_line_px_per_point: 1.0,
            world_line_pipeline: None,
            clear_color: DEFAULT_CLEAR_COLOR,
            sprite2d_pipeline: Some(sprite2d_pipeline),
            sprite2d_batches: Vec::new(),
            sample_count,
            msaa_color: None,
            msaa_depth: None,
            msaa_depth_id: 0,
            depth_resolve_pipeline: None,
            depth_resolve_layout: None,
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            authored_postprocess: postprocess_config.clone(),
            postprocess_config,
            post_config_generation: next_post_config_generation(),
            quality: crate::quality::QualitySettings {
                msaa: sample_count,
                ..Default::default()
            },
            render_scale: 1.0,
            surface_size: (width, height),
            scaled_depth: None,
            depth_upscale: None,
            ground_fog: None,
            surface_format,
            camera_offset: [0.0, 0.0],
            ortho_height: 10.0,
            aspect_ratio: 16.0 / 9.0,
            bitmap_font_cache: HashMap::new(),
            bitmap_font_scope: None,
            scene_dir: None,
            asset_scope: std::sync::Mutex::new(None),
            pixels_per_point: 1.0,
            style_frame_serial: std::sync::atomic::AtomicU64::new(0),
            style_history_epoch: 0,
            grass_time: 0.0,
            grass_wooze: None,
            device_lost: false,
        }
    }

    /// Load an imported model into the GPU mesh cache
    pub fn load_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("furniture")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        self.mesh_cache
            .upload_imported(device, name, import_result, default_color);

        // Upload textures referenced by materials, namespaced by asset name
        let both = self.upload_model_textures(device, queue, name, import_result);

        // Patch material texture references on cached meshes to use namespaced names
        if let Some(gpu_meshes) = self.mesh_cache.get_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                namespace_textures(&mut mesh.material, name, &both);
            }
        }
    }

    /// Load skinned meshes from an imported model into the GPU mesh cache
    pub fn load_skinned_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("character")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        if let Some(sp) = &self.skinned_pipeline {
            self.mesh_cache.upload_skinned(
                device,
                name,
                import_result,
                default_color,
                &sp.bone_bind_group_layout,
            );
        }

        // Also upload textures, namespaced by asset name
        let both = self.upload_model_textures(device, queue, name, import_result);

        // Patch material texture references on cached skinned meshes
        if let Some(gpu_meshes) = self.mesh_cache.get_skinned_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                namespace_textures(&mut mesh.material, name, &both);
            }
        }
    }

    /// Upload a model's textures as `<asset>::<texture>`, each in the colour
    /// space its materials read it in: normal and metallic-roughness maps
    /// linear, base-colour and emissive maps (and unreferenced textures)
    /// sRGB. A texture used both ways is uploaded twice, the linear copy as
    /// `<asset>::<texture>#linear`; returns the names of those, for
    /// [`namespace_textures`].
    fn upload_model_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) -> HashSet<String> {
        let mut colour = HashSet::new();
        let mut data = HashSet::new();
        for m in &import_result.materials {
            colour.extend(m.base_color_texture.iter().chain(&m.emissive_texture));
            data.extend(m.normal_texture.iter().chain(&m.metallic_roughness_texture));
        }
        let mut both = HashSet::new();
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                let namespaced = format!("{}::{}", name, texture.name);
                let is_data = data.contains(&texture.name);
                if !is_data || colour.contains(&texture.name) {
                    cache.upload(device, queue, &namespaced, texture, false);
                }
                if is_data && colour.contains(&texture.name) {
                    cache.upload(
                        device,
                        queue,
                        &format!("{namespaced}#linear"),
                        texture,
                        true,
                    );
                    both.insert(texture.name.clone());
                } else if is_data {
                    cache.upload(device, queue, &namespaced, texture, true);
                }
            }
        }
        both
    }

    /// Upload an entity's bone matrices to the GPU.
    ///
    /// Each skinned entity gets its own storage buffer (created on first use),
    /// so several entities instancing the same asset can play different clips.
    /// The asset's shared buffer is also written so code paths that draw the
    /// asset without an entity (previews) keep animating.
    pub fn update_bone_matrices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        asset_name: &str,
        matrices: &[[[f32; 4]; 4]],
    ) {
        if self.device_lost {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(matrices);
        let capacity = crate::skinned_pipeline::MAX_BONES * std::mem::size_of::<[[f32; 4]; 4]>();
        let buffer = self
            .entity_bone_buffers
            .entry(entity_id)
            .or_insert_with(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!(
                        "{} Bone Buffer (entity {:?})",
                        asset_name, entity_id
                    )),
                    size: capacity as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
        queue.write_buffer(buffer, 0, &bytes[..bytes.len().min(capacity)]);
        let kept = &matrices[..matrices.len().min(crate::skinned_pipeline::MAX_BONES)];
        let entity = self.entity_bone_matrices.entry(entity_id).or_default();
        entity.clear();
        entity.extend_from_slice(kept);
        if let Some(asset) = self.asset_bone_matrices.get_mut(asset_name) {
            asset.clear();
            asset.extend_from_slice(kept);
        } else {
            self.asset_bone_matrices
                .insert(asset_name.to_string(), kept.to_vec());
        }
        // Its sub-meshes share one buffer
        if let Some(mesh) = self
            .mesh_cache
            .get_skinned(asset_name)
            .and_then(|meshes| meshes.first())
        {
            queue.write_buffer(&mesh.bone_buffer, 0, bytes);
        }
    }

    /// Whether per-entity styles draw only inside their entities' screen
    /// bounds (#385). On by default (`FLINT_STYLE_SCISSOR=0` starts it
    /// off); the frame is the same either way, only cheaper with it on.
    pub fn set_style_scissor(&mut self, on: bool) {
        self.style_scissor = on;
    }

    pub fn style_scissor(&self) -> bool {
        self.style_scissor
    }

    /// Where each style id's entities are on screen under `camera`, from
    /// the draws of the last [`update_from_world`](Self::update_from_world)
    /// (#385): what the per-entity style scissor reads.
    pub fn style_screen_bounds(&self, camera: &Camera) -> crate::style_mask::StyleBounds {
        self.style_bounds(&camera.view_projection_matrix())
    }

    /// Write part of an entity's own bones, from bone `first`, and nothing else.
    ///
    /// This is [`update_bone_matrices`](Self::update_bone_matrices) for a palette
    /// that changes every frame on many entities (#212): that one writes the
    /// asset's shared buffer too, which forty trees would do forty times to no
    /// effect, since each draws from its own.
    pub fn write_entity_bones(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        first: usize,
        matrices: &[[[f32; 4]; 4]],
    ) {
        if self.device_lost {
            return;
        }
        let buffer = self
            .entity_bone_buffers
            .entry(entity_id)
            .or_insert_with(|| Self::create_entity_bones(device, "Entity Bone Buffer"));
        write_bones(queue, buffer, first, matrices);
    }

    /// Give an entity's shadow its own bones, from the shadow pass's next frame,
    /// or write them again.
    pub fn write_entity_shadow_bones(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        matrices: &[[[f32; 4]; 4]],
    ) {
        if self.device_lost {
            return;
        }
        let buffer = self
            .entity_shadow_bone_buffers
            .entry(entity_id)
            .or_insert_with(|| Self::create_entity_bones(device, "Entity Shadow Bone Buffer"));
        write_bones(queue, buffer, 0, matrices);
    }

    fn create_entity_bones(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (crate::skinned_pipeline::MAX_BONES * std::mem::size_of::<[[f32; 4]; 4]>())
                as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Write part of the bones of echo `echo` of a skinned entity, from bone
    /// `first`, and say how opaque it draws. Writing an echo is what makes it
    /// exist. It then draws from the next extraction, after everything opaque,
    /// in the entity's own colours and with no depth write, in `echo` order. An
    /// entity that's never written one pays nothing.
    ///
    /// Write the whole palette first: until then its bones are zero, which puts
    /// it nowhere.
    #[allow(clippy::too_many_arguments)]
    pub fn write_entity_echo_bones(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        entity_id: flint_core::EntityId,
        echo: usize,
        opacity: f32,
        first: usize,
        matrices: &[[[f32; 4]; 4]],
    ) {
        if self.device_lost {
            return;
        }
        let Some(pipeline) = &self.skinned_pipeline else {
            return;
        };
        let echoes = self.entity_echoes.entry(entity_id).or_default();
        while echoes.len() <= echo {
            let bones = Self::create_entity_bones(device, "Entity Echo Bone Buffer");
            let bind_group =
                Self::create_bone_bind(device, &pipeline.bone_bind_group_layout, &bones);
            echoes.push(EntityEcho {
                bones,
                bind_group: Arc::new(bind_group),
                opacity: 0.0,
            });
        }
        echoes[echo].opacity = opacity;
        write_bones(queue, &echoes[echo].bones, first, matrices);
    }

    /// Stop drawing an entity's echoes from `count` on. Their buffers stay, ready
    /// for the next write, so a slider sweeping a count allocates nothing on its
    /// way back up: an opacity of 0 is what leaves one out at extraction.
    pub fn set_entity_echo_count(&mut self, entity_id: flint_core::EntityId, count: usize) {
        if let Some(echoes) = self.entity_echoes.get_mut(&entity_id) {
            for echo in echoes.iter_mut().skip(count) {
                echo.opacity = 0.0;
            }
        }
    }

    /// How an entity's echoes meet what's behind them: over it at their
    /// opacity (`false`, the default), or `lighten`, the brighter of the two,
    /// so an echo can only add light and never darken the form it's behind.
    pub fn set_entity_echo_lighten(&mut self, entity_id: flint_core::EntityId, lighten: bool) {
        if lighten {
            self.entity_echo_lighten.insert(entity_id);
        } else {
            self.entity_echo_lighten.remove(&entity_id);
        }
    }

    /// Multiply the vertex colour of one part of an entity's skinned mesh by
    /// `tint`. Materials are shared by content, so a few distinct tints cost a
    /// few materials, however many entities use them: keep them few.
    pub fn set_entity_part_tint(
        &mut self,
        entity_id: flint_core::EntityId,
        part: usize,
        tint: [f32; 3],
    ) {
        let tints = self.entity_part_tints.entry(entity_id).or_default();
        if tints.len() <= part {
            tints.resize(part + 1, [1.0; 3]);
        }
        tints[part] = tint;
    }

    /// How soft one part of an entity's skinned mesh is at its silhouette
    /// (#582), 0 to 1: above 0 its alpha falls where the surface turns away
    /// from the eye, so the outline feathers, and the part draws translucent,
    /// after everything opaque; its echoes feather with it. 0 is a hard edge,
    /// drawn exactly as with no softness at all. Shared by content like a
    /// tint, so a few distinct values cost a few materials.
    pub fn set_entity_part_softness(
        &mut self,
        entity_id: flint_core::EntityId,
        part: usize,
        softness: f32,
    ) {
        let softness = if softness.is_finite() {
            softness.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let parts = self.entity_part_softness.entry(entity_id).or_default();
        if parts.len() <= part {
            parts.resize(part + 1, 0.0);
        }
        parts[part] = softness;
    }

    /// Drop per-entity bone buffers (call when a scene is unloaded).
    pub fn clear_entity_bone_buffers(&mut self) {
        self.entity_bone_buffers.clear();
        self.entity_bone_matrices.clear();
        self.asset_bone_matrices.clear();
        self.entity_shadow_bone_buffers.clear();
        self.entity_part_tints.clear();
        self.entity_part_softness.clear();
        self.entity_echoes.clear();
        self.entity_echo_lighten.clear();
        self.skinned_bone_binds.clear();
    }

    /// Tell the renderer the scene's project and folder: where its styles
    /// find palettes, glyph sets, papers and strokes (ADR 0114). Sets
    /// `scene_dir` too. A host that only sets `scene_dir` gets the project
    /// found from that folder, once.
    pub fn set_asset_scope(&mut self, scope: flint_core::project::AssetScope) {
        self.scene_dir = Some(scope.scene_dir.clone());
        *self.asset_scope.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((self.scene_dir.clone(), Arc::new(scope)));
    }

    /// The project and folder the styles' files are searched from: the
    /// scope the host set, else the project found from `scene_dir`, else
    /// (no scene) the working directory's.
    pub fn asset_scope(&self) -> Arc<flint_core::project::AssetScope> {
        let mut kept = self.asset_scope.lock().unwrap_or_else(|e| e.into_inner());
        match &*kept {
            Some((dir, scope)) if *dir == self.scene_dir => scope.clone(),
            _ => {
                let dir = self.scene_dir.as_deref().unwrap_or(Path::new("."));
                let scope = Arc::new(flint_core::project::AssetScope::for_dir(dir));
                *kept = Some((self.scene_dir.clone(), scope.clone()));
                scope
            }
        }
    }

    /// Load the font a `ui_text.font` names (`builtin`, a glyph set, or a
    /// `.font.toml`; ADR 0113) and upload its atlas to the texture cache,
    /// once per name and scope. Returns whether the font is ready to draw.
    ///
    /// A name is only a name within one scope: after a transition into
    /// another folder, `pixel` is that folder's `fonts/pixel.glyphs.toml`.
    /// So a load from a scope other than the last one's forgets every font
    /// loaded before it, their atlases too (#312).
    pub fn load_ui_font(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spec: &str,
        scope: &flint_core::project::AssetScope,
    ) -> bool {
        if self.bitmap_font_scope.as_ref() != Some(scope) {
            self.forget_ui_fonts();
            self.bitmap_font_scope = Some(scope.clone());
        }
        if let Some(found) = self.bitmap_font_cache.get(spec) {
            return found.is_some();
        }
        let font = match BitmapFont::resolve(spec, scope) {
            Ok(font) => match &mut self.texture_cache {
                Some(cache) => {
                    let (rgba, [w, h]) = font.rgba();
                    match cache.upload_rgba(device, queue, &font.texture, w, h, &rgba, false) {
                        Ok(_) => Some(font),
                        Err(e) => {
                            tracing::warn!("ui_text font `{spec}`: {e}");
                            None
                        }
                    }
                }
                None => None,
            },
            Err(e) => {
                tracing::warn!("ui_text font `{spec}`: {e}");
                None
            }
        };
        let ready = font.is_some();
        self.bitmap_font_cache.insert(spec.to_string(), font);
        ready
    }

    /// Drop every loaded `ui_text` font and its uploaded atlas, so the next
    /// [`load_ui_font`](Self::load_ui_font) reads and uploads afresh (the
    /// texture cache keeps a name's first upload otherwise).
    fn forget_ui_fonts(&mut self) {
        for font in self.bitmap_font_cache.drain().filter_map(|(_, f)| f) {
            if let Some(cache) = &mut self.texture_cache {
                cache.remove_texture(&font.texture);
            }
        }
        self.bitmap_font_scope = None;
    }

    /// The `ui_text` font `spec` names, if it loaded and its atlas is
    /// uploaded: what the scene's `ui_text` draws with.
    pub fn loaded_ui_font(&self, spec: &str) -> Option<&BitmapFont> {
        self.ui_font(self.texture_cache.as_ref()?, spec)
    }

    /// A loaded `ui_text` font whose atlas is in `tex_cache`.
    pub(super) fn ui_font(&self, tex_cache: &TextureCache, spec: &str) -> Option<&BitmapFont> {
        self.bitmap_font_cache
            .get(spec)?
            .as_ref()
            .filter(|f| tex_cache.contains(&f.texture))
    }

    /// Load a texture from an image file into the texture cache
    pub fn load_texture_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.load_file(device, queue, name, path)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load a texture from an image file with a full mip chain (see
    /// [`TextureCache::load_file_mipmapped`](crate::TextureCache::load_file_mipmapped)).
    pub fn load_texture_file_mipmapped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.load_file_mipmapped(device, queue, name, path)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load raw RGBA8 pixel data into the texture cache.
    ///
    /// Returns `Ok(true)` if newly uploaded, `Ok(false)` if already cached.
    pub fn load_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.upload_rgba(device, queue, name, width, height, data, is_normal_map)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Create or replace the named mesh from raw vertex/index data.
    ///
    /// Any entity whose `model.asset` is `name` draws it from the next
    /// `update_from_world`. Replacing a mesh frees the old GPU buffers once
    /// the current frame's draw lists let go of them, so a mesh can be
    /// rebuilt as often as needed without growing GPU memory. Vertex colours
    /// are used when `material.use_vertex_color` is set; they are linear,
    /// so convert sRGB colours with [`crate::srgb_to_linear`].
    pub fn load_procedural_mesh(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        vertices: &[crate::primitives::Vertex],
        indices: &[u32],
        material: flint_import::ImportedMaterial,
    ) {
        self.ray_meshes.remove(name);
        self.mesh_cache
            .upload_procedural(device, name, vertices, indices, material);
    }

    /// [`load_procedural_mesh`](Self::load_procedural_mesh) for a mesh of
    /// several parts, each with its own material, all drawn under `name`.
    pub fn load_procedural_mesh_parts(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        parts: Vec<(
            Vec<crate::primitives::Vertex>,
            Vec<u32>,
            flint_import::ImportedMaterial,
        )>,
    ) {
        self.ray_meshes.remove(name);
        self.mesh_cache.upload_procedural_parts(device, name, parts);
    }

    /// Upload raw RGBA8 pixels as the texture `name`, replacing one already
    /// there. Cached material binds name their textures, so they are dropped
    /// and remade on the next `update_from_world` to pick up the new one:
    /// fine for a swap (a placeholder giving way), costly every frame.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<(), String> {
        self.remove_texture(name);
        self.load_texture_rgba(device, queue, name, width, height, data, is_normal_map)
            .map(|_| ())
    }

    /// Remove the texture `name`, if loaded, and drop the cached material
    /// binds that may hold it. Returns whether it was there.
    pub fn remove_texture(&mut self, name: &str) -> bool {
        let Some(cache) = &mut self.texture_cache else {
            return false;
        };
        if !cache.contains(name) {
            return false;
        }
        cache.remove_texture(name);
        self.draw_bind_cache.clear();
        true
    }

    /// Make the mesh `name` a ray target (ADR 0105): from the next
    /// `update_from_world`, every entity drawing it places `mesh` in
    /// [`ray_scene`](Self::ray_scene) at its world matrix. Build `mesh` from
    /// the positions and indices the mesh was uploaded with. `None` stops it
    /// being one. Uploading the mesh again, or removing it, also stops it,
    /// so set it again after each upload.
    pub fn set_ray_mesh(&mut self, name: &str, mesh: Option<Arc<flint_core::ray_mesh::RayMesh>>) {
        match mesh {
            Some(m) => {
                self.ray_meshes.insert(name.to_string(), m);
            }
            None => {
                self.ray_meshes.remove(name);
            }
        }
    }

    /// The ray targets as the last `update_from_world` found them: each
    /// entity drawing a mesh made one with [`set_ray_mesh`](Self::set_ray_mesh),
    /// at its world matrix (ADR 0105). Cast rays through it to find what's
    /// drawn along a line. A new scene is made each update, so a clone held
    /// across frames goes on answering for the frame it came from.
    pub fn ray_scene(&self) -> Arc<flint_core::ray_mesh::RayScene> {
        self.ray_scene.clone()
    }

    /// Remove the named mesh. Returns whether it existed. Entities still
    /// naming it in `model.asset` draw nothing (or their `bounds` box, if
    /// they have one) until a mesh with that name is uploaded again (ADR 0074).
    pub fn remove_mesh(&mut self, name: &str) -> bool {
        self.ray_meshes.remove(name);
        self.mesh_cache.remove(name)
    }

    /// Get a mutable reference to the mesh cache
    pub fn mesh_cache_mut(&mut self) -> &mut MeshCache {
        &mut self.mesh_cache
    }

    /// Get a reference to the mesh cache
    pub fn mesh_cache(&self) -> &MeshCache {
        &self.mesh_cache
    }

    /// Read-only access to the current debug state
    pub fn debug_state(&self) -> &DebugState {
        &self.debug_state
    }

    /// Mutable access to the debug state
    pub fn debug_state_mut(&mut self) -> &mut DebugState {
        &mut self.debug_state
    }

    /// Set the shading debug mode
    pub fn set_debug_mode(&mut self, mode: DebugMode) {
        self.debug_state.mode = mode;
    }

    /// Collect rendering statistics for the current frame.
    ///
    /// Timing (`fps`, `frame_time_ms`) and `resolution` are left at zero —
    /// the caller fills these from their own timing and context.
    /// `scene_resolution` is the renderer's own.
    pub fn collect_stats(&self) -> crate::render_stats::RenderStats {
        use crate::grass_pipeline::BLADE_INDEX_COUNT;

        let entity_draws = self.entity_draws.len() as u32;
        let skinned_draws = self.skinned_entity_draws.len() as u32;
        let terrain_draws = self.terrain_visible_chunks;
        let echo_draws = self.echo_draws.len() as u32;
        let transparent_draws = (self.transparent_draws.len()
            + self.transparent_skinned_draws.len()
            + self.echo_draws.len()) as u32;
        let billboard_draws = self.billboard_draws.len() as u32;
        let particle_draws = self.particle_draws.len() as u32;
        let sprite_batches = self.sprite2d_batches.len() as u32;
        let grass_draw_calls = if self.grass_drawing { 1u32 } else { 0 };

        let draw_calls = entity_draws
            + skinned_draws
            + terrain_draws
            + transparent_draws
            + billboard_draws
            + particle_draws
            + sprite_batches
            + grass_draw_calls;

        // Triangles: sum index_count/3 for types that have it, fixed counts for others
        let mut triangles: u32 = 0;
        for d in &self.entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.skinned_entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_skinned_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.echo_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.terrain_draws {
            triangles += d.index_count / 3;
        }
        // Billboards: each is a fixed quad (2 triangles)
        triangles += billboard_draws * 2;
        // Particles: instanced quads (2 triangles per instance)
        let particle_instances: u32 = self.particle_draws.iter().map(|d| d.instance_count).sum();
        triangles += particle_instances * 2;
        // Sprites: instanced quads (2 triangles per instance)
        let sprite_instances: u32 = self.sprite2d_batches.iter().map(|b| b.instance_count).sum();
        triangles += sprite_instances * 2;
        // Grass: BLADE_INDEX_COUNT indices per instance
        triangles += self.grass_instance_count * BLADE_INDEX_COUNT / 3;

        // Shadow stats: estimate as main pass × the cascades drawn, counting
        // only the draws that cast (ADR 0100)
        let cascade_count = match &self.shadow_pass {
            Some(sp) if self.shadows_drawn() => sp.cascades() as u32,
            _ => 0,
        };
        let casting = self.entity_draws.iter().filter(|d| d.casts_shadow);
        let casting_skinned =
            Self::skinned_shadow_draws(&self.skinned_entity_draws, &self.transparent_skinned_draws);
        let shadow_entity_draws =
            (casting.clone().count() + casting_skinned.clone().count()) as u32 + terrain_draws;
        let shadow_draw_calls = shadow_entity_draws * cascade_count;
        let shadow_triangles = {
            let mut t: u32 = 0;
            for d in casting {
                t += d.index_count / 3;
            }
            for d in casting_skinned {
                t += d.index_count / 3;
            }
            for d in &self.terrain_draws {
                t += d.index_count / 3;
            }
            t * cascade_count
        };

        let style_memory = self.style_target_summary();
        let fxaa_target = self
            .postprocess_resources
            .as_ref()
            .is_some_and(|r| r.fxaa.is_some()) as u32;
        crate::render_stats::RenderStats {
            draw_calls,
            triangles,
            echo_draws,
            entity_draws,
            culled_draws: (self.culled_entity_draws.len() + self.culled_transparent_draws.len())
                as u32,
            lod_culled_draws: self.lod_culled,
            skinned_draws,
            terrain_draws,
            terrain_total_chunks: self.terrain_total_chunks,
            transparent_draws,
            billboard_draws,
            particle_draws,
            particle_instances,
            sprite_batches,
            grass_instances: self.grass_instance_count,
            grass_draw_calls,
            shadow_draw_calls,
            shadow_triangles,
            gpu_passes: self
                .gpu_timer
                .as_ref()
                .map(|t| t.timings().passes.clone())
                .unwrap_or_default(),
            gpu_frame_ms: self
                .gpu_timer
                .as_ref()
                .map_or(0.0, |t| t.timings().frame_ms),
            scene_resolution: self.scene_size().into(),
            style_targets: style_memory.count,
            style_target_bytes: style_memory.bytes,
            full_size_targets: style_memory.full_size + fxaa_target,
            ..Default::default()
        }
    }

    /// The style chain's textures (#302): see
    /// [`StyleTargets::summary`](crate::stylize::StyleTargets::summary).
    pub fn style_target_summary(&self) -> crate::stylize::TargetSummary {
        self.postprocess_resources
            .as_ref()
            .map(|r| {
                r.styles.summary(
                    self.surface_format,
                    self.surface_size.0,
                    self.surface_size.1,
                )
            })
            .unwrap_or_default()
    }

    /// Each of the style chain's textures with its size and bytes (#412):
    /// the built-ins' and every user style's targets (`<style>.<target>`,
    /// `.a`/`.b` for a history pair), sorted by name.
    pub fn style_target_entries(&self) -> Vec<crate::stylize::TargetEntry> {
        self.postprocess_resources
            .as_ref()
            .map(|r| r.styles.entries())
            .unwrap_or_default()
    }

    /// Time the frame's phases on the GPU, or stop (ADR 0108): the grass
    /// compute, the shadows, the scene, the post-process chain, its last
    /// pass (the composite, or FXAA when it's on) and the world lines, each
    /// a scope, into [`RenderStats::gpu_passes`]. Off by default. The
    /// device must have been created with `Features::TIMESTAMP_QUERY`
    /// (Flint's contexts ask for it where the adapter has it); without it
    /// timing stays off. Returns whether timing is on.
    ///
    /// The host ends each frame with
    /// [`finish_gpu_timings`](Self::finish_gpu_timings), after its last
    /// timed submission.
    ///
    /// [`RenderStats::gpu_passes`]: crate::RenderStats::gpu_passes
    pub fn set_gpu_timing(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, on: bool) -> bool {
        if !on {
            self.gpu_timer = None;
        } else if self.gpu_timer.is_none() {
            self.gpu_timer = crate::gpu_timer::GpuTimer::new(device, queue);
        }
        self.gpu_timer.is_some()
    }

    /// Whether the frame's phases are being timed.
    pub fn gpu_timing(&self) -> bool {
        self.gpu_timer.is_some()
    }

    /// The timer, while timing is on, for a host to time its own work (its
    /// UI, say) in the same frame: [`GpuTimer::begin`] and
    /// [`GpuTimer::end`] round it.
    ///
    /// [`GpuTimer::begin`]: crate::GpuTimer::begin
    /// [`GpuTimer::end`]: crate::GpuTimer::end
    pub fn gpu_timer_mut(&mut self) -> Option<&mut crate::gpu_timer::GpuTimer> {
        self.gpu_timer.as_mut()
    }

    /// End the frame's GPU timing: pick up the times read back so far and
    /// send this frame's back. Call once a frame after the last timed
    /// submission, before presenting. Does nothing while timing is off.
    pub fn finish_gpu_timings(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(timer) = &mut self.gpu_timer {
            timer.finish_frame(device, queue);
        }
    }

    /// [`finish_gpu_timings`](Self::finish_gpu_timings) in the frame's own
    /// encoder, after its last timed scope: the resolve rides the frame's
    /// one submission, and [`frame_submitted`](Self::frame_submitted)
    /// starts the read-back (#139).
    pub fn encode_gpu_timings(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(timer) = &mut self.gpu_timer {
            timer.resolve_into(device, encoder);
        }
    }

    /// Toggle normal direction arrows on/off, returns the new state
    pub fn toggle_normal_arrows(&mut self) -> bool {
        self.debug_state.show_normals = !self.debug_state.show_normals;
        self.debug_state.show_normals
    }

    /// Toggle skeleton overlay on/off, returns the new state
    pub fn toggle_skeleton_overlay(&mut self) -> bool {
        self.debug_state.show_skeleton = !self.debug_state.show_skeleton;
        self.debug_state.show_skeleton
    }

    /// Set the skeleton overlay mesh (line-list geometry for bone visualization)
    pub fn set_skeleton_overlay(&mut self, device: &wgpu::Device, mesh: &crate::primitives::Mesh) {
        self.skeleton_overlay_draws.clear();
        if mesh.indices.is_empty() {
            return;
        }
        let tex_cache = self.texture_cache.as_ref().expect("texture cache");
        let draw = Self::create_draw_call(
            device,
            &self.pipeline,
            mesh,
            true,
            TransformUniforms::new(),
            MaterialUniforms::procedural(),
            tex_cache,
        );
        self.skeleton_overlay_draws.push(draw);
    }

    /// Clear skeleton overlay draw calls
    pub fn clear_skeleton_overlay(&mut self) {
        self.skeleton_overlay_draws.clear();
    }

    /// Show or hide the ground-plane grid at runtime
    pub fn set_show_grid(&mut self, device: &wgpu::Device, show: bool) {
        if show && self.grid_draw.is_none() {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            let tc = self
                .texture_cache
                .as_ref()
                .expect("texture cache must exist");
            self.grid_draw = Some(Self::create_draw_call(
                device,
                &self.pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                tc,
            ));
        } else if !show {
            self.grid_draw = None;
        }
    }

    /// Whether the ground-plane grid is currently visible
    pub fn show_grid(&self) -> bool {
        self.grid_draw.is_some()
    }

    /// Enable or disable tone mapping
    pub fn set_tonemapping(&mut self, enabled: bool) {
        self.tonemapping_enabled = enabled;
    }

    /// Enable or disable shadow mapping
    pub fn set_shadows(&mut self, enabled: bool) {
        if let Some(sp) = &mut self.shadow_pass {
            sp.enabled = enabled;
        }
    }

    /// Recreate the shadow pass with a different resolution
    pub fn set_shadow_resolution(&mut self, device: &wgpu::Device, resolution: u32) {
        let was_enabled = self.shadow_pass.as_ref().map_or(false, |sp| sp.enabled);
        let mut shadow_pass = ShadowPass::new(device, resolution);
        shadow_pass.enabled = was_enabled;
        if let Some(old) = &self.shadow_pass {
            shadow_pass.set_cascades(old.cascades());
            shadow_pass.set_distance(old.distance());
        }

        // Recreate light bind group with new shadow pass resources
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(device, &self.pipeline, &self.light_uniforms, &shadow_pass);
        self.light_buffer = light_buffer;
        self.light_bind_group = light_bind_group;
        self.shadow_pass = Some(shadow_pass);
    }

    /// Draw no skybox until the next [`load_skybox`](Self::load_skybox): a
    /// scene that names none starts without the last scene's (#723).
    pub fn clear_skybox(&mut self) {
        self.skybox_uniform_buffer = None;
        self.skybox_uniform_bind_group = None;
        self.skybox_texture_bind_group = None;
    }

    /// Whether a skybox is loaded and will be drawn.
    pub fn has_skybox(&self) -> bool {
        self.skybox_texture_bind_group.is_some()
    }

    /// Load an equirectangular panorama image as a skybox
    pub fn load_skybox(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) {
        let skybox_pipeline = match &self.skybox_pipeline {
            Some(p) => p,
            None => {
                tracing::warn!("Skybox pipeline not available");
                return;
            }
        };

        // Load panorama image
        let img = match open_image(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("Failed to load skybox '{}': {:?}", path.display(), e);
                return;
            }
        };
        let (width, height) = img.dimensions();

        // Create GPU texture
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Skybox Panorama Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            texture_size,
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Skybox Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skybox Uniform Buffer"),
            contents: bytemuck::cast_slice(&[SkyboxUniforms {
                inv_view_proj: identity_matrix(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create bind groups
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("Skybox Uniform Bind Group"),
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("Skybox Texture Bind Group"),
        });

        self.skybox_uniform_buffer = Some(uniform_buffer);
        self.skybox_uniform_bind_group = Some(uniform_bind_group);
        self.skybox_texture_bind_group = Some(texture_bind_group);

        println!("Loaded skybox: {} ({}x{})", path.display(), width, height);
    }

    pub fn toggle_shadows(&mut self) -> bool {
        if let Some(sp) = &mut self.shadow_pass {
            sp.enabled = !sp.enabled;
            sp.enabled
        } else {
            false
        }
    }

    /// Whether shadow rendering is currently enabled
    pub fn shadows_enabled(&self) -> bool {
        self.shadow_pass.as_ref().is_some_and(|sp| sp.enabled)
    }

    /// The scene's `[environment] shadows` (#627). False keeps the shadow
    /// pass from drawing at all, over the quality tier and F4,
    /// which keep their own setting for the next scene.
    pub fn set_scene_shadows(&mut self, on: bool) {
        self.scene_shadows = on;
    }

    /// The scene's `[environment] default_lights` (#627). False lights a
    /// scene with no light entities by its ambient alone (and emission),
    /// instead of the built-in key and fill. Takes effect at the next
    /// `update_from_world`.
    pub fn set_default_lights(&mut self, on: bool) {
        self.default_lights = on;
    }

    /// Whether this frame draws the shadow cascades (#627): shadows are on,
    /// the scene allows them, and its strongest directional light gives
    /// light or god rays. A sun of intensity 0 contributes nothing the
    /// maps could darken, so its pass is skipped and every caster with it.
    pub fn shadows_drawn(&self) -> bool {
        self.shadows_enabled() && self.scene_shadows && self.sun_casts()
    }

    /// The light the cascades follow, directional 0 after the sort that
    /// puts the strongest first, gives light or god rays.
    fn sun_casts(&self) -> bool {
        self.light_uniforms.directional_count > 0 && {
            let sun = &self.light_uniforms.directional_lights[0];
            sun.intensity > 0.0 || sun.volumetric_intensity > 0.0
        }
    }

    /// Current shadow map resolution per cascade
    pub fn shadow_resolution(&self) -> u32 {
        self.shadow_pass
            .as_ref()
            .map_or(crate::shadow::DEFAULT_SHADOW_RESOLUTION, |sp| sp.resolution)
    }

    fn default_archetype_visuals() -> HashMap<String, ArchetypeVisual> {
        let mut archetype_visuals = HashMap::new();

        archetype_visuals.insert(
            "room".to_string(),
            ArchetypeVisual {
                color: [0.27, 0.53, 1.0, 0.5],
                wireframe: true,
                default_size: [10.0, 4.0, 10.0],
            },
        );

        archetype_visuals.insert(
            "door".to_string(),
            ArchetypeVisual {
                color: [1.0, 0.53, 0.27, 1.0],
                wireframe: false,
                default_size: [1.0, 2.0, 0.1],
            },
        );

        archetype_visuals.insert(
            "furniture".to_string(),
            ArchetypeVisual {
                color: [0.27, 1.0, 0.53, 1.0],
                wireframe: false,
                default_size: [1.0, 1.0, 1.0],
            },
        );

        archetype_visuals.insert(
            "character".to_string(),
            ArchetypeVisual {
                color: [1.0, 1.0, 0.27, 1.0],
                wireframe: false,
                default_size: [0.5, 1.8, 0.5],
            },
        );

        archetype_visuals.insert(
            "wall".to_string(),
            ArchetypeVisual {
                color: [0.76, 0.70, 0.60, 1.0],
                wireframe: false,
                default_size: [10.0, 4.0, 0.3],
            },
        );

        archetype_visuals.insert(
            "floor".to_string(),
            ArchetypeVisual {
                color: [0.55, 0.55, 0.52, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "ceiling".to_string(),
            ArchetypeVisual {
                color: [0.65, 0.62, 0.58, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "pillar".to_string(),
            ArchetypeVisual {
                color: [0.70, 0.65, 0.55, 1.0],
                wireframe: false,
                default_size: [0.5, 4.0, 0.5],
            },
        );

        archetype_visuals
    }

    /// Set visual representation for an archetype
    pub fn set_archetype_visual(&mut self, archetype: &str, visual: ArchetypeVisual) {
        self.archetype_visuals.insert(archetype.to_string(), visual);
    }

    // ── Slim update_from_world: dispatch loop calling extraction helpers ──

    /// Update meshes from the world state
    #[profiling::function]
    pub fn update_from_world(&mut self, world: &FlintWorld, device: &wgpu::Device) {
        if self.device_lost {
            return;
        }
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.transparent_draws.clear();
        self.transparent_skinned_draws.clear();
        self.echo_draws.clear();
        self.culled_entity_draws.clear();
        self.culled_transparent_draws.clear();
        self.billboard_draws.clear();
        self.sprite2d_batches.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();
        self.extract_frame = self.extract_frame.wrapping_add(1);

        // The hierarchy, for the selection outline's subtree (#602)
        self.entity_children.clear();
        for entity in world.all_entities() {
            if let Some(parent) = world.get_parent(entity.id) {
                self.entity_children
                    .entry(parent)
                    .or_default()
                    .push(entity.id);
            }
        }

        self.sync_debug_styles();
        // Ids for every styled entity: they say which styles are on for
        // entities alone (#266) as well as what the mask holds (#263)
        if self.postprocess_resources.is_some() && self.postprocess_config.enabled {
            crate::style_mask::resolve_blended(
                world,
                &self.postprocess_config,
                &mut self.style_ids,
                &mut self.style_by_entity,
                &mut self.style_blend_by_entity,
            );
        } else {
            self.style_ids.set(Vec::new());
            self.style_by_entity.clear();
            self.style_blend_by_entity.clear();
        }

        // Extract lights from scene entities
        self.extract_lights_from_world(world);

        // Extract ocean params (regenerates the wave spectrum only on change)
        self.extract_ocean_from_world(world);
        self.extract_ocean_contact_from_world(world);
        self.extract_water_bodies_from_world(world, device);
        if self.water_active() {
            // Ocean always binds group 3; make sure the placeholder exists
            // even on paths where the grab pass never runs.
            self.ensure_ocean_grab_dummy(device);
        }

        // Extract procedural sky params (+ optional ambient override)
        self.extract_sky_from_world(world);

        let need_overlay = self.debug_state.mode == DebugMode::WireframeOverlay
            || self.debug_state.mode == DebugMode::WireframeOnly;
        let need_normals = self.debug_state.show_normals;
        let arrow_length = self.debug_state.normal_arrow_length;

        // Collect sprite2d instances for batching (texture_name, layer, instance data)
        let mut sprite2d_collected: Vec<(String, i32, Sprite2dInstanceGpu)> = Vec::new();

        // Temporarily take texture_cache to avoid borrow conflicts
        let tex_cache = self.texture_cache.take();
        let tex_cache_ref = tex_cache.as_ref().unwrap();

        // The ray targets, placed afresh (ADR 0105).
        let mut ray_scene = flint_core::ray_mesh::RayScene::new();

        // What `visibility { visible = false }` hides, descendants included
        // (#628): empty and unwalked in a scene that never hides anything.
        let hidden = world.hidden_entities();

        for entity in world.all_entities() {
            if hidden.contains(&entity.id) {
                continue;
            }
            let archetype = entity.archetype.as_deref().unwrap_or("unknown");
            let visual =
                self.archetype_visuals
                    .get(archetype)
                    .cloned()
                    .unwrap_or(ArchetypeVisual {
                        color: [0.5, 0.5, 0.5, 1.0],
                        wireframe: false,
                        default_size: [1.0, 1.0, 1.0],
                    });

            let model_matrix = world
                .get_world_matrix(entity.id)
                .unwrap_or_else(|| Transform::default().to_matrix());
            let world_pos = [model_matrix[3][0], model_matrix[3][1], model_matrix[3][2]];

            // Check if entity has a model component
            let model_asset = world
                .get_components(entity.id)
                .and_then(|components| components.get(comp::MODEL).cloned())
                .and_then(|model| {
                    model
                        .get("asset")
                        .and_then(|v| v.as_str().map(String::from))
                });

            if let Some(asset_name) = &model_asset {
                if let Some(mesh) = self.ray_meshes.get(asset_name) {
                    ray_scene.add(entity.id, mesh.clone(), model_matrix);
                }

                // Check for skinned meshes first
                if self.extract_skinned_entity(
                    device,
                    tex_cache_ref,
                    world,
                    entity.id,
                    asset_name,
                    model_matrix,
                    need_overlay,
                ) {
                    continue;
                }

                // Check for standard (non-skinned) model
                if self.extract_model_entity(
                    device,
                    tex_cache_ref,
                    world,
                    entity.id,
                    asset_name,
                    model_matrix,
                    need_overlay,
                    need_normals,
                    arrow_length,
                ) {
                    continue;
                }
            }

            // Check for sprite component — render as billboard or 2D sprite
            if let Some(components) = world.get_components(entity.id) {
                if let Some(sprite) = components.get(comp::SPRITE) {
                    self.extract_sprite_entity(
                        device,
                        tex_cache_ref,
                        world,
                        entity.id,
                        components,
                        sprite,
                        world_pos,
                        &mut sprite2d_collected,
                    );
                    continue;
                }
            }

            // Standalone ui_text: entities with ui_text but no sprite
            if let Some(components) = world.get_components(entity.id) {
                if self.extract_ui_text_entity(
                    tex_cache_ref,
                    entity.id,
                    components,
                    world_pos,
                    &mut sprite2d_collected,
                ) {
                    continue;
                }
            }

            // Only draw fallback geometry for entities that explicitly have bounds
            if let Some(components) = world.get_components(entity.id) {
                let has_bounds = components.get(comp::BOUNDS).is_some();
                if !has_bounds {
                    continue;
                }
            }

            // Fall back to procedural bounds geometry
            self.extract_bounds_entity(
                device,
                tex_cache_ref,
                world,
                entity.id,
                &visual,
                model_matrix,
                need_overlay,
                need_normals,
                arrow_length,
            );
        }

        // Sort & batch sprite2d instances
        self.batch_sprite2d_instances(device, tex_cache_ref, sprite2d_collected);

        self.apply_shallow_wooze(device);

        // Drop cached resources nothing used this frame (despawned entities,
        // entities that changed shape/model).
        let frame = self.extract_frame;
        self.draw_bind_cache.retain_frame(frame);
        self.skinned_bone_binds.retain(|_, b| b.touched == frame);
        self.ray_scene = Arc::new(ray_scene);

        // Put texture cache back
        self.texture_cache = tex_cache;
    }

    /// Replace which entities have shallow wooze this frame, and how far along
    /// each is (#215). What's named here shows from the next `update_from_world`.
    /// An empty list, the default, costs a scene nothing.
    pub fn set_shallow_wooze(
        &mut self,
        draws: impl IntoIterator<Item = (flint_core::EntityId, ShallowWoozeDraw)>,
    ) {
        self.shallow_wooze.clear();
        self.shallow_wooze.extend(draws);
    }

    /// Which of shallow wooze's pipelines exist: standard, skinned and shadow.
    #[cfg(test)]
    pub(crate) fn wooze_built(&self) -> (bool, bool, bool) {
        (
            self.wooze_standard.is_some(),
            self.wooze_skinned.is_some(),
            self.shadow_pass
                .as_ref()
                .is_some_and(|sp| sp.wooze.is_some()),
        )
    }

    /// Give this frame's draws of woozy entities their drift, grow their bounds to
    /// match, and build the pipelines they need the first time they need them.
    fn apply_shallow_wooze(&mut self, device: &wgpu::Device) {
        if self.shallow_wooze.is_empty() {
            return;
        }
        let map = &self.shallow_wooze;
        let drift = |id: Option<flint_core::EntityId>| Some(map.get(&id?)?.uniforms());

        let mut any_standard = false;
        for draw in self
            .entity_draws
            .iter_mut()
            .chain(self.transparent_draws.iter_mut())
            .filter(|d| !d.is_wireframe)
        {
            let Some(wooze) = drift(draw.entity_id) else {
                continue;
            };
            if let Some(bounds) = &mut draw.world_aabb {
                grow_bounds(bounds, &wooze);
            }
            draw.wooze = Some(wooze);
            any_standard = true;
        }

        let mut any_skinned = false;
        for draw in self
            .skinned_entity_draws
            .iter_mut()
            .chain(self.transparent_skinned_draws.iter_mut())
        {
            let Some(wooze) = drift(draw.entity_id) else {
                continue;
            };
            draw.wooze = Some(wooze);
            any_skinned = true;
        }

        if any_standard && self.wooze_standard.is_none() {
            self.wooze_standard = Some(WoozePipelineSet::standard(device, &self.pipeline));
        }
        if any_skinned && self.wooze_skinned.is_none() {
            if let Some(skinned) = &self.skinned_pipeline {
                self.wooze_skinned =
                    Some(WoozePipelineSet::skinned(device, &self.pipeline, skinned));
            }
        }
        if any_standard || any_skinned {
            if let Some(shadow_pass) = &mut self.shadow_pass {
                shadow_pass.ensure_wooze(device);
            }
        }
    }

    /// Transform + material resources for one draw of `key`, created on
    /// first use and reused afterwards; the material side is looked up again
    /// when `material_key` changes, and shared with every other draw of the
    /// same key (see `SharedMaterial`). Free-standing over the cache and pipeline so
    /// callers holding other `self` fields borrowed can still use it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cached_binds<'a>(
        cache: &'a mut DrawBindCache,
        frame: u64,
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        key: (flint_core::EntityId, u32),
        transform_uniforms: &TransformUniforms,
        material_uniforms: &MaterialUniforms,
        material_key: u64,
        base_color: &wgpu::TextureView,
        normal: &wgpu::TextureView,
        metallic_roughness: &wgpu::TextureView,
        emissive: &wgpu::TextureView,
    ) -> &'a CachedDrawBinds {
        let (mb, mbg) = Self::shared_material(
            cache,
            frame,
            device,
            pipeline,
            material_uniforms,
            material_key,
            base_color,
            normal,
            metallic_roughness,
            emissive,
        );
        let arena = &mut cache.arena;
        let entry = cache.draws.entry(key).or_insert_with(|| {
            let (slot, page) = arena.alloc(device);
            arena.stage(slot, bytemuck::bytes_of(transform_uniforms));
            let tbg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &page,
                        offset: slot.offset(),
                        // The whole slot: a plain draw reads its first 208
                        // bytes, and shallow wooze's the 48 past them too.
                        size: wgpu::BufferSize::new(crate::bind_cache::SLOT_STRIDE),
                    }),
                }],
                label: Some("Transform Bind Group (arena)"),
            });
            CachedDrawBinds {
                transform: TransformTarget::Slot(slot),
                transform_bind_group: Arc::new(tbg),
                material_buffer: mb.clone(),
                material_bind_group: mbg.clone(),
                material_key,
                touched: frame,
            }
        });
        if entry.material_key != material_key {
            entry.material_buffer = mb;
            entry.material_bind_group = mbg;
            entry.material_key = material_key;
        }
        entry.touched = frame;
        entry
    }

    /// The material for `material_key`, made on first use and shared with every
    /// draw of that key.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shared_material(
        cache: &mut DrawBindCache,
        frame: u64,
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        material_uniforms: &MaterialUniforms,
        material_key: u64,
        base_color: &wgpu::TextureView,
        normal: &wgpu::TextureView,
        metallic_roughness: &wgpu::TextureView,
        emissive: &wgpu::TextureView,
    ) -> (Arc<MaterialBuffer>, Arc<wgpu::BindGroup>) {
        let shared = cache.materials.entry(material_key).or_insert_with(|| {
            let (b, bg) = Self::create_material_bind_with_textures(
                device,
                pipeline,
                material_uniforms,
                base_color,
                normal,
                metallic_roughness,
                emissive,
            );
            SharedMaterial {
                buffer: Arc::new(MaterialBuffer::new(b)),
                bind_group: Arc::new(bg),
                touched: frame,
            }
        });
        shared.touched = frame;
        (shared.buffer.clone(), shared.bind_group.clone())
    }

    /// The uploaded box for `key`, building it with `build` on a miss.
    pub(super) fn cached_box_mesh<'a>(
        cache: &'a mut HashMap<u64, CachedBoxMesh>,
        device: &wgpu::Device,
        key: u64,
        build: impl FnOnce() -> crate::primitives::Mesh,
    ) -> &'a CachedBoxMesh {
        cache.entry(key).or_insert_with(|| {
            let mesh = build();
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Box Vertex Buffer (cached)"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Box Index Buffer (cached)"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            CachedBoxMesh {
                vertex_buffer: Arc::new(vertex_buffer),
                index_buffer: Arc::new(index_buffer),
                index_count: mesh.indices.len() as u32,
            }
        })
    }

    /// A draw call over cached geometry and cached binds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_from_cache(
        vertex_buffer: &Arc<wgpu::Buffer>,
        index_buffer: &Arc<wgpu::Buffer>,
        index_count: u32,
        is_wireframe: bool,
        binds: &CachedDrawBinds,
        model: [[f32; 4]; 4],
        model_inv_transpose: [[f32; 4]; 4],
        entity_id: flint_core::EntityId,
        blend_mode: BlendMode,
    ) -> DrawCall {
        DrawCall {
            vertex_buffer: vertex_buffer.clone(),
            index_buffer: index_buffer.clone(),
            index_count,
            is_wireframe,
            casts_shadow: true,
            transform: binds.transform.clone(),
            transform_bind_group: binds.transform_bind_group.clone(),
            material_buffer: binds.material_buffer.clone(),
            material_bind_group: binds.material_bind_group.clone(),
            model,
            model_inv_transpose,
            entity_id: Some(entity_id),
            blend_mode,
            sort_depth: 0.0,
            world_aabb: None,
            style_aabb: None,
            lod: None,
            wooze: None,
            double_sided: false,
        }
    }

    /// The distance levers in force (#56).
    pub fn lod_settings(&self) -> LodSettings {
        self.lod_settings
    }

    /// Change the distance levers (#56): takes effect at the next
    /// `render_to`, no reload. Scales are clamped to
    /// [`LodSettings::MIN_SCALE`] and non-finite values are ignored.
    pub fn set_lod_settings(&mut self, settings: LodSettings) {
        let clamp = |v: f32, old: f32| {
            if v.is_finite() {
                v.max(LodSettings::MIN_SCALE)
            } else {
                old
            }
        };
        self.lod_settings = LodSettings {
            lod_scale: clamp(settings.lod_scale, self.lod_settings.lod_scale),
            draw_distance_scale: clamp(
                settings.draw_distance_scale,
                self.lod_settings.draw_distance_scale,
            ),
        };
    }

    /// Draws set aside by the last `render_to` for their level of detail or
    /// draw distance (they are also in `culled_draws`).
    pub fn lod_culled_draws(&self) -> u32 {
        self.lod_culled
    }

    /// Move draws whose level of detail isn't the one for a camera at `eye`,
    /// or which are past their draw distance, into the culled lists, before
    /// the shadow pass: a far chunk's shadow comes from its simple level too.
    fn cull_draws_to_lod(&mut self, eye: [f32; 3]) {
        let LodSettings {
            lod_scale,
            draw_distance_scale,
        } = self.lod_settings;
        let mut culled = 0u32;
        let mut split = |draws: &mut Vec<DrawCall>, out: &mut Vec<DrawCall>| {
            if !draws.iter().any(|d| d.lod.is_some()) {
                return;
            }
            let (keep, cull): (Vec<_>, Vec<_>) = draws.drain(..).partition(|d| {
                d.lod.map_or(true, |l| {
                    l.visible_from(eye, lod_scale, draw_distance_scale)
                })
            });
            *draws = keep;
            culled += cull.len() as u32;
            out.extend(cull);
        };
        split(&mut self.entity_draws, &mut self.culled_entity_draws);
        split(
            &mut self.transparent_draws,
            &mut self.culled_transparent_draws,
        );
        self.lod_culled = culled;
    }

    /// Move opaque and transparent draws whose world bounds miss the camera
    /// frustum into the culled lists. Wireframe draws and draws without
    /// bounds always stay (ADR 0074).
    fn cull_draws_to_camera(&mut self) {
        let Some(frustum) = &self.camera_frustum else {
            return;
        };
        let split = |draws: &mut Vec<DrawCall>, culled: &mut Vec<DrawCall>| {
            let (keep, cull): (Vec<_>, Vec<_>) = draws
                .drain(..)
                .partition(|d| d.is_wireframe || draw_visible(d, frustum));
            *draws = keep;
            culled.extend(cull);
        };
        split(&mut self.entity_draws, &mut self.culled_entity_draws);
        split(
            &mut self.transparent_draws,
            &mut self.culled_transparent_draws,
        );
    }

    // ── Draw call factory methods ──

    fn create_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        mesh: &crate::primitives::Mesh,
        is_wireframe: bool,
        transform_uniforms: TransformUniforms,
        material_uniforms: MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> DrawCall {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (transform_buffer, transform_bind_group) =
            Self::create_transform_bind(device, pipeline, &transform_uniforms);
        let (material_buffer, material_bind_group) =
            Self::create_material_bind(device, pipeline, &material_uniforms, texture_cache);

        DrawCall {
            vertex_buffer: Arc::new(vertex_buffer),
            index_buffer: Arc::new(index_buffer),
            index_count: mesh.indices.len() as u32,
            is_wireframe,
            casts_shadow: true,
            transform: TransformTarget::Own(Arc::new(transform_buffer)),
            transform_bind_group: Arc::new(transform_bind_group),
            material_buffer: Arc::new(MaterialBuffer::new(material_buffer)),
            material_bind_group: Arc::new(material_bind_group),
            model: transform_uniforms.model,
            model_inv_transpose: transform_uniforms.model_inv_transpose,
            entity_id: None,
            blend_mode: BlendMode::Alpha,
            sort_depth: 0.0,
            world_aabb: None,
            style_aabb: None,
            lod: None,
            wooze: None,
            double_sided: false,
        }
    }

    /// A skinned draw's bind group over `bones`, the entity's own bone
    /// buffer or the asset's shared one.
    pub(super) fn create_bone_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        bones: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bones.as_entire_binding(),
            }],
            label: Some("Skinned Draw Bone Bind Group"),
        })
    }

    fn create_transform_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &TransformUniforms,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Transform Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.transform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Transform Bind Group"),
        });

        (buffer, bind_group)
    }

    fn create_material_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        Self::create_material_bind_with_textures(
            device,
            pipeline,
            uniforms,
            &texture_cache.default_white.view,
            &texture_cache.default_normal.view,
            &texture_cache.default_metallic_roughness.view,
            &texture_cache.default_white.view,
        )
    }

    fn create_material_bind_with_textures(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &MaterialUniforms,
        base_color_view: &wgpu::TextureView,
        normal_view: &wgpu::TextureView,
        mr_view: &wgpu::TextureView,
        emissive_view: &wgpu::TextureView,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base_color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(mr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(emissive_view),
                },
            ],
            label: Some("Material Bind Group"),
        });

        (buffer, bind_group)
    }

    /// Resolve a texture reference, returning the view, sampler, and whether a real texture was found
    fn resolve_texture<'a>(
        cache: &'a TextureCache,
        name: Option<&str>,
        default: &'a crate::texture_cache::GpuTexture,
    ) -> (&'a wgpu::TextureView, &'a wgpu::Sampler, bool) {
        if let Some(name) = name {
            if let Some(gpu_tex) = cache.get(name) {
                return (&gpu_tex.view, &gpu_tex.sampler, true);
            }
        }
        (&default.view, &default.sampler, false)
    }

    fn create_light_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &LightUniforms,
        shadow_pass: &ShadowPass,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.light_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_pass.shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&shadow_pass.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: shadow_pass.shadow_uniforms_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&pipeline.material_sampler),
                },
            ],
            label: Some("Light Bind Group"),
        });

        (buffer, bind_group)
    }

    // ── Ocean extraction ──

    /// Extract the first `ocean` component from the world. The wave spectrum
    /// is regenerated only when simulation params change (cheap: N sin/cos
    /// coefficient sets), so live-tweaking any field takes effect next frame.
    fn extract_ocean_from_world(&mut self, world: &FlintWorld) {
        self.ocean_active = false;
        for &entity_id in world.entities_with_component(comp::OCEAN) {
            let ocean_comp = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::OCEAN).cloned());
            let Some(ocean_comp) = ocean_comp else {
                continue;
            };

            let params = flint_core::ocean::OceanParams::from_component(&ocean_comp);
            let needs_regen = self
                .ocean_spectrum
                .as_ref()
                .map(|s| s.params != params)
                .unwrap_or(true);
            if needs_regen {
                self.ocean_spectrum = Some(flint_core::ocean::WaveSpectrum::generate(&params));
            }
            self.ocean_visuals = crate::ocean_pipeline::OceanVisuals::from_component(&ocean_comp);
            self.ocean_active = self.ocean_pipeline.is_some();
            break; // one ocean per scene
        }
    }

    // ── Water bodies (ADR 0094) ──

    /// Whether any water draws this frame: the sea, or a body.
    pub(crate) fn water_active(&self) -> bool {
        self.ocean_active || self.has_water_bodies()
    }

    fn has_water_bodies(&self) -> bool {
        self.ocean_pipeline.is_some()
            && (!self.water_bodies.is_empty()
                || self.scene_water_bodies.values().any(|(_, b)| b.is_some()))
    }

    /// Every body to draw this frame: the scene's, then the game's.
    pub(crate) fn water_bodies_to_draw(
        &self,
    ) -> impl Iterator<Item = &crate::water_body::GpuWaterBody> {
        self.scene_water_bodies
            .values()
            .filter_map(|(_, b)| b.as_ref())
            .chain(self.water_bodies.values())
    }

    fn build_water_body(
        &self,
        device: &wgpu::Device,
        label: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<crate::water_body::GpuWaterBody, String> {
        let (Some(op), Some(ocean_buf)) = (&self.ocean_pipeline, &self.ocean_uniform_buffer) else {
            return Err("this device has no ocean pipeline".into());
        };
        mesh.validate()?;
        if mesh.indices.is_empty() {
            return Err("water body has no triangles".into());
        }
        Ok(crate::water_body::GpuWaterBody::new(
            device,
            &op.ocean_bind_group_layout,
            ocean_buf,
            label,
            mesh,
            look,
        ))
    }

    /// Build the bodies `water_body` components describe. A body is rebuilt
    /// only when its component changes, and dropped with its entity.
    fn extract_water_bodies_from_world(&mut self, world: &FlintWorld, device: &wgpu::Device) {
        let mut seen = std::collections::BTreeSet::new();
        for &entity_id in world.entities_with_component(comp::WATER_BODY) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(value) = components.get(comp::WATER_BODY) else {
                continue;
            };
            let name = world
                .get_name(entity_id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{entity_id:?}"));
            seen.insert(name.clone());
            if self
                .scene_water_bodies
                .get(&name)
                .is_some_and(|(v, _)| v == value)
            {
                continue;
            }
            let look = crate::water_body::WaterBodyLook::from_component(value);
            let body = crate::water_body::mesh_from_component(value)
                .and_then(|mesh| self.build_water_body(device, &name, &mesh, look));
            let body = match body {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("water_body `{name}`: {e}");
                    None
                }
            };
            self.scene_water_bodies.insert(name, (value.clone(), body));
        }
        self.scene_water_bodies
            .retain(|name, _| seen.contains(name));

        // Bodies take the sea's look and waves; a scene with bodies and no
        // ocean gets the default ocean's.
        if !self.ocean_active && self.has_water_bodies() && self.ocean_spectrum.is_none() {
            self.ocean_spectrum = Some(flint_core::ocean::WaveSpectrum::generate(
                &flint_core::ocean::OceanParams::default(),
            ));
        }
    }

    /// Lay a ground fog on the floors of `fog`'s map (ADR 0106), replacing
    /// any set: a fog with a floor of its own under each texel, so a mist
    /// can fill hollows at many heights. How it looks (its density, colour,
    /// falloff and steps) is the post-process config's `ground_fog_*`, and
    /// it shows only while `ground_fog_density` is over 0. Errs when the
    /// map is bad, and keeps what was set; a map with no fog on it clears it.
    pub fn set_ground_fog(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fog: &crate::ground_fog::GroundFog,
    ) -> Result<(), String> {
        self.ground_fog = crate::ground_fog::GroundFogGpu::new(device, queue, fog)?;
        Ok(())
    }

    /// Remove the ground fog. Returns whether one was set.
    pub fn clear_ground_fog(&mut self) -> bool {
        self.ground_fog.take().is_some()
    }

    /// Whether a ground fog is set (with fog somewhere on its map).
    pub fn has_ground_fog(&self) -> bool {
        self.ground_fog.is_some()
    }

    /// Draw water on a game's own mesh (ADR 0094): a lake at its level, a
    /// river down a valley. It shades as the sea does, with the body's own
    /// tint, wave scale and flow, and the sea's (or the default ocean's)
    /// foam, refraction and reflection. Replaces any body set with this
    /// `id`. Errs when the mesh's indices are bad or it has no triangles.
    pub fn set_water_body(
        &mut self,
        device: &wgpu::Device,
        id: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<(), String> {
        let body = self.build_water_body(device, id, mesh, look)?;
        self.water_bodies.insert(id.to_string(), body);
        if !self.ocean_active && self.ocean_spectrum.is_none() {
            self.ocean_spectrum = Some(flint_core::ocean::WaveSpectrum::generate(
                &flint_core::ocean::OceanParams::default(),
            ));
        }
        Ok(())
    }

    /// Change a body's look and keep its mesh. Returns whether it existed.
    pub fn set_water_body_look(
        &mut self,
        id: &str,
        look: crate::water_body::WaterBodyLook,
    ) -> bool {
        match self.water_bodies.get_mut(id) {
            Some(b) => {
                b.look = look;
                true
            }
            None => false,
        }
    }

    /// Remove the body set with `id`. Returns whether it existed.
    pub fn remove_water_body(&mut self, id: &str) -> bool {
        self.water_bodies.remove(id).is_some()
    }

    /// Remove every body a game set (the scene's own stay).
    pub fn clear_water_bodies(&mut self) {
        self.water_bodies.clear();
    }

    // ── Contact foam extraction ──

    /// Extract the first `ocean_contact` entity: hull center/yaw/extents for
    /// the splash ring, plus hull velocity differentiated across frames
    /// (drift + heave both count as impact against the moving water).
    fn extract_ocean_contact_from_world(&mut self, world: &FlintWorld) {
        use flint_core::toml_util::{toml_f32, toml_vec3};
        self.ocean_contact = None;
        for &entity_id in world.entities_with_component(comp::OCEAN_CONTACT) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(contact) = components.get(comp::OCEAN_CONTACT) else {
                continue;
            };
            let Some(tf) = components.get(comp::TRANSFORM) else {
                continue;
            };

            let pos = tf.get("position").and_then(toml_vec3).unwrap_or([0.0; 3]);
            let rot = tf.get("rotation").and_then(toml_vec3).unwrap_or([0.0; 3]);
            let yaw = rot[1].to_radians(); // engine Euler is degrees; Y = yaw
            let half_x = contact.get("half_x").and_then(toml_f32).unwrap_or(1.0);
            let half_z = contact.get("half_z").and_then(toml_f32).unwrap_or(1.0);

            self.ocean_contact = Some((
                [pos[0], pos[2], yaw.cos(), yaw.sin()],
                [half_x.max(0.01), half_z.max(0.01)],
            ));

            // Hull velocity on the same clock as the waves. Clamped against
            // teleports/scene loads, lightly smoothed against frame jitter.
            if let Some((prev_t, prev_pos)) = self.ocean_contact_prev {
                let dt = (self.ocean_time - prev_t) as f32;
                if dt > 1e-4 {
                    let k = (10.0 * dt).min(1.0);
                    for i in 0..3 {
                        let v = ((pos[i] - prev_pos[i]) / dt).clamp(-5.0, 5.0);
                        self.ocean_contact_vel[i] += (v - self.ocean_contact_vel[i]) * k;
                    }
                }
            }
            self.ocean_contact_prev = Some((self.ocean_time, pos));
            break; // one contact hull per scene
        }
        if self.ocean_contact.is_none() {
            self.ocean_contact_prev = None;
            self.ocean_contact_vel = [0.0; 3];
        }
    }

    // ── Sky extraction ──

    /// Extract the first `sky` component. When present, the procedural sky
    /// replaces the texture skybox, and its optional ambient fields override
    /// the hemisphere ambient in the light uniforms (so a time-of-day script
    /// can darken nights by writing component fields).
    fn extract_sky_from_world(&mut self, world: &FlintWorld) {
        self.sky_active = false;
        for &entity_id in world.entities_with_component(comp::SKY) {
            let sky_comp = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::SKY).cloned());
            let Some(sky_comp) = sky_comp else { continue };

            self.sky_params = crate::sky_pipeline::SkyParams::from_component(&sky_comp);
            if let Some(ambient_sky) = self.sky_params.ambient_sky {
                self.light_uniforms.ambient_sky = ambient_sky;
            }
            if let Some(ambient_ground) = self.sky_params.ambient_ground {
                self.light_uniforms.ambient_ground = ambient_ground;
            }
            self.sky_active = self.sky_pipeline.is_some();
            break;
        }
    }

    // ── Light extraction ──

    /// Extract light entities from the world and update the light uniform buffer
    /// Override the hemisphere ambient colors (linear RGB). Scenes set this
    /// via `[environment] ambient_sky / ambient_ground`; when never called,
    /// the built-in default ambient is byte-identical to prior behavior.
    pub fn set_ambient(&mut self, sky: [f32; 3], ground: [f32; 3]) {
        self.ambient_override = Some((sky, ground));
        self.apply_ambient_override();
    }

    /// Forget what styles remember of earlier frames (ASCII's temporal
    /// stability, #277): call it when the picture stops following on from
    /// the last one, on a scene change or a camera cut. The next frame is
    /// drawn as if it were the first.
    pub fn reset_style_history(&mut self) {
        self.style_history_epoch = self.style_history_epoch.wrapping_add(1);
    }

    /// Clear any scene-authored ambient override, restoring the built-in
    /// default (needed on scene transitions so an old scene's ambient never
    /// leaks into a scene that doesn't author one).
    pub fn reset_ambient(&mut self) {
        self.ambient_override = None;
        self.diffuse_wrap_override = None;
        self.oren_nayar_override = None;
        self.sheen_override = None;
        let [sr, sg, sb] = LightUniforms::DEFAULT_AMBIENT_SKY;
        let [gr, gg, gb] = LightUniforms::DEFAULT_AMBIENT_GROUND;
        self.light_uniforms.ambient_sky = [sr, sg, sb, 1.0];
        self.light_uniforms.ambient_ground = [gr, gg, gb, 1.0];
        self.light_uniforms.sheen_color_strength = [0.0; 4];
    }

    /// Soften the diffuse terminator (0 = physically sharp / legacy shading).
    /// Scenes set this via `[environment] diffuse_wrap`.
    pub fn set_diffuse_wrap(&mut self, wrap: f32) {
        self.diffuse_wrap_override = Some(wrap.max(0.0));
        self.apply_ambient_override();
    }

    /// Blend the diffuse term from Lambert toward the Fujii Oren-Nayar
    /// approximation (0 = exact legacy shading, 1 = full Oren-Nayar; sigma
    /// comes from material roughness). Scenes set this via
    /// `[environment] oren_nayar` (ADR 0048).
    pub fn set_oren_nayar(&mut self, blend: f32) {
        self.oren_nayar_override = Some(blend.max(0.0));
        self.apply_ambient_override();
    }

    /// Tinted Charlie-sheen rim: `color` is a linear RGB tint, `strength`
    /// 0..~0.3 (0 = exact legacy shading). Scenes set this via
    /// `[environment] sheen_color / sheen_strength` (ADR 0048).
    pub fn set_sheen(&mut self, color: [f32; 3], strength: f32) {
        self.sheen_override = Some((color, strength.max(0.0)));
        self.apply_ambient_override();
    }

    /// Snapshot of the current lighting-lever state (overrides if set, else
    /// the built-in neutral defaults). Read by the render debug panel.
    pub fn lighting_levers(&self) -> LightingLevers {
        let (ambient_sky, ambient_ground) = self.ambient_override.unwrap_or((
            LightUniforms::DEFAULT_AMBIENT_SKY,
            LightUniforms::DEFAULT_AMBIENT_GROUND,
        ));
        let (sheen_color, sheen_strength) = self.sheen_override.unwrap_or(([0.0; 3], 0.0));
        LightingLevers {
            ambient_sky,
            ambient_ground,
            diffuse_wrap: self.diffuse_wrap_override.unwrap_or(0.0),
            oren_nayar: self.oren_nayar_override.unwrap_or(0.0),
            sheen_color,
            sheen_strength,
        }
    }

    fn apply_ambient_override(&mut self) {
        if let Some((sky, ground)) = self.ambient_override {
            self.light_uniforms.ambient_sky = [sky[0], sky[1], sky[2], 1.0];
            self.light_uniforms.ambient_ground = [ground[0], ground[1], ground[2], 1.0];
        }
        if let Some(wrap) = self.diffuse_wrap_override {
            // Shader decodes wrap = ambient_sky.w - 1.0
            self.light_uniforms.ambient_sky[3] = 1.0 + wrap;
        }
        if let Some(oren) = self.oren_nayar_override {
            // Shader decodes oren = ambient_ground.w - 1.0
            self.light_uniforms.ambient_ground[3] = 1.0 + oren;
        }
        if let Some((color, strength)) = self.sheen_override {
            self.light_uniforms.sheen_color_strength = [color[0], color[1], color[2], strength];
        }
    }

    fn extract_lights_from_world(&mut self, world: &FlintWorld) {
        let mut dir_count = 0u32;
        let mut point_count = 0u32;
        let mut spot_count = 0u32;
        let mut directionals = [DirectionalLight::default(); MAX_DIRECTIONAL_LIGHTS];
        let mut points = [PointLight::default(); MAX_POINT_LIGHTS];
        let mut spots = [SpotLight::default(); MAX_SPOT_LIGHTS];

        // Deterministic iteration: the component index is a HashSet whose
        // order changes per process, which silently reassigned the one
        // shadow-casting directional slot (index 0) between runs. Sort by
        // entity name (then id) so light order is stable and authored.
        let mut light_entities: Vec<_> = world
            .entities_with_component(comp::LIGHT)
            .iter()
            .copied()
            .collect();
        // A hidden entity's light goes out with it (#628)
        let hidden = world.hidden_entities();
        if !hidden.is_empty() {
            light_entities.retain(|id| !hidden.contains(id));
        }
        light_entities.sort_by(|a, b| {
            world
                .get_name(*a)
                .unwrap_or("")
                .cmp(world.get_name(*b).unwrap_or(""))
                .then(a.cmp(b))
        });

        for entity_id in light_entities {
            let light_component = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::LIGHT).cloned());

            if let Some(light) = light_component {
                let light_type = light
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("directional");

                let color = Self::extract_light_vec3(&light, "color").unwrap_or([1.0, 1.0, 1.0]);
                let intensity = light.get("intensity").and_then(toml_f32).unwrap_or(1.0);

                match light_type {
                    "directional" => {
                        if (dir_count as usize) < MAX_DIRECTIONAL_LIGHTS {
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            let volumetric_intensity = light
                                .get("volumetric_intensity")
                                .and_then(toml_f32)
                                .unwrap_or(0.0);
                            let volumetric_color =
                                Self::extract_light_vec3(&light, "volumetric_color")
                                    .unwrap_or(color);
                            // Apparent source angular size, authored in
                            // degrees (sun ~0.5, softbox 2-5); drives PCSS
                            // penumbra (ADR 0056). Stored in radians.
                            let angular_size = light
                                .get("angular_size")
                                .and_then(toml_f32)
                                .unwrap_or(0.0)
                                .to_radians();
                            directionals[dir_count as usize] = DirectionalLight {
                                direction,
                                volumetric_intensity,
                                color,
                                intensity,
                                volumetric_color,
                                angular_size,
                            };
                            dir_count += 1;
                        }
                    }
                    "point" => {
                        if (point_count as usize) < MAX_POINT_LIGHTS {
                            let light_pos =
                                world.get_world_position(entity_id).unwrap_or(Vec3::ZERO);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            let source_radius =
                                light.get("source_radius").and_then(toml_f32).unwrap_or(0.0);
                            points[point_count as usize] = PointLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                color,
                                intensity,
                                source_radius,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            };
                            point_count += 1;
                        }
                    }
                    "spot" => {
                        if (spot_count as usize) < MAX_SPOT_LIGHTS {
                            let light_pos =
                                world.get_world_position(entity_id).unwrap_or(Vec3::ZERO);
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            let inner_angle =
                                light.get("inner_angle").and_then(toml_f32).unwrap_or(0.3);
                            let outer_angle =
                                light.get("outer_angle").and_then(toml_f32).unwrap_or(0.5);
                            let source_radius =
                                light.get("source_radius").and_then(toml_f32).unwrap_or(0.0);
                            spots[spot_count as usize] = SpotLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                direction,
                                inner_angle,
                                color,
                                outer_angle,
                                intensity,
                                source_radius,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            };
                            spot_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Strongest directional casts the shadows: only index 0 gets CSM, so
        // put the highest-intensity directional first (sun over fill),
        // independent of entity naming. Stable sort keeps name order on ties.
        directionals[..dir_count as usize].sort_by(|a, b| {
            b.intensity
                .partial_cmp(&a.intensity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // If no lights found in scene, use defaults
        if dir_count == 0 && point_count == 0 && spot_count == 0 && self.default_lights {
            self.light_uniforms = LightUniforms::default_scene_lights();
            self.apply_ambient_override();
        } else {
            self.light_uniforms.directional_lights = directionals;
            self.light_uniforms.point_lights = points;
            self.light_uniforms.spot_lights = spots;
            self.light_uniforms.directional_count = dir_count;
            self.light_uniforms.point_count = point_count;
            self.light_uniforms.spot_count = spot_count;
        }
    }

    fn extract_light_vec3(table: &toml::Value, key: &str) -> Option<[f32; 3]> {
        flint_core::toml_util::toml_vec3(table.get(key)?)
    }

    // ── Render entry points ──

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), wgpu::SurfaceError> {
        if self.device_lost {
            return Ok(());
        }
        self.render_to(
            &context.device,
            &context.queue,
            context.depth_target(),
            camera,
            view,
        );
        Ok(())
    }

    /// Resize post-processing resources (call on window resize).
    /// `width` x `height` is the surface's size; the HDR chain is that at the
    /// render scale.
    pub fn resize_postprocess(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let same_surface = self.surface_size == (width, height);
        self.surface_size = (width, height);
        let (w, h) = crate::render_scale::scaled_size(width, height, self.render_scale);
        // Stylize targets live in the resources, so they (and every bind
        // group over one) go with the old size (#262). Every bind group
        // does; but when only the render scale moved (`--render-scale
        // auto` stepping, #270) the surface is the same size, so the
        // display-stage targets are too, and they hold ASCII's per-cell
        // history (#277): keep them, and a step doesn't reset it. HDR-stage
        // targets in the set are remade at the new size on their next use.
        let kept = same_surface
            .then(|| self.postprocess_resources.take())
            .flatten()
            .map(|old| old.styles);
        let mut resources = PostProcessResources::new(device, w, h);
        if let Some(styles) = kept {
            resources.styles = styles;
        }
        self.postprocess_resources = Some(resources);
        // Below the surface's size the scene keeps a depth buffer of its own
        // (#122); at scale 1 it draws into the caller's, as it always has.
        if (w, h) == (width, height) {
            self.scaled_depth = None;
            self.depth_upscale = None;
        } else {
            let depth = crate::render_scale::ScaledDepth::new(device, w, h);
            self.depth_upscale = Some(crate::render_scale::DepthUpscale::new(device, &depth));
            self.scaled_depth = Some(std::sync::Arc::new(depth));
        }
        // The recreated resources drop the FXAA intermediate; re-allocate it
        // at the new size when the pass is active (ADR 0050).
        self.ensure_fxaa_resources(device);
    }

    /// Draw the scene at `scale` of the surface's size on each side, and
    /// upscale it in the composite pass (#122). What's drawn over the
    /// finished frame (world lines, world text, the HUD) stays at the
    /// surface's size. Clamped to [`MIN_RENDER_SCALE`]`..=1`; 1, the
    /// default, draws the scene exactly as before. Rebuilds the HDR chain
    /// when the size it gives changes.
    ///
    /// [`MIN_RENDER_SCALE`]: crate::MIN_RENDER_SCALE
    pub fn set_render_scale(&mut self, device: &wgpu::Device, scale: f32) {
        let scale = crate::render_scale::clamp_render_scale(scale);
        if scale == self.render_scale {
            return;
        }
        let (width, height) = self.surface_size;
        let before = crate::render_scale::scaled_size(width, height, self.render_scale);
        self.render_scale = scale;
        if crate::render_scale::scaled_size(width, height, scale) != before {
            self.resize_postprocess(device, width, height);
        }
    }

    /// Whether this frame draws the entity style mask: post-processing is
    /// up and a style that reads it is on (#263).
    fn style_mask_wanted(&self) -> bool {
        self.postprocess_resources.is_some()
            && crate::stylize::any_uses_mask(&self.postprocess_config, &self.style_ids)
    }

    /// Keep the `style` debug view's style (`mask_view`) on while that
    /// mode is set, whatever config was set since, and off once it isn't
    /// unless the config asked for it itself.
    fn sync_debug_styles(&mut self) {
        let styles = &mut self.postprocess_config.styles;
        if self.debug_state.mode == DebugMode::StyleMask {
            if !styles.is_enabled("mask_view") {
                styles.set_enabled("mask_view", true);
                self.mask_view_forced = true;
            }
        } else if std::mem::take(&mut self.mask_view_forced) {
            styles.set_enabled("mask_view", false);
        }
        // The `edges` view (#349) is drawn by the ASCII style itself, so a
        // scene without it gets it, at its defaults, for the view
        if self.debug_state.mode == DebugMode::StyleEdges {
            if !styles.is_enabled("ascii") {
                styles.set_enabled("ascii", true);
                self.ascii_forced = true;
            }
        } else if std::mem::take(&mut self.ascii_forced) {
            styles.set_enabled("ascii", false);
        }
    }

    /// Bind groups the post chain keeps (its styles' among them). Steady
    /// from frame to frame while nothing new is switched on, so tests can
    /// show a per-frame change makes none (#269).
    pub fn kept_post_binds(&self) -> usize {
        self.postprocess_resources
            .as_ref()
            .map_or(0, |r| r.binds.len())
    }

    /// Bind groups the post chain has made since it was last rebuilt, the
    /// ones it made for a single frame included: a steady frame adds none
    /// once the caller's depth comes with an id (#379).
    pub fn post_binds_made(&self) -> u64 {
        self.postprocess_resources
            .as_ref()
            .map_or(0, |r| r.binds.made())
    }

    /// The style ids the mask uses: which number each `style_mask` style
    /// name has this frame.
    pub fn style_ids(&self) -> &crate::style_mask::StyleIds {
        &self.style_ids
    }

    /// The style mask id of each styled entity, as of the last
    /// [`update_from_world`](Self::update_from_world) (empty while no
    /// style reads the mask).
    pub fn styled_entities(&self) -> &HashMap<flint_core::EntityId, u16> {
        &self.style_by_entity
    }

    /// How much of its style each styled entity drawn at less than 1 is
    /// drawn with (#278); an entity missing here is fully styled.
    pub fn styled_blends(&self) -> &HashMap<flint_core::EntityId, f32> {
        &self.style_blend_by_entity
    }

    /// Record the style mask pass over this frame's visible draws of
    /// styled entities: terrain chunks, opaque static, procedural and
    /// skinned draws, then transparents over them (#310). The mask is
    /// cleared even with none, so a style never reads a stale frame.
    fn encode_style_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey,
    ) {
        self.style_mask_veiled = false;
        let Some(resources) = &self.postprocess_resources else {
            return;
        };
        let Some(skinned) = &self.skinned_pipeline else {
            return;
        };
        let pass = self.style_mask_pass.get_or_insert_with(|| {
            crate::style_mask::StyleMaskPass::new(
                device,
                &crate::style_mask::MaskLayouts {
                    transform: &self.pipeline.transform_bind_group_layout,
                    material: &self.pipeline.material_bind_group_layout,
                    light: &self.pipeline.light_bind_group_layout,
                    bones: &skinned.bone_bind_group_layout,
                },
            )
        });
        // Under MSAA the mask is drawn 4x against the scene's own samples
        // and resolved, so a feature thinner than a pixel keeps the pixels
        // the scene's samples gave it (#306).
        let msaa_depth = match &self.msaa_depth {
            Some((texture, view))
                if self.sample_count == crate::style_mask::MSAA_SAMPLES
                    && texture.size().width == resources.width
                    && texture.size().height == resources.height =>
            {
                pass.ensure_msaa(
                    device,
                    &self.pipeline.transform_bind_group_layout,
                    &skinned.bone_bind_group_layout,
                );
                Some(crate::style_mask::MsaaDepth {
                    view,
                    id: self.msaa_depth_id,
                })
            }
            _ => None,
        };
        let veil_target = resources.styles.get(
            device,
            crate::style_mask::VEIL_TARGET,
            crate::style_mask::VEIL_FORMAT,
            resources.width,
            resources.height,
        );
        let mask = resources.styles.get(
            device,
            crate::style_mask::MASK_TARGET,
            crate::style_mask::STYLE_MASK_FORMAT,
            resources.width,
            resources.height,
        );
        let ids = &self.style_by_entity;
        let blends = &self.style_blend_by_entity;
        let id_of = |e: &Option<flint_core::EntityId>| e.as_ref().and_then(|e| ids.get(e).copied());
        let blend_of = |e: &Option<flint_core::EntityId>| {
            e.as_ref()
                .and_then(|e| blends.get(e).copied())
                .unwrap_or(1.0)
        };
        let mut draws = Vec::new();
        if !ids.is_empty() {
            // Terrain first (#310): the props on it are drawn after, and
            // each fragment's depth test decides who owns a pixel anyway
            if let Some(id) = id_of(&self.terrain_entity) {
                let blend = blend_of(&self.terrain_entity);
                let frustum = self.camera_frustum.as_ref();
                let visible = |d: &&TerrainDrawCall| {
                    frustum.is_none_or(|f| f.aabb_visible(d.aabb_min, d.aabb_max))
                };
                draws.extend(self.terrain_draws.iter().filter(visible).map(|d| {
                    crate::style_mask::MaskDraw {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        bones: None,
                        id,
                        blend,
                    }
                }));
            }
            draws.extend(
                self.entity_draws
                    .iter()
                    .filter(|d| !d.is_wireframe)
                    .filter_map(|d| {
                        Some(crate::style_mask::MaskDraw {
                            vertex_buffer: &d.vertex_buffer,
                            index_buffer: &d.index_buffer,
                            index_count: d.index_count,
                            transform_bind_group: &d.transform_bind_group,
                            bones: None,
                            id: id_of(&d.entity_id)?,
                            blend: blend_of(&d.entity_id),
                        })
                    }),
            );
            draws.extend(self.skinned_entity_draws.iter().filter_map(|d| {
                Some(crate::style_mask::MaskDraw {
                    vertex_buffer: &d.vertex_buffer,
                    index_buffer: &d.index_buffer,
                    index_count: d.index_count,
                    transform_bind_group: &d.transform_bind_group,
                    bones: Some(&d.bone_bind_group),
                    id: id_of(&d.entity_id)?,
                    blend: blend_of(&d.entity_id),
                })
            }));
            // Styled transparents last, back to front as the scene drew
            // them (#310). They write no depth, so the depth test keeps them
            // behind the opaques in front of them and nothing else: where
            // one shows, it is the nearest styled surface and its id
            // replaces what's behind. A styled glass is a lens, drawing
            // what's seen through it in its style.
            draws.extend(
                self.transparent_draws
                    .iter()
                    .filter(|d| !d.is_wireframe)
                    .filter_map(|d| {
                        Some(crate::style_mask::MaskDraw {
                            vertex_buffer: &d.vertex_buffer,
                            index_buffer: &d.index_buffer,
                            index_count: d.index_count,
                            transform_bind_group: &d.transform_bind_group,
                            bones: None,
                            id: id_of(&d.entity_id)?,
                            blend: blend_of(&d.entity_id),
                        })
                    }),
            );
            draws.extend(self.transparent_skinned_draws.iter().filter_map(|d| {
                Some(crate::style_mask::MaskDraw {
                    vertex_buffer: &d.vertex_buffer,
                    index_buffer: &d.index_buffer,
                    index_count: d.index_count,
                    transform_bind_group: &d.transform_bind_group,
                    bones: Some(&d.bone_bind_group),
                    id: id_of(&d.entity_id)?,
                    blend: blend_of(&d.entity_id),
                })
            }));
        }
        // What's in front of them (#290): transparents that aren't styled
        // themselves, and every particle
        let mut veil = crate::style_mask::Veil {
            light_bind_group: &self.light_bind_group,
            meshes: Vec::new(),
            particles: None,
        };
        if !draws.is_empty() {
            let unstyled = |e: &Option<flint_core::EntityId>| id_of(e).is_none();
            veil.meshes.extend(
                self.transparent_draws
                    .iter()
                    .filter(|d| !d.is_wireframe && unstyled(&d.entity_id))
                    .map(|d| crate::style_mask::VeilMesh {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        material_bind_group: &d.material_bind_group,
                        bones: None,
                    }),
            );
            veil.meshes.extend(
                self.transparent_skinned_draws
                    .iter()
                    .filter(|d| unstyled(&d.entity_id))
                    .map(|d| crate::style_mask::VeilMesh {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        material_bind_group: &d.material_bind_group,
                        bones: Some(&d.bone_bind_group),
                    }),
            );
            veil.particles =
                self.particle_pipeline
                    .as_ref()
                    .map(|pipeline| crate::style_mask::VeilParticles {
                        pipeline,
                        draws: &self.particle_draws,
                    });
        }
        let veiled = !veil.is_empty();
        if let Some(pass) = &mut self.style_mask_pass {
            pass.encode(
                device,
                queue,
                encoder,
                &resources.binds,
                &mask,
                &veil_target.view,
                depth_view,
                depth_key,
                msaa_depth,
                &draws,
                &veil,
            );
        }
        self.style_mask_veiled = veiled;
    }

    /// Whether some pixel of the last style mask may hold a blend below 1
    /// (#396): an entity drawn part-way into its style (#278), or a veil
    /// over a styled one (#290). While neither, the framework's
    /// cross-fade is skipped.
    pub fn style_mask_fades(&self) -> bool {
        !self.style_blend_by_entity.is_empty() || self.style_mask_veiled
    }

    /// The entity style mask's per-draw uniform bytes and how many styled
    /// draws the last frame masked (#311); `None` before a frame needed
    /// the mask.
    pub fn style_mask_draws(&self) -> Option<(u64, usize)> {
        self.style_mask_pass
            .as_ref()
            .map(|p| (p.draw_buffer_bytes(), p.last_draws()))
    }

    /// The render scale [`set_render_scale`](Self::set_render_scale) set.
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    /// The size the scene is drawn at: the HDR chain's, which is the
    /// surface's at scale 1.
    pub fn scene_size(&self) -> (u32, u32) {
        self.postprocess_resources
            .as_ref()
            .map_or(self.surface_size, |r| (r.width, r.height))
    }

    /// Ensure FXAA GPU resources exist. Every frame does this itself
    /// ([`encode_to`](Self::encode_to), #297), as it builds styles; calling
    /// it after setting a config moves the shader compile off the first
    /// frame that draws FXAA. No-op when `fxaa` is disabled — the
    /// default-off path allocates nothing (ADR 0050, Kuwahara lazy pattern).
    pub fn ensure_fxaa_resources(&mut self, device: &wgpu::Device) {
        if !self.postprocess_config.fxaa_enabled {
            return;
        }
        if let Some(pp) = &mut self.postprocess_pipeline {
            if pp.fxaa.is_none() {
                pp.fxaa = Some(crate::postprocess::FxaaPipeline::new(
                    device,
                    self.surface_format,
                ));
            }
        }
        if let Some(resources) = &mut self.postprocess_resources {
            if resources.fxaa.is_none() {
                let (w, h) = (resources.width, resources.height);
                resources.fxaa = Some(crate::postprocess::FxaaResources::new(
                    device,
                    w,
                    h,
                    self.surface_format,
                ));
            }
        }
    }

    /// This frame's drawing for the shading boil (#214), from the wooze field.
    pub fn set_boil(&mut self, boil: flint_core::wooze::BoilFrame) {
        self.postprocess_config.boil = boil;
        self.authored_postprocess.boil = boil;
    }

    /// The post-processing configuration as set: the scene's look, before
    /// the quality caps. Change a copy and hand it back with
    /// [`set_post_process_config`](Self::set_post_process_config).
    pub fn post_process_config(&self) -> &PostProcessConfig {
        &self.authored_postprocess
    }

    /// The configuration drawn: [`post_process_config`](Self::post_process_config)
    /// with the quality settings' caps applied.
    pub fn effective_post_process_config(&self) -> &PostProcessConfig {
        &self.postprocess_config
    }

    /// Set the post-processing configuration. It's drawn under the quality
    /// settings' caps (`set_quality`), which a scene load doesn't lift.
    pub fn set_post_process_config(&mut self, config: PostProcessConfig) {
        self.postprocess_config = crate::quality::mask_post_process(&config, &self.quality);
        self.authored_postprocess = config;
        self.post_config_generation = next_post_config_generation();
    }

    /// A number that changes whenever [`effective_post_process_config`]
    /// may have: a host keeping a copy of it re-reads it only then.
    ///
    /// [`effective_post_process_config`]: Self::effective_post_process_config
    pub fn post_config_generation(&self) -> u64 {
        self.post_config_generation
    }

    /// The quality settings in force; see [`set_quality`](Self::set_quality).
    pub fn quality(&self) -> &crate::quality::QualitySettings {
        &self.quality
    }

    /// Apply runtime quality settings (FlintKart#55) and return them as
    /// applied (clamped). Call between frames, never with a frame's passes
    /// half encoded: an MSAA change rebuilds every scene pipeline and its
    /// targets, a shadow-resolution change the shadow maps, and a render
    /// scale change the HDR chain. Only what changed is rebuilt, so calling
    /// it every frame with the same settings costs nothing.
    ///
    /// - `render_scale` sets the scene's scale as
    ///   [`set_render_scale`](Self::set_render_scale) does (a player stepping
    ///   the scale itself keeps calling that; `render_scale_auto` is the
    ///   player's and ignored here).
    /// - `msaa`: 1 or 4 samples for the scene passes.
    /// - `shadows`, `shadow_resolution`, `shadow_cascades`, `shadow_distance`.
    /// - `post_process`, `bloom`, `ssao`, `ssao_samples`, `fog`,
    ///   `volumetric` cap the post config: off turns the effect off, on
    ///   leaves it as set.
    pub fn set_quality(
        &mut self,
        device: &wgpu::Device,
        quality: &crate::quality::QualitySettings,
    ) -> crate::quality::QualitySettings {
        let q = quality.clamped();
        self.set_render_scale(device, q.render_scale);
        self.set_msaa(device, q.msaa);
        if q.shadow_resolution != self.shadow_resolution() {
            self.set_shadow_resolution(device, q.shadow_resolution);
        }
        self.set_shadows(q.shadows);
        if let Some(sp) = &mut self.shadow_pass {
            sp.set_cascades(q.shadow_cascades as usize);
            sp.set_distance(q.shadow_distance);
        }
        self.quality = q;
        self.postprocess_config = crate::quality::mask_post_process(&self.authored_postprocess, &q);
        self.post_config_generation = next_post_config_generation();
        q
    }

    /// The MSAA sample count the scene passes draw with.
    pub fn msaa(&self) -> u32 {
        self.sample_count
    }

    /// Draw the scene passes with `samples` MSAA samples, 1 or 4 (anything
    /// else is 1, with a warning). Rebuilds every scene pipeline, whose
    /// sample count is baked in, and drops the MSAA targets so the next
    /// frame makes them at the new count. Bind groups, meshes and textures
    /// are kept: the new pipelines are made against layouts equal to the
    /// old ones, which wgpu (and WebGPU) treat as the same layout.
    pub fn set_msaa(&mut self, device: &wgpu::Device, samples: u32) -> u32 {
        let samples = Self::resolve_sample_count(samples);
        if samples == self.sample_count {
            return samples;
        }
        let format = HDR_FORMAT;
        self.sample_count = samples;
        self.quality.msaa = samples;

        let f = RenderPipeline::new(device, format, samples);
        let p = &mut self.pipeline;
        p.pipeline = f.pipeline;
        p.pipeline_double_sided = f.pipeline_double_sided;
        p.line_pipeline = f.line_pipeline;
        p.overlay_line_pipeline = f.overlay_line_pipeline;
        p.skeleton_line_pipeline = f.skeleton_line_pipeline;
        p.transparent_alpha_pipeline = f.transparent_alpha_pipeline;
        p.transparent_alpha_back_pipeline = f.transparent_alpha_back_pipeline;
        p.transparent_additive_pipeline = f.transparent_additive_pipeline;
        p.transparent_multiply_pipeline = f.transparent_multiply_pipeline;

        let layouts = &self.pipeline;
        if let Some(s) = &mut self.skinned_pipeline {
            let f = SkinnedPipeline::new(
                device,
                format,
                &layouts.transform_bind_group_layout,
                &layouts.material_bind_group_layout,
                &layouts.light_bind_group_layout,
                samples,
            );
            s.pipeline = f.pipeline;
            s.pipeline_double_sided = f.pipeline_double_sided;
            s.wire_line_pipeline = f.wire_line_pipeline;
            s.transparent_alpha_pipeline = f.transparent_alpha_pipeline;
            s.transparent_alpha_back_pipeline = f.transparent_alpha_back_pipeline;
            s.transparent_additive_pipeline = f.transparent_additive_pipeline;
            s.transparent_multiply_pipeline = f.transparent_multiply_pipeline;
        }
        if let Some(b) = &mut self.billboard_pipeline {
            let f = BillboardPipeline::new(device, format, samples);
            b.pipeline = f.pipeline;
        }
        if let Some(t) = &mut self.terrain_pipeline {
            let f = TerrainPipeline::new(
                device,
                format,
                &layouts.transform_bind_group_layout,
                &layouts.light_bind_group_layout,
                samples,
            );
            t.pipeline = f.pipeline;
        }
        if let Some(o) = &mut self.ocean_pipeline {
            let f = crate::ocean_pipeline::OceanPipeline::new(
                device,
                format,
                &layouts.transform_bind_group_layout,
                &layouts.light_bind_group_layout,
                samples,
            );
            o.pipeline = f.pipeline;
            o.body_pipeline = f.body_pipeline;
            o.blit_pipeline = f.blit_pipeline;
        }
        if let Some(s) = &mut self.sky_pipeline {
            s.pipeline = crate::sky_pipeline::SkyPipeline::new(device, format, samples).pipeline;
        }
        if let Some(g) = &mut self.grass_pipeline {
            if let Some(f) = GrassPipeline::new(
                device,
                format,
                &layouts.transform_bind_group_layout,
                &layouts.light_bind_group_layout,
                samples,
            ) {
                g.render_pipeline = f.render_pipeline;
                g.shadow_pipeline = f.shadow_pipeline;
            }
        }
        if let Some(p) = &mut self.particle_pipeline {
            p.rebuild_for_samples(device, format, samples);
        }
        if let Some(s) = &mut self.sprite2d_pipeline {
            s.pipeline = Sprite2dPipeline::new(device, format, samples).pipeline;
        }
        if let Some(s) = &mut self.skybox_pipeline {
            s.pipeline = SkyboxPipeline::new(device, format, samples).pipeline;
        }

        // The targets are remade at the new count by the next frame.
        self.msaa_color = None;
        self.msaa_depth = None;
        tracing::info!("[quality] MSAA {samples}x");
        samples
    }

    /// Build the pipelines of every stylize pass the config switches on
    /// that isn't built yet (#262). Every frame does this itself; calling
    /// it after setting a config moves the shader compile off the first
    /// frame that draws the style. A style's textures are made on that
    /// frame.
    ///
    /// Building runs under `catch_unwind` because some GPU drivers (Intel
    /// Skylake) crash with unrecoverable device loss compiling these
    /// shaders; a style that fails stays off and marks the device lost.
    pub fn ensure_style_resources(&mut self, device: &wgpu::Device) {
        let Some(pp) = &mut self.postprocess_pipeline else {
            return;
        };
        let surface_format = pp.surface_format;
        for failed in pp.styles.ensure(
            device,
            surface_format,
            &self.postprocess_config,
            &self.style_ids,
        ) {
            if !crate::stylize::is_builtin(failed.name) {
                // A user style (ADR 0112) that panicked building is its
                // author's bug, not a lost device: it stays off and says so
                crate::stylize::custom::report_build_panic(failed.name);
                self.postprocess_config
                    .styles
                    .set_enabled(failed.name, false);
                continue;
            }
            tracing::warn!(
                "{} style unavailable: GPU driver crashed during shader compilation. \
                 Try updating your graphics drivers.",
                failed.label
            );
            self.postprocess_config
                .styles
                .set_enabled(failed.name, false);
            self.authored_postprocess
                .styles
                .set_enabled(failed.name, false);
            self.device_lost = true;
        }
    }

    /// How many times the shared style edge pass (#301) has been drawn
    /// since this renderer was made: one a frame while any style that
    /// needs the edges is on, however many do.
    /// Have glyph styles (ASCII) keep each frame's character grid as text
    /// (#317), for [`style_text`](Self::style_text). Costs one pass of a
    /// fragment per cell and a copy to the CPU a frame, so it's off unless
    /// asked for. Does nothing without post-processing, where no style runs.
    pub fn set_style_text_capture(&mut self, on: bool) {
        if let Some(pp) = &mut self.postprocess_pipeline {
            pp.styles.set_text_capture(on);
        }
    }

    /// The character grid style `name` drew in the last frame rendered
    /// with text capture on, once its submission is done (this waits on
    /// the device). `None` if that style draws no glyphs or didn't run.
    pub fn style_text(
        &self,
        device: &wgpu::Device,
        name: &str,
    ) -> Option<crate::stylize::GlyphText> {
        self.postprocess_pipeline
            .as_ref()?
            .styles
            .read_text(name, device)
    }

    pub fn style_edge_runs(&self) -> u64 {
        self.postprocess_pipeline
            .as_ref()
            .map_or(0, |pp| pp.styles.edge_runs())
    }

    /// How many times the framework's cross-fade (#396) has been drawn:
    /// once after each style it fades, on each frame some mask pixel is
    /// below a blend of 1.
    pub fn style_fade_runs(&self) -> u64 {
        self.postprocess_pipeline
            .as_ref()
            .map_or(0, |pp| pp.styles.fade_runs())
    }

    /// The old name of [`ensure_style_resources`](Self::ensure_style_resources),
    /// from when Kuwahara was the only style.
    pub fn ensure_kuwahara_resources(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        self.ensure_style_resources(device);
    }

    /// Validate the configured MSAA sample count (ADR 0058): 1 or 4;
    /// anything else clamps to 1 with a warning.
    fn resolve_sample_count(requested: u32) -> u32 {
        match requested {
            1 | 4 => requested,
            other => {
                tracing::warn!(
                    "Unsupported MSAA sample count {} - falling back to 1 (supported: 1, 4)",
                    other
                );
                1
            }
        }
    }

    /// Lazily (re)create the MSAA color+depth targets at the given size.
    fn ensure_msaa_targets(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if let Some((_, _, w, h)) = &self.msaa_color {
            if *w == width && *h == height {
                return;
            }
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Scene Color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Scene Depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        self.msaa_color = Some((color, color_view, width, height));
        self.msaa_depth = Some((depth, depth_view));
        self.msaa_depth_id = crate::bind_cache::gpu_id();
    }

    /// Lazily create the sample-0 depth-resolve pipeline (ADR 0058).
    fn ensure_depth_resolve_pipeline(&mut self, device: &wgpu::Device) {
        if self.depth_resolve_pipeline.is_some() {
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Depth Resolve Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../depth_resolve.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Depth Resolve Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Depth Resolve Pipeline Layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Depth Resolve Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_resolve"),
                targets: &[],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        self.depth_resolve_layout = Some(layout);
        self.depth_resolve_pipeline = Some(pipeline);
    }

    /// Copy sample 0 of the MSAA depth into a single-sample depth view via
    /// a fullscreen frag-depth pass, so SSAO/DoF/fog/volumetric/ocean-grab
    /// consumers stay unchanged (ADR 0058).
    fn resolve_depth_into(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target_depth_view: &wgpu::TextureView,
    ) {
        let (pipeline, layout) = match (&self.depth_resolve_pipeline, &self.depth_resolve_layout) {
            (Some(p), Some(l)) => (p, l),
            _ => return,
        };
        let msaa_depth_view = match &self.msaa_depth {
            Some((_, v)) => v,
            None => return,
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Depth Resolve Bind Group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(msaa_depth_view),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Depth Resolve Pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target_depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    // ── Slim render_to: orchestrates shadow → uniforms → main pass → postprocess ──

    /// Render the scene to an arbitrary texture view with explicit device/queue/depth
    pub fn render_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Frame Encoder"),
        });
        self.encode_to(device, queue, &mut encoder, depth, camera, target_view);
        queue.submit(std::iter::once(encoder.finish()));
        self.frame_submitted();
    }

    /// [`render_to`](Self::render_to) into `encoder`, for a host that
    /// draws more into the frame (its HUD) and submits once (#139). On the
    /// web each submission crosses into JS; the frame used to make twenty.
    ///
    /// The frame's uniforms are written to `queue` now and reach the GPU
    /// with the next submission, so encode one camera per submission: a
    /// second camera's uniforms would overwrite this one's. After
    /// submitting, call [`frame_submitted`](Self::frame_submitted).
    #[profiling::function]
    ///
    /// `depth` is the caller's depth buffer: a `&TextureView`, or a
    /// [`DepthTarget`](crate::bind_cache::DepthTarget) with an id
    /// ([`RenderContext::depth_target`](crate::RenderContext::depth_target))
    /// so the bind groups over it are kept from frame to frame (#379).
    pub fn encode_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let depth = depth.into();
        let depth_view = depth.view;
        // A style switched on by any route (scene load, F4, a script's
        // config) is built before the frame that first needs it (#262).
        // When nothing new is on this is a walk over the registry.
        self.sync_debug_styles();
        self.ensure_style_resources(device);
        // FXAA likewise (#297): a host that only sets the config (the
        // viewer's hot reload and scene/default toggle, a script's post
        // override) drew none until a resize. Two `Option` checks when built.
        self.ensure_fxaa_resources(device);
        if self.device_lost {
            return;
        }
        // Taken for the frame, so the phases can be scoped while self is
        // borrowed; put back at the end (ADR 0108).
        let mut gpu_timer = self.gpu_timer.take();
        // Below render scale 1 the scene draws against a depth buffer of
        // its own size, copied into the caller's once the frame is drawn
        // (#122). Every scene pass and effect below sees that one.
        let surface_depth_view = depth_view;
        let scaled_depth = self.scaled_depth.clone();
        let depth_view = scaled_depth.as_ref().map_or(depth_view, |d| &d.view);
        // Who vouches for that view, so bind groups over it can be kept:
        // the chain for its own, the caller's id for the caller's (#379).
        let depth_key = if scaled_depth.is_some() {
            crate::bind_cache::DepthKey::Owned
        } else {
            crate::bind_cache::DepthKey::caller(depth.id)
        };
        let clear_color = wgpu::Color {
            r: self.clear_color[0] as f64,
            g: self.clear_color[1] as f64,
            b: self.clear_color[2] as f64,
            a: self.clear_color[3] as f64,
        };
        let view_proj = camera.view_projection_matrix();
        self.camera_frustum = Some(crate::frustum::Frustum::from_view_projection(&view_proj));

        // Draws an earlier render_to culled (another camera, same extract)
        // come back for the shadow pass and this camera's test.
        self.entity_draws.append(&mut self.culled_entity_draws);
        self.transparent_draws
            .append(&mut self.culled_transparent_draws);

        // Pick each model's level of detail for this camera and drop what is
        // past its draw distance (#56), before the shadow pass sees them.
        self.cull_draws_to_lod(camera.position_array());

        // Count visible terrain chunks for stats
        self.terrain_visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain_draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain_draws.len() as u32
        };

        let camera_pos = camera.position_array();
        let debug_mode_u32 = self.debug_state.mode.as_u32();
        let wireframe_only = self.debug_state.mode == DebugMode::WireframeOnly;

        // Update light uniforms
        queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&[self.light_uniforms]),
        );

        // Grass compute (ADR 0092): scatters this camera's blades, culled to
        // its view, and counts them into the indirect draw arguments. It goes
        // before the shadow pass so the cascades cast this frame's blades.
        // Nothing waits on it: the draws read the count on the GPU, and the
        // stats get it a frame or so late.
        self.grass_drawing = false;
        if self.grass_needs_realloc {
            if let Some(config) = self.grass_config.clone() {
                self.reload_grass_config(device, config);
            }
        }
        if self.grass_config.as_ref().is_some_and(|c| c.enabled) {
            let grass_time = self.grass_time;
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Grass"));
            self.dispatch_grass_compute(device, queue, encoder, camera, grass_time);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
            // Its count is read back once the frame is submitted
            self.grass_readback_due = true;
        } else {
            self.grass_instance_count = 0;
        }

        // Shadow pass: render depth from light perspective. It sees every
        // draw (culled per cascade), since a caster outside the view can
        // still shadow what's in it.
        let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Shadows"));
        self.render_shadow_pass(device, queue, encoder, camera, camera_pos);
        if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
            timer.end(encoder, scope);
        }

        // Set aside draws outside the camera frustum for the rest of the frame.
        self.cull_draws_to_camera();

        // All scene pipelines target Rgba16Float, so we always render to the HDR
        // buffer and composite to sRGB.
        let has_postprocess =
            self.postprocess_pipeline.is_some() && self.postprocess_resources.is_some();

        // Shader-side tonemapping is always OFF when compositing through the HDR
        // buffer (the composite pass handles ACES + gamma).
        let tonemapping_u32: u32 = if !has_postprocess && self.tonemapping_enabled {
            1
        } else {
            0
        };

        // Grab-pass refraction runs only with an active ocean + postprocess
        // (the HDR buffer is what gets snapshotted) in normal render mode.
        // Water bodies refract through the same grab (ADR 0094).
        self.ocean_grab_this_frame = self.water_active()
            && has_postprocess
            && !wireframe_only
            && self.ocean_pipeline.is_some();
        self.ocean_camera_near_far = (camera.near, camera.far);
        if self.ocean_grab_this_frame {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.ensure_ocean_grab_resources(device, w, h);
            if self.ocean_grab_bind_group.is_none() {
                self.ocean_grab_this_frame = false;
            }
        }

        // Update all per-frame uniforms and sort transparent draws
        self.update_per_frame_uniforms(
            queue,
            view_proj,
            camera_pos,
            debug_mode_u32,
            tonemapping_u32,
            camera,
        );

        // MSAA (ADR 0058): scene passes draw into MSAA color/depth and
        // resolve color into the HDR buffer; depth resolves (sample 0) into
        // the caller's depth view after each opaque phase so every existing
        // depth consumer stays single-sample. Requires the HDR chain (both
        // constructors always build it).
        let msaa_active = self.sample_count > 1 && has_postprocess;
        if msaa_active {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.ensure_msaa_targets(device, w, h);
            self.ensure_depth_resolve_pipeline(device);
        }

        // Choose render target: HDR buffer or direct to surface
        let scene_target_view = if has_postprocess {
            &self.postprocess_resources.as_ref().unwrap().hdr_view
        } else {
            target_view
        };

        // Scene-pass attachments: MSAA views with a color resolve when
        // active, the plain single-sample pair otherwise.
        let (pass_color_view, pass_resolve_target, pass_depth_view) = if msaa_active {
            (
                &self.msaa_color.as_ref().unwrap().1,
                Some(scene_target_view),
                &self.msaa_depth.as_ref().unwrap().1,
            )
        } else {
            (scene_target_view, None, depth_view)
        };

        let scene_scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Scene"));

        if !self.ocean_grab_this_frame {
            // Single-pass path (unchanged for scenes without an ocean).
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pass_color_view,
                    resolve_target: pass_resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: pass_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.render_main_pass(
                &mut render_pass,
                wireframe_only,
                queue,
                camera,
                RenderPhase::All,
            );
            drop(render_pass);
            if msaa_active {
                self.resolve_depth_into(device, encoder, depth_view);
            }
        } else {
            // ── Grab-pass split: opaque scene → snapshot → ocean + rest ──
            // Pass A: sky + all opaque geometry (including the legs).
            {
                let mut pass_a = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass A (pre-ocean)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: pass_color_view,
                        resolve_target: pass_resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear_color),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: pass_depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.render_main_pass(
                    &mut pass_a,
                    wireframe_only,
                    queue,
                    camera,
                    RenderPhase::PreOcean,
                );
            }

            // With MSAA, the grab blit below reads the caller's single-sample
            // depth view — resolve the opaque depth into it first (ADR 0058).
            if msaa_active {
                self.resolve_depth_into(device, encoder, depth_view);
            }

            // Blit: snapshot opaque color + depth into sampleable copies.
            {
                let op = self.ocean_pipeline.as_ref().unwrap();
                // The grab runs only into the HDR chain, so the bind group
                // is the chain's to keep while the depth is its too (#139).
                let resources = self.postprocess_resources.as_ref().unwrap();
                let blit_bind_group =
                    resources
                        .binds
                        .get_over(depth_key, "ocean grab blit", 0, || {
                            device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &op.blit_bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(
                                            scene_target_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                ],
                                label: Some("Grab Blit Bind Group"),
                            })
                        });
                let grab_color_view = &self.ocean_grab_color.as_ref().unwrap().1;
                let grab_depth_view = &self.ocean_grab_depth.as_ref().unwrap().1;
                let mut blit_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Ocean Grab Blit"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_color_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_depth_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                blit_pass.set_pipeline(&op.blit_pipeline);
                blit_pass.set_bind_group(0, &*blit_bind_group, &[]);
                blit_pass.draw(0..3, 0..1);
            }

            // Pass B: ocean (sampling the snapshots) + transparents/particles.
            {
                let mut pass_b = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass B (ocean + transparents)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: pass_color_view,
                        resolve_target: pass_resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: pass_depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.render_main_pass(
                    &mut pass_b,
                    wireframe_only,
                    queue,
                    camera,
                    RenderPhase::PostOcean,
                );
            }

            // Final depth resolve so postprocess (SSAO/DoF/fog/volumetric)
            // sees the complete frame's depth (ADR 0058).
            if msaa_active {
                self.resolve_depth_into(device, encoder, depth_view);
            }
        }

        if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scene_scope) {
            timer.end(encoder, scope);
        }

        // The entity style mask (#263): the styled entities' ids, drawn
        // against the frame's single-sample depth, while a style reads it.
        if has_postprocess && self.style_mask_wanted() {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Style mask"));
            self.encode_style_mask(device, queue, encoder, depth_view, depth_key);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // The editor's selection (#602): its draws into a mask, against the
        // same depth. Nothing selected, nothing drawn.
        let selection = self.selection_set();
        let outlining = !selection.is_empty();
        if outlining {
            let scope = gpu_timer
                .as_mut()
                .map(|t| t.begin(encoder, "Selection mask"));
            self.encode_selection_mask(device, encoder, depth_view, &selection);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // Composite: always needed to convert HDR → sRGB surface
        if has_postprocess {
            self.render_postprocess(
                device,
                queue,
                encoder,
                depth_view,
                depth_key,
                target_view,
                camera,
                gpu_timer.as_mut(),
            );
        }

        // The scaled scene's depth, into the surface-sized view the overlays
        // below and the caller's own (world text, gizmos) test against.
        if let (Some(_), Some(upscale)) = (&scaled_depth, &self.depth_upscale) {
            upscale.record(encoder, surface_depth_view);
        }

        // The selection outline, over the finished image: after every
        // style and the tonemap, so its colour is the one asked for (#602)
        if outlining {
            let scope = gpu_timer
                .as_mut()
                .map(|t| t.begin(encoder, "Selection outline"));
            self.encode_selection_outline(queue, encoder, target_view);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // World lines (ADR 0083) go over the finished image, tested against
        // the frame's single-sample depth. Without the HDR chain an MSAA depth
        // is never resolved, so they're skipped there.
        if !self.world_lines.is_empty() && (has_postprocess || self.sample_count == 1) {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "World lines"));
            self.render_world_lines(
                device,
                queue,
                encoder,
                surface_depth_view,
                target_view,
                camera,
            );
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }
        self.gpu_timer = gpu_timer;
    }

    /// [`encode_to`](Self::encode_to) a second time in a frame, for the other
    /// half of a split view (#238), with the GPU timer left out: its scopes are
    /// the first drawing's. Submit the first drawing before calling this, since
    /// what changed between the two (bones, the boil) reaches the GPU through
    /// the queue's writes, and those land with the next submission.
    pub fn encode_again_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        let timer = self.gpu_timer.take();
        self.encode_to(device, queue, encoder, depth_view, camera, target_view);
        self.gpu_timer = timer;
    }

    /// Start what waits on the frame [`encode_to`](Self::encode_to)
    /// recorded: the grass count's read-back and, once
    /// [`encode_gpu_timings`](Self::encode_gpu_timings) resolved them, the
    /// GPU timer's. Call once that frame is submitted.
    pub fn frame_submitted(&mut self) {
        if std::mem::take(&mut self.grass_readback_due) {
            if let Some(readback) = &mut self.grass_count_readback {
                readback.start_map();
            }
        }
        if let Some(timer) = &mut self.gpu_timer {
            timer.start_readback();
        }
    }

    fn render_world_lines(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        if self.postprocess_resources.is_none() {
            return;
        }
        // Drawn into the surface, whatever the scene's render scale
        let viewport = [self.surface_size.0 as f32, self.surface_size.1 as f32];
        let format = self.surface_format;
        let pipeline = self.world_line_pipeline.get_or_insert_with(|| {
            crate::world_line_pipeline::WorldLinePipeline::new(device, format)
        });
        let view = crate::world_line_pipeline::LineView {
            view_proj: camera.view_projection_matrix(),
            eye: camera.position_array(),
            viewport,
            px_per_point: self.world_line_px_per_point,
            linear_output: format.is_srgb(),
        };
        pipeline.draw(
            device,
            queue,
            encoder,
            target_view,
            depth_view,
            &self.world_lines,
            &view,
        );
    }
}

/// Point a model material's texture references at the names
/// [`SceneRenderer::upload_model_textures`] gave them: `<asset>::<texture>`,
/// and for a data map whose texture is also a colour map, the linear copy.
fn namespace_textures(
    material: &mut flint_import::ImportedMaterial,
    asset: &str,
    both: &HashSet<String>,
) {
    let colour = |tex: &mut Option<String>| {
        if let Some(t) = tex {
            *t = format!("{asset}::{t}");
        }
    };
    let data = |tex: &mut Option<String>| {
        if let Some(t) = tex {
            let suffix = if both.contains(t.as_str()) {
                "#linear"
            } else {
                ""
            };
            *t = format!("{asset}::{t}{suffix}");
        }
    };
    colour(&mut material.base_color_texture);
    colour(&mut material.emissive_texture);
    data(&mut material.normal_texture);
    data(&mut material.metallic_roughness_texture);
}

#[cfg(test)]
mod tests {
    use super::transform_aabb;

    fn translate_scale(t: [f32; 3], s: [f32; 3]) -> [[f32; 4]; 4] {
        [
            [s[0], 0.0, 0.0, 0.0],
            [0.0, s[1], 0.0, 0.0],
            [0.0, 0.0, s[2], 0.0],
            [t[0], t[1], t[2], 1.0],
        ]
    }

    #[test]
    fn aabb_follows_translation_and_scale() {
        let m = translate_scale([10.0, 0.0, -5.0], [2.0, 1.0, -3.0]);
        let (min, max) = transform_aabb(&m, [-1.0, 0.0, 0.0], [1.0, 1.0, 2.0]);
        assert_eq!(min, [8.0, 0.0, -11.0]);
        assert_eq!(max, [12.0, 1.0, -5.0]);
    }

    #[test]
    fn aabb_encloses_a_rotated_box() {
        // 45 degrees about Y: a unit box's corners reach sqrt(2) on X and Z.
        let (s, c) = std::f32::consts::FRAC_PI_4.sin_cos();
        let m = [
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let (min, max) = transform_aabb(&m, [-1.0; 3], [1.0; 3]);
        let r = 2.0_f32.sqrt();
        for (got, want) in min.iter().zip([-r, -1.0, -r]) {
            assert!((got - want).abs() < 1e-5, "{min:?}");
        }
        for (got, want) in max.iter().zip([r, 1.0, r]) {
            assert!((got - want).abs() < 1e-5, "{max:?}");
        }
    }

    /// A sheared tile (#625): its culling bounds hold every corner the
    /// matrix moves, and its normals, through the inverse transpose, stay
    /// perpendicular to the sheared surface rather than leaning with it.
    #[test]
    fn a_sheared_entity_is_bounded_and_lit_by_its_own_matrix() {
        use super::helpers::mat4_inv_transpose;
        use flint_core::{Transform, Vec3};
        // An 8 m tile, 20 m wide, leaning 0.5 m sideways per metre along z
        // and climbing 0.25 m per metre along z, then yawed 30 degrees.
        let t = Transform::default()
            .with_position(Vec3::new(3.0, 0.0, 1.0))
            .with_rotation(Vec3::new(0.0, 30.0, 0.0))
            .with_shear([0.0, 0.5, 0.0, 0.25, 0.0, 0.0]);
        let m = t.to_matrix();
        let apply = |p: [f32; 3]| {
            let mut out = [m[3][0], m[3][1], m[3][2]];
            for (row, o) in out.iter_mut().enumerate() {
                for (col, v) in p.iter().enumerate() {
                    *o += m[col][row] * v;
                }
            }
            out
        };
        let (lo, hi) = ([-10.0, 0.0, -4.0], [10.0, 0.1, 4.0]);
        let (min, max) = transform_aabb(&m, lo, hi);
        for corner in 0..8 {
            let p = [
                if corner & 1 == 0 { lo[0] } else { hi[0] },
                if corner & 2 == 0 { lo[1] } else { hi[1] },
                if corner & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let w = apply(p);
            for axis in 0..3 {
                assert!(w[axis] >= min[axis] - 1e-4 && w[axis] <= max[axis] + 1e-4);
            }
        }

        // Two edges of the tile's top face, in world space.
        let o = apply([0.0, 0.0, 0.0]);
        let along_x = apply([1.0, 0.0, 0.0]);
        let along_z = apply([0.0, 0.0, 1.0]);
        let ex: Vec<f32> = (0..3).map(|i| along_x[i] - o[i]).collect();
        let ez: Vec<f32> = (0..3).map(|i| along_z[i] - o[i]).collect();
        let it = mat4_inv_transpose(&m);
        let n: Vec<f32> = (0..3).map(|row| it[1][row]).collect(); // local +Y
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert!(dot(&n, &ex).abs() < 1e-5, "normal leans along x");
        assert!(dot(&n, &ez).abs() < 1e-5, "normal leans along z");
        // The plain model matrix would have tilted it with the shear.
        let naive: Vec<f32> = (0..3).map(|row| m[1][row]).collect();
        assert!(dot(&naive, &ez).abs() > 0.1);
    }

    /// A render-scale step keeps the surface's size, so it keeps the
    /// display-stage style targets (ASCII's cell history, #277) and the
    /// history in them (#270); a real resize still drops them.
    #[test]
    fn a_render_scale_step_keeps_the_style_targets() {
        let Ok(ctx) = pollster::block_on(crate::HeadlessContext::new(64, 64)) else {
            return; // no adapter here
        };
        let mut renderer = super::SceneRenderer::new_headless(
            &ctx.device,
            &ctx.queue,
            ctx.format,
            64,
            64,
            super::RendererConfig {
                show_grid: false,
                sample_count: 1,
            },
        );
        let probe = |r: &super::SceneRenderer| {
            r.postprocess_resources.as_ref().unwrap().styles.get(
                &ctx.device,
                "probe",
                wgpu::TextureFormat::Rgba16Uint,
                8,
                8,
            )
        };
        let before = probe(&renderer);
        renderer.set_render_scale(&ctx.device, 0.5);
        assert_eq!(renderer.scene_size(), (32, 32));
        assert!(std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
        renderer.set_render_scale(&ctx.device, 1.0);
        assert!(std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
        renderer.resize_postprocess(&ctx.device, 128, 64);
        assert!(!std::sync::Arc::ptr_eq(&before, &probe(&renderer)));
    }
}

/// The grass draw's indirect arguments (ADR 0092): the compute pass writes
/// the instance count, and the draws read the buffer as `INDIRECT`.
fn create_grass_draw_args_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Grass Draw Args"),
        contents: bytemuck::bytes_of(&GrassDrawArgs::RESET),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
    })
}

/// Upload one grass ground texture (heightmap, splat map or mask) and
/// return it with its view. `bytes_per_texel` is 4 for R32Float and
/// Rgba8Unorm, 1 for R8Unorm.
#[allow(clippy::too_many_arguments)]
fn upload_grass_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    data: &[u8],
    bytes_per_texel: u32,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}
