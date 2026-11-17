//! The frame's draws as one list of items (ADR 0118 decision 4, #737).
//!
//! Every pass used to walk its own choice of nine draw lists, and a list a
//! pass forgot was a silent fault that only a styled scene showed (the
//! style veil never saw echoes). Now extraction fills one [`DrawList`] of
//! [`DrawItem`]s, and each pass is a filter over it and a selector that
//! matches on [`Draw`] with no wildcard arm: a new kind of draw is one new
//! variant, and the compiler asks every pass what to do with it.
//!
//! Items hold cache handles (`Arc`s of the bind cache's buffers and bind
//! groups), never buffers of their own, and the list keeps its allocation
//! from frame to frame. It is held in [`Layer`] order, which is the order
//! the main pass and the style mask draw in, so a pass that walks the whole
//! list walks it in that order and the main pass draws a layer at a time.

use super::{DrawCall, EchoDrawCall, SkinnedDrawCall};
use crate::billboard_pipeline::BillboardDrawCall;
use crate::pipeline::BlendMode;
use crate::wooze_pipeline::ShallowWoozeUniforms;
use flint_core::EntityId;
use std::collections::HashMap;

/// What an item draws: its kind, and what only that kind carries.
pub(super) enum Draw {
    /// A model's sub-mesh or a procedural box, opaque or blended, or a
    /// wireframe box (`DrawCall::is_wireframe`).
    Static(DrawCall),
    /// A skinned mesh's sub-mesh, posed by its bone bind group.
    Skinned(SkinnedDrawCall),
    /// One part of one echo of a living prop (#228): its entity's skinned
    /// mesh under the echo's bones, translucent, writing no depth.
    Echo(EchoDrawCall),
    /// A camera-facing sprite, cut out at half alpha, writing depth.
    Billboard(BillboardDrawCall),
    /// Chunk `n` of the bound terrain (`TerrainRenderer::draws`), which
    /// `bind_scene` loads once a scene rather than every extraction.
    Terrain(u32),
    /// Draw `n` of the frame's particle upload (`particle_draws`).
    Particles(u32),
}

impl Draw {
    /// The kind's name, for GPU debuggers' draw markers (#1083).
    pub fn kind(&self) -> &'static str {
        match self {
            Draw::Static(d) if d.is_wireframe => "Wire box",
            Draw::Static(_) => "Static",
            Draw::Skinned(_) => "Skinned",
            Draw::Echo(_) => "Echo",
            Draw::Billboard(_) => "Billboard",
            Draw::Terrain(_) => "Terrain chunk",
            Draw::Particles(_) => "Particles",
        }
    }
}

/// How an item's colour goes onto what's behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Blend {
    /// Replaces it, and writes depth. Billboards are opaque: their alpha
    /// only cuts them out.
    Opaque,
    Alpha,
    /// A particle's colour already multiplied by its alpha.
    Premultiplied,
    Additive,
    Multiply,
    /// May only add light (an echo's `echo_blend = "lighten"`).
    Lighten,
}

impl Blend {
    /// A mesh's: opaque unless it's transparent, then its material's mode.
    pub fn of_mesh(transparent: bool, mode: BlendMode) -> Self {
        match (transparent, mode) {
            (false, _) => Blend::Opaque,
            (true, BlendMode::Alpha) => Blend::Alpha,
            (true, BlendMode::Additive) => Blend::Additive,
            (true, BlendMode::Multiply) => Blend::Multiply,
        }
    }

    /// A particle emitter's.
    pub fn of_particles(mode: flint_particles::ParticleBlendMode) -> Self {
        use flint_particles::ParticleBlendMode as P;
        match mode {
            P::Alpha => Blend::Alpha,
            P::Premultiplied => Blend::Premultiplied,
            P::Additive => Blend::Additive,
            P::Multiply => Blend::Multiply,
        }
    }

    /// Whether it lets what's behind it show: what the style veil takes.
    pub fn blends(self) -> bool {
        self != Blend::Opaque
    }

    /// The mesh pipelines' mode for a blended static or skinned draw.
    pub fn mesh_mode(self) -> BlendMode {
        match self {
            Blend::Additive => BlendMode::Additive,
            Blend::Multiply => BlendMode::Multiply,
            Blend::Opaque | Blend::Alpha | Blend::Premultiplied | Blend::Lighten => {
                BlendMode::Alpha
            }
        }
    }
}

/// Which vertex stage an item's mesh goes through.
#[derive(Clone, Copy, Debug)]
pub(super) enum VertexStage {
    Plain,
    /// Shallow wooze (#215): its vertices drift by this frame's field, so
    /// every pass that draws its geometry needs its woozy pipeline.
    Wooze(ShallowWoozeUniforms),
}

/// Why an item is left out of this camera's frame (set by `render_to`'s
/// culls, cleared at the start of each `render_to`).
#[derive(Clone, Copy, Debug, Default)]
struct Cull {
    /// Not its level of detail from here, or past its draw distance (#56):
    /// out of the shadow pass too.
    lod: bool,
    /// Outside the camera's frustum: still in the shadow pass (ADR 0074).
    camera: bool,
    /// Its entity is hidden by a replay's controls (#1089): out of every
    /// pass, the shadows too.
    hidden: bool,
    /// Past a replay's scene draw limit (#1089): out of the scene pass and
    /// what follows it, still in the shadows.
    past_limit: bool,
    /// Past a replay's draw limit on another pass (#1204): a shadow layer,
    /// the style mask or the albedo, whichever the controls step. Only
    /// that pass's walk reads it ([`DrawList::within`]).
    past_pass_limit: bool,
}

/// One draw of the frame.
pub(super) struct DrawItem {
    /// Who it draws: the style mask, the veil, the selection outline and
    /// the stats ask. The terrain's is the one `bind_scene` named.
    pub entity: Option<EntityId>,
    pub blend: Blend,
    pub vertex: VertexStage,
    /// The entity's `material.cast_shadows` (ADR 0100). What casting means
    /// for a kind is the shadow pass's selector's to say (a translucent
    /// skinned mesh casts only when it is translucent for its soft edge).
    pub casts_shadow: bool,
    cull: Cull,
    pub draw: Draw,
}

impl DrawItem {
    pub fn new(entity: Option<EntityId>, blend: Blend, casts_shadow: bool, draw: Draw) -> Self {
        Self {
            entity,
            blend,
            vertex: VertexStage::Plain,
            casts_shadow,
            cull: Cull::default(),
            draw,
        }
    }

    /// The layer it is kept and drawn in.
    pub fn layer(&self) -> Layer {
        let opaque = self.blend == Blend::Opaque;
        match &self.draw {
            Draw::Terrain(_) => Layer::Terrain,
            Draw::Static(_) if opaque => Layer::OpaqueStatic,
            Draw::Static(_) => Layer::BlendedStatic,
            Draw::Skinned(_) if opaque => Layer::OpaqueSkinned,
            Draw::Skinned(_) => Layer::BlendedSkinned,
            Draw::Billboard(_) => Layer::Billboard,
            Draw::Echo(_) => Layer::Echo,
            Draw::Particles(_) => Layer::Particles,
        }
    }

    /// Its drift, when it goes through the woozy vertex stage.
    pub fn wooze(&self) -> Option<&ShallowWoozeUniforms> {
        match &self.vertex {
            VertexStage::Wooze(w) => Some(w),
            VertexStage::Plain => None,
        }
    }

    /// Whether this camera draws it: neither culled for its level of
    /// detail nor outside the frustum.
    pub fn drawn(&self) -> bool {
        !self.cull.lod && !self.cull.camera && !self.cull.hidden && !self.cull.past_limit
    }

    /// Whether it is past its level of detail or draw distance from here,
    /// which leaves it out of the shadow pass too.
    pub fn lod_culled(&self) -> bool {
        self.cull.lod
    }

    /// Whether the shadow pass leaves it out: past its level of detail, or
    /// hidden by a replay.
    pub fn out_of_shadows(&self) -> bool {
        self.cull.lod || self.cull.hidden
    }

    /// Whether a replay's controls (#1089) left it out, and why.
    pub fn replay_cull(&self) -> Option<&'static str> {
        if self.cull.hidden {
            Some("hidden")
        } else if self.cull.past_limit {
            Some("past the draw limit")
        } else {
            None
        }
    }

    pub fn set_lod_culled(&mut self, culled: bool) {
        self.cull.lod = culled;
    }

    pub fn set_camera_culled(&mut self, culled: bool) {
        self.cull.camera = culled;
    }
}

/// The groups the list is kept in, in the order the main pass draws them
/// (the main pass draws other things between them: grass and the ocean
/// after the terrain, the grid after the billboards, 2D sprites before the
/// blended statics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layer {
    Terrain,
    OpaqueStatic,
    OpaqueSkinned,
    Billboard,
    /// Sorted back to front for each camera.
    BlendedStatic,
    /// Sorted back to front for each camera.
    BlendedSkinned,
    /// Sorted back to front by their entity's origin for each camera; an
    /// entity's echoes keep their order (farthest from now first).
    Echo,
    /// In the order the particle upload sorted them.
    Particles,
}

impl Layer {
    /// Its name as a debug group in a GPU debugger (#1083).
    pub fn label(self) -> &'static str {
        match self {
            Layer::Terrain => "Terrain",
            Layer::OpaqueStatic => "Opaque static",
            Layer::OpaqueSkinned => "Opaque skinned",
            Layer::Billboard => "Billboards",
            Layer::BlendedStatic => "Blended static",
            Layer::BlendedSkinned => "Blended skinned",
            Layer::Echo => "Echoes",
            Layer::Particles => "Particles",
        }
    }

    pub const COUNT: usize = 8;
    pub const ALL: [Layer; Layer::COUNT] = [
        Layer::Terrain,
        Layer::OpaqueStatic,
        Layer::OpaqueSkinned,
        Layer::Billboard,
        Layer::BlendedStatic,
        Layer::BlendedSkinned,
        Layer::Echo,
        Layer::Particles,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// Whether extraction fills it. Terrain items follow the bound
    /// terrain and particle items the particle upload, so a frame's
    /// extraction leaves those two be.
    fn extracted(self) -> bool {
        match self {
            Layer::Terrain | Layer::Particles => false,
            Layer::OpaqueStatic
            | Layer::OpaqueSkinned
            | Layer::Billboard
            | Layer::BlendedStatic
            | Layer::BlendedSkinned
            | Layer::Echo => true,
        }
    }
}

/// The frame's draws, a vector a layer, each kept across frames.
#[derive(Default)]
pub(super) struct DrawList {
    layers: [Vec<DrawItem>; Layer::COUNT],
    /// Each drawn entity's name, for the draw markers GPU debuggers show
    /// (#1083). Filled only while they are on, and kept, so a frame copies
    /// only a name that is new or changed (ids are never reused).
    names: HashMap<EntityId, String>,
    /// A frame debugger's controls this frame (#1089), and what they cut
    /// of the scene pass's draws that aren't items.
    controls: Option<Controls>,
}

/// [`DrawControls`](super::DrawControls) as the list holds them, with the
/// scene pass's extras the limit cut.
#[derive(Default)]
struct Controls {
    hidden: std::collections::HashSet<EntityId>,
    limit: Option<super::DrawLimit>,
    /// The grass is past the limit.
    grass_cut: bool,
    /// How many 2D sprite batches are within it.
    sprites: usize,
}

impl DrawList {
    /// Take the frame's controls (`FrameInputs::draw_controls`). `None`,
    /// every frame but a replay's, keeps nothing.
    pub fn set_controls(&mut self, controls: Option<&super::DrawControls>) {
        self.controls = controls.map(|c| Controls {
            hidden: c.hidden.iter().copied().collect(),
            limit: c.limit,
            grass_cut: false,
            sprites: usize::MAX,
        });
    }

    /// Hide the items of the entities the controls hide, before the shadow
    /// pass (`render_to`'s culls, after the LOD cull).
    pub fn cull_hidden(&mut self) {
        let Some(c) = &self.controls else { return };
        if c.hidden.is_empty() {
            return;
        }
        for layer in &mut self.layers {
            for item in layer.iter_mut() {
                item.cull.hidden = item.entity.is_some_and(|e| c.hidden.contains(&e));
            }
        }
    }

    /// Cut the scene pass after the controls' draw limit, once the camera's
    /// culls are made: `counts(item)` is whether the scene pass draws it,
    /// `grass` whether it draws grass (after the terrain), `sprites` how many
    /// 2D batches (after the billboards), as the frame's description counts
    /// them. Returns nothing; the cut is kept for [`grass_cut`](Self::grass_cut)
    /// and [`sprites_drawn`](Self::sprites_drawn).
    pub fn cut_to_limit(
        &mut self,
        counts: impl Fn(&DrawItem) -> bool,
        grass: bool,
        sprites: usize,
    ) {
        let Some(limit) = self.limit_of(super::SteppedPass::Scene) else {
            return;
        };
        let mut n = 0usize;
        let (mut grass_cut, mut sprites_in) = (false, sprites);
        for layer in Layer::ALL {
            for item in self.layers[layer.index()].iter_mut() {
                if item.drawn() && counts(item) {
                    n += 1;
                    item.cull.past_limit = n > limit;
                }
            }
            match layer {
                Layer::Terrain if grass => {
                    n += 1;
                    grass_cut = n > limit;
                }
                Layer::Billboard => {
                    sprites_in = limit.saturating_sub(n).min(sprites);
                    n += sprites;
                }
                Layer::Terrain
                | Layer::OpaqueStatic
                | Layer::OpaqueSkinned
                | Layer::BlendedStatic
                | Layer::BlendedSkinned
                | Layer::Echo
                | Layer::Particles => {}
            }
        }
        if let Some(c) = &mut self.controls {
            c.grass_cut = grass_cut;
            c.sprites = sprites_in;
        }
    }

    /// The controls' draw limit, when it is `pass`'s (#1204).
    pub fn limit_of(&self, pass: super::SteppedPass) -> Option<usize> {
        let l = self.controls.as_ref()?.limit?;
        (l.pass == pass).then_some(l.draws)
    }

    /// Cut `pass` (a shadow layer, the style mask or the albedo; the
    /// scene's is [`cut_to_limit`](Self::cut_to_limit)) after the controls'
    /// draw limit (#1204): `counts(item)` is whether that pass draws the
    /// item, as its description lists it, walked in list order. Nothing
    /// unless the limit is `pass`'s.
    pub fn cut_pass(&mut self, pass: super::SteppedPass, counts: impl Fn(&DrawItem) -> bool) {
        let Some(limit) = self.limit_of(pass) else {
            return;
        };
        let mut n = 0usize;
        for item in self.iter_mut() {
            if counts(item) {
                n += 1;
                item.cull.past_pass_limit = n > limit;
            }
        }
    }

    /// Whether `pass` draws `item` under the controls' limit (#1204): false
    /// only for an item past the limit of the pass the controls step.
    pub fn within(&self, item: &DrawItem, pass: super::SteppedPass) -> bool {
        !item.cull.past_pass_limit || self.limit_of(pass).is_none()
    }

    /// Whether a replay's draw limit leaves the grass out of the scene pass.
    pub fn grass_cut(&self) -> bool {
        self.controls.as_ref().is_some_and(|c| c.grass_cut)
    }

    /// How many 2D sprite batches the scene pass draws (all, without a limit).
    pub fn sprites_drawn(&self) -> usize {
        self.controls.as_ref().map_or(usize::MAX, |c| c.sprites)
    }

    /// Learn the names of the entities the items draw, for
    /// [`mark`](Self::mark). Only while draw markers are on.
    pub fn name_entities<'w>(&mut self, name_of: impl Fn(EntityId) -> Option<&'w str>) {
        for id in self.layers.iter().flatten().filter_map(|i| i.entity) {
            let Some(name) = name_of(id) else { continue };
            if self.names.get(&id).is_none_or(|n| n != name) {
                self.names.insert(id, name.to_owned());
            }
        }
    }

    /// Mark the next draw in `pass` with `item`'s entity and kind
    /// (`Barrel · Static`, `ground · Terrain chunk 3`), for GPU debuggers.
    pub fn mark(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        label: &mut crate::gpu_labels::DrawLabel,
        item: &DrawItem,
    ) {
        let name = item.entity.and_then(|id| self.names.get(&id));
        let id = item.entity.map(|id| id.0);
        let who: &dyn std::fmt::Display = match (&name, &id) {
            (Some(name), _) => name,
            (None, Some(id)) => id,
            (None, None) => &"(no entity)",
        };
        match item.draw {
            Draw::Terrain(n) => label.mark(pass, format_args!("{who} · Terrain chunk {n}")),
            Draw::Particles(n) => label.mark(pass, format_args!("Particles · draw {n}")),
            Draw::Static(_) | Draw::Skinned(_) | Draw::Echo(_) | Draw::Billboard(_) => {
                label.mark(pass, format_args!("{who} · {}", item.draw.kind()))
            }
        }
    }

    /// Empty the layers extraction fills, keeping their allocations.
    pub fn clear_extracted(&mut self) {
        for layer in Layer::ALL.into_iter().filter(|l| l.extracted()) {
            self.layers[layer.index()].clear();
        }
    }

    pub fn push(&mut self, item: DrawItem) {
        self.layers[item.layer().index()].push(item);
    }

    pub fn layer(&self, layer: Layer) -> &[DrawItem] {
        &self.layers[layer.index()]
    }

    pub fn layer_mut(&mut self, layer: Layer) -> &mut Vec<DrawItem> {
        &mut self.layers[layer.index()]
    }

    /// How many items there are, in every layer.
    pub fn len(&self) -> usize {
        self.layers.iter().map(Vec::len).sum()
    }

    /// Every item, in layer order.
    pub fn iter(&self) -> impl Iterator<Item = &DrawItem> + Clone {
        self.layers.iter().flatten()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut DrawItem> {
        self.layers.iter_mut().flatten()
    }

    /// Every item this camera draws, in layer order.
    pub fn drawn(&self) -> impl Iterator<Item = &DrawItem> + Clone {
        self.iter().filter(|i| i.drawn())
    }

    /// Bring every item back before a camera's culls (`render_to`).
    pub fn uncull(&mut self) {
        for item in self.iter_mut() {
            item.cull = Cull::default();
        }
    }

    /// One terrain item a chunk, for the `chunks` the bound terrain has
    /// and the entity `bind_scene` named. Called at each `render_to`, so a
    /// host that edits terrain without extracting (the terrain editor)
    /// still draws it.
    pub fn sync_terrain(&mut self, chunks: usize, entity: Option<EntityId>) {
        let layer = &mut self.layers[Layer::Terrain.index()];
        if layer.len() == chunks && layer.first().is_none_or(|i| i.entity == entity) {
            return;
        }
        layer.clear();
        layer.extend(
            (0..chunks as u32)
                .map(|n| DrawItem::new(entity, Blend::Opaque, true, Draw::Terrain(n))),
        );
    }

    /// One particle item a particle draw, after an upload.
    pub fn set_particles<'a>(
        &mut self,
        draws: impl Iterator<Item = &'a crate::particle_pipeline::ParticleDrawCall>,
    ) {
        let layer = &mut self.layers[Layer::Particles.index()];
        layer.clear();
        layer.extend(draws.enumerate().map(|(n, d)| {
            DrawItem::new(
                None,
                Blend::of_particles(d.blend),
                false,
                Draw::Particles(n as u32),
            )
        }));
    }

    /// Back-to-front order for this camera of the three blended layers,
    /// by squared distance from `camera` (stable, so equally far items keep
    /// their order: an entity's parts, an entity's echoes).
    pub fn sort_blended(&mut self, camera: [f32; 3]) {
        flint_core::scope!("sort_blended");
        let distance = |p: [f32; 3]| {
            let (dx, dy, dz) = (p[0] - camera[0], p[1] - camera[1], p[2] - camera[2]);
            dx * dx + dy * dy + dz * dz
        };
        for layer in [Layer::BlendedStatic, Layer::BlendedSkinned, Layer::Echo] {
            let items = &mut self.layers[layer.index()];
            for item in items.iter_mut() {
                match &mut item.draw {
                    Draw::Static(d) => d.sort_depth = distance(origin(&d.model)),
                    Draw::Skinned(d) => d.sort_depth = distance(origin(&d.model)),
                    Draw::Echo(d) => d.sort_depth = distance(d.origin),
                    Draw::Billboard(_) | Draw::Terrain(_) | Draw::Particles(_) => {}
                }
            }
            // A permutation of the items by their keys, then each item moved
            // once: an item is several hundred bytes, and a grove's 600 echoes
            // cost a merge sort's many moves 0.07 ms a frame. Equal keys keep
            // their order, as a stable sort's do.
            items.sort_by_cached_key(|item| farthest_first(sort_depth(item)));
        }
    }
}

/// An echo's place among its entity's echoes, which draw in that order.
pub(super) fn echo_order(item: &DrawItem) -> usize {
    match &item.draw {
        Draw::Echo(d) => d.echo,
        Draw::Static(_)
        | Draw::Skinned(_)
        | Draw::Billboard(_)
        | Draw::Terrain(_)
        | Draw::Particles(_) => 0,
    }
}

/// A key that sorts squared distances farthest first: the bits of a finite
/// `f32` ordered as the floats are, reversed.
fn farthest_first(distance: f32) -> std::cmp::Reverse<u32> {
    let bits = distance.to_bits();
    std::cmp::Reverse(if bits >> 31 == 1 {
        !bits
    } else {
        bits | 1 << 31
    })
}

fn origin(model: &[[f32; 4]; 4]) -> [f32; 3] {
    [model[3][0], model[3][1], model[3][2]]
}

/// The distance a blended item sorts by (0 for kinds that don't sort).
fn sort_depth(item: &DrawItem) -> f32 {
    match &item.draw {
        Draw::Static(d) => d.sort_depth,
        Draw::Skinned(d) => d.sort_depth,
        Draw::Echo(d) => d.sort_depth,
        Draw::Billboard(_) | Draw::Terrain(_) | Draw::Particles(_) => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layer_is_in_all_once_and_in_order() {
        for (n, layer) in Layer::ALL.into_iter().enumerate() {
            assert_eq!(layer.index(), n, "{layer:?}");
        }
    }

    #[test]
    fn extraction_leaves_the_terrain_and_particle_items_be() {
        let mut list = DrawList::default();
        list.sync_terrain(3, None);
        list.push(DrawItem::new(
            None,
            Blend::Additive,
            false,
            Draw::Particles(0),
        ));
        list.clear_extracted();
        assert_eq!(list.layer(Layer::Terrain).len(), 3);
        assert_eq!(list.layer(Layer::Particles).len(), 1);
        assert_eq!(list.iter().count(), 4);
    }

    #[test]
    fn terrain_items_follow_the_chunks_and_the_entity() {
        let mut list = DrawList::default();
        let ground = EntityId::from_raw(7);
        list.sync_terrain(2, None);
        list.sync_terrain(2, Some(ground));
        assert!(list
            .layer(Layer::Terrain)
            .iter()
            .all(|i| i.entity == Some(ground)));
        list.sync_terrain(0, Some(ground));
        assert!(list.layer(Layer::Terrain).is_empty());
        list.sync_terrain(4, Some(ground));
        let chunks: Vec<u32> = (list.layer(Layer::Terrain).iter())
            .filter_map(|i| {
                if let Draw::Terrain(n) = i.draw {
                    Some(n)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(chunks, [0, 1, 2, 3]);
    }

    #[test]
    fn a_culled_item_comes_back_for_the_next_camera() {
        let mut list = DrawList::default();
        list.sync_terrain(1, None);
        for item in list.iter_mut() {
            item.set_lod_culled(true);
            item.set_camera_culled(true);
        }
        assert_eq!(list.drawn().count(), 0);
        list.uncull();
        assert_eq!(list.drawn().count(), 1);
    }

    #[test]
    fn farthest_first_orders_as_the_distances_do() {
        let d = [0.0f32, 1e-30, 0.5, 1.0, 2.0, 1e30, f32::INFINITY];
        for w in d.windows(2) {
            assert!(farthest_first(w[0]) > farthest_first(w[1]), "{w:?}");
        }
        assert_eq!(farthest_first(3.0), farthest_first(3.0));
    }

    #[test]
    fn only_opaque_doesnt_blend() {
        let blends = [
            Blend::Opaque,
            Blend::Alpha,
            Blend::Premultiplied,
            Blend::Additive,
            Blend::Multiply,
            Blend::Lighten,
        ];
        for b in blends {
            assert_eq!(b.blends(), b != Blend::Opaque, "{b:?}");
        }
    }
}
