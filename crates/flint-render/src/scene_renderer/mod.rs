//! Scene renderer - converts FlintWorld entities to GPU meshes

mod assets;
mod draws;
mod extract;
mod grass;
mod grass_ground;
mod helpers;
mod lights;
mod msaa;
mod panels;
mod particles;
mod pipelines;
mod post;
mod render_passes;
mod selection;
mod skinning;
mod sky;
mod stats;
mod style_mask;
mod terrain;
mod water;

pub use grass_ground::{load_grass_ground_in, load_scene_grass_ground, LoadedGrassGround};
pub use particles::load_scene_particle_textures;
use post::next_post_config_generation;
use render_passes::RenderPhase;

pub use lights::LightingLevers;
use skinning::{CachedBoneBind, SHADOW_BONE_KEY};

use crate::bind_cache::{ArenaSlot, TransformArena};
use crate::bitmap_font::BitmapFont;
use crate::camera::Camera;
use crate::context::RenderContext;
use crate::debug::{DebugMode, DebugState};
use crate::gpu_mesh::MeshCache;
use crate::grass_pipeline::{
    GrassEntityPosition, GrassGround, GrassHeight, GrassInstanceGpu, MAX_GRASS_ENTITIES,
};
use crate::particle_pipeline::{ParticleBuffers, ParticleDrawCall};
use crate::pipeline::{BlendMode, MaterialUniforms, SceneLayouts, TransformUniforms};
use crate::postprocess::{
    PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT,
};
use crate::primitives::create_grid_mesh;
use crate::shadow::{ShadowPass, DEFAULT_SHADOW_RESOLUTION};
use crate::sprite2d_pipeline::Sprite2dInstanceGpu;
use crate::texture_cache::TextureCache;
use crate::wooze_pipeline::{grow_bounds, ShallowWoozeDraw, ShallowWoozeUniforms};
use flint_core::components as comp;
use flint_core::Transform;
use flint_ecs::FlintWorld;
use flint_import::ImportResult;
use pipelines::{Present, ScenePipelines};
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

/// How long a material nothing draws with stays in the cache: about 2 s.
const MATERIAL_GRACE_FRAMES: u64 = 120;

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

/// Renders a FlintWorld to the screen
pub struct SceneRenderer {
    /// The bind group layouts every scene pipeline is made against, made once.
    layouts: SceneLayouts,
    /// Every pipeline that bakes in the MSAA sample count, and that count,
    /// replaced whole by `set_msaa` (ADR 0118, #735).
    pipelines: ScenePipelines,
    /// Shallow wooze (#215): each entity's drift this frame, from the caller.
    /// Its pipelines are in `pipelines`, built when the first draw needs them.
    shallow_wooze: HashMap<flint_core::EntityId, ShallowWoozeDraw>,
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
    ray_meshes: HashMap<String, Arc<flint_geometry::ray_mesh::RayMesh>>,
    /// Where the ray targets stood at the last `update_from_world`.
    ray_scene: Arc<flint_geometry::ray_mesh::RayScene>,
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
    /// The light uniforms, their bind group, and the scene's lighting
    /// levers (ADR 0118, #736).
    lights: lights::LightState,
    texture_cache: Option<TextureCache>,
    shadow_pass: Option<ShadowPass>,
    selected_entity: Option<flint_core::EntityId>,
    /// Each entity's children, as the last `update_from_world` saw them,
    /// so the selection outline covers a selected entity's subtree (#602)
    entity_children: HashMap<flint_core::EntityId, Vec<flint_core::EntityId>>,
    /// The selection outline's pipelines and mask, made the first frame
    /// something is selected (#602)
    selection_outline: Option<crate::selection_outline::SelectionOutline>,
    /// The procedural sky and the texture skybox (ADR 0118, #736).
    sky: sky::SkyRenderer,
    /// The loaded terrain's chunk draws and material (ADR 0118, #736).
    terrain: terrain::TerrainRenderer,
    camera_frustum: Option<crate::frustum::Frustum>,
    /// Level-of-detail and draw-distance scales (#56)
    lod_settings: LodSettings,
    /// Draws the last `render_to` set aside by level of detail
    lod_culled: u32,
    /// Grass (ADR 0089, 0092): the compute scatter's and the draw's
    /// resources and the ground it grows on (ADR 0118, #736).
    grass: grass::GrassRenderer,
    /// Times the frame's phases on the GPU, while timing is on (ADR 0108)
    gpu_timer: Option<crate::gpu_timer::GpuTimer>,
    /// Comic panels (#880, ADR 0120): the layout and each panel's view
    panel_state: panels::PanelState,
    /// Per-entity styles (#263): who is styled, how far, the mask and
    /// albedo passes (ADR 0118, #736).
    style_mask: style_mask::StyleMaskState,
    /// The `style` debug view switched `mask_view` on, so it switches it off.
    mask_view_forced: bool,
    /// Whether the `edges` view switched ASCII on itself (#349).
    ascii_forced: bool,
    /// The sea, water bodies and their refraction grab (ADR 0118, #736).
    water: water::WaterRenderer,
    /// Time in seconds driving wave phases. Set by the player each frame from
    /// the game clock — the same clock scripts see via total_time(), which is
    /// what keeps script-side ocean_height() queries in sync with the GPU.
    pub ocean_time: f64,
    /// Motion on twos (#405): who is drawn on ticks, what they last showed,
    /// and the clock hosts stamp with [`Self::set_motion_time`].
    motion_hold: crate::motion_hold::MotionHold,
    /// Seconds of game time this frame stepped, for styles whose history
    /// is a rate (ASCII's smoothing, #342): the player sets it from its
    /// clock's `delta_time` (so it follows `--frame-step`), the viewer from
    /// its frame time. Hosts that don't set it get 1/60, which is what a
    /// per-frame weight always meant.
    pub style_dt: f32,
    // Particles (ADR 0068): their pipeline is in `pipelines`, and what it
    // draws from here, so instances uploaded before an MSAA change still draw.
    particle_buffers: Option<ParticleBuffers>,
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
    sprite2d_batches: Vec<crate::sprite2d_pipeline::Sprite2dBatch>,
    /// MSAA's targets and depth resolve (ADR 0058), made the first frame
    /// that draws at the sample count `pipelines` was built with.
    msaa: msaa::MsaaTargets,
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
    /// Per-entity bones, shadow bones, echoes, part tints and softness,
    /// and the skinned draws' bone binds (ADR 0118, #736).
    skinning: skinning::SkinningState,
}

impl SceneRenderer {
    /// A renderer for a window: the context's device, queue, surface format
    /// and size.
    pub fn new(context: &RenderContext, config: RendererConfig) -> Self {
        Self::build(
            &context.device,
            &context.queue,
            context.view_format,
            (context.config.width, context.config.height),
            config,
        )
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
        Self::build(device, queue, format, (width, height), config)
    }

    /// The one constructor body (ADR 0118, #736): `surface_format` is what
    /// the composite pass writes, `size` that surface's size in pixels.
    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
        config: RendererConfig,
    ) -> Self {
        // Scene geometry renders to HDR; the composite pass tonemaps to the
        // surface (a window's, or the headless readback target).
        let scene_format = HDR_FORMAT;
        let sample_count = Self::resolve_sample_count(config.sample_count);

        let layouts = SceneLayouts::new(device);
        let pipelines =
            ScenePipelines::build(device, scene_format, sample_count, &layouts, Present::NEW);
        let archetype_visuals = Self::default_archetype_visuals();
        let texture_cache = TextureCache::new(device, queue);

        // Create grid draw call (only for debug/inspection modes)
        let grid_draw = if config.show_grid {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            Some(Self::create_draw_call(
                device,
                &layouts,
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

        let lights = lights::LightState::new(device, &layouts, &shadow_pass);

        let water = water::WaterRenderer::new(
            device,
            pipelines.ocean.as_deref(),
            &layouts.transform_bind_group_layout,
        );
        let sky = sky::SkyRenderer::new(device, pipelines.sky.as_deref());
        let particle_buffers = pipelines
            .particle
            .as_ref()
            .map(|p| ParticleBuffers::new(device, p));

        // Create post-processing pipeline and resources
        let postprocess_config = PostProcessConfig::default();
        let postprocess_pipeline = PostProcessPipeline::new(device, queue, surface_format);
        let postprocess_resources = PostProcessResources::new(device, size.0, size.1);

        Self {
            layouts,
            pipelines,
            shallow_wooze: HashMap::new(),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            draw_bind_cache: DrawBindCache::default(),
            box_mesh_cache: HashMap::new(),
            extract_frame: 0,
            ray_meshes: HashMap::new(),
            ray_scene: Arc::default(),
            skinning: Default::default(),
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
            lights,
            texture_cache: Some(texture_cache),
            shadow_pass: Some(shadow_pass),
            selected_entity: None,
            entity_children: HashMap::new(),
            selection_outline: None,
            sky,
            terrain: Default::default(),
            camera_frustum: None,
            lod_settings: LodSettings::from_env(),
            lod_culled: 0,
            gpu_timer: None,
            panel_state: Default::default(),
            style_mask: style_mask::StyleMaskState::new(),
            mask_view_forced: false,
            ascii_forced: false,
            water,
            ocean_time: 0.0,
            motion_hold: Default::default(),
            style_dt: 1.0 / 60.0,
            // Particles (ADR 0068): no instances until a caller steps a
            // ParticleSystem, so a scene without one draws none.
            particle_buffers,
            particle_draws: Vec::new(),
            particle_texture_bind_groups: HashMap::new(),
            particle_upload_scratch: Vec::new(),
            debug_overlay_draws: Vec::new(),
            world_lines: Vec::new(),
            world_line_px_per_point: 1.0,
            world_line_pipeline: None,
            clear_color: DEFAULT_CLEAR_COLOR,
            sprite2d_batches: Vec::new(),
            msaa: Default::default(),
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
            surface_size: size,
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
            grass: Default::default(),
            grass_time: 0.0,
            grass_wooze: None,
            device_lost: false,
        }
    }

    /// Load terrain chunks and create GPU resources for terrain rendering.
    #[allow(clippy::too_many_arguments)]
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
        let scope = self.asset_scope();
        self.terrain.load_terrain(
            device,
            queue,
            self.pipelines.terrain.as_deref(),
            &mut self.texture_cache,
            &self.layouts.transform_bind_group_layout,
            &scope,
            chunks,
            transform,
            texture_tile,
            metallic,
            roughness,
            splat_path,
            layer_paths,
            scene_dir,
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
        self.terrain.reload_terrain_geometry(
            device,
            &self.layouts.transform_bind_group_layout,
            chunks,
            transform,
        );
    }

    /// Load terrain from raw data (heightmap-generated chunks + raw splat RGBA).
    /// Like `load_terrain()` but accepts raw splat data instead of a file path.
    #[allow(clippy::too_many_arguments)]
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
        self.terrain.load_terrain_from_data(
            device,
            queue,
            self.pipelines.terrain.as_deref(),
            &mut self.texture_cache,
            &self.layouts.transform_bind_group_layout,
            chunks,
            transform,
            texture_tile,
            metallic,
            roughness,
            splat_data,
            splat_res,
            layer_paths,
            spec_dir,
        );
    }

    /// Name the entity the loaded terrain belongs to (#310): a
    /// `style_mask` (or a style's own component) on it, or on an ancestor,
    /// then masks the terrain's chunks like any styled mesh. Hosts call it
    /// beside [`load_terrain`](Self::load_terrain); `None` leaves the
    /// terrain unmasked, as it was before.
    pub fn set_terrain_entity(&mut self, entity: Option<flint_core::EntityId>) {
        self.terrain.entity = entity;
    }

    /// Clear terrain draw calls (for scene transitions)
    pub fn clear_terrain(&mut self) {
        self.terrain.clear();
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
        self.skinning.update_bone_matrices(
            device,
            queue,
            &mut self.motion_hold,
            &self.mesh_cache,
            entity_id,
            asset_name,
            matrices,
        );
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
        self.skinning
            .write_entity_bones(device, queue, entity_id, first, matrices);
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
        self.skinning
            .write_entity_shadow_bones(device, queue, entity_id, matrices);
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
        self.skinning.write_entity_echo_bones(
            self.pipelines
                .skinned
                .as_deref()
                .map(|p| &p.bone_bind_group_layout),
            device,
            queue,
            entity_id,
            echo,
            opacity,
            first,
            matrices,
        );
    }

    /// Stop drawing an entity's echoes from `count` on. Their buffers stay, ready
    /// for the next write, so a slider sweeping a count allocates nothing on its
    /// way back up: an opacity of 0 is what leaves one out at extraction.
    pub fn set_entity_echo_count(&mut self, entity_id: flint_core::EntityId, count: usize) {
        self.skinning.set_entity_echo_count(entity_id, count);
    }

    /// How an entity's echoes meet what's behind them: over it at their
    /// opacity (`false`, the default), or `lighten`, the brighter of the two,
    /// so an echo can only add light and never darken the form it's behind.
    pub fn set_entity_echo_lighten(&mut self, entity_id: flint_core::EntityId, lighten: bool) {
        self.skinning.set_entity_echo_lighten(entity_id, lighten);
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
        self.skinning.set_entity_part_tint(entity_id, part, tint);
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
        self.skinning
            .set_entity_part_softness(entity_id, part, softness);
    }

    /// This frame's game time, for motion held on twos (#405): call it each
    /// frame before uploading bones and before `update_from_world`, with the
    /// clock the post time follows (the player's `total_time`, a capture's
    /// moment), so poses and a style's `boil` turn on the same frames. A
    /// renderer never stamped holds nothing: every pose is live.
    pub fn set_motion_time(&mut self, seconds: f64) {
        self.motion_hold.set_time(seconds);
    }

    /// Whether anything is drawn on ticks this frame (#405).
    pub fn motion_held(&self) -> bool {
        self.motion_hold.any()
    }

    /// Let `entity`'s next pose and position through a hold now, and hold
    /// those (#871): what an editor calls when it changed the pose by hand
    /// (a layer weight, a posed bone) rather than by time, so the change
    /// shows within the tick, paused or not.
    pub fn release_motion_hold(&mut self, entity: flint_core::EntityId) {
        self.motion_hold.release(entity);
    }

    /// Drop per-entity bone buffers (call when a scene is unloaded).
    pub fn clear_entity_bone_buffers(&mut self) {
        self.motion_hold.clear();
        self.skinning.clear();
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
        self.lights.rebind(device, &self.layouts, &shadow_pass);
        self.shadow_pass = Some(shadow_pass);
    }

    /// Draw no skybox until the next [`load_skybox`](Self::load_skybox): a
    /// scene that names none starts without the last scene's (#723).
    pub fn clear_skybox(&mut self) {
        self.sky.clear_skybox();
    }

    /// Whether a skybox is loaded and will be drawn.
    pub fn has_skybox(&self) -> bool {
        self.sky.has_skybox()
    }

    /// Load an equirectangular panorama image as a skybox
    pub fn load_skybox(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) {
        self.sky
            .load_skybox(device, queue, self.pipelines.skybox.as_deref(), path);
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
        self.lights.set_scene_shadows(on);
    }

    /// The scene's `[environment] default_lights` (#627). False lights a
    /// scene with no light entities by its ambient alone (and emission),
    /// instead of the built-in key and fill. Takes effect at the next
    /// `update_from_world`.
    pub fn set_default_lights(&mut self, on: bool) {
        self.lights.set_default_lights(on);
    }

    /// Whether this frame draws the shadow cascades (#627): shadows are on,
    /// the scene allows them, and its strongest directional light gives
    /// light or god rays. A sun of intensity 0 contributes nothing the
    /// maps could darken, so its pass is skipped and every caster with it.
    pub fn shadows_drawn(&self) -> bool {
        self.shadows_enabled() && self.lights.scene_shadows() && self.lights.sun_casts()
    }

    /// Current shadow map resolution per cascade
    pub fn shadow_resolution(&self) -> u32 {
        self.shadow_pass
            .as_ref()
            .map_or(crate::shadow::DEFAULT_SHADOW_RESOLUTION, |sp| sp.resolution)
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
            self.style_mask.resolve(world, &self.postprocess_config);
        } else {
            self.style_mask.clear();
        }

        // Who is drawn on ticks this frame (#405): empty, and unwalked, in a
        // scene with no motion_hold and no whole-frame rate. Before the
        // lights, which a held entity carries on its ticks too (#873)
        self.motion_hold.set_rates(crate::motion_hold::held_rates(
            world,
            self.postprocess_config.motion_hold,
        ));
        if self.motion_hold.any() {
            // Bones went up before the rates were known: a hold that starts
            // this frame keeps what they show
            for id in self.skinning.bone_buffers.keys() {
                self.motion_hold.bones_shown(*id);
            }
        }

        // Extract lights from scene entities
        self.lights.extract(world, &mut self.motion_hold);

        // Extract ocean params (regenerates the wave spectrum only on change)
        self.water
            .extract_ocean(world, self.pipelines.ocean.is_some());
        self.water.extract_contact(world, self.ocean_time);
        self.water
            .extract_bodies(world, device, self.pipelines.ocean.as_deref());
        if self.water_active() {
            // Ocean always binds group 3; make sure the placeholder exists
            // even on paths where the grab pass never runs.
            self.water
                .ensure_grab_dummy(device, self.pipelines.ocean.as_deref());
        }

        // Extract procedural sky params (+ optional ambient override)
        self.sky.extract(
            world,
            self.pipelines.sky.is_some(),
            &mut self.lights.uniforms,
        );

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
        let mut ray_scene = flint_geometry::ray_mesh::RayScene::new();

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
            // Held on twos (#405): where it was drawn when its tick began
            let model_matrix = self.motion_hold.matrix(entity.id, model_matrix);
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
        self.skinning.bone_binds.retain(|_, b| b.touched == frame);
        self.ray_scene = Arc::new(ray_scene);

        // Put texture cache back
        self.texture_cache = tex_cache;
    }

    // ── Water (WaterRenderer, ADR 0118) ──

    /// Whether any water draws this frame: the sea, or a body.
    pub(crate) fn water_active(&self) -> bool {
        self.water.any_active(self.pipelines.ocean.is_some())
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
        self.water
            .set_body(device, self.pipelines.ocean.as_deref(), id, mesh, look)
    }

    /// Change a body's look and keep its mesh. Returns whether it existed.
    pub fn set_water_body_look(
        &mut self,
        id: &str,
        look: crate::water_body::WaterBodyLook,
    ) -> bool {
        self.water.set_body_look(id, look)
    }

    /// Remove the body set with `id`. Returns whether it existed.
    pub fn remove_water_body(&mut self, id: &str) -> bool {
        self.water.remove_body(id)
    }

    /// Remove every body a game set (the scene's own stay).
    pub fn clear_water_bodies(&mut self) {
        self.water.clear_bodies();
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

    // ── Lighting levers (LightState, ADR 0118) ──

    /// Override the hemisphere ambient colors (linear RGB). Scenes set this
    /// via `[environment] ambient_sky / ambient_ground`; when never called,
    /// the built-in default ambient is byte-identical to prior behavior.
    pub fn set_ambient(&mut self, sky: [f32; 3], ground: [f32; 3]) {
        self.lights.set_ambient(sky, ground);
    }

    /// Clear any scene-authored ambient override, restoring the built-in
    /// default (needed on scene transitions so an old scene's ambient never
    /// leaks into a scene that doesn't author one).
    pub fn reset_ambient(&mut self) {
        self.lights.reset_ambient();
    }

    /// Soften the diffuse terminator (0 = physically sharp / legacy shading).
    /// Scenes set this via `[environment] diffuse_wrap`.
    pub fn set_diffuse_wrap(&mut self, wrap: f32) {
        self.lights.set_diffuse_wrap(wrap);
    }

    /// Blend the diffuse term from Lambert toward the Fujii Oren-Nayar
    /// approximation (0 = exact legacy shading, 1 = full Oren-Nayar; sigma
    /// comes from material roughness). Scenes set this via
    /// `[environment] oren_nayar` (ADR 0048).
    pub fn set_oren_nayar(&mut self, blend: f32) {
        self.lights.set_oren_nayar(blend);
    }

    /// Tinted Charlie-sheen rim: `color` is a linear RGB tint, `strength`
    /// 0..~0.3 (0 = exact legacy shading). Scenes set this via
    /// `[environment] sheen_color / sheen_strength` (ADR 0048).
    pub fn set_sheen(&mut self, color: [f32; 3], strength: f32) {
        self.lights.set_sheen(color, strength);
    }

    /// Snapshot of the current lighting-lever state (overrides if set, else
    /// the built-in neutral defaults). Read by the render debug panel.
    pub fn lighting_levers(&self) -> LightingLevers {
        self.lights.levers()
    }

    // ── Render entry points ──

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), crate::SurfaceError> {
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
        // A tear is a script's, set every frame, maybe after the world was
        // read: a prop whose blend follows it (#397) is off while the
        // config this frame draws has none
        self.style_mask
            .ids
            .set_tear(crate::postprocess::tear_active(&self.postprocess_config));
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
        self.terrain.visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain
                .draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain.draws.len() as u32
        };

        let camera_pos = camera.position_array();
        let debug_mode_u32 = self.debug_state.mode.as_u32();
        let wireframe_only = self.debug_state.mode == DebugMode::WireframeOnly;

        // Update light uniforms
        self.lights.upload(queue);

        // Grass compute (ADR 0092): scatters this camera's blades, culled to
        // its view, and counts them into the indirect draw arguments. It goes
        // before the shadow pass so the cascades cast this frame's blades.
        // Nothing waits on it: the draws read the count on the GPU, and the
        // stats get it a frame or so late.
        self.grass.drawing = false;
        if self.grass.needs_realloc {
            if let Some(config) = self.grass.config.clone() {
                self.reload_grass_config(device, config);
            }
        }
        if self.grass.config.as_ref().is_some_and(|c| c.enabled) {
            let grass_time = self.grass_time;
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Grass"));
            self.dispatch_grass_compute(device, queue, encoder, camera, grass_time);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
            // Its count is read back once the frame is submitted
            self.grass.readback_due = true;
        } else {
            self.grass.instance_count = 0;
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
        self.water.grab_this_frame = self.water_active()
            && has_postprocess
            && !wireframe_only
            && self.pipelines.ocean.is_some();
        self.water.camera_near_far = (camera.near, camera.far);
        if self.water.grab_this_frame {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.water
                .ensure_grab(device, self.pipelines.ocean.as_deref(), w, h);
            if self.water.grab.bind_group.is_none() {
                self.water.grab_this_frame = false;
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
        // depth consumer stays single-sample. Requires the HDR chain (`build`
        // always makes it).
        let msaa_active = self.pipelines.samples() > 1 && has_postprocess;
        if msaa_active {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.msaa.prepare(device, self.pipelines.samples(), w, h);
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
                self.msaa.color_view().unwrap(),
                Some(scene_target_view),
                self.msaa.depth_view().unwrap(),
            )
        } else {
            (scene_target_view, None, depth_view)
        };

        let scene_scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Scene"));

        if !self.water.grab_this_frame {
            // Single-pass path (unchanged for scenes without an ocean).
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pass_color_view,
                    resolve_target: pass_resolve_target,
                    depth_slice: None,
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
                multiview_mask: None,
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
                self.msaa.resolve_depth_into(device, encoder, depth_view);
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
                        depth_slice: None,
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
                    multiview_mask: None,
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
                self.msaa.resolve_depth_into(device, encoder, depth_view);
            }

            // Blit: snapshot opaque color + depth into sampleable copies.
            {
                let op = self.pipelines.ocean.as_deref().unwrap();
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
                let grab_color_view = &self.water.grab.color.as_ref().unwrap().1;
                let grab_depth_view = &self.water.grab.depth.as_ref().unwrap().1;
                let mut blit_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Ocean Grab Blit"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_color_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_depth_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
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
                        depth_slice: None,
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
                    multiview_mask: None,
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
                self.msaa.resolve_depth_into(device, encoder, depth_view);
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

        // The style albedo (#869): the opaque meshes' own colour, while a
        // style divides the picture by it
        if has_postprocess
            && crate::stylize::any_uses_albedo(&self.postprocess_config, &self.style_mask.ids)
        {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Style albedo"));
            self.encode_style_albedo(device, encoder, depth_view);
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
        if !self.world_lines.is_empty() && (has_postprocess || self.pipelines.samples() == 1) {
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
        if std::mem::take(&mut self.grass.readback_due) {
            if let Some(readback) = &mut self.grass.count_readback {
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

#[cfg(test)]
mod tests;
