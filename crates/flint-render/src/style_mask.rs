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
//! the mask works at any render scale and the scene pipelines don't
//! change.
//!
//! Under MSAA (#306) the mask is drawn 4x instead, into a multisampled
//! target of its own, each covered sample testing itself against that
//! sample of the scene's multisampled depth, and a resolve folds it into
//! the single-sample mask with any styled sample beating the empty ones.
//! Drawn at pixel centres against the sample-0 depth, a feature about a
//! pixel thin (a bench seat's edge at a grazing angle) lost the pixels
//! the scene's samples gave it, and an outline style drew the gaps.
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
//! skinned), every living prop's echo (#737: front faces only, as the scene
//! draws them) and every particle into an `R8Unorm` target, depth-tested
//! against the scene with no depth write, each with the alpha it is drawn
//! with (an additive particle by its alpha times its brightest channel),
//! "over" blended so layers add up to their combined coverage. The mask
//! then writes each styled pixel's blend times what the veil leaves
//! uncovered. The rule: **what's in front unmasks by its coverage.** A
//! style that honours the blend (ASCII, user styles through
//! `flint_style_blend`) shows the fire as fire and the stone through any
//! uncovered share; a fully covered pixel keeps its id at blend 0. A
//! transparent that is itself styled is left out of the veil: it is in
//! the mask itself, as a lens (below).
//!
//! ## Terrain and transparents (#310)
//!
//! Terrain chunks are masked when the terrain's entity (named by
//! `SceneRenderer::set_terrain_entity`) is styled: they use the standard
//! `Vertex` and transform layout, so the static pipeline draws them as is.
//!
//! A styled transparent is a lens. Its draws come after every opaque one,
//! back to front as the scene drew them; they write no depth, but each
//! fragment still tests against the opaques' depth, so where one shows it
//! is the nearest styled surface and its id replaces what's behind it. The
//! style then draws the whole pixel, glass and what's seen through it.
//!
//! Particles, sprites, grass and water stay unmasked (particles and
//! unstyled transparents only veil): grass is instanced, and the others
//! write no depth. A sprite (billboard) needs no veil either: its alpha cuts
//! it out and what's kept writes depth, so the mask's depth test leaves it
//! out of whatever styled entity it stands in front of.
//!
//! ## Shallow wooze (#737)
//!
//! A `shallow_wooze` mesh's vertices drift in the main and shadow passes'
//! own vertex stages (#215). The mask redraws it through
//! `style_mask_wooze.wgsl`'s stages, in a second module
//! ([`woozy_mask_source`]) built the first frame a styled draw drifts, so
//! its id covers where it was drawn; at its rest pose it had holes where
//! the mesh had moved off and spill where it had moved from.
//!
//! The veil does the same for a drifting translucent in front (#1241): it
//! pairs `wooze_vertex.wgsl`'s stages, the main pass's own, with `fs_veil`,
//! built the first frame one is veiled
//! ([`ensure_veil_wooze`](StyleMaskPass::ensure_veil_wooze)), so a woozy
//! pane hands back the glyphs behind where it is drawn, not where it was.
//!
//! [`StyleDescriptor::uses_mask`]: crate::stylize::StyleDescriptor::uses_mask
//! [`StyleDescriptor::component`]: crate::stylize::StyleDescriptor::component

use crate::gpu_counts::{CountedDevice, CountedQueue};
use crate::postprocess::PostProcessConfig;
use crate::stylize::{StyleDescriptor, StyleParam, StyleRegistry};
use crate::TrustedShader;
use bytemuck::{Pod, Zeroable};
use flint_core::style_field::{FieldShape, StyleField, MAX_FIELD_SHAPES};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, BTreeSet, HashMap};

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

/// The field that says what an entity's blend follows (#397): `"own"` (the
/// default), its `blend` alone; `"tear"`, its `blend` times the reality
/// tear's mask at each pixel, so the style shows only where render modes
/// 1-4 and 6 tear; or `"field"` (#859), its `blend` times the screen field
/// the game fills that frame (`flint_core::style_field`), so it shows only
/// under a shockwave's ring or a flashlight's cone. Like the blend, it is
/// the root's, inherited.
pub const BLEND_SOURCE_FIELD: &str = "blend_source";

/// [`BLEND_SOURCE_FIELD`]'s choices, in [`BlendSource`]'s order: the
/// entity's own blend, the tear, or the game's screen field.
pub const BLEND_SOURCES: [&str; 3] = ["own", "tear", "field"];

/// What a styled root's blend follows ([`BLEND_SOURCE_FIELD`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendSource {
    /// Its `blend` alone.
    #[default]
    Own,
    /// Its `blend` times the reality tear's mask (#397).
    Tear,
    /// Its `blend` times the frame's screen field (#859).
    Field,
}

impl BlendSource {
    /// Every source, in [`BLEND_SOURCES`]' order.
    pub const ALL: [BlendSource; 3] = [BlendSource::Own, BlendSource::Tear, BlendSource::Field];

    /// Its name, as a scene writes it.
    pub fn name(self) -> &'static str {
        BLEND_SOURCES[self as usize]
    }

    /// The source a scene's word names, if any.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// What the mask shader's `MaskDraw.source` holds for it.
    fn code(self) -> u32 {
        self as u32
    }
}

/// A blend in the mask's `.g` encoding.
pub fn encode_blend(blend: f32) -> u32 {
    (blend.clamp(0.0, 1.0) * BLEND_ONE as f32).round() as u32
}

/// The mask's shader: the tear's mask (#397), a mesh's vertex side in a mesh
/// block (`wgsl_sources::MASK_VERTEX`'s two files, #1244: the transform at
/// group 0, `world_of` / `clip_of`, `skin_transform`), then
/// `style_mask.wgsl`.
pub(crate) const STYLE_MASK_WGSL: &str = concat!(
    include_str!("tear_mask.wgsl"),
    "\n//FLINT_MESH {\n",
    include_str!("mesh_vertex.wgsl"),
    "\n",
    include_str!("skinning.wgsl"),
    "\n//FLINT_MESH }\n",
    include_str!("style_mask.wgsl")
);

/// The mask's shader for woozy draws (#737): the wooze noise, the mask
/// shader with its transform's drift field on, then the woozy vertex
/// stages. The plain module stays [`STYLE_MASK_WGSL`], so a frame with no
/// woozy styled draw compiles and runs exactly what it did.
pub(crate) fn woozy_mask_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("wooze_noise.wgsl"),
        crate::wgsl_sources::woozy(STYLE_MASK_WGSL),
        include_str!("style_mask_wooze.wgsl")
    )
}

/// The mask's shader for billboards (#1011): the mask shader without its
/// mesh blocks (the lines between `//FLINT_MESH {` and `//FLINT_MESH }`:
/// the transform, the bones and the mesh vertex stages, whose groups the
/// sprite's bind groups take), then the billboard's vertex stage and
/// fragments. The plain module stays [`STYLE_MASK_WGSL`], so a frame with
/// no styled billboard compiles and runs exactly what it did.
pub(crate) fn billboard_mask_source() -> String {
    let mut out = String::new();
    let mut mesh = false;
    for line in STYLE_MASK_WGSL.lines() {
        match line.trim() {
            "//FLINT_MESH {" => mesh = true,
            "//FLINT_MESH }" => mesh = false,
            _ if !mesh => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    out.push_str(include_str!("style_mask_billboard.wgsl"));
    out
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
/// rasterisers' small disagreements (and, under MSAA, the half pixel
/// between the centre's depth and a sample's) without letting a hidden
/// face through.
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
    /// The body a `grid = "local"` variant's strokes ride on (#544): the
    /// styled root and its world matrix this frame, as it is drawn.
    pub local: Option<LocalFrame>,
}

/// A styled root's own frame (#544): what a style laying marks on the
/// surfaces in the entity's space (pencil's `grid = "local"`) needs to
/// take a world position back into it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalFrame {
    /// The styled root, so a motion hold can give the matrix it draws.
    pub root: EntityId,
    /// Local to world, column-major (`m[col][row]`), as `get_world_matrix`.
    pub matrix: [[f32; 4]; 4],
}

impl LocalFrame {
    /// World to local as the top three rows of the inverse (`[r0, r1,
    /// r2]`, each `[x, y, z, w]`, so `local.i = dot(r_i, [p, 1])`), or
    /// `None` when the matrix is singular (a prop scaled to nothing).
    pub fn inverse_rows(&self) -> Option<[[f32; 4]; 3]> {
        let m = &self.matrix;
        // The 3x3 part's columns, and the translation
        let c = |i: usize| [m[i][0] as f64, m[i][1] as f64, m[i][2] as f64];
        let (c0, c1, c2) = (c(0), c(1), c(2));
        let t = [m[3][0] as f64, m[3][1] as f64, m[3][2] as f64];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        // The inverse's rows are the cross products of the other two
        // columns over the determinant
        let rows = [cross(c1, c2), cross(c2, c0), cross(c0, c1)];
        let det = dot(c0, rows[0]);
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        Some(rows.map(|r| {
            let r = r.map(|x| x / det);
            [r[0] as f32, r[1] as f32, r[2] as f32, -dot(r, t) as f32]
        }))
    }
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
    /// Beside `names`: whether every entity drawn in the id is at blend 0
    /// (#400). A faded id keeps its number and variant, so a ramp out of 0
    /// finds them where it left them, but asks for nothing: a style whose
    /// entities have all faded is off, its pass and the mask not drawn.
    faded: Vec<bool>,
    /// Beside `names` (#397, #859): for each [`BlendSource`], whether a
    /// root drawn in the id following it is above blend 0. A torn root
    /// shows only while a tear does ([`Self::set_tear`]), one following the
    /// field only while the frame has one ([`Self::set_field`]).
    shown: Vec<[bool; 3]>,
    /// Whether the frame tears ([`crate::postprocess::tear_active`]).
    tear_on: bool,
    /// Whether the frame's screen field holds any shape (#859).
    field_on: bool,
    /// Beside `names` (#740): the first styled root (by name) drawn in
    /// each id, the entity a cap names when it leaves the id out.
    entities: Vec<String>,
}

impl StyleIds {
    /// Pass each `grid = "local"` variant's frame through `held` (#544):
    /// the renderer gives the matrix a motion hold draws the root with, so
    /// strokes stay on a prop held on twos instead of sliding between its
    /// drawings. Names and ids don't change.
    pub(crate) fn hold_local_frames(
        &mut self,
        mut held: impl FnMut(EntityId, [[f32; 4]; 4]) -> [[f32; 4]; 4],
    ) {
        for f in self.variants.iter_mut().filter_map(|v| v.local.as_mut()) {
            f.matrix = held(f.root, f.matrix);
        }
    }

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

    /// Whether an entity asks to be drawn in `style`: an opt-out doesn't,
    /// nor does an id whose entities are all at blend 0 ([`Self::is_faded`]).
    pub fn requests(&self, style: &str) -> bool {
        self.variants
            .iter()
            .zip(&self.faded)
            .any(|(v, &faded)| v.style == style && !v.off && !faded)
    }

    /// Whether every entity drawn in id `id` is at blend 0 (#400).
    pub fn is_faded(&self, id: u16) -> bool {
        (id as usize)
            .checked_sub(1)
            .and_then(|i| self.faded.get(i))
            .copied()
            .unwrap_or(false)
    }

    /// Say which ids show: `shown(id)[source]` is whether a root following
    /// that [`BlendSource`] is above blend 0, for every id.
    fn set_shown(&mut self, shown: impl Fn(u16) -> [bool; 3]) {
        for (i, slot) in self.shown.iter_mut().enumerate() {
            *slot = shown((i + 1) as u16);
        }
        self.refade();
    }

    /// Whether the frame tears (#397): an id whose shown roots all follow
    /// the tear is faded while it doesn't, so with no tear a torn prop's
    /// style is off, pass and mask skipped, as at blend 0. Hosts call it
    /// each frame with the config the frame draws (a script's `set_tear`
    /// lands after the world is read). Returns whether it changed.
    pub fn set_tear(&mut self, on: bool) -> bool {
        if self.tear_on == on {
            return false;
        }
        self.tear_on = on;
        self.refade();
        true
    }

    /// Whether the frame's screen field holds a shape (#859): as
    /// [`Self::set_tear`] for the tear, an id whose shown roots all follow
    /// the field is faded while there is none. Hosts call it each frame
    /// with the field the frame draws. Returns whether it changed.
    pub fn set_field(&mut self, on: bool) -> bool {
        if self.field_on == on {
            return false;
        }
        self.field_on = on;
        self.refade();
        true
    }

    fn refade(&mut self) {
        for (faded, &[own, torn, field]) in self.faded.iter_mut().zip(&self.shown) {
            *faded = !(own || (torn && self.tear_on) || (field && self.field_on));
        }
    }

    /// Whether any entity names `style`, opt-outs included.
    pub fn mentions(&self, style: &str) -> bool {
        self.variants.iter().any(|v| v.style == style)
    }

    /// Make the table hold exactly `entries` (any order; a repeated name
    /// keeps its first variant). Returns whether the set of names changed;
    /// the variants (their anchors move) are taken either way, and no id
    /// is faded until [`resolve_blended`] says so.
    pub fn set(&mut self, mut entries: Vec<(String, StyleVariant)>) -> bool {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        // The mask holds an id in 16 bits, 0 for none
        let cap = u16::MAX as usize - 1;
        if let Some((name, _)) = entries.get(cap) {
            crate::stylize::variant_table::report_drop(
                "style mask",
                "style ids",
                cap,
                name,
                "drawn unstyled",
            );
        }
        entries.truncate(cap);
        let same = self.names.len() == entries.len()
            && self.names.iter().zip(&entries).all(|(a, (b, _))| a == b);
        if same {
            for (slot, (_, v)) in self.variants.iter_mut().zip(entries) {
                *slot = v;
            }
            self.faded.fill(false);
            self.shown.fill([true; 3]);
            return false;
        }
        (self.names, self.variants) = entries.into_iter().unzip();
        self.entities.clear();
        self.faded = vec![false; self.names.len()];
        self.shown = vec![[true; 3]; self.names.len()];
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

    /// The first styled root (by name) drawn in id `id`: the entity a
    /// style's cap names when it leaves the id out (#740). The id's own
    /// name when the entities aren't known (a table made with [`Self::set`]
    /// alone).
    pub fn entity(&self, id: u16) -> Option<&str> {
        let i = (id as usize).checked_sub(1)?;
        self.entities
            .get(i)
            .filter(|e| !e.is_empty())
            .or_else(|| self.names.get(i))
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

/// Where one style id's entities can be on screen this frame (#385).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IdBounds {
    /// None of its draws is in view.
    Nowhere,
    /// Inside `[x0, y0, x1, y1]` in UV (0..1, y down).
    Uv([f32; 4]),
    /// Anywhere: a draw with no bounds, or one crossing the camera plane.
    Everywhere,
}

impl IdBounds {
    /// The bounds holding both.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Everywhere, _) | (_, Self::Everywhere) => Self::Everywhere,
            (Self::Nowhere, b) | (b, Self::Nowhere) => b,
            (Self::Uv(a), Self::Uv(b)) => Self::Uv([
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]),
        }
    }

    /// Where world box `min..max` lands under `view_proj` (column-major,
    /// as [`Camera::view_projection_matrix`](crate::Camera::view_projection_matrix)):
    /// the projection of its 8 corners, clamped to the screen. A box
    /// wholly behind the camera is nowhere; one crossing the camera plane
    /// could be anywhere.
    pub fn of_box(view_proj: &[[f32; 4]; 4], min: [f32; 3], max: [f32; 3]) -> Self {
        let m = view_proj;
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        let mut behind = 0;
        for i in 0..8 {
            let p = [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ];
            let row = |r: usize| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
            let w = row(3);
            if !w.is_finite() {
                return Self::Everywhere;
            }
            if w <= 1e-6 {
                behind += 1;
                continue;
            }
            let uv = [row(0) / w * 0.5 + 0.5, 0.5 - row(1) / w * 0.5];
            for k in 0..2 {
                lo[k] = lo[k].min(uv[k]);
                hi[k] = hi[k].max(uv[k]);
            }
        }
        if behind == 8 {
            return Self::Nowhere;
        }
        if behind > 0 || !(lo[0].is_finite() && lo[1].is_finite()) {
            return Self::Everywhere;
        }
        if hi[0] < 0.0 || hi[1] < 0.0 || lo[0] > 1.0 || lo[1] > 1.0 {
            return Self::Nowhere;
        }
        Self::Uv([
            lo[0].clamp(0.0, 1.0),
            lo[1].clamp(0.0, 1.0),
            hi[0].clamp(0.0, 1.0),
            hi[1].clamp(0.0, 1.0),
        ])
    }
}

/// Each style id's screen bounds this frame (#385), from the world bounds
/// of the draws the mask draws for it. A table the renderer didn't fill
/// knows nothing, so every id in it is [`IdBounds::Everywhere`].
#[derive(Clone, Debug, Default)]
pub struct StyleBounds {
    known: bool,
    /// Index = style id.
    by_id: Vec<IdBounds>,
}

impl StyleBounds {
    /// An empty table that knows its ids are nowhere until told otherwise.
    pub fn known() -> Self {
        Self {
            known: true,
            by_id: Vec::new(),
        }
    }

    /// Widen id `id`'s bounds by `b`.
    pub fn add(&mut self, id: u16, b: IdBounds) {
        let i = id as usize;
        if self.by_id.len() <= i {
            self.by_id.resize(i + 1, IdBounds::Nowhere);
        }
        self.by_id[i] = self.by_id[i].union(b);
    }

    /// Where id `id`'s entities can be.
    pub fn get(&self, id: u16) -> IdBounds {
        if !self.known {
            return IdBounds::Everywhere;
        }
        self.by_id
            .get(id as usize)
            .copied()
            .unwrap_or(IdBounds::Nowhere)
    }
}

/// The blend a styled root asks for (#278): its style component's `blend`,
/// else its `style_mask`'s, else 1. A number outside 0..1 is clamped.
pub(crate) fn requested_blend(world: &FlintWorld, styles: &StyleRegistry, root: EntityId) -> f32 {
    let from = |c: &str| {
        world
            .get_component(root, c)?
            .get(BLEND_FIELD)
            .and_then(flint_core::toml_util::toml_f64)
    };
    styles
        .iter()
        .filter_map(|d| d.component)
        .chain([STYLE_MASK_COMPONENT])
        .find_map(from)
        .map_or(1.0, |b| (b as f32).clamp(0.0, 1.0))
}

/// What a styled root's blend follows (#397, #859): its style component's
/// `blend_source`, else its `style_mask`'s. A value that names no source
/// is its own blend (the schema warns about a wrong one).
pub(crate) fn blend_source(
    world: &FlintWorld,
    styles: &StyleRegistry,
    root: EntityId,
) -> BlendSource {
    let from = |c: &str| {
        world
            .get_component(root, c)?
            .get(BLEND_SOURCE_FIELD)
            .and_then(|v| v.as_str())
            .map(|s| BlendSource::from_name(s).unwrap_or_default())
    };
    styles
        .iter()
        .filter_map(|d| d.component)
        .chain([STYLE_MASK_COMPONENT])
        .find_map(from)
        .unwrap_or_default()
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
/// options as strings, its parameters as numbers, and a colour `x` (a
/// 3-array, [`StyleColor`](crate::stylize::StyleColor)) as the parameters
/// `x_r`, `x_g`, `x_b`. Anything else (`enabled`, a key the
/// style doesn't take, a value of the wrong type) is skipped, having been
/// said against the component's schema: a built-in's by the scene loader,
/// a user style's (which has no schema file) by `bind_scene` and `flint
/// check` against [`component_schema`](crate::stylize::component_schema)
/// (#556).
pub fn read_overrides(d: &StyleDescriptor, table: &toml::Table, variant: &mut StyleVariant) {
    let number = |v: &toml::Value| flint_core::toml_util::toml_f64(v).map(|n| n as f32);
    for (key, value) in table {
        if d.options.iter().any(|o| o.name == key) {
            if let Some(text) = value.as_str() {
                variant.options.insert(key.clone(), text.to_string());
            }
        } else if d.params.iter().any(|p| p.name == key) {
            if let Some(n) = number(value) {
                variant.params.insert(key.clone(), n);
            }
        } else if let Some(color) = d.color_setting(key) {
            if let Some(rgb) = crate::stylize::color_from_toml(value) {
                for (p, n) in color.channels.iter().zip(rgb) {
                    variant.params.insert(p.name.to_string(), n);
                }
            }
        }
    }
}

/// `blend`'s range, a setting every per-entity style takes (#278).
pub const BLEND_RANGE: StyleParam = StyleParam::new(BLEND_FIELD, 1.0, 0.0..=1.0);

/// The name a variant goes by: its style, `@root` when it is anchored to
/// one, then its settings in key order (`{off}` for an opt-out). Equal
/// settings give equal names, so their entities share an id.
fn variant_name(v: &StyleVariant, root: &str) -> String {
    let mut name = v.style.clone();
    if v.anchor.is_some() || v.local.is_some() {
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
pub(crate) fn request(
    world: &FlintWorld,
    styles: &StyleRegistry,
    config: &PostProcessConfig,
    root: EntityId,
) -> Option<(String, StyleVariant)> {
    let from_component = styles.iter().find_map(|d| {
        let table = world.get_component(root, d.component?)?.as_table()?;
        let mut v = StyleVariant::plain(d.name);
        if table.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
            v.off = true;
        } else {
            read_overrides(d, table, &mut v);
        }
        Some((d, v))
    });
    let (descriptor, mut variant) = match from_component {
        Some((d, v)) => (Some(d), v),
        None => {
            let style = requested_style(world, root)?;
            (styles.find(style), StyleVariant::plain(style))
        }
    };
    // An object-anchored grid: one variant per root, pinned to its origin
    if let Some(d) = descriptor.filter(|d| d.component.is_some() && !variant.off) {
        match variant.option(d, config, "grid") {
            "object" => {
                let p = world.get_world_position(root).unwrap_or_default();
                variant.anchor = Some([p.x, p.y, p.z]);
            }
            // Strokes on the root's own surfaces (#544): its frame, which
            // the renderer swaps for the held one under a motion hold
            "local" => {
                let matrix = world
                    .get_world_matrix(root)
                    .unwrap_or_else(|| flint_core::Transform::default().to_matrix());
                variant.local = Some(LocalFrame { root, matrix });
            }
            _ => {}
        }
    }
    let name = variant_name(&variant, world.get_name(root).unwrap_or(""));
    Some((name, variant))
}

/// Whether `id` asks for a style itself: it carries a `style_mask` or a
/// style's own component, so it roots a styled subtree.
pub fn is_style_root(world: &FlintWorld, styles: &StyleRegistry, id: EntityId) -> bool {
    world.get_component(id, STYLE_MASK_COMPONENT).is_some()
        || styles
            .iter()
            .filter_map(|d| d.component)
            .any(|c| world.get_component(id, c).is_some())
}

/// The styled root `id` takes its style from (#372): itself, else its
/// nearest ancestor that asks for one, as [`resolve`] hands ids down.
/// `None` when no entity on its chain to the top is styled.
pub fn style_root(world: &FlintWorld, styles: &StyleRegistry, id: EntityId) -> Option<EntityId> {
    let mut at = Some(id);
    // A parent cycle can't load, but don't spin if one is built at run time
    for _ in 0..4096 {
        let e = at?;
        if is_style_root(world, styles, e) {
            return Some(e);
        }
        at = world.get_parent(e);
    }
    None
}

/// Whether any entity carries a `style_mask` or a style's own component.
pub(crate) fn any_styled(world: &FlintWorld, styles: &StyleRegistry) -> bool {
    !world
        .entities_with_component(STYLE_MASK_COMPONENT)
        .is_empty()
        || styles
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
    styles: &StyleRegistry,
    config: &PostProcessConfig,
    ids: &mut StyleIds,
    out: &mut HashMap<EntityId, u16>,
) {
    resolve_blended(
        world,
        styles,
        config,
        ids,
        out,
        &mut HashMap::new(),
        &mut HashMap::new(),
    );
}

/// [`resolve`], and the blend of every styled entity drawn at less than 1
/// into `blends` (cleared first; #278). The blend is the root's, carried
/// down its subtree with its id. It isn't part of the variant: entities
/// fading at different rates still share one id, so a ramp makes no new
/// variant a frame. An id all of whose roots are at blend 0 is marked
/// faded in `ids` (#400): it keeps its number and its entities stay in
/// `out`, so the mask is the same whenever another style draws it, but it
/// no longer switches its style on.
///
/// Entities whose root's blend follows the tear (#397) or the screen field
/// (#859, [`blend_source`]) go into `sources` (cleared first); their ids
/// are faded while `config` draws no tear, or the frame no field, as
/// [`StyleIds::set_tear`] and [`StyleIds::set_field`] keep them.
pub(crate) fn resolve_blended(
    world: &FlintWorld,
    styles: &StyleRegistry,
    config: &PostProcessConfig,
    ids: &mut StyleIds,
    out: &mut HashMap<EntityId, u16>,
    blends: &mut HashMap<EntityId, f32>,
    sources: &mut HashMap<EntityId, BlendSource>,
) {
    out.clear();
    blends.clear();
    sources.clear();
    ids.set_tear(crate::postprocess::tear_active(config));
    if !any_styled(world, styles) {
        ids.set(Vec::new());
        return;
    }
    let mut roots: BTreeSet<EntityId> = world.entities_with_component(STYLE_MASK_COMPONENT).clone();
    for c in styles.iter().filter_map(|d| d.component) {
        roots.extend(world.entities_with_component(c));
    }
    let requests: Vec<(EntityId, Option<(String, StyleVariant)>)> = roots
        .iter()
        .map(|&e| (e, request(world, styles, config, e)))
        .collect();
    let changed = ids.set(requests.iter().filter_map(|(_, r)| r.clone()).collect());
    if changed || ids.entities.len() != ids.len() {
        // Roots come in name order, so the first an id meets is its first
        let mut entities = vec![String::new(); ids.len()];
        for (root, req) in &requests {
            if let Some(id) = req.as_ref().and_then(|(n, _)| ids.id(n)) {
                let slot = &mut entities[id as usize - 1];
                if slot.is_empty() {
                    *slot = world.get_name(*root).unwrap_or_default().to_string();
                }
            }
        }
        ids.entities = entities;
    }
    let mut stack = Vec::new();
    // Ids with a root above blend 0 (#400), a flag per id and blend source
    // (#397, #859)
    let mut shown = vec![[false; 3]; ids.len() + 1];
    for (root, req) in &requests {
        let root = *root;
        let Some(id) = req.as_ref().and_then(|(n, _)| ids.id(n)) else {
            continue; // an opt-out: its subtree stays unstyled
        };
        let blend = requested_blend(world, styles, root);
        let source = blend_source(world, styles, root);
        // As the mask's `.g` holds it: a blend that rounds to 0 shows nothing
        let above = encode_blend(blend) > 0;
        shown[id as usize][source as usize] |= above;
        stack.clear();
        stack.push(root);
        while let Some(e) = stack.pop() {
            out.insert(e, id);
            if blend < 1.0 {
                blends.insert(e, blend);
            }
            if source != BlendSource::Own {
                sources.insert(e, source);
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
    ids.set_shown(|id| shown[id as usize]);
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

/// A word for [`debug_color`]'s hue of `id`, so a legend can say "blue"
/// rather than a triple (#309). `NO_STYLE` is "black".
pub fn debug_color_name(id: u16) -> &'static str {
    if id == NO_STYLE {
        return "black";
    }
    let degrees = (id as f32 * 0.618_034).fract() * 360.0;
    const NAMES: [(f32, &str); 12] = [
        (15.0, "red"),
        (40.0, "orange"),
        (68.0, "yellow"),
        (100.0, "lime"),
        (145.0, "green"),
        (170.0, "mint"),
        (195.0, "cyan"),
        (220.0, "azure"),
        (255.0, "blue"),
        (285.0, "violet"),
        (325.0, "magenta"),
        (345.0, "pink"),
    ];
    NAMES
        .iter()
        .find(|(below, _)| degrees < *below)
        .map_or("red", |(_, name)| name)
}

/// One row of the style mask view's legend: an id, the style variant it
/// stands for and the colour the view paints it.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendEntry {
    pub id: u16,
    /// The variant's name, as [`StyleIds`] holds it (`ascii`, `ascii{off}`).
    pub name: String,
    /// The view's display colour, 0..1.
    pub color: [f32; 3],
    /// A word for the colour ([`debug_color_name`]).
    pub color_name: &'static str,
}

/// The legend for the style mask view (#309): each id in `ids`, in id
/// order, with its hue.
pub fn legend(ids: &StyleIds) -> Vec<LegendEntry> {
    ids.iter()
        .map(|(id, name)| LegendEntry {
            id,
            name: name.to_string(),
            color: debug_color(id),
            color_name: debug_color_name(id),
        })
        .collect()
}

/// The legend on one line, as `flint render` prints it:
/// `style mask: 1 ascii (blue), 2 pencil (green)`, or a note that nothing
/// is styled.
pub fn legend_line(ids: &StyleIds) -> String {
    if ids.is_empty() {
        return "style mask: no styled entities".to_string();
    }
    let rows: Vec<String> = legend(ids)
        .iter()
        .map(|e| format!("{} {} ({})", e.id, e.name, e.color_name))
        .collect();
    format!("style mask: {}", rows.join(", "))
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
    /// What the blend is scaled by at each pixel: 0 nothing, 1 the tear
    /// (#397), 2 the screen field (#859) ([`BlendSource::code`]).
    source: u32,
    /// The frame's tear (#746): `postprocess::tear_params` (amount, scale,
    /// iris, post time), then the mask's size in pixels.
    tear: [f32; 4],
    size: [f32; 2],
    _pad: [f32; 2],
}

/// The frame's tear (`[post_process] tear`, #397, #746), for the mask
/// draws whose blend follows it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Tear {
    /// Whether the frame tears at all; while it doesn't, a torn draw
    /// writes blend 0 (the fbm reaches past the threshold of a tear of 0
    /// in places, and nothing tears below 0.001).
    pub on: bool,
    /// `postprocess::tear_params`: amount, scale, iris, post time.
    pub params: [f32; 4],
}

/// The frame's screen field (#859) as the mask shader reads it: a count,
/// then [`MAX_FIELD_SHAPES`] shapes of two `vec4`s each.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FieldUniform {
    count: u32,
    _pad: [u32; 3],
    shapes: [[[f32; 4]; 2]; MAX_FIELD_SHAPES],
}

impl FieldUniform {
    /// Each shape as `style_mask.wgsl`'s `FieldShape`: `a` = (x, y, p0,
    /// p1), `b` = (p2, soft, kind, strength), where a disc's p0 is its
    /// radius, a ring's p0 its radius and p1 half its width, a cone's p0
    /// its length, p1 its direction and p2 its half spread.
    fn of(field: &StyleField) -> Self {
        let mut out = Self::zeroed();
        for (slot, shape) in out.shapes.iter_mut().zip(field.shapes()) {
            let kind = shape.kind() as f32;
            *slot = match *shape {
                FieldShape::Disc {
                    center,
                    radius,
                    soft,
                    strength,
                } => [
                    [center[0], center[1], radius, 0.0],
                    [0.0, soft, kind, strength],
                ],
                FieldShape::Ring {
                    center,
                    radius,
                    width,
                    soft,
                    strength,
                } => [
                    [center[0], center[1], radius, width * 0.5],
                    [0.0, soft, kind, strength],
                ],
                FieldShape::Cone {
                    apex,
                    direction,
                    spread,
                    length,
                    soft,
                    strength,
                } => [
                    [apex[0], apex[1], length, direction],
                    [spread, soft, kind, strength],
                ],
            };
        }
        out.count = field.shapes().len().min(MAX_FIELD_SHAPES) as u32;
        out
    }
}

/// What a masked draw draws: a mesh with its cached transform (static, a
/// procedural box, a terrain chunk, or skinned with its bones), or a
/// billboard's quad with the sprite's own bind groups (#1011).
pub(crate) enum MaskShape<'a> {
    Mesh {
        vertex_buffer: &'a wgpu::Buffer,
        index_buffer: &'a wgpu::Buffer,
        index_count: u32,
        transform_bind_group: &'a wgpu::BindGroup,
        /// The bone matrices, for a skinned draw.
        bones: Option<&'a wgpu::BindGroup>,
    },
    /// The sprite's uniforms (group 0 of the billboard pipeline) and its
    /// texture (group 1 there, group 2 in the mask's).
    Billboard {
        billboard_bind_group: &'a wgpu::BindGroup,
        texture_bind_group: &'a wgpu::BindGroup,
    },
}

/// One masked draw.
pub(crate) struct MaskDraw<'a> {
    pub shape: MaskShape<'a>,
    pub id: u16,
    /// How much of the style the entity is drawn with, 0..1 (#278).
    pub blend: f32,
    /// What the blend is scaled by at each pixel: the tear (#397), the
    /// screen field (#859), or nothing.
    pub source: BlendSource,
    /// Whether its vertices drift on the wooze field (#215), as the main
    /// pass drew them: it is masked through the woozy vertex stages (#737).
    pub woozy: bool,
}

impl MaskDraw<'_> {
    /// The bone matrices of a skinned mesh draw.
    pub fn bones(&self) -> Option<&wgpu::BindGroup> {
        match &self.shape {
            MaskShape::Mesh { bones, .. } => *bones,
            MaskShape::Billboard { .. } => None,
        }
    }

    pub fn is_billboard(&self) -> bool {
        matches!(self.shape, MaskShape::Billboard { .. })
    }
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
    /// A living prop's echo (#737): skinned, front faces only, as the main
    /// pass draws it, so a two-sided leaf card covers once.
    pub echo: bool,
    /// Drawn through shallow wooze's vertex stage (#1241), as the main
    /// pass drew it, so it covers where it drifted to and not its rest
    /// pose. An echo is never woozy.
    pub woozy: bool,
}

/// The frame's particles, for the veil: the particle pipeline's shared
/// bind groups and its sorted draws.
pub(crate) struct VeilParticles<'a> {
    /// The layouts the veil's own particle pipelines are made against.
    pub pipeline: &'a crate::particle_pipeline::ParticlePipeline,
    pub buffers: &'a crate::particle_pipeline::ParticleBuffers,
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

/// The veil's woozy pipelines (#1241): `wooze_vertex.wgsl`'s stages, the
/// ones the main pass drifts a mesh with, over the same `fs_veil`s.
struct WoozyVeil {
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
}

/// The veil's particle pipelines: alpha-like modes and additive.
struct ParticleVeil {
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
}

/// A veil pipeline: `module`'s `vs`/`fs`, single-sample, depth-tested
/// against the scene and never writing it, its coverage "over" what's
/// there (red only). Both faces unless `cull_mode` says otherwise.
fn veil_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    (vs, fs): (&str, &str),
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    veil_pipeline_split(
        device,
        label,
        (module, vs),
        (module, fs),
        layouts,
        buffers,
        cull_mode,
    )
}

/// [`veil_pipeline`] with its vertex stage from another module: shallow
/// wooze's (#1241), whose stages hand over what the plain ones do.
fn veil_pipeline_split(
    device: &wgpu::Device,
    label: &str,
    (vertex, vs): (&wgpu::ShaderModule, &str),
    (fragment, fs): (&wgpu::ShaderModule, &str),
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[wgpu::VertexBufferLayout<'static>],
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: vertex,
            entry_point: Some(vs),
            buffers: &buffers.iter().cloned().map(Some).collect::<Vec<_>>(),
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment,
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
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
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
    /// The woozy vertex stages (#737), made the first frame a styled draw
    /// drifts, and the module they come from (the 4x path's are made from
    /// it too).
    woozy: Option<(wgpu::ShaderModule, MaskStages)>,
    /// The billboard pipeline (#1011), made the first frame a styled
    /// billboard is masked.
    billboard: Option<BillboardMask>,
    veil_static: wgpu::RenderPipeline,
    veil_skinned: wgpu::RenderPipeline,
    /// `veil_skinned` on front faces only, for echoes (#737).
    veil_echo: wgpu::RenderPipeline,
    /// Made the first frame a blended draw in front drifts (#1241).
    veil_woozy: Option<WoozyVeil>,
    /// Made the first frame with particles (the particle pipeline is too).
    veil_particles: Option<ParticleVeil>,
    draw_bgl: wgpu::BindGroupLayout,
    /// The 4x path (#306), made the first frame the scene draws with MSAA.
    msaa: Option<MsaaMask>,
    /// The mask's shader, kept for the 4x path's pipelines.
    shader: wgpu::ShaderModule,
    /// One [`DRAW_STRIDE`] slot a draw, [`draw_slots`](Self::draw_slots)
    /// of them; replaced by a bigger one when a frame needs more.
    draw_buffer: wgpu::Buffer,
    draw_slots: usize,
    /// The uniform bytes, kept so a frame reuses the allocation.
    scratch: Vec<u8>,
    /// Styled draws the last frame masked.
    last_draws: usize,
    /// The screen field's uniform (#859), binding 4 of both draw layouts:
    /// made once, written only on a frame a draw follows a field that
    /// differs from the one it holds.
    field_buffer: wgpu::Buffer,
    /// What `field_buffer` holds.
    field_written: StyleField,
}

/// One vertex stage's mask pipelines, static and skinned.
struct MaskStages {
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
}

impl MaskStages {
    /// The woozy pair (#737) from `shader`, the woozy module, at `samples`
    /// with `fragment` (`fs_main`, or `fs_msaa` at 4x) over `draw_bgl`.
    fn woozy(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        (fragment, samples): (&str, u32),
        draw_bgl: &wgpu::BindGroupLayout,
        transform_bgl: &wgpu::BindGroupLayout,
        bone_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let label = |skinned: bool| match (skinned, samples > 1) {
            (false, false) => "Style Mask Woozy Pipeline",
            (true, false) => "Style Mask Woozy Skinned Pipeline",
            (false, true) => "Style Mask MSAA Woozy Pipeline",
            (true, true) => "Style Mask MSAA Woozy Skinned Pipeline",
        };
        Self {
            static_pipeline: mask_pipeline(
                device,
                shader,
                label(false),
                "vs_static_wooze",
                fragment,
                samples,
                &[transform_bgl, draw_bgl],
                Some(crate::primitives::Vertex::desc()),
            ),
            skinned_pipeline: mask_pipeline(
                device,
                shader,
                label(true),
                "vs_skinned_wooze",
                fragment,
                samples,
                &[transform_bgl, draw_bgl, bone_bgl],
                Some(crate::primitives::SkinnedVertex::desc()),
            ),
        }
    }
}

/// The mask's billboard pipeline (#1011) and what the 4x one is made from:
/// the billboard module, the sprite's two layouts, and the billboard
/// pipeline's quad index buffer the draws are indexed by.
struct BillboardMask {
    shader: wgpu::ShaderModule,
    uniforms_bgl: wgpu::BindGroupLayout,
    texture_bgl: wgpu::BindGroupLayout,
    quad: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
}

/// The billboard mask pipeline (#1011) from `shader` at `samples` over
/// `draw_bgl`: the sprite's uniforms in group 0, the mask's draw group in 1
/// (the meshes' slot), the sprite's texture in 2.
fn billboard_mask_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    (uniforms, texture): (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout),
    draw_bgl: &wgpu::BindGroupLayout,
    samples: u32,
) -> wgpu::RenderPipeline {
    let (label, fragment) = if samples > 1 {
        ("Style Mask MSAA Billboard Pipeline", "fs_billboard_msaa")
    } else {
        ("Style Mask Billboard Pipeline", "fs_billboard")
    };
    mask_pipeline(
        device,
        shader,
        label,
        "vs_billboard",
        fragment,
        samples,
        &[uniforms, draw_bgl, texture],
        None,
    )
}

fn mask_draw_buffer(device: &wgpu::Device, slots: usize) -> wgpu::Buffer {
    device.create_buffer_counted(&wgpu::BufferDescriptor {
        label: Some("Style Mask Draws"),
        size: DRAW_STRIDE * slots as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The mask's 4x path (#306): pipelines drawing into a multisampled mask
/// against the scene's multisampled depth, and the resolve that folds it
/// into the single-sample mask.
struct MsaaMask {
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
    /// The woozy vertex stages at 4x (#737), made with the single-sample ones.
    woozy: Option<MaskStages>,
    /// The billboard pipeline at 4x (#1011), made with the single-sample one.
    billboard: Option<wgpu::RenderPipeline>,
    draw_bgl: wgpu::BindGroupLayout,
    resolve_pipeline: wgpu::RenderPipeline,
    resolve_bgl: wgpu::BindGroupLayout,
    /// The 4x mask, its size and the resolve's bind group over it.
    target: Option<(wgpu::TextureView, u32, u32, wgpu::BindGroup)>,
}

/// The scene's multisampled depth, for the mask's 4x path (#306). A bind
/// group over it is kept while it is the same view (#960).
pub(crate) struct MsaaDepth<'a> {
    pub view: &'a wgpu::TextureView,
}

/// The scene's sample count the mask matches under MSAA.
pub const MSAA_SAMPLES: u32 = 4;

#[allow(clippy::too_many_arguments)]
fn mask_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    label: &str,
    entry: &str,
    fragment: &str,
    samples: u32,
    layouts: &[&wgpu::BindGroupLayout],
    buffer: Option<wgpu::VertexBufferLayout<'static>>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(entry),
            // A billboard's quad has none: its corners come from the index
            buffers: &buffer.into_iter().map(Some).collect::<Vec<_>>(),
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
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
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

/// The per-draw uniforms' entry, binding 0 of both draw layouts.
fn draw_uniform_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<MaskDrawUniform>() as u64),
        },
        count: None,
    }
}

/// The screen field's entry (#859), binding 4 of both draw layouts.
fn field_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 4,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<FieldUniform>() as u64),
        },
        count: None,
    }
}

/// A depth texture entry of a draw layout.
fn depth_entry(binding: u32, multisampled: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled,
        },
        count: None,
    }
}

/// The style veil's entry (#290): a single-sample coverage texture, also
/// under MSAA, where every sample of a pixel reads the same veil.
fn veil_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl MsaaMask {
    fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        transform_bgl: &wgpu::BindGroupLayout,
        bone_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Mask MSAA Draw BGL"),
            entries: &[
                draw_uniform_entry(),
                veil_entry(2),
                depth_entry(3, true),
                field_entry(),
            ],
        });
        let static_pipeline = mask_pipeline(
            device,
            shader,
            "Style Mask MSAA Pipeline",
            "vs_static",
            "fs_msaa",
            MSAA_SAMPLES,
            &[transform_bgl, &draw_bgl],
            Some(crate::primitives::Vertex::desc()),
        );
        let skinned_pipeline = mask_pipeline(
            device,
            shader,
            "Style Mask MSAA Skinned Pipeline",
            "vs_skinned",
            "fs_msaa",
            MSAA_SAMPLES,
            &[transform_bgl, &draw_bgl, bone_bgl],
            Some(crate::primitives::SkinnedVertex::desc()),
        );
        let resolve_shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Mask Resolve Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("style_mask_resolve.wgsl").into()),
        });
        let resolve_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Mask Resolve BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
                count: None,
            }],
        });
        let resolve_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Style Mask Resolve Layout"),
            bind_group_layouts: &[Some(&resolve_bgl)],
            immediate_size: 0,
        });
        let resolve_pipeline =
            device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
                label: Some("Style Mask Resolve Pipeline"),
                layout: Some(&resolve_layout),
                vertex: wgpu::VertexState {
                    module: &resolve_shader,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &resolve_shader,
                    entry_point: Some("fs_resolve"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: STYLE_MASK_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        Self {
            static_pipeline,
            skinned_pipeline,
            woozy: None,
            billboard: None,
            draw_bgl,
            resolve_pipeline,
            resolve_bgl,
            target: None,
        }
    }

    /// Make the 4x mask at `width` x `height`, again on a size change.
    fn ensure_target(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if matches!(&self.target, Some((_, w, h, _)) if (*w, *h) == (width, height)) {
            return;
        }
        let texture = device.create_texture_counted(&wgpu::TextureDescriptor {
            label: Some("Style Mask MSAA"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: STYLE_MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            label: Some("Style Mask Resolve BG"),
            layout: &self.resolve_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.target = Some((view, width, height, bind));
    }
}

impl StyleMaskPass {
    pub(crate) fn new(device: &wgpu::Device, layouts: &MaskLayouts<'_>) -> Self {
        let (transform_bgl, bone_bgl) = (layouts.transform, layouts.bones);
        let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Mask Shader"),
            source: wgpu::ShaderSource::Wgsl(STYLE_MASK_WGSL.into()),
        });
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Style Mask Draw BGL"),
            entries: &[
                draw_uniform_entry(),
                depth_entry(1, false),
                veil_entry(2),
                field_entry(),
            ],
        });
        let static_pipeline = mask_pipeline(
            device,
            &shader,
            "Style Mask Pipeline",
            "vs_static",
            "fs_main",
            1,
            &[transform_bgl, &draw_bgl],
            Some(crate::primitives::Vertex::desc()),
        );
        let skinned_pipeline = mask_pipeline(
            device,
            &shader,
            "Style Mask Skinned Pipeline",
            "vs_skinned",
            "fs_main",
            1,
            &[transform_bgl, &draw_bgl, bone_bgl],
            Some(crate::primitives::SkinnedVertex::desc()),
        );
        let pbr = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SCENE.into()),
        });
        let skinned = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil Skinned Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SKINNED.into()),
        });
        let veil_static = veil_pipeline(
            device,
            "Style Veil Pipeline",
            &pbr,
            ("vs_main", "fs_veil"),
            &[transform_bgl, layouts.material, layouts.light],
            &[crate::primitives::Vertex::desc()],
            None,
        );
        let veil_skinned = veil_pipeline(
            device,
            "Style Veil Skinned Pipeline",
            &skinned,
            ("vs_skinned", "fs_veil"),
            &[transform_bgl, layouts.material, layouts.light, bone_bgl],
            &[crate::primitives::SkinnedVertex::desc()],
            None,
        );
        let veil_echo = veil_pipeline(
            device,
            "Style Veil Echo Pipeline",
            &skinned,
            ("vs_skinned", "fs_veil"),
            &[transform_bgl, layouts.material, layouts.light, bone_bgl],
            &[crate::primitives::SkinnedVertex::desc()],
            Some(wgpu::Face::Back),
        );
        let draw_buffer = mask_draw_buffer(device, INITIAL_MASK_SLOTS);
        // Zeroed at creation: no shapes
        let field_buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
            label: Some("Style Mask Field"),
            size: std::mem::size_of::<FieldUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            static_pipeline,
            skinned_pipeline,
            woozy: None,
            billboard: None,
            veil_static,
            veil_skinned,
            veil_echo,
            veil_woozy: None,
            veil_particles: None,
            draw_bgl,
            msaa: None,
            shader,
            draw_buffer,
            draw_slots: INITIAL_MASK_SLOTS,
            scratch: Vec::new(),
            last_draws: 0,
            field_buffer,
            field_written: StyleField::default(),
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

    /// Build the 4x path (#306) if it isn't yet: the first frame the scene
    /// draws with MSAA.
    pub(crate) fn ensure_msaa(
        &mut self,
        device: &wgpu::Device,
        transform_bgl: &wgpu::BindGroupLayout,
        bone_bgl: &wgpu::BindGroupLayout,
    ) {
        if self.msaa.is_none() {
            self.msaa = Some(MsaaMask::new(device, &self.shader, transform_bgl, bone_bgl));
        }
    }

    /// Build the woozy vertex stages (#737) if they aren't yet, at 1x and,
    /// once the 4x path exists, at 4x: the first frame a styled draw
    /// drifts on the wooze field. Call it after [`ensure_msaa`](Self::ensure_msaa).
    pub(crate) fn ensure_wooze(
        &mut self,
        device: &wgpu::Device,
        transform_bgl: &wgpu::BindGroupLayout,
        bone_bgl: &wgpu::BindGroupLayout,
    ) {
        if self.woozy.is_none() {
            let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Style Mask Woozy Shader"),
                source: wgpu::ShaderSource::Wgsl(woozy_mask_source().into()),
            });
            let stages = MaskStages::woozy(
                device,
                &shader,
                ("fs_main", 1),
                &self.draw_bgl,
                transform_bgl,
                bone_bgl,
            );
            self.woozy = Some((shader, stages));
        }
        if let (Some(msaa), Some((shader, _))) = (&mut self.msaa, &self.woozy) {
            if msaa.woozy.is_none() {
                msaa.woozy = Some(MaskStages::woozy(
                    device,
                    shader,
                    ("fs_msaa", MSAA_SAMPLES),
                    &msaa.draw_bgl,
                    transform_bgl,
                    bone_bgl,
                ));
            }
        }
    }

    /// Build the veil's woozy pipelines (#1241) if they aren't yet: the
    /// first frame a blended draw in front of a styled one drifts on the
    /// wooze field. A scene without one never makes them, so its frames
    /// are what they were.
    pub(crate) fn ensure_veil_wooze(&mut self, device: &wgpu::Device, layouts: &MaskLayouts<'_>) {
        if self.veil_woozy.is_some() {
            return;
        }
        let vertex = crate::wooze_pipeline::vertex_module(device);
        let pbr = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil Woozy PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SCENE.into()),
        });
        let skinned = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Style Veil Woozy Skinned Shader"),
            source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::SKINNED.into()),
        });
        let (transform, material, light) = (layouts.transform, layouts.material, layouts.light);
        self.veil_woozy = Some(WoozyVeil {
            static_pipeline: veil_pipeline_split(
                device,
                "Style Veil Woozy Pipeline",
                (&vertex, "vs_wooze"),
                (&pbr, "fs_veil"),
                &[transform, material, light],
                &[crate::primitives::Vertex::desc()],
                None,
            ),
            skinned_pipeline: veil_pipeline_split(
                device,
                "Style Veil Woozy Skinned Pipeline",
                (&vertex, "vs_skinned_wooze"),
                (&skinned, "fs_veil"),
                &[transform, material, light, layouts.bones],
                &[crate::primitives::SkinnedVertex::desc()],
                None,
            ),
        });
    }

    /// The veil pipeline a mesh draws through: the main pass's vertex
    /// stage (#1241), and front faces only for an echo (#737).
    fn veil_mesh_pipeline(&self, skinned: bool, echo: bool, woozy: bool) -> &wgpu::RenderPipeline {
        let woozy = if woozy {
            self.veil_woozy.as_ref()
        } else {
            None
        };
        match (echo, skinned, woozy) {
            (true, _, _) => &self.veil_echo,
            (false, true, Some(w)) => &w.skinned_pipeline,
            (false, false, Some(w)) => &w.static_pipeline,
            (false, true, None) => &self.veil_skinned,
            (false, false, None) => &self.veil_static,
        }
    }

    /// Build the billboard pipeline (#1011) if it isn't yet, at 1x and,
    /// once the 4x path exists, at 4x: the first frame a styled billboard
    /// is masked. `uniforms` and `texture` are the billboard pipeline's
    /// layouts, `quad` its index buffer. Call it after
    /// [`ensure_msaa`](Self::ensure_msaa).
    pub(crate) fn ensure_billboard(
        &mut self,
        device: &wgpu::Device,
        uniforms: &wgpu::BindGroupLayout,
        texture: &wgpu::BindGroupLayout,
        quad: &wgpu::Buffer,
    ) {
        if self.billboard.is_none() {
            let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Style Mask Billboard Shader"),
                source: wgpu::ShaderSource::Wgsl(billboard_mask_source().into()),
            });
            let pipeline =
                billboard_mask_pipeline(device, &shader, (uniforms, texture), &self.draw_bgl, 1);
            self.billboard = Some(BillboardMask {
                shader,
                uniforms_bgl: uniforms.clone(),
                texture_bgl: texture.clone(),
                quad: quad.clone(),
                pipeline,
            });
        }
        if let (Some(msaa), Some(mask)) = (&mut self.msaa, &self.billboard) {
            if msaa.billboard.is_none() {
                msaa.billboard = Some(billboard_mask_pipeline(
                    device,
                    &mask.shader,
                    (&mask.uniforms_bgl, &mask.texture_bgl),
                    &msaa.draw_bgl,
                    MSAA_SAMPLES,
                ));
            }
        }
    }

    /// The per-draw uniforms' binding, the same in both draw layouts.
    fn uniform_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.draw_buffer,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<MaskDrawUniform>() as u64),
            }),
        }
    }

    /// The screen field's binding (#859), the same in both draw layouts.
    fn field_entry(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: 4,
            resource: self.field_buffer.as_entire_binding(),
        }
    }

    /// The bind group of the per-draw uniforms and `scene_depth`, kept in
    /// `binds` over the depth view, the draw buffer and the veil (#960: a
    /// grown buffer's group takes the old one's place).
    fn draw_bind(
        &self,
        device: &wgpu::Device,
        binds: &crate::bind_cache::BindCache,
        scene_depth: &wgpu::TextureView,
        veil: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey<'_>,
    ) -> std::sync::Arc<wgpu::BindGroup> {
        let over = [
            crate::bind_cache::Bound::Buffer(&self.draw_buffer),
            crate::bind_cache::Bound::View(veil),
        ];
        binds.get_over(depth_key, "Style Mask Draw BG", 0, &over, || {
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Style Mask Draw BG"),
                layout: &self.draw_bgl,
                entries: &[
                    self.uniform_entry(),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(scene_depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(veil),
                    },
                    self.field_entry(),
                ],
            })
        })
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
            let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Style Veil Particle Shader"),
                source: wgpu::ShaderSource::Wgsl(crate::wgsl_sources::PARTICLE.into()),
            });
            let layouts = [
                &pp.uniform_bind_group_layout,
                &pp.instance_bind_group_layout,
                &pp.texture_bind_group_layout,
            ];
            let make = |label, fs| {
                veil_pipeline(
                    device,
                    label,
                    &shader,
                    ("vs_particle", fs),
                    &layouts,
                    &[],
                    None,
                )
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
                depth_slice: None,
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
            multiview_mask: None,
        });
        if veil.is_empty() {
            return;
        }
        let mut bound = None;
        for m in &veil.meshes {
            let key = (m.bones.is_some(), m.echo, m.woozy);
            if bound != Some(key) {
                pass.set_pipeline(self.veil_mesh_pipeline(key.0, key.1, key.2));
                pass.set_bind_group(2, veil.light_bind_group, &[]);
                bound = Some(key);
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
            let pp = particles.buffers;
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

    /// The 4x path's bind group over the veil, the scene's multisampled
    /// depth and the uniform buffer, kept over those three (#960): a new
    /// depth texture or a grown buffer (#311) gets a group in the old one's
    /// place.
    fn msaa_draw_bind(
        &self,
        device: &wgpu::Device,
        binds: &crate::bind_cache::BindCache,
        msaa: &MsaaMask,
        veil: &wgpu::TextureView,
        depth: &MsaaDepth<'_>,
    ) -> std::sync::Arc<wgpu::BindGroup> {
        let over = [
            crate::bind_cache::Bound::Buffer(&self.draw_buffer),
            crate::bind_cache::Bound::View(veil),
            crate::bind_cache::Bound::View(depth.view),
        ];
        binds.get_bound("Style Mask MSAA Draw BG", 0, &over, || {
            device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                label: Some("Style Mask MSAA Draw BG"),
                layout: &msaa.draw_bgl,
                entries: &[
                    self.uniform_entry(),
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(veil),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(depth.view),
                    },
                    self.field_entry(),
                ],
            })
        })
    }

    /// Draw the veil into `veil_view`, then clear `mask` and draw `draws`
    /// into it, each tested against `scene_depth` (single-sample, the
    /// mask's size) and its blend scaled by what the veil leaves uncovered.
    /// With `msaa_depth` (the scene drew 4x, and
    /// [`ensure_msaa`](Self::ensure_msaa) ran), they're drawn 4x against
    /// that depth instead, and resolved into `mask` with any sample a
    /// style drew winning (#306).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        binds: &crate::bind_cache::BindCache,
        mask: &crate::stylize::StyleTexture,
        veil_view: &wgpu::TextureView,
        scene_depth: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey<'_>,
        msaa_depth: Option<MsaaDepth<'_>>,
        draws: &[MaskDraw<'_>],
        veil: &Veil<'_>,
        tear: Tear,
        field: &StyleField,
    ) {
        // Only a styled pixel reads the veil: with none, it isn't drawn
        if !draws.is_empty() {
            self.encode_veil(device, encoder, veil_view, scene_depth, veil);
        }
        let slots = mask_slots_for(self.draw_slots, draws.len());
        if slots != self.draw_slots {
            self.draw_buffer = mask_draw_buffer(device, slots);
            self.draw_slots = slots;
        }
        self.last_draws = draws.len();
        self.scratch.clear();
        self.scratch.resize(draws.len() * DRAW_STRIDE as usize, 0);
        let mut follows_field = false;
        for (i, d) in draws.iter().enumerate() {
            let at = i * DRAW_STRIDE as usize;
            // With no tear, or no field, a draw following it is at blend 0
            let on = match d.source {
                BlendSource::Own => true,
                BlendSource::Tear => tear.on,
                BlendSource::Field => !field.is_empty(),
            };
            follows_field |= d.source == BlendSource::Field && on;
            let uniform = MaskDrawUniform {
                id: u32::from(d.id),
                slope_tolerance: SLOPE_TOLERANCE,
                blend: encode_blend(if on { d.blend } else { 0.0 }),
                source: if on { d.source.code() } else { 0 },
                tear: tear.params,
                size: [mask.width as f32, mask.height as f32],
                _pad: [0.0; 2],
            };
            self.scratch[at..at + std::mem::size_of::<MaskDrawUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
        }
        if !self.scratch.is_empty() {
            queue.write_buffer_counted(&self.draw_buffer, 0, &self.scratch);
        }
        // The field (#859): written only when a draw reads it and it moved
        if follows_field && *field != self.field_written {
            let uniform = FieldUniform::of(field);
            queue.write_buffer_counted(&self.field_buffer, 0, bytemuck::bytes_of(&uniform));
            self.field_written.clone_from(field);
        }

        // The 4x path, while the scene drew 4x
        let msaa_depth = msaa_depth.filter(|_| self.msaa.is_some());
        if let Some(depth) = msaa_depth {
            let mut msaa = self.msaa.take().expect("checked above");
            msaa.ensure_target(device, mask.width, mask.height);
            let draw_bg = (!draws.is_empty())
                .then(|| self.msaa_draw_bind(device, binds, &msaa, veil_view, &depth));
            let (ms_view, _, _, resolve_bg) = msaa.target.as_ref().expect("made above");
            {
                let mut pass = clear_pass(encoder, "Style Mask MSAA Pass", ms_view);
                if let Some(draw_bg) = &draw_bg {
                    let plain = (&msaa.static_pipeline, &msaa.skinned_pipeline);
                    let billboard = msaa
                        .billboard
                        .as_ref()
                        .zip(self.billboard.as_ref().map(|b| &b.quad));
                    let kinds = DrawKinds {
                        plain,
                        woozy: msaa.woozy.as_ref(),
                        billboard,
                    };
                    record_draws(&mut pass, draws, draw_bg, kinds);
                }
            }
            let mut pass = clear_pass(encoder, "Style Mask Resolve Pass", &mask.view);
            pass.set_pipeline(&msaa.resolve_pipeline);
            pass.set_bind_group(0, resolve_bg, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            self.msaa = Some(msaa);
            return;
        }

        let draw_bg = (!draws.is_empty())
            .then(|| self.draw_bind(device, binds, scene_depth, veil_view, depth_key));
        let mut pass = clear_pass(encoder, "Style Mask Pass", &mask.view);
        if let Some(draw_bg) = &draw_bg {
            let kinds = DrawKinds {
                plain: (&self.static_pipeline, &self.skinned_pipeline),
                woozy: self.woozy.as_ref().map(|(_, stages)| stages),
                billboard: self.billboard.as_ref().map(|b| (&b.pipeline, &b.quad)),
            };
            record_draws(&mut pass, draws, draw_bg, kinds);
        }
    }
}

/// A pass over `view` alone, cleared to no style.
fn clear_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// The pipelines one sample count draws the mask with: static and skinned,
/// the woozy pair once made (#737), and the billboard's with its quad's
/// index buffer once made (#1011).
struct DrawKinds<'p> {
    plain: (&'p wgpu::RenderPipeline, &'p wgpu::RenderPipeline),
    woozy: Option<&'p MaskStages>,
    billboard: Option<(&'p wgpu::RenderPipeline, &'p wgpu::Buffer)>,
}

/// Which of [`DrawKinds`]' pipelines is bound.
#[derive(Clone, Copy, PartialEq)]
enum BoundKind {
    Mesh { skinned: bool, woozy: bool },
    Billboard,
}

/// Record every masked draw, switching between the static and skinned
/// pipelines, plain or woozy (#737), and the billboard's (#1011), as the
/// list does. A woozy draw with no woozy stages yet (the caller didn't
/// [`ensure_wooze`](StyleMaskPass::ensure_wooze)) is drawn plain; a
/// billboard with no billboard pipeline (no
/// [`ensure_billboard`](StyleMaskPass::ensure_billboard)) isn't drawn.
fn record_draws(
    pass: &mut wgpu::RenderPass<'_>,
    draws: &[MaskDraw<'_>],
    draw_bg: &wgpu::BindGroup,
    kinds: DrawKinds<'_>,
) {
    let (static_pipeline, skinned_pipeline) = kinds.plain;
    let mut bound = None;
    for (i, d) in draws.iter().enumerate() {
        let offset = [(i as u64 * DRAW_STRIDE) as u32];
        match &d.shape {
            MaskShape::Mesh {
                vertex_buffer,
                index_buffer,
                index_count,
                transform_bind_group,
                bones,
            } => {
                let key = BoundKind::Mesh {
                    skinned: bones.is_some(),
                    woozy: d.woozy && kinds.woozy.is_some(),
                };
                if bound != Some(key) {
                    pass.set_pipeline(match (key, kinds.woozy) {
                        (
                            BoundKind::Mesh {
                                skinned: true,
                                woozy: true,
                            },
                            Some(w),
                        ) => &w.skinned_pipeline,
                        (
                            BoundKind::Mesh {
                                skinned: false,
                                woozy: true,
                            },
                            Some(w),
                        ) => &w.static_pipeline,
                        (BoundKind::Mesh { skinned: true, .. }, _) => skinned_pipeline,
                        _ => static_pipeline,
                    });
                    bound = Some(key);
                }
                pass.set_bind_group(0, *transform_bind_group, &[]);
                pass.set_bind_group(1, draw_bg, &offset);
                if let Some(bones) = bones {
                    pass.set_bind_group(2, *bones, &[]);
                }
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*index_count, 0, 0..1);
            }
            MaskShape::Billboard {
                billboard_bind_group,
                texture_bind_group,
            } => {
                let Some((pipeline, quad)) = kinds.billboard else {
                    continue;
                };
                if bound != Some(BoundKind::Billboard) {
                    pass.set_pipeline(pipeline);
                    pass.set_index_buffer(quad.slice(..), wgpu::IndexFormat::Uint32);
                    bound = Some(BoundKind::Billboard);
                }
                pass.set_bind_group(0, *billboard_bind_group, &[]);
                pass.set_bind_group(1, draw_bg, &offset);
                pass.set_bind_group(2, *texture_bind_group, &[]);
                pass.draw_indexed(0..6, 0, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built-in style's component schema gives every number the range
    /// its `StyleParam` has (#492): the schema is how the scene loader and
    /// `flint validate` say `ascii_style { cell = 400 }` is out of range,
    /// and `--style` and the scene's table use the param's.
    #[test]
    #[allow(clippy::disallowed_methods)] // reads the engine's schema files from the checkout
    fn built_in_style_schemas_carry_their_params_ranges() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let registry = flint_schema::SchemaRegistry::load_from_directory(&dir).unwrap();
        let mut checked = Vec::new();
        for d in crate::stylize::builtins() {
            let Some(component) = d.component else {
                continue;
            };
            let Some(schema) = registry.get_component(component) else {
                // A user or game style a test registered: no schema file
                continue;
            };
            // A param with no field is the whole frame's alone (ASCII's
            // `smoothing`); which fields there are, each style's own test
            // holds
            let range = |field: &str| {
                let f = schema.fields.get(field)?;
                Some((f.min.map(|m| m as f32), f.max.map(|m| m as f32)))
            };
            let mut fields = 0;
            for p in d.params.iter().filter(|p| d.color_of(p.name).is_none()) {
                if let Some(range) = range(p.name) {
                    assert_eq!(range, (Some(p.min), Some(p.max)), "{component}.{}", p.name);
                    fields += 1;
                }
            }
            assert!(fields > 0, "{component} has a field for a parameter");
            for color in d.colors() {
                let [r, g, b] = color.channels;
                assert!(
                    (r.min, r.max) == (g.min, g.max) && (g.min, g.max) == (b.min, b.max),
                    "{}.{}'s channels share one range",
                    d.name,
                    color.base
                );
                if let Some(range) = range(color.base) {
                    assert_eq!(
                        range,
                        (Some(r.min), Some(r.max)),
                        "{component}.{}",
                        color.base
                    );
                }
            }
            let blend = (Some(BLEND_RANGE.min), Some(BLEND_RANGE.max));
            assert_eq!(range(BLEND_FIELD), Some(blend), "{component}.blend");
            checked.push(component);
        }
        for c in ["ascii_style", "pencil_style", "watercolor_style"] {
            assert!(checked.contains(&c), "{c} checked: {checked:?}");
        }
    }

    /// #1063: each style's (and `ui_text`'s) asset field lists, as its
    /// `builtins`, exactly the names its loader takes with no file, so
    /// `flint validate` passes `amber` and `none` and calls `ambr` missing.
    /// The schema can't ask this crate, so this holds the two lists equal.
    #[test]
    #[allow(clippy::disallowed_methods)] // reads the engine's schema files from the checkout
    fn the_asset_fields_name_the_built_ins_their_loaders_take() {
        use flint_core::project::AssetKind;
        use flint_schema::FieldType;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let registry = flint_schema::SchemaRegistry::load_from_directory(&dir).unwrap();
        let none = [crate::stylize::images::NO_IMAGE];
        let glyphs = [crate::glyph_atlas::BUILTIN_SET];
        let fields: [(&str, &str, &[AssetKind], &[&str]); 6] = [
            (
                "ascii_style",
                "palette",
                &[AssetKind::Palettes],
                crate::palette::BUILTIN_PALETTES,
            ),
            ("ascii_style", "font", &[AssetKind::GlyphSets], &glyphs),
            ("pencil_style", "paper_image", &[AssetKind::Papers], &none),
            ("pencil_style", "stroke_image", &[AssetKind::Strokes], &none),
            (
                "watercolor_style",
                "paper_image",
                &[AssetKind::Papers],
                &none,
            ),
            (
                "ui_text",
                "font",
                &[AssetKind::GlyphSets, AssetKind::Fonts],
                &glyphs,
            ),
        ];
        for (component, field, want_kinds, want_builtins) in fields {
            let schema = registry
                .get_component(component)
                .and_then(|c| c.get_field(field))
                .unwrap_or_else(|| panic!("no field {component}.{field}"));
            let FieldType::Asset {
                kinds, builtins, ..
            } = &schema.field_type
            else {
                panic!("{component}.{field} is no asset field");
            };
            assert_eq!(kinds.as_slice(), want_kinds, "{component}.{field}'s kinds");
            assert_eq!(builtins, want_builtins, "{component}.{field}'s builtins");
        }
        // Each image option's no-file name is the one the schema lists
        let mut images = 0;
        for style in ["pencil", "watercolor"] {
            let d = crate::stylize::builtin(style).unwrap();
            for option in d.options.iter().filter(|o| o.name.ends_with("_image")) {
                assert_eq!(option.default, crate::stylize::images::NO_IMAGE);
                images += 1;
            }
        }
        assert_eq!(images, 3, "pencil's paper and strokes, watercolor's paper");
    }

    #[test]
    fn a_box_projects_to_its_screen_rect_and_behind_the_camera_to_nothing() {
        let camera = crate::Camera {
            position: flint_core::Vec3::new(0.0, 0.0, 10.0),
            target: flint_core::Vec3::ZERO,
            ..Default::default()
        };
        let vp = camera.view_projection_matrix();
        // A unit box at the origin, straight ahead: around the centre
        let IdBounds::Uv([x0, y0, x1, y1]) = IdBounds::of_box(&vp, [-0.5; 3], [0.5; 3]) else {
            panic!("in view");
        };
        assert!(x0 < 0.5 && x1 > 0.5 && y0 < 0.5 && y1 > 0.5);
        assert!(x1 - x0 < 0.3 && y1 - y0 < 0.3, "small at 10 m");
        // Above the centre is up the screen, which is y down
        let IdBounds::Uv([_, _, _, top_y1]) =
            IdBounds::of_box(&vp, [-0.5, 2.0, -0.5], [0.5, 3.0, 0.5])
        else {
            panic!("in view");
        };
        assert!(top_y1 < 0.5);
        // Behind the camera, off to the side, and around the camera
        assert_eq!(
            IdBounds::of_box(&vp, [-0.5, -0.5, 20.0], [0.5, 0.5, 21.0]),
            IdBounds::Nowhere
        );
        assert_eq!(
            IdBounds::of_box(&vp, [100.0, -0.5, -0.5], [101.0, 0.5, 0.5]),
            IdBounds::Nowhere
        );
        assert_eq!(
            IdBounds::of_box(&vp, [-1.0, -1.0, 9.0], [1.0, 1.0, 11.0]),
            IdBounds::Everywhere
        );
    }

    #[test]
    fn style_bounds_union_per_id_and_know_nothing_unfilled() {
        assert_eq!(StyleBounds::default().get(1), IdBounds::Everywhere);
        let mut b = StyleBounds::known();
        assert_eq!(b.get(3), IdBounds::Nowhere);
        b.add(2, IdBounds::Uv([0.1, 0.2, 0.3, 0.4]));
        b.add(2, IdBounds::Uv([0.0, 0.3, 0.2, 0.5]));
        b.add(2, IdBounds::Nowhere);
        assert_eq!(b.get(2), IdBounds::Uv([0.0, 0.2, 0.3, 0.5]));
        b.add(1, IdBounds::Uv([0.1, 0.2, 0.3, 0.4]));
        b.add(1, IdBounds::Everywhere);
        assert_eq!(b.get(1), IdBounds::Everywhere);
    }

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
        assert_eq!(std::mem::size_of::<MaskDrawUniform>(), 48);
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

    #[test]
    fn the_legend_names_each_id_and_its_hue() {
        assert_eq!(debug_color_name(NO_STYLE), "black");
        assert_eq!(debug_color_name(1), "blue");
        assert_eq!(debug_color_name(2), "lime");
        let mut ids = StyleIds::default();
        assert_eq!(legend_line(&ids), "style mask: no styled entities");
        ids.set(vec![
            ("pencil".into(), StyleVariant::plain("pencil")),
            ("ascii".into(), StyleVariant::plain("ascii")),
        ]);
        let rows = legend(&ids);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].id, rows[0].name.as_str()), (1, "ascii"));
        assert_eq!(rows[1].color, debug_color(2));
        assert_eq!(
            legend_line(&ids),
            "style mask: 1 ascii (blue), 2 pencil (lime)"
        );
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
        resolve(
            &world,
            &crate::stylize::StyleRegistry::new(),
            &PostProcessConfig::default(),
            &mut ids,
            &mut out,
        );
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
        resolve(
            world,
            &crate::stylize::StyleRegistry::new(),
            config,
            &mut ids,
            &mut out,
        );
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

    /// #376: `--style ascii:off` turns the style off everywhere, the props
    /// that switch it on for their own pixels included, and gives their
    /// pixels to nobody, so a whole-frame style draws over them as scene.
    /// Switching it back on (a later `--style ascii`) undoes it.
    #[test]
    fn style_off_everywhere_silences_per_entity_props() {
        let mut world = FlintWorld::new();
        with_component(&mut world, "barrel", "ascii_style", "grid = \"object\"");
        with_component(&mut world, "lamp", "ascii_style", "cell = 12");
        let ascii = crate::stylize::builtin("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        crate::stylize::apply_style_spec(
            &mut config,
            "pencil",
            &crate::stylize::StyleRegistry::new(),
        )
        .unwrap();
        let (ids, out) = resolved(&world, &config);
        let barrel = out[&world.get_id("barrel").unwrap()];
        assert!(ids.requests("ascii") && ascii.is_on(&config, &ids));
        let ascii_index = crate::stylize::StyleRegistry::new()
            .index_of(ascii.name)
            .unwrap() as u32;
        assert_eq!(
            crate::stylize::StyleRegistry::new().owner_table(&config, &ids)[barrel as usize],
            ascii_index
        );

        crate::stylize::apply_style_spec(
            &mut config,
            "ascii:off",
            &crate::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(config.styles.is_off_everywhere("ascii"));
        assert!(!ascii.is_on(&config, &ids), "the props still asked");
        assert!(!ascii.reads_mask(&config, &ids));
        assert_eq!(
            crate::stylize::StyleRegistry::new().owner_table(&config, &ids)[barrel as usize],
            crate::stylize::ownership::NONE,
            "pencil yields to a style that isn't drawn"
        );
        let pencil = crate::stylize::builtin("pencil").unwrap();
        assert!(crate::stylize::StyleRegistry::new()
            .foreign_ids(pencil, &config, &ids)
            .is_empty());

        crate::stylize::apply_style_spec(
            &mut config,
            "ascii",
            &crate::stylize::StyleRegistry::new(),
        )
        .unwrap();
        assert!(!config.styles.is_off_everywhere("ascii"));
        assert!(ascii.is_on(&config, &ids), "a later word switches it back");

        // A scene table's `enabled = false` is still only the table's
        let mut table_off = PostProcessConfig::default();
        table_off.styles.set_enabled("ascii", false);
        assert!(ascii.is_on(&table_off, &ids));
    }

    /// #400: an id whose every entity is at blend 0 keeps its number but
    /// asks for nothing, so its style and the mask stay off until one of
    /// them rises again.
    #[test]
    fn an_id_at_blend_zero_keeps_its_number_and_asks_for_nothing() {
        let mut world = FlintWorld::new();
        with_component(&mut world, "barrel", "ascii_style", "blend = 0.0");
        with_component(&mut world, "crate", "ascii_style", "blend = 0");
        with_component(&mut world, "lamp", "ascii_style", "cell = 12\nblend = 0.0");
        let config = PostProcessConfig::default();
        let resolve_at = |world: &FlintWorld, ids: &mut StyleIds| {
            let mut out = HashMap::new();
            resolve_blended(
                world,
                &crate::stylize::StyleRegistry::new(),
                &config,
                ids,
                &mut out,
                &mut HashMap::new(),
                &mut HashMap::new(),
            );
            out
        };
        let mut ids = StyleIds::default();
        let out = resolve_at(&world, &mut ids);
        let barrel = out[&world.get_id("barrel").unwrap()];
        let lamp = out[&world.get_id("lamp").unwrap()];
        assert_eq!(out.len(), 3, "faded entities stay in the mask's map");
        assert!(ids.is_faded(barrel) && ids.is_faded(lamp));
        assert!(!ids.requests("ascii"), "all at 0: the style is off");
        let ascii = crate::stylize::builtin("ascii").unwrap();
        assert!(!ascii.is_on(&config, &ids));
        assert!(!crate::stylize::StyleRegistry::new().any_uses_mask(&config, &ids));

        // One of the two plain ascii entities rises: its id is shown, the
        // lamp's own stays faded, and nothing is renumbered
        let before = ids.clone();
        with_component(&mut world, "crate", "ascii_style", "blend = 0.004");
        let out = resolve_at(&world, &mut ids);
        assert_eq!(out[&world.get_id("barrel").unwrap()], barrel);
        let same = |ids: &StyleIds| ids.iter().eq(before.iter());
        assert!(same(&ids), "a rise renumbered the ids");
        assert!(!ids.is_faded(barrel), "a root above 0 shows its id");
        assert!(ids.is_faded(lamp));
        assert!(ids.requests("ascii") && ascii.is_on(&config, &ids));

        // A blend under half a step of the mask's encoding is 0
        with_component(&mut world, "crate", "ascii_style", "blend = 0.000001");
        resolve_at(&world, &mut ids);
        assert!(!ids.requests("ascii"));
        assert!(same(&ids), "a fall renumbered the ids");

        // A `scope = "entities"` table is only the entities' preset: it
        // doesn't keep the style on (#278's demo), a screen table does
        let mut preset = PostProcessConfig::default();
        preset.styles.set_enabled("ascii", true);
        assert!(ascii.is_on(&preset, &ids), "a screen table draws the frame");
        preset.styles.set_option("ascii", "scope", "entities");
        assert!(!ascii.is_on(&preset, &ids));
        assert!(!crate::stylize::StyleRegistry::new().any_uses_mask(&preset, &ids));
        with_component(&mut world, "crate", "ascii_style", "blend = 1.0");
        resolve_at(&world, &mut ids);
        assert!(ascii.is_on(&preset, &ids));

        // Under a whole-frame pencil a faded prop is still ASCII's (#384's
        // ownership): pencil leaves it as it came, a hole, and draws the
        // mask for it, though ASCII itself is off
        with_component(&mut world, "crate", "ascii_style", "blend = 0.0");
        resolve_at(&world, &mut ids);
        let mut pencil_frame = PostProcessConfig::default();
        pencil_frame.styles.set_enabled("pencil", true);
        let pencil = crate::stylize::builtin("pencil").unwrap();
        assert!(!ascii.is_on(&pencil_frame, &ids));
        assert_eq!(
            crate::stylize::StyleRegistry::new().foreign_ids(pencil, &pencil_frame, &ids),
            vec![barrel, lamp]
        );
        assert!(crate::stylize::StyleRegistry::new().any_uses_mask(&pencil_frame, &ids));
    }

    #[test]
    fn an_entity_falls_back_on_the_scenes_settings_then_the_defaults() {
        let ascii = crate::stylize::builtin("ascii").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_param("ascii", "cell", 10.0);
        config.styles.set_option("ascii", "color", "tint");
        let mut v = StyleVariant::plain("ascii");
        v.params.insert("cell".into(), 16.0);
        assert_eq!(v.param(ascii, &config, "cell"), 16.0);
        assert_eq!(v.option(ascii, &config, "color"), "tint");
        assert_eq!(v.param(ascii, &config, "ink"), 1.0);
    }

    /// `grid = "local"` (#544): one variant per root, carrying the root's
    /// world matrix, whose inverse rows take a point on it back to where it
    /// sits in the root's own space wherever the root has moved and turned.
    #[test]
    fn a_local_grid_carries_its_roots_frame_and_its_inverse() {
        let mut world = FlintWorld::new();
        with_component(&mut world, "cask", "pencil_style", "grid = \"local\"");
        with_component(
            &mut world,
            "cask",
            "transform",
            "position = [1.0, 2.0, 3.0]\nrotation = [0.0, 90.0, 30.0]\nscale = [2.0, 1.0, 0.5]",
        );
        let hoop = world.spawn("hoop").unwrap();
        world.set_parent_by_name("hoop", "cask").unwrap();
        let (ids, out) = resolved(&world, &PostProcessConfig::default());
        let cask_id = world.get_id("cask").unwrap();
        let id = out[&cask_id];
        assert_eq!(out[&hoop], id, "a child rides on its root's frame");
        assert_eq!(ids.name(id), Some("pencil@cask{grid=local}"));
        let v = ids.variant(id).unwrap();
        assert_eq!(v.anchor, None, "no screen anchor");
        let f = v.local.expect("a local frame");
        assert_eq!(f.root, cask_id);
        let m = world.get_world_matrix(cask_id).unwrap();
        assert_eq!(f.matrix, m);

        // A local point goes out to the world and back
        let rows = f.inverse_rows().unwrap();
        let local = [0.3, -0.7, 1.9];
        let w: Vec<f32> = (0..3)
            .map(|r| m[0][r] * local[0] + m[1][r] * local[1] + m[2][r] * local[2] + m[3][r])
            .collect();
        for (i, row) in rows.iter().enumerate() {
            let back = row[0] * w[0] + row[1] * w[1] + row[2] * w[2] + row[3];
            assert!((back - local[i]).abs() < 1e-5, "axis {i}: {back}");
        }

        // A root scaled to nothing has no inverse
        let flat = LocalFrame {
            root: cask_id,
            matrix: [
                [0.0; 4],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        assert_eq!(flat.inverse_rows(), None);

        // A motion hold's matrix replaces the live one, names untouched
        let mut held = ids.clone();
        let kept = flint_core::Transform::default().to_matrix();
        held.hold_local_frames(|root, _| {
            assert_eq!(root, cask_id);
            kept
        });
        assert_eq!(held.variant(id).unwrap().local.unwrap().matrix, kept);
        assert_eq!(held.name(id), ids.name(id));
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
        let ascii = crate::stylize::builtin("ascii").unwrap();
        let invert = crate::stylize::builtin("invert").unwrap();
        let config = PostProcessConfig::default();
        let mut world = FlintWorld::new();
        with_component(&mut world, "bench", "style_mask", "style = \"ascii\"");
        with_component(&mut world, "crate", "style_mask", "style = \"invert\"");
        let (ids, _) = resolved(&world, &config);
        assert!(ascii.is_on(&config, &ids));
        assert!(ascii.reads_mask(&config, &ids));
        assert!(!invert.is_on(&config, &ids), "invert isn't per-entity");
        assert!(crate::stylize::StyleRegistry::new().any_uses_mask(&config, &ids));
        // Nobody asking: off, and the mask isn't drawn
        let (none, _) = resolved(&FlintWorld::new(), &config);
        assert!(!ascii.is_on(&config, &none));
        assert!(!crate::stylize::StyleRegistry::new().any_uses_mask(&config, &none));
    }

    /// #350: whole-frame ASCII asks for the mask only while it has edges
    /// to draw and something styled to outline, or another style's entity
    /// to pass through.
    #[test]
    fn whole_frame_ascii_asks_for_the_mask_only_when_it_reads_it() {
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("ascii", true);
        let empty = FlintWorld::new();
        let (none, _) = resolved(&empty, &config);
        assert!(
            !crate::stylize::StyleRegistry::new().any_uses_mask(&config, &none),
            "nothing styled"
        );

        let mut world = FlintWorld::new();
        with_component(&mut world, "crate", "style_mask", "style = \"invert\"");
        let (other, _) = resolved(&world, &config);
        assert!(
            crate::stylize::StyleRegistry::new().any_uses_mask(&config, &other),
            "edges outline it"
        );
        config.styles.set_param("ascii", "edges", 0.0);
        assert!(
            crate::stylize::StyleRegistry::new().any_uses_mask(&config, &other),
            "another style's entity still passes through"
        );

        let mut world = FlintWorld::new();
        with_component(&mut world, "bench", "ascii_style", "ramp = \"blocks\"");
        let (ours, _) = resolved(&world, &config);
        assert!(
            crate::stylize::StyleRegistry::new().any_uses_mask(&config, &ours),
            "per-entity rows are found by mask id"
        );

        // ASCII's edges off in a scene with no style_mask: no mask
        let (none, _) = resolved(&empty, &config);
        assert!(!crate::stylize::StyleRegistry::new().any_uses_mask(&config, &none));
    }

    /// #384: each pixel is one style's. A whole-frame style's foreign ids
    /// are the entities another running style owns: not its own, not an
    /// opt-out, not a style that isn't on.
    #[test]
    fn a_whole_frame_style_leaves_another_styles_entities() {
        let ascii = crate::stylize::builtin("ascii").unwrap();
        let pencil = crate::stylize::builtin("pencil").unwrap();
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("ascii", true);
        config.styles.set_param("ascii", "edges", 0.0);
        let mut world = FlintWorld::new();
        with_component(&mut world, "barrel", "style_mask", "style = \"ascii\"");
        with_component(&mut world, "cask", "style_mask", "style = \"pencil\"");
        with_component(&mut world, "crate", "pencil_style", "spacing = 6.0");
        with_component(&mut world, "tag", "pencil_style", "enabled = false");
        with_component(&mut world, "box", "style_mask", "style = \"invert\"");
        let (ids, out) = resolved(&world, &config);
        let id = |name: &str| out[&world.get_id(name).unwrap()];

        let foreign = crate::stylize::StyleRegistry::new().foreign_ids(ascii, &config, &ids);
        assert!(foreign.contains(&id("cask")), "a style_mask naming pencil");
        assert!(foreign.contains(&id("crate")), "a pencil_style");
        assert!(!foreign.contains(&id("barrel")), "its own");
        assert!(!foreign.contains(&id("tag")), "an opt-out is nobody's");
        assert!(!foreign.contains(&id("box")), "invert isn't on");
        // And the other way round, pencil leaves ASCII's barrel
        assert_eq!(
            crate::stylize::StyleRegistry::new().foreign_ids(pencil, &config, &ids),
            vec![id("barrel")]
        );

        let table = crate::stylize::StyleRegistry::new().owner_table(&config, &ids);
        assert_eq!(table[0], crate::stylize::ownership::NONE, "unstyled");
        assert_eq!(table[id("tag") as usize], crate::stylize::ownership::NONE);
        assert_eq!(table[id("cask") as usize], table[id("crate") as usize]);
        assert_ne!(table[id("cask") as usize], table[id("barrel") as usize]);

        // A whole-frame invert now owns the box: ASCII leaves it too
        config.styles.set_enabled("invert", true);
        assert!(crate::stylize::StyleRegistry::new()
            .foreign_ids(ascii, &config, &ids)
            .contains(&id("box")));

        // Neither Kuwahara nor invert reads the mask, but Kuwahara yields
        // to invert's box, so the mask is drawn for it
        let mut config = PostProcessConfig::default();
        config.styles.set_enabled("kuwahara", true);
        config.styles.set_enabled("invert", true);
        let mut world = FlintWorld::new();
        with_component(&mut world, "box", "style_mask", "style = \"invert\"");
        let (only_box, _) = resolved(&world, &config);
        assert!(crate::stylize::StyleRegistry::new().any_uses_mask(&config, &only_box));
        config.styles.set_enabled("kuwahara", false);
        assert!(
            !crate::stylize::StyleRegistry::new().any_uses_mask(&config, &only_box),
            "invert alone has nobody to yield to"
        );
    }

    /// #384: the views that look at the whole picture on purpose don't
    /// yield; every other built-in does, with no code of its own.
    #[test]
    fn only_debug_views_and_the_glow_draw_over_entities() {
        let over: Vec<&str> = crate::stylize::builtins()
            .iter()
            .filter(|d| !d.yields)
            .map(|d| d.name)
            .collect();
        // The reality styles (#746) were the composite's: they draw over
        // every pixel as it did, so a prop in a tear reads their picture
        assert_eq!(
            over,
            [
                "display_bloom",
                "mask_view",
                "edges_view",
                "matrix",
                "blood",
                "drunk",
                "tron",
                "underwater"
            ]
        );
    }

    /// #397: a root whose blend follows the tear is faded while the frame
    /// draws no tear (its style off, as at blend 0), shown while one is on,
    /// and its entities are named as torn for the mask.
    #[test]
    fn a_prop_that_follows_the_tear_shows_only_while_the_frame_tears() {
        let mut world = FlintWorld::new();
        with_component(
            &mut world,
            "barrel",
            "ascii_style",
            "blend_source = \"tear\"",
        );
        with_component(&mut world, "bench", "ascii_style", "ramp = \"blocks\"");
        let resolve_at = |world: &FlintWorld, config: &PostProcessConfig, ids: &mut StyleIds| {
            let (mut out, mut torn) = (HashMap::new(), HashMap::new());
            resolve_blended(
                world,
                &crate::stylize::StyleRegistry::new(),
                config,
                ids,
                &mut out,
                &mut HashMap::new(),
                &mut torn,
            );
            (out, torn)
        };
        let barrel_e = world.get_id("barrel").unwrap();
        let mut calm = PostProcessConfig::default();
        let mut ids = StyleIds::default();
        let (out, torn) = resolve_at(&world, &calm, &mut ids);
        let (barrel, bench) = (out[&barrel_e], out[&world.get_id("bench").unwrap()]);
        assert_ne!(barrel, bench, "a ramp of its own is a variant of its own");
        assert_eq!(
            torn.into_iter().collect::<Vec<_>>(),
            [(barrel_e, BlendSource::Tear)]
        );
        assert!(ids.is_faded(barrel), "no tear: the torn prop is at blend 0");
        assert!(!ids.is_faded(bench), "a prop at its own blend doesn't care");

        // A tear: shown, without resolving the world again (a script sets
        // it each frame, after the world was read)
        let mut tearing = calm.clone();
        tearing.tear = 0.5;
        assert!(crate::postprocess::tear_active(&tearing));
        assert!(ids.set_tear(true));
        assert!(!ids.is_faded(barrel));
        assert!(!ids.set_tear(true), "no change, no work");
        resolve_at(&world, &tearing, &mut ids);
        assert!(!ids.is_faded(barrel), "resolving under the tear agrees");

        // Any tear above the cut-off tears; 0, the cut-off itself and a
        // frame with post-processing off don't
        for amount in [0.01, 0.5, 1.0] {
            tearing.tear = amount;
            assert!(crate::postprocess::tear_active(&tearing), "{amount}");
        }
        for amount in [0.0, 0.001] {
            calm.tear = amount;
            assert!(!crate::postprocess::tear_active(&calm), "{amount}");
        }
        let mut off = tearing.clone();
        off.enabled = false;
        assert!(!crate::postprocess::tear_active(&off));
        assert!(ids.set_tear(false));
        assert!(ids.is_faded(barrel));

        // A torn root at blend 0 stays faded even under a tear
        with_component(
            &mut world,
            "barrel",
            "ascii_style",
            "blend_source = \"tear\"\nblend = 0.0",
        );
        tearing.tear = 0.5;
        let mut ids = StyleIds::default();
        let (out, _) = resolve_at(&world, &tearing, &mut ids);
        assert!(ids.is_faded(out[&barrel_e]));
    }

    /// #397: `blend_source` is the root's, from its style component, else
    /// its `style_mask`; anything but "tear" is its own blend.
    #[test]
    fn blend_source_comes_from_the_style_component_then_the_mask() {
        let mut world = FlintWorld::new();
        with_component(
            &mut world,
            "a",
            "style_mask",
            "style = \"ascii\"\nblend_source = \"tear\"",
        );
        with_component(&mut world, "b", "ascii_style", "blend_source = \"own\"");
        with_component(
            &mut world,
            "b",
            "style_mask",
            "style = \"ascii\"\nblend_source = \"tear\"",
        );
        with_component(
            &mut world,
            "c",
            "ascii_style",
            "blend_source = \"sideways\"",
        );
        with_component(&mut world, "d", "pencil_style", "blend_source = \"tear\"");
        with_component(&mut world, "e", "ascii_style", "blend_source = \"field\"");
        let follows = |n: &str| {
            blend_source(
                &world,
                &crate::stylize::StyleRegistry::new(),
                world.get_id(n).unwrap(),
            )
        };
        assert_eq!(follows("a"), BlendSource::Tear);
        assert_eq!(
            follows("b"),
            BlendSource::Own,
            "the style component's own wins"
        );
        assert_eq!(
            follows("c"),
            BlendSource::Own,
            "an unknown source is the entity's own"
        );
        assert_eq!(follows("d"), BlendSource::Tear);
        assert_eq!(follows("e"), BlendSource::Field);
        for s in BlendSource::ALL {
            assert_eq!(BlendSource::from_name(s.name()), Some(s));
        }
    }

    /// #859: a root whose blend follows the screen field is faded while the
    /// frame has no field (its style off, as at blend 0) and shown while it
    /// has one, whatever the tear does; its entities are named for the mask.
    #[test]
    fn a_prop_that_follows_the_field_shows_only_while_the_frame_has_one() {
        let mut world = FlintWorld::new();
        let barrel_e = world.spawn("barrel").unwrap();
        world.spawn("lid").unwrap();
        world
            .set_parent(world.get_id("lid").unwrap(), barrel_e)
            .unwrap();
        world.spawn("bench").unwrap();
        with_component(
            &mut world,
            "barrel",
            "ascii_style",
            "blend_source = \"field\"",
        );
        with_component(&mut world, "bench", "ascii_style", "ramp = \"blocks\"");
        let config = PostProcessConfig::default();
        let mut ids = StyleIds::default();
        let (mut out, mut sources) = (HashMap::new(), HashMap::new());
        resolve_blended(
            &world,
            &crate::stylize::StyleRegistry::new(),
            &config,
            &mut ids,
            &mut out,
            &mut HashMap::new(),
            &mut sources,
        );
        let barrel = out[&barrel_e];
        let lid = world.get_id("lid").unwrap();
        assert_eq!(sources.get(&lid), Some(&BlendSource::Field), "inherited");
        assert_eq!(sources.len(), 2, "the bench follows its own blend");
        assert!(ids.is_faded(barrel), "no field: the prop is at blend 0");
        let bench = out[&world.get_id("bench").unwrap()];
        assert!(!ids.is_faded(bench), "a prop at its own blend doesn't care");
        assert!(ids.set_field(true));
        assert!(!ids.is_faded(barrel), "a field: shown");
        assert!(!ids.set_field(true), "no change, no work");
        assert!(ids.set_tear(true));
        assert!(!ids.is_faded(barrel), "the tear changes nothing for it");
        assert!(ids.set_field(false));
        assert!(ids.is_faded(barrel), "the tear doesn't show it");
    }

    /// #859: the field's uniform is the shader's `Field` byte for byte, and
    /// each shape packs its lengths where `field_at` reads them.
    #[test]
    fn the_field_uniform_is_the_shaders_field() {
        let module = naga::front::wgsl::parse_str(STYLE_MASK_WGSL).expect("parses");
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { span, .. } if ty.name.as_deref() == Some("Field") => {
                    Some(*span)
                }
                _ => None,
            })
            .expect("Field in the shader");
        assert_eq!(span as usize, std::mem::size_of::<FieldUniform>());
        let mut field = StyleField::default();
        field.push(FieldShape::Ring {
            center: [0.25, 0.75],
            radius: 0.3,
            width: 0.1,
            soft: 0.02,
            strength: 0.5,
        });
        field.push(FieldShape::Cone {
            apex: [0.5, 1.0],
            direction: 0.1,
            spread: 0.4,
            length: 0.9,
            soft: 0.05,
            strength: 1.0,
        });
        let u = FieldUniform::of(&field);
        assert_eq!(u.count, 2);
        assert_eq!(
            u.shapes[0],
            [[0.25, 0.75, 0.3, 0.05], [0.0, 0.02, 1.0, 0.5]]
        );
        assert_eq!(u.shapes[1], [[0.5, 1.0, 0.9, 0.1], [0.4, 0.05, 2.0, 1.0]]);
        assert_eq!(u.shapes[2], [[0.0; 4]; 2], "the rest are zero");
    }

    /// The woozy module (#737) validates, has both woozy vertex stages and
    /// both fragment stages, and its transform is the 256-byte arena slot
    /// the drift rides in (the plain one stops at the 208 bytes before it).
    #[test]
    fn the_woozy_mask_module_validates_and_reads_the_whole_slot() {
        let size_of_transform = |src: &str| {
            let module = naga::front::wgsl::parse_str(src).expect("parses");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("validates");
            let entries: Vec<_> = module.entry_points.iter().map(|e| e.name.clone()).collect();
            let span = module
                .types
                .iter()
                .find_map(|(_, ty)| match &ty.inner {
                    naga::TypeInner::Struct { span, .. }
                        if ty.name.as_deref() == Some("TransformUniforms") =>
                    {
                        Some(*span)
                    }
                    _ => None,
                })
                .expect("TransformUniforms");
            (span, entries)
        };
        let (plain, plain_entries) = size_of_transform(STYLE_MASK_WGSL);
        let (woozy, entries) = size_of_transform(&woozy_mask_source());
        assert_eq!(plain, 208);
        assert_eq!(woozy, 256, "the drift is the slot's last 48 bytes");
        for e in ["vs_static_wooze", "vs_skinned_wooze", "fs_main", "fs_msaa"] {
            assert!(entries.iter().any(|n| n == e), "{e} in {entries:?}");
        }
        assert!(!plain_entries.iter().any(|n| n.ends_with("_wooze")));
    }

    /// The billboard module (#1011) validates with the billboard's own
    /// stages and none of a mesh's, and nothing in it is bound twice: the
    /// sprite's groups 0 and 2 take the slots the mesh blocks left. The
    /// plain module keeps its mesh stages.
    #[test]
    fn the_billboard_mask_module_validates_without_the_mesh_blocks() {
        let src = billboard_mask_source();
        let module = naga::front::wgsl::parse_str(&src).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("validates");
        let entries: Vec<_> = module.entry_points.iter().map(|e| e.name.clone()).collect();
        for e in ["vs_billboard", "fs_billboard", "fs_billboard_msaa"] {
            assert!(entries.iter().any(|n| n == e), "{e} in {entries:?}");
        }
        for e in ["vs_static", "vs_skinned"] {
            assert!(!entries.iter().any(|n| n == e), "{e} left in {entries:?}");
        }
        assert!(
            !src.contains("//FLINT_MESH {") && !src.contains("//FLINT_MESH }"),
            "every block is cut"
        );
        assert!(STYLE_MASK_WGSL.matches("//FLINT_MESH {").count() == 3);
        let mut bindings: Vec<_> = module
            .global_variables
            .iter()
            .filter_map(|(_, g)| g.binding.as_ref().map(|b| (b.group, b.binding)))
            .collect();
        let n = bindings.len();
        bindings.sort_unstable();
        bindings.dedup();
        assert_eq!(bindings.len(), n, "a binding declared twice: {bindings:?}");
        for b in [(0, 0), (0, 1), (1, 0), (2, 0), (2, 1)] {
            assert!(bindings.contains(&b), "{b:?} in {bindings:?}");
        }
        let plain = naga::front::wgsl::parse_str(STYLE_MASK_WGSL).expect("parses");
        assert!(plain.entry_points.iter().any(|e| e.name == "vs_static"));
        assert!(!plain
            .entry_points
            .iter()
            .any(|e| e.name.contains("billboard")));
    }

    /// A billboard's box holds its quad's corners under any camera basis
    /// (#1011), at the anchors a sprite takes.
    #[test]
    fn a_sprites_bounds_hold_its_quad_however_the_camera_turns() {
        use crate::billboard_pipeline::{sprite_bounds, SpriteInstance};
        for anchor_y in [0.0, 0.5, 1.0, -0.25] {
            let sprite = SpriteInstance {
                world_pos: [1.0, 2.0, -3.0],
                width: 0.8,
                height: 1.6,
                frame: 0,
                frames_x: 1,
                frames_y: 1,
                anchor_y,
                fullbright: 1,
                selection_highlight: 0,
                _pad1: 0.0,
            };
            let (min, max) = sprite_bounds(&sprite);
            for turn in 0..24 {
                let (a, b) = (turn as f32 * 0.27, turn as f32 * 0.61 - 1.0);
                // An orthonormal right and up from two angles
                let right = [a.cos(), 0.0, a.sin()];
                let up = [-a.sin() * b.sin(), b.cos(), a.cos() * b.sin()];
                for (x, y) in [(-0.5, 0.0), (0.5, 0.0), (-0.5, 1.0), (0.5, 1.0)] {
                    for k in 0..3 {
                        let c = sprite.world_pos[k]
                            + right[k] * x * sprite.width
                            + up[k] * (y - anchor_y) * sprite.height;
                        assert!(
                            c >= min[k] - 1e-5 && c <= max[k] + 1e-5,
                            "anchor {anchor_y}, turn {turn}: corner {k} {c} outside {min:?}..{max:?}"
                        );
                    }
                }
            }
        }
    }

    /// The Rust uniform is laid out as the WGSL `MaskDraw` reads it (#397
    /// grew it from 16 to 48 bytes), and the composed shader validates.
    #[test]
    fn the_mask_uniform_matches_the_wgsl_and_the_shader_validates() {
        let module = naga::front::wgsl::parse_str(STYLE_MASK_WGSL).expect("style mask parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("style mask validates");
        let members = module
            .types
            .iter()
            .find_map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { members, span }
                    if ty.name.as_deref() == Some("MaskDraw") =>
                {
                    Some((members.clone(), *span))
                }
                _ => None,
            })
            .expect("MaskDraw in the shader");
        use std::mem::offset_of;
        let rust = [
            ("id", offset_of!(MaskDrawUniform, id)),
            (
                "slope_tolerance",
                offset_of!(MaskDrawUniform, slope_tolerance),
            ),
            ("blend", offset_of!(MaskDrawUniform, blend)),
            ("source", offset_of!(MaskDrawUniform, source)),
            ("tear", offset_of!(MaskDrawUniform, tear)),
            ("size", offset_of!(MaskDrawUniform, size)),
            ("_pad", offset_of!(MaskDrawUniform, _pad)),
        ];
        let wgsl: Vec<_> = members
            .0
            .iter()
            .map(|m| (m.name.as_deref().unwrap_or(""), m.offset as usize))
            .collect();
        assert_eq!(wgsl, rust);
        assert_eq!(members.1 as usize, std::mem::size_of::<MaskDrawUniform>());
    }
}
