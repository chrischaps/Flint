//! The entity style mask (#263): a per-pixel style id that says which
//! style, if any, drew each pixel, so a style pass can stylize chosen
//! entities and leave the rest of the scene alone.
//!
//! An entity asks for a style with the `style_mask` component:
//!
//! ```toml
//! [entities.barrel.style_mask]
//! style = "ascii"
//! ```
//!
//! Its descendants inherit the style unless they carry a `style_mask` of
//! their own; `enabled = false` takes an entity and its subtree out again.
//!
//! ## Per-entity settings (#266)
//!
//! A style that takes settings per entity names a component of its own
//! ([`StyleDescriptor::component`], `ascii_style` for ASCII). Its fields are
//! the style's parameters and options, and any it leaves out come from the
//! scene's `[post_process.styles.<name>]` table, then the style's defaults:
//!
//! ```toml
//! [entities.barrel.ascii_style]
//! ramp = "blocks"
//! tint = [1.0, 0.7, 0.3]   # a 3-array fills a style's tint_r/g/b
//! ```
//!
//! Each distinct set of settings is a [`StyleVariant`] with an id of its
//! own, so entities that say the same thing share an id and a pass reads
//! the settings of every pixel's id from one small table. A style whose
//! `grid` option is `object` gets one variant per styled root, anchored at
//! that root's world origin. `enabled = false` on the component is an
//! opt-out variant: it keeps the entity out of that style even where the
//! style covers the whole screen. A plain `style_mask { style = "ascii" }`
//! is the same as an empty `ascii_style`.
//!
//! ## How the mask is drawn
//!
//! The ASCII spike (#261) measured the two ways to write it and chose a
//! redraw over a new attachment on every scene pipeline. After the scene
//! pass (and the MSAA depth resolve), each visible opaque draw of a styled
//! entity is drawn again into one `Rg16Uint` target at scene resolution,
//! with its cached transform bind group (and, for skinned meshes, its bone
//! bind group), so the per-entity loop makes no buffer and no bind group.
//! There's no depth attachment: each fragment tests itself against the
//! frame's single-sample depth with a tolerance of its own depth slope, so
//! the mask works the same under MSAA (sample-0 resolve) and at any render
//! scale, and the scene pipelines don't change.
//!
//! Each draw's id goes into one dynamic-offset uniform buffer, uploaded in
//! a single write a frame. The mask target lives in the post chain's
//! [`StyleTargets`](crate::stylize::StyleTargets), so a resize drops it with
//! every bind group over it, and it is only made, and only drawn, while a
//! style that reads it is on ([`StyleDescriptor::uses_mask`]) or the
//! `style` debug view is showing.
//!
//! ## Blending in and out (#278)
//!
//! A `blend` field on the style component (or the `style_mask`) draws the
//! entity with that much of the style, 0 to 1. It rides in the mask beside
//! the id (the `.g` channel), not in the variant, so a script ramping it
//! every frame keeps the entity's id and makes no new variant. A style
//! mixes its own pixel with what it was given by that amount; ASCII can
//! also dissolve, turning that share of its cells.
//!
//! ## What's in front: the veil (#290)
//!
//! Transparents and particles write no depth, so a depth-tested redraw
//! alone would leave a styled stone masked where fire or glass sits in
//! front of it, and the style would glyphize the fire as stone. Just before
//! the mask, a veil pass draws every visible transparent mesh (static and
//! skinned) and every particle into an `R8Unorm` target, depth-tested
//! against the scene with no depth write, each with the alpha it is drawn
//! with (an additive particle by its alpha times its brightest channel),
//! "over" blended so layers add up to their combined coverage. The mask
//! then writes each styled pixel's blend times what the veil leaves
//! uncovered. The rule: **what's in front unmasks by its coverage.** A
//! style that honours the blend (ASCII, user styles through
//! `flint_style_blend`) shows the fire as fire and the stone through any
//! uncovered share; a fully covered pixel keeps its id at blend 0. A
//! transparent that is itself styled is left out of the veil (whether it
//! should be stylized is #310).
//!
//! Sprites, grass, terrain and water stay unmasked and out of the veil:
//! they write no mask (and, but for terrain, no depth either).
//!
//! [`StyleDescriptor::uses_mask`]: crate::stylize::StyleDescriptor::uses_mask
//! [`StyleDescriptor::component`]: crate::stylize::StyleDescriptor::component

use crate::postprocess::PostProcessConfig;
use crate::stylize::StyleDescriptor;
use bytemuck::{Pod, Zeroable};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The mask's format: one style id a pixel in `.r`, 0 where no style
/// drew, and the entity's blend in `.g` (#278), `0..=BLEND_ONE` for 0..1.
/// A reader that only wants the id reads `.r`, as it did when the mask
/// was `R16Uint`.
pub const STYLE_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Uint;

/// The mask's `.g` for a blend of 1: the style fully on.
pub const BLEND_ONE: u32 = u16::MAX as u32;

/// The field of a style component (or `style_mask`) that says how much of
/// the style an entity is drawn with (#278): 0 is its own shading, 1 the
/// style's, anything between a mix (a cross-fade, or for ASCII's
/// `transition = "dissolve"` the share of cells that have turned).
pub const BLEND_FIELD: &str = "blend";

/// A blend in the mask's `.g` encoding.
pub fn encode_blend(blend: f32) -> u32 {
    (blend.clamp(0.0, 1.0) * BLEND_ONE as f32).round() as u32
}

/// The id of a pixel no style drew.
pub const NO_STYLE: u16 = 0;

/// The component an entity names its style in.
pub const STYLE_MASK_COMPONENT: &str = "style_mask";

/// The name of the mask in the post chain's style targets.
pub(crate) const MASK_TARGET: &str = "style mask";

/// The name of the veil (#290) in the post chain's style targets: how much
/// of each pixel transparents and particles in front of the scene cover.
pub(crate) const VEIL_TARGET: &str = "style veil";

/// The veil's format: one blendable coverage a pixel.
pub const VEIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The scene depth's format, which the veil tests against.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How far behind the scene's depth a mask fragment may be, in multiples
/// of its own depth slope (`fwidth(z)`). The spike's value: it absorbs the
/// sample-0 resolve under MSAA without letting a hidden face through.
pub const SLOPE_TOLERANCE: f32 = 2.0;

/// The per-draw slots the mask's uniform buffer starts with (4 KB, #311).
/// A frame with more styled draws grows it to the next power of two, so a
/// scene pays for the draws it has and every one of them is masked.
pub const INITIAL_MASK_SLOTS: usize = 16;

/// The slots a buffer holding `capacity` needs to grow to for `draws`:
/// `capacity` while it fits, else the next power of two.
pub fn mask_slots_for(capacity: usize, draws: usize) -> usize {
    if draws <= capacity {
        capacity
    } else {
        draws.next_power_of_two().max(INITIAL_MASK_SLOTS)
    }
}

/// The dynamic-offset stride of the per-draw uniforms: the alignment
/// wgpu's default limits ask for.
const DRAW_STRIDE: u64 = 256;

// ---------------------------------------------------------------------------
// Style names -> ids
// ---------------------------------------------------------------------------

/// What one style id stands for (#266): the style that draws it and the
/// settings its entities give that differ from the scene's.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StyleVariant {
    /// The style's registered name (or whatever a `style_mask` names).
    pub style: String,
    /// Parameters set on the entity's style component.
    pub params: BTreeMap<String, f32>,
    /// Options set on the entity's style component.
    pub options: BTreeMap<String, String>,
    /// An opt-out (`enabled = false` on the style component): these pixels
    /// are left alone even where the style covers the whole screen.
    pub off: bool,
    /// The world-space point an object-anchored grid (`grid = "object"`)
    /// is pinned to: the styled root's origin, this frame.
    pub anchor: Option<[f32; 3]>,
}

impl StyleVariant {
    /// A variant with nothing of its own.
    pub fn plain(style: &str) -> Self {
        Self {
            style: style.to_string(),
            ..Default::default()
        }
    }

    /// Parameter `name`: the entity's, else the scene's, else the default.
    pub fn param(&self, d: &StyleDescriptor, config: &PostProcessConfig, name: &str) -> f32 {
        self.params
            .get(name)
            .copied()
            .unwrap_or_else(|| d.param(config, name))
    }

    /// Option `name`: the entity's, else the scene's, else the default.
    pub fn option<'a>(
        &'a self,
        d: &StyleDescriptor,
        config: &'a PostProcessConfig,
        name: &str,
    ) -> &'a str {
        match self.options.get(name) {
            Some(v) => v,
            None => d.option(config, name),
        }
    }
}

/// The styles entities ask for, numbered from 1 in name order. Ids change
/// only when the set of names does, and are the same for the same scene
/// on every run (no entity id or hash order goes into them).
///
/// A name is a style's, with its per-entity settings spelled after it when
/// it has any (`ascii{ramp=blocks}`, `ascii@barrel`, `ascii{off}`), so
/// entities asking for the same thing share a name and so an id.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StyleIds {
    names: Vec<String>,
    /// What each name stands for, beside `names`.
    variants: Vec<StyleVariant>,
}

impl StyleIds {
    /// What id `id` stands for.
    pub fn variant(&self, id: u16) -> Option<&StyleVariant> {
        (id as usize)
            .checked_sub(1)
            .and_then(|i| self.variants.get(i))
    }

    /// `(id, variant)` for every variant of style `style`, in id order.
    pub fn variants_of<'a>(
        &'a self,
        style: &'a str,
    ) -> impl Iterator<Item = (u16, &'a StyleVariant)> + 'a {
        self.variants
            .iter()
            .enumerate()
            .filter(move |(_, v)| v.style == style)
            .map(|(i, v)| ((i + 1) as u16, v))
    }

    /// Whether an entity asks to be drawn in `style` (an opt-out doesn't).
    pub fn requests(&self, style: &str) -> bool {
        self.variants.iter().any(|v| v.style == style && !v.off)
    }

    /// Whether any entity names `style`, opt-outs included.
    pub fn mentions(&self, style: &str) -> bool {
        self.variants.iter().any(|v| v.style == style)
    }

    /// Make the table hold exactly `entries` (any order; a repeated name
    /// keeps its first variant). Returns whether the set of names changed;
    /// the variants (their anchors move) are taken either way.
    pub fn set(&mut self, mut entries: Vec<(String, StyleVariant)>) -> bool {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        entries.truncate(u16::MAX as usize - 1);
        let same = self.names.len() == entries.len()
            && self.names.iter().zip(&entries).all(|(a, (b, _))| a == b);
        if same {
            for (slot, (_, v)) in self.variants.iter_mut().zip(entries) {
                *slot = v;
            }
            return false;
        }
        (self.names, self.variants) = entries.into_iter().unzip();
        true
    }

    /// The id of style `name`, if any entity asks for it.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.names
            .binary_search_by(|n| n.as_str().cmp(name))
            .ok()
            .map(|i| (i + 1) as u16)
    }

    /// The style id `id` stands for.
    pub fn name(&self, id: u16) -> Option<&str> {
        (id as usize)
            .checked_sub(1)
            .and_then(|i| self.names.get(i))
            .map(String::as_str)
    }

    /// `(id, name)` for every style, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(i, n)| ((i + 1) as u16, n.as_str()))
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Make the table hold exactly `names` (any order, repeats allowed).
    /// Allocates only when the set changed. Returns whether it did.
    pub fn sync(&mut self, names: &mut Vec<&str>) -> bool {
        self.set(
            names
                .iter()
                .map(|n| (n.to_string(), StyleVariant::plain(n)))
                .collect(),
        )
    }
}

/// The blend a styled root asks for (#278): its style component's `blend`,
/// else its `style_mask`'s, else 1. A number outside 0..1 is clamped.
fn requested_blend(world: &FlintWorld, root: EntityId) -> f32 {
    let from = |c: &str| {
        world
            .get_component(root, c)?
            .get(BLEND_FIELD)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
    };
    crate::stylize::registry()
        .iter()
        .filter_map(|d| d.component)
        .chain([STYLE_MASK_COMPONENT])
        .find_map(from)
        .map_or(1.0, |b| (b as f32).clamp(0.0, 1.0))
}

/// What one entity's `style_mask` component asks for: `Some(name)` for a
/// style, `None` to opt out (`enabled = false`, or an empty name).
fn requested_style(world: &FlintWorld, id: EntityId) -> Option<&str> {
    let c = world.get_component(id, STYLE_MASK_COMPONENT)?;
    if c.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let name = c.get("style").and_then(|v| v.as_str()).unwrap_or("default");
    (!name.is_empty()).then_some(name)
}

/// Read a style component's fields into `variant`: the descriptor's
/// options as strings, its parameters as numbers, and a 3-array `x` as the
/// parameters `x_r`, `x_g`, `x_b`. Anything else (`enabled`, a key the
/// style doesn't take, a value of the wrong type) is skipped: the scene
/// loader has already warned about it against the component's schema.
pub fn read_overrides(d: &StyleDescriptor, table: &toml::Table, variant: &mut StyleVariant) {
    let number = |v: &toml::Value| {
        v.as_float()
            .or_else(|| v.as_integer().map(|i| i as f64))
            .map(|n| n as f32)
    };
    for (key, value) in table {
        if d.options.iter().any(|o| o.name == key) {
            if let Some(text) = value.as_str() {
                variant.options.insert(key.clone(), text.to_string());
            }
        } else if d.params.iter().any(|p| p.name == key) {
            if let Some(n) = number(value) {
                variant.params.insert(key.clone(), n);
            }
        } else if let Some([r, g, b]) = value.as_array().map(Vec::as_slice) {
            let channels = ["r", "g", "b"].map(|c| format!("{key}_{c}"));
            if channels
                .iter()
                .all(|c| d.params.iter().any(|p| p.name == c))
            {
                for (c, v) in channels.into_iter().zip([r, g, b]) {
                    if let Some(n) = number(v) {
                        variant.params.insert(c, n);
                    }
                }
            }
        }
    }
}

/// The name a variant goes by: its style, `@root` when it is anchored to
/// one, then its settings in key order (`{off}` for an opt-out). Equal
/// settings give equal names, so their entities share an id.
fn variant_name(v: &StyleVariant, root: &str) -> String {
    let mut name = v.style.clone();
    if v.anchor.is_some() {
        name.push('@');
        name.push_str(root);
    }
    let mut settings: Vec<String> = Vec::new();
    if v.off {
        settings.push("off".into());
    } else {
        // Both maps are sorted; merge them so the spelling is canonical
        let mut all: Vec<(&str, String)> = v
            .params
            .iter()
            .map(|(k, n)| (k.as_str(), format!("{k}={n}")))
            .chain(
                v.options
                    .iter()
                    .map(|(k, s)| (k.as_str(), format!("{k}={s}"))),
            )
            .collect();
        all.sort_by(|a, b| a.0.cmp(b.0));
        settings.extend(all.into_iter().map(|(_, s)| s));
    }
    if !settings.is_empty() {
        name.push('{');
        name.push_str(&settings.join(","));
        name.push('}');
    }
    name
}

/// What a styled root asks for: its variant's name and the variant, or
/// `None` for a `style_mask` opt-out. A style component (in registry
/// order) wins over a `style_mask` on the same entity.
fn request(
    world: &FlintWorld,
    config: &PostProcessConfig,
    root: EntityId,
) -> Option<(String, StyleVariant)> {
    let from_component = crate::stylize::registry().iter().find_map(|d| {
        let table = world.get_component(root, d.component?)?.as_table()?;
        let mut v = StyleVariant::plain(d.name);
        if table.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
            v.off = true;
        } else {
            read_overrides(d, table, &mut v);
        }
        Some((*d, v))
    });
    let (descriptor, mut variant) = match from_component {
        Some((d, v)) => (Some(d), v),
        None => {
            let style = requested_style(world, root)?;
            (crate::stylize::find(style), StyleVariant::plain(style))
        }
    };
    // An object-anchored grid: one variant per root, pinned to its origin
    if let Some(d) = descriptor.filter(|d| d.component.is_some() && !variant.off) {
        if variant.option(d, config, "grid") == "object" {
            let p = world.get_world_position(root).unwrap_or_default();
            variant.anchor = Some([p.x, p.y, p.z]);
        }
    }
    let name = variant_name(&variant, world.get_name(root).unwrap_or(""));
    Some((name, variant))
}

/// Whether any entity carries a `style_mask` or a style's own component.
pub(crate) fn any_styled(world: &FlintWorld) -> bool {
    !world
        .entities_with_component(STYLE_MASK_COMPONENT)
        .is_empty()
        || crate::stylize::registry()
            .iter()
            .filter_map(|d| d.component)
            .any(|c| !world.entities_with_component(c).is_empty())
}

/// Find every styled entity's id: those with a `style_mask` or a style's
/// own component (`ascii_style`), and their descendants, the nearest
/// winning. Fills `out` (cleared first) with the non-zero ids only and sets
/// `ids` to the variants used. `config` supplies the scene's settings a
/// variant falls back on (its grid).
#[cfg(test)]
pub(crate) fn resolve(
    world: &FlintWorld,
    config: &PostProcessConfig,
    ids: &mut StyleIds,
    out: &mut HashMap<EntityId, u16>,
) {
    resolve_blended(world, config, ids, out, &mut HashMap::new());
}

/// [`resolve`], and the blend of every styled entity drawn at less than 1
/// into `blends` (cleared first; #278). The blend is the root's, carried
/// down its subtree with its id. It isn't part of the variant: entities
/// fading at different rates still share one id, so a ramp makes no new
/// variant a frame.
pub(crate) fn resolve_blended(
    world: &FlintWorld,
    config: &PostProcessConfig,
    ids: &mut StyleIds,
    out: &mut HashMap<EntityId, u16>,
    blends: &mut HashMap<EntityId, f32>,
) {
    out.clear();
    blends.clear();
    if !any_styled(world) {
        ids.set(Vec::new());
        return;
    }
    let mut roots: HashSet<EntityId> = world.entities_with_component(STYLE_MASK_COMPONENT).clone();
    for c in crate::stylize::registry()
        .iter()
        .filter_map(|d| d.component)
    {
        roots.extend(world.entities_with_component(c));
    }
    let requests: Vec<(EntityId, Option<(String, StyleVariant)>)> = roots
        .iter()
        .map(|&e| (e, request(world, config, e)))
        .collect();
    ids.set(requests.iter().filter_map(|(_, r)| r.clone()).collect());
    let mut stack = Vec::new();
    for (root, req) in &requests {
        let root = *root;
        let Some(id) = req.as_ref().and_then(|(n, _)| ids.id(n)) else {
            continue; // an opt-out: its subtree stays unstyled
        };
        let blend = requested_blend(world, root);
        stack.clear();
        stack.push(root);
        while let Some(e) = stack.pop() {
            out.insert(e, id);
            if blend < 1.0 {
                blends.insert(e, blend);
            }
            // A descendant with a style_mask of its own is a root itself
            stack.extend(
                world
                    .get_children(e)
                    .into_iter()
                    .filter(|c| !roots.contains(c)),
            );
        }
    }
}

/// The debug view's colour for style `id`: hues a golden angle apart, so
/// neighbouring ids never look alike. `NO_STYLE` is black.
pub fn debug_color(id: u16) -> [f32; 3] {
    if id == NO_STYLE {
        return [0.0; 3];
    }
    let h = (id as f32 * 0.618_034).fract() * 6.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    // Lifted toward white a little so dark hues stay visible over a scene
    [0.25 + 0.75 * r, 0.25 + 0.75 * g, 0.25 + 0.75 * b]
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaskDrawUniform {
    id: u32,
    slope_tolerance: f32,
    /// The draw's blend in the mask's `.g` encoding (#278).
    blend: u32,
    _pad: f32,
}

/// One masked static (or procedural box) draw.
pub(crate) struct MaskDraw<'a> {
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
    pub index_count: u32,
    pub transform_bind_group: &'a wgpu::BindGroup,
    /// The bone matrices, for a skinned draw.
    pub bones: Option<&'a wgpu::BindGroup>,
    pub id: u16,
    /// How much of the style the entity is drawn with, 0..1 (#278).
    pub blend: f32,
}

/// One transparent mesh in front of whatever it covers, for the veil
/// (#290): its own cached transform and material bind groups.
pub(crate) struct VeilMesh<'a> {
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
    pub index_count: u32,
    pub transform_bind_group: &'a wgpu::BindGroup,
    pub material_bind_group: &'a wgpu::BindGroup,
    /// The bone matrices, for a skinned draw.
    pub bones: Option<&'a wgpu::BindGroup>,
}

/// The frame's particles, for the veil: the particle pipeline's shared
/// bind groups and its sorted draws.
pub(crate) struct VeilParticles<'a> {
    pub pipeline: &'a crate::particle_pipeline::ParticlePipeline,
    pub draws: &'a [crate::particle_pipeline::ParticleDrawCall],
}

/// What the veil pass draws this frame.
pub(crate) struct Veil<'a> {
    /// The light group, whose `material_sampler` the PBR veil samples with.
    pub light_bind_group: &'a wgpu::BindGroup,
    pub meshes: Vec<VeilMesh<'a>>,
    pub particles: Option<VeilParticles<'a>>,
}

impl Veil<'_> {
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty() && self.particles.as_ref().is_none_or(|p| p.draws.is_empty())
    }
}

/// The veil's particle pipelines: alpha-like modes and additive.
struct ParticleVeil {
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
}

/// A veil pipeline: `module`'s `vs`/`fs`, both faces, single-sample,
/// depth-tested against the scene and never writing it, its coverage
/// "over" what's there (red only).
fn veil_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    (vs, fs): (&str, &str),
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            buffers,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format: VEIL_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::RED,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// The bind group layouts the mask and the veil share with the scene's
/// pipelines.
pub(crate) struct MaskLayouts<'a> {
    pub transform: &'a wgpu::BindGroupLayout,
    pub material: &'a wgpu::BindGroupLayout,
    pub light: &'a wgpu::BindGroupLayout,
    pub bones: &'a wgpu::BindGroupLayout,
}

/// The mask's pipelines and its per-draw uniform buffer, made the first
/// time a frame needs the mask.
pub struct StyleMaskPass {
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
    veil_static: wgpu::RenderPipeline,
    veil_skinned: wgpu::RenderPipeline,
    /// Made the first frame with particles (the particle pipeline is too).
    veil_particles: Option<ParticleVeil>,
    draw_bgl: wgpu::BindGroupLayout,
    /// One [`DRAW_STRIDE`] slot a draw, [`draw_slots`](Self::draw_slots)
    /// of them; replaced by a bigger one when a frame needs more.
    draw_buffer: wgpu::Buffer,
    draw_slots: usize,
    /// The [`gpu_id`](crate::bind_cache::gpu_id) of `draw_buffer`, in the
    /// cached bind group's key so a grown buffer gets a bind group of its
    /// own (the old one goes with the chain's cache on the next resize).
    draw_buffer_id: u64,
    /// The uniform bytes, kept so a frame reuses the allocation.
    scratch: Vec<u8>,
    /// Styled draws the last frame masked.
    last_draws: usize,
}

fn mask_draw_buffer(device: &wgpu::Device, slots: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Style Mask Draws"),
        size: DRAW_STRIDE * slots as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl StyleMaskPass {
    pub(crate) fn new(device: &wgpu::Device, layouts: &MaskLayouts<'_>) -> Self {
        let (transform_bgl, bone_bgl) = (layouts.transform, layouts.bones);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Mask Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("style_mask.wgsl").into()),
        });
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Mask Draw BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<MaskDrawUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline = |label: &str,
                        entry: &str,
                        layouts: &[&wgpu::BindGroupLayout],
                        buffer: wgpu::VertexBufferLayout<'static>| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    buffers: &[buffer],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: STYLE_MASK_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                // Both faces, as the scene's opaque pipelines draw them
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let static_pipeline = pipeline(
            "Style Mask Pipeline",
            "vs_static",
            &[transform_bgl, &draw_bgl],
            crate::primitives::Vertex::desc(),
        );
        let skinned_pipeline = pipeline(
            "Style Mask Skinned Pipeline",
            "vs_skinned",
            &[transform_bgl, &draw_bgl, bone_bgl],
            crate::primitives::SkinnedVertex::desc(),
        );
        let pbr = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let skinned = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil Skinned Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("skinned_shader.wgsl").into()),
        });
        let veil_static = veil_pipeline(
            device,
            "Style Veil Pipeline",
            &pbr,
            ("vs_main", "fs_veil"),
            &[transform_bgl, layouts.material, layouts.light],
            &[crate::primitives::Vertex::desc()],
        );
        let veil_skinned = veil_pipeline(
            device,
            "Style Veil Skinned Pipeline",
            &skinned,
            ("vs_skinned", "fs_veil"),
            &[transform_bgl, layouts.material, layouts.light, bone_bgl],
            &[crate::primitives::SkinnedVertex::desc()],
        );
        let draw_buffer = mask_draw_buffer(device, INITIAL_MASK_SLOTS);
        Self {
            static_pipeline,
            skinned_pipeline,
            veil_static,
            veil_skinned,
            veil_particles: None,
            draw_bgl,
            draw_buffer,
            draw_slots: INITIAL_MASK_SLOTS,
            draw_buffer_id: crate::bind_cache::gpu_id(),
            scratch: Vec::new(),
            last_draws: 0,
        }
    }

    /// The bytes of the per-draw uniform buffer.
    pub fn draw_buffer_bytes(&self) -> u64 {
        self.draw_buffer.size()
    }

    /// How many styled draws the last frame masked.
    pub fn last_draws(&self) -> usize {
        self.last_draws
    }

    /// The bind group of the per-draw uniforms and `scene_depth`: kept in
    /// `binds` while the chain owns the depth, else made for this frame
    /// (once a frame, never per draw).
    fn draw_bind(
        &self,
        device: &wgpu::Device,
        binds: &crate::bind_cache::BindCache,
        scene_depth: &wgpu::TextureView,
        veil: &wgpu::TextureView,
        depth_owned: bool,
    ) -> std::sync::Arc<wgpu::BindGroup> {
        binds.get_if(
            depth_owned,
            "Style Mask Draw BG",
            self.draw_buffer_id,
            || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Style Mask Draw BG"),
                    layout: &self.draw_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.draw_buffer,
                                offset: 0,
                                size: wgpu::BufferSize::new(
                                    std::mem::size_of::<MaskDrawUniform>() as u64
                                ),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(scene_depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(veil),
                        },
                    ],
                })
            },
        )
    }

    /// Clear `veil_view` and draw what's in front into it (#290): each
    /// transparent mesh and particle, depth-tested against `scene_depth`.
    fn encode_veil(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        veil_view: &wgpu::TextureView,
        scene_depth: &wgpu::TextureView,
        veil: &Veil<'_>,
    ) {
        if let Some(pp) = veil
            .particles
            .as_ref()
            .filter(|p| !p.draws.is_empty() && self.veil_particles.is_none())
            .map(|p| p.pipeline)
        {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Style Veil Particle Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("particle_shader.wgsl").into()),
            });
            let layouts = [
                &pp.uniform_bind_group_layout,
                &pp.instance_bind_group_layout,
                &pp.texture_bind_group_layout,
            ];
            let make = |label, fs| {
                veil_pipeline(device, label, &shader, ("vs_particle", fs), &layouts, &[])
            };
            self.veil_particles = Some(ParticleVeil {
                alpha: make("Style Veil Particle Pipeline", "fs_veil"),
                additive: make("Style Veil Additive Particle Pipeline", "fs_veil_additive"),
            });
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Veil Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: veil_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            // Read-only: the scene's depth, which nothing here writes
            depth_stencil_attachment: (!veil.is_empty()).then_some(
                wgpu::RenderPassDepthStencilAttachment {
                    view: scene_depth,
                    depth_ops: None,
                    stencil_ops: None,
                },
            ),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if veil.is_empty() {
            return;
        }
        let mut skinned = None;
        for m in &veil.meshes {
            let is_skinned = m.bones.is_some();
            if skinned != Some(is_skinned) {
                pass.set_pipeline(if is_skinned {
                    &self.veil_skinned
                } else {
                    &self.veil_static
                });
                pass.set_bind_group(2, veil.light_bind_group, &[]);
                skinned = Some(is_skinned);
            }
            pass.set_bind_group(0, m.transform_bind_group, &[]);
            pass.set_bind_group(1, m.material_bind_group, &[]);
            if let Some(bones) = m.bones {
                pass.set_bind_group(3, bones, &[]);
            }
            pass.set_vertex_buffer(0, m.vertex_buffer.slice(..));
            pass.set_index_buffer(m.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
        if let (Some(particles), Some(pipelines)) = (&veil.particles, &self.veil_particles) {
            let pp = particles.pipeline;
            pass.set_index_buffer(pp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_bind_group(0, &pp.uniform_bind_group, &[]);
            pass.set_bind_group(1, pp.instance_bind_group(), &[]);
            let mut additive = None;
            for d in particles.draws {
                let is_additive = d.blend == flint_particles::ParticleBlendMode::Additive;
                if additive != Some(is_additive) {
                    pass.set_pipeline(if is_additive {
                        &pipelines.additive
                    } else {
                        &pipelines.alpha
                    });
                    additive = Some(is_additive);
                }
                pass.set_bind_group(2, d.texture_bind_group.as_ref(), &[]);
                let first = d.first_instance;
                pass.draw_indexed(0..6, 0, first..first + d.instance_count);
            }
        }
    }

    /// Draw the veil into `veil_view`, then clear `mask` and draw `draws`
    /// into it, each tested against `scene_depth` (single-sample, the
    /// mask's size) and its blend scaled by what the veil leaves uncovered.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        binds: &crate::bind_cache::BindCache,
        mask: &wgpu::TextureView,
        veil_view: &wgpu::TextureView,
        scene_depth: &wgpu::TextureView,
        depth_owned: bool,
        draws: &[MaskDraw<'_>],
        veil: &Veil<'_>,
    ) {
        // Only a styled pixel reads the veil: with none, it isn't drawn
        if !draws.is_empty() {
            self.encode_veil(device, encoder, veil_view, scene_depth, veil);
        }
        let slots = mask_slots_for(self.draw_slots, draws.len());
        if slots != self.draw_slots {
            self.draw_buffer = mask_draw_buffer(device, slots);
            self.draw_slots = slots;
            self.draw_buffer_id = crate::bind_cache::gpu_id();
        }
        self.last_draws = draws.len();
        self.scratch.clear();
        self.scratch.resize(draws.len() * DRAW_STRIDE as usize, 0);
        for (i, d) in draws.iter().enumerate() {
            let at = i * DRAW_STRIDE as usize;
            let uniform = MaskDrawUniform {
                id: u32::from(d.id),
                slope_tolerance: SLOPE_TOLERANCE,
                blend: encode_blend(d.blend),
                _pad: 0.0,
            };
            self.scratch[at..at + std::mem::size_of::<MaskDrawUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
        }
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &self.scratch);
        }
        let draw_bg = (!draws.is_empty())
            .then(|| self.draw_bind(device, binds, scene_depth, veil_view, depth_owned));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Style Mask Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: mask,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let Some(draw_bg) = draw_bg else { return };
        let mut skinned = None;
        for (i, d) in draws.iter().enumerate() {
            let is_skinned = d.bones.is_some();
            if skinned != Some(is_skinned) {
                pass.set_pipeline(if is_skinned {
                    &self.skinned_pipeline
                } else {
                    &self.static_pipeline
                });
                skinned = Some(is_skinned);
            }
            pass.set_bind_group(0, d.transform_bind_group, &[]);
            pass.set_bind_group(1, &*draw_bg, &[(i as u64 * DRAW_STRIDE) as u32]);
            if let Some(bones) = d.bones {
                pass.set_bind_group(2, bones, &[]);
            }
            pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
            pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..d.index_count, 0, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_name_order_and_survive_repeats() {
        let mut ids = StyleIds::default();
        assert!(ids.sync(&mut vec!["pencil", "ascii", "pencil"]));
        assert_eq!(ids.id("ascii"), Some(1));
        assert_eq!(ids.id("pencil"), Some(2));
        assert_eq!(ids.id("watercolor"), None);
        assert_eq!(ids.name(2), Some("pencil"));
        assert_eq!(ids.name(NO_STYLE), None);
        // The same set in another order changes nothing
        assert!(!ids.sync(&mut vec!["ascii", "pencil"]));
        assert!(ids.sync(&mut vec!["ascii"]));
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn uniform_fits_its_stride() {
        assert_eq!(std::mem::size_of::<MaskDrawUniform>(), 16);
        assert!(std::mem::size_of::<MaskDrawUniform>() as u64 <= DRAW_STRIDE);
    }

    #[test]
    fn the_draw_buffer_grows_by_doubling_and_never_shrinks() {
        let start = INITIAL_MASK_SLOTS;
        assert!(start as u64 * DRAW_STRIDE <= 4096, "the start is a few KB");
        assert_eq!(mask_slots_for(start, 0), start);
        assert_eq!(mask_slots_for(start, 8), start);
        assert_eq!(mask_slots_for(start, start), start);
        assert_eq!(mask_slots_for(start, start + 1), 2 * start);
        assert_eq!(mask_slots_for(start, 5000), 8192);
        // A quieter frame keeps the bigger buffer (no churn back and forth)
        assert_eq!(mask_slots_for(8192, 3), 8192);
    }

    #[test]
    fn debug_colors_are_distinct_and_visible() {
        assert_eq!(debug_color(NO_STYLE), [0.0; 3]);
        for a in 1..16u16 {
            let ca = debug_color(a);
            assert!(ca.iter().all(|c| (0.25..=1.0).contains(c)));
            let cb = debug_color(a + 1);
            let d: f32 = ca.iter().zip(cb).map(|(x, y)| (x - y).abs()).sum();
            assert!(d > 0.3, "{a} and {} look alike", a + 1);
        }
    }

    fn world_with(entities: &[(&str, Option<&str>, Option<&str>)]) -> FlintWorld {
        let mut world = FlintWorld::new();
        for (name, _, style) in entities {
            let id = world.spawn(*name).unwrap();
            if let Some(style) = style {
                let mut t = toml::Table::new();
                if *style == "-" {
                    t.insert("enabled".into(), false.into());
                } else {
                    t.insert("style".into(), (*style).into());
                }
                world
                    .set_component(id, STYLE_MASK_COMPONENT, toml::Value::Table(t))
                    .unwrap();
            }
        }
        for (name, parent, _) in entities {
            if let Some(parent) = parent {
                world.set_parent_by_name(name, parent).unwrap();
            }
        }
        world
    }

    #[test]
    fn descendants_inherit_until_a_nearer_mask() {
        let world = world_with(&[
            ("cart", None, Some("ascii")),
            ("wheel", Some("cart"), None),
            ("hub", Some("wheel"), Some("pencil")),
            ("bolt", Some("hub"), None),
            ("flag", Some("cart"), Some("-")),
            ("cloth", Some("flag"), None),
            ("tree", None, None),
        ]);
        let mut ids = StyleIds::default();
        let mut out = HashMap::new();
        resolve(&world, &PostProcessConfig::default(), &mut ids, &mut out);
        let get = |n: &str| out.get(&world.get_id(n).unwrap()).copied();
        let ascii = ids.id("ascii");
        let pencil = ids.id("pencil");
        assert_eq!(get("cart"), ascii);
        assert_eq!(get("wheel"), ascii);
        assert_eq!(get("hub"), pencil);
        assert_eq!(get("bolt"), pencil);
        assert_eq!(get("flag"), None);
        assert_eq!(get("cloth"), None);
        assert_eq!(get("tree"), None);
    }

    fn with_component(world: &mut FlintWorld, name: &str, component: &str, toml_src: &str) {
        let id = world
            .get_id(name)
            .unwrap_or_else(|| world.spawn(name).unwrap());
        let table: toml::Table = toml::from_str(toml_src).unwrap();
        world
            .set_component(id, component, toml::Value::Table(table))
            .unwrap();
    }

    fn resolved(
        world: &FlintWorld,
        config: &PostProcessConfig,
    ) -> (StyleIds, HashMap<EntityId, u16>) {
        let mut ids = StyleIds::default();
        let mut out = HashMap::new();
        resolve(world, config, &mut ids, &mut out);
        (ids, out)
    }

    #[test]
    fn equal_settings_share_an_id_and_others_get_their_own() {
        let mut world = FlintWorld::new();
        with_component(
            &mut world,
            "barrel",
            "ascii_style",
            "ramp = \"blocks\"\ncell = 12",
        );
        with_component(
            &mut world,
            "crate",
            "ascii_style",
            "cell = 12.0\nramp = \"blocks\"",
        );
        with_component(&mut world, "lamp", "ascii_style", "tint = [1.0, 0.5, 0]");
        with_component(&mut world, "bench", "ascii_style", "");
        with_component(&mut world, "stool", "style_mask", "style = \"ascii\"");
        // The schema's default for `enabled` rides along and changes nothing
        with_component(&mut world, "table", "ascii_style", "enabled = true");
        let (ids, out) = resolved(&world, &PostProcessConfig::default());
        let get = |n: &str| out[&world.get_id(n).unwrap()];
        assert_eq!(get("barrel"), get("crate"), "integer 12 and 12.0 agree");
        assert_ne!(get("barrel"), get("lamp"));
        assert_eq!(
            get("bench"),
            get("stool"),
            "an empty ascii_style is a plain style_mask"
        );
        assert_eq!(get("bench"), get("table"));
        assert_eq!(ids.name(get("bench")), Some("ascii"));
        assert_eq!(ids.name(get("barrel")), Some("ascii{cell=12,ramp=blocks}"));

        let lamp = ids.variant(get("lamp")).unwrap();
        assert_eq!(lamp.style, "ascii");
        assert_eq!(
            lamp.params.get("tint_g"),
            Some(&0.5),
            "a 3-array fills tint_r/g/b"
        );
        assert_eq!(lamp.params.get("tint_b"), Some(&0.0));
        assert!(ids.requests("ascii"));
        assert_eq!(ids.variants_of("ascii").count(), 3);
    }

    #[test]
    fn an_opt_out_is_a_variant_that_asks_for_nothing() {
        let mut world = FlintWorld::new();
        with_component(
            &mut world,
            "cart",
            "ascii_style",
            "enabled = false\ncell = 4",
        );
        let (ids, out) = resolved(&world, &PostProcessConfig::default());
        let id = out[&world.get_id("cart").unwrap()];
        assert_eq!(
            ids.name(id),
            Some("ascii{off}"),
            "an opt-out's settings don't matter"
        );
        assert!(ids.variant(id).unwrap().off);
        assert!(
            !ids.requests("ascii"),
            "an opt-out alone doesn't switch the style on"
        );
        assert!(ids.mentions("ascii"));
    }

    #[test]
    fn an_entity_falls_back_on_the_scenes_settings_then_the_defaults() {
        let ascii = crate::stylize::find("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_param("ascii", "cell", 10.0);
        config.styles.set_option("ascii", "color", "tint");
        let mut v = StyleVariant::plain("ascii");
        v.params.insert("cell".into(), 16.0);
        assert_eq!(v.param(ascii, &config, "cell"), 16.0);
        assert_eq!(v.option(ascii, &config, "color"), "tint");
        assert_eq!(v.param(ascii, &config, "ink"), 1.0);
    }

    #[test]
    fn an_object_grid_anchors_each_root_and_its_children_ride_along() {
        let mut world = FlintWorld::new();
        with_component(&mut world, "cart", "ascii_style", "grid = \"object\"");
        with_component(
            &mut world,
            "cart",
            "transform",
            "position = [1.0, 2.0, 3.0]",
        );
        with_component(&mut world, "wagon", "ascii_style", "grid = \"object\"");
        let wheel = world.spawn("wheel").unwrap();
        world.set_parent_by_name("wheel", "cart").unwrap();
        let (ids, out) = resolved(&world, &PostProcessConfig::default());
        let cart = out[&world.get_id("cart").unwrap()];
        assert_eq!(out[&wheel], cart, "a child shares its root's grid");
        assert_ne!(
            cart,
            out[&world.get_id("wagon").unwrap()],
            "one grid per root"
        );
        assert_eq!(ids.name(cart), Some("ascii@cart{grid=object}"));
        assert_eq!(ids.variant(cart).unwrap().anchor, Some([1.0, 2.0, 3.0]));

        // The scene's table can ask for object grids for everyone
        let mut world = FlintWorld::new();
        with_component(&mut world, "bench", "style_mask", "style = \"ascii\"");
        let mut config = PostProcessConfig::default();
        config.styles.set_option("ascii", "grid", "object");
        let (ids, out) = resolved(&world, &config);
        let bench = out[&world.get_id("bench").unwrap()];
        assert_eq!(ids.name(bench), Some("ascii@bench"));
    }

    #[test]
    fn a_style_component_beats_a_style_mask_on_the_same_entity() {
        let mut world = FlintWorld::new();
        with_component(&mut world, "barrel", "style_mask", "style = \"pencil\"");
        with_component(&mut world, "barrel", "ascii_style", "ramp = \"blocks\"");
        let (ids, out) = resolved(&world, &PostProcessConfig::default());
        let id = out[&world.get_id("barrel").unwrap()];
        assert_eq!(ids.variant(id).unwrap().style, "ascii");
        assert!(!ids.mentions("pencil"));
    }

    #[test]
    fn entities_switch_a_per_entity_style_on_without_a_table() {
        let ascii = crate::stylize::find("ascii").unwrap();
        let invert = crate::stylize::find("invert").unwrap();
        let config = PostProcessConfig::default();
        let mut world = FlintWorld::new();
        with_component(&mut world, "bench", "style_mask", "style = \"ascii\"");
        with_component(&mut world, "crate", "style_mask", "style = \"invert\"");
        let (ids, _) = resolved(&world, &config);
        assert!(ascii.is_on(&config, &ids));
        assert!(ascii.reads_mask(&config, &ids));
        assert!(!invert.is_on(&config, &ids), "invert isn't per-entity");
        assert!(crate::stylize::any_uses_mask(&config, &ids));
        // Nobody asking: off, and the mask isn't drawn
        let (none, _) = resolved(&FlintWorld::new(), &config);
        assert!(!ascii.is_on(&config, &none));
        assert!(!crate::stylize::any_uses_mask(&config, &none));
    }
}
