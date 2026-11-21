//! Render pass helpers for `render_to()`.
//!
//! Each method handles one stage of the multi-pass rendering pipeline.

use super::helpers::identity_matrix;
use super::{Blend, Draw, DrawItem, DrawList, Layer, SceneRenderer, SteppedPass};
use crate::billboard_pipeline::BillboardUniforms;
use crate::camera::Camera;
use crate::debug::DebugMode;
use crate::gpu_counts::{CountedDevice, CountedQueue};
use crate::gpu_labels::{DrawLabel, PassGroup};
use crate::particle_pipeline::ParticleUniforms;
use crate::pipeline::TransformUniforms;
use crate::shadow::{ShadowDrawUniforms, SHADOW_DRAW_STRIDE};
use crate::sprite2d_pipeline::Sprite2dUniforms;

/// Which slice of the main pass to render: the opaque scene (sky, terrain,
/// grass, meshes, billboards, the grid), the ocean, and the rest (2D
/// sprites, the blended draws, echoes, particles, the overlays). `ALL` is
/// the single pass. The ocean's refraction grab ends the pass after the
/// opaque scene (ADR 0094), and the fog veil (#513) after the ocean, to keep
/// a copy of the picture behind the translucent draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RenderPhase {
    opaque: bool,
    ocean: bool,
    rest: bool,
}

impl RenderPhase {
    pub const ALL: Self = Self::of(true, true, true);
    /// Before the ocean's grab
    pub const PRE_OCEAN: Self = Self::of(true, false, false);
    /// After the ocean's grab
    pub const POST_OCEAN: Self = Self::of(false, true, true);
    /// After the grab, before the fog veil's copy
    pub const OCEAN: Self = Self::of(false, true, false);
    /// No grab, before the fog veil's copy
    pub const OPAQUE_AND_OCEAN: Self = Self::of(true, true, false);
    /// After the fog veil's copy
    pub const REST: Self = Self::of(false, false, true);

    const fn of(opaque: bool, ocean: bool, rest: bool) -> Self {
        Self {
            opaque,
            ocean,
            rest,
        }
    }
    fn draws_opaque(self) -> bool {
        self.opaque
    }
    fn draws_ocean(self) -> bool {
        self.ocean
    }
    fn draws_rest(self) -> bool {
        self.rest
    }
}

/// Shadow cascades the grass casts into: the nearest ones.
const GRASS_SHADOW_CASCADES: usize = 2;

/// What the main pass's selector keeps between the items of a layer, so it
/// rebinds only what changes.
#[derive(Default)]
struct MainPassState<'a> {
    /// Opaque skinned: woozy or not, double-sided or not.
    skinned: Option<(bool, bool)>,
    /// Echoes: lighten or not, and the transform and bones bound.
    echo_lighten: Option<bool>,
    transform: Option<&'a std::sync::Arc<wgpu::BindGroup>>,
    bones: Option<&'a std::sync::Arc<wgpu::BindGroup>>,
    /// Particles: the blend bound.
    particles: Option<flint_particles::ParticleBlendMode>,
}

/// What the shadow map draws an item as.
enum Caster<'a> {
    Mesh(&'a super::DrawCall),
    Skinned(&'a super::SkinnedDrawCall),
    Terrain(&'a crate::terrain_pipeline::TerrainDrawCall),
}

impl Caster<'_> {
    /// Whether it may be inside a cascade's `frustum`. Skinned meshes
    /// always may: their bounds move with their bones.
    fn visible(&self, frustum: &crate::frustum::Frustum) -> bool {
        match self {
            Caster::Mesh(d) => super::draw_visible(d, frustum),
            Caster::Skinned(_) => true,
            Caster::Terrain(d) => frustum.aabb_visible(d.aabb_min, d.aabb_max),
        }
    }

    fn model(&self) -> [[f32; 4]; 4] {
        match self {
            Caster::Mesh(d) => d.model,
            Caster::Skinned(d) => d.model,
            Caster::Terrain(d) => d.model,
        }
    }
}

/// The shadow pass's selector: what an item casts as, if anything. Casting
/// is the entity's `material.cast_shadows` (ADR 0100); a wireframe casts
/// nothing, a translucent skinned mesh only when it is translucent for its
/// soft edge alone (#582), and a draw past its level of detail nothing.
fn shadow_caster<'a>(
    item: &'a DrawItem,
    terrain: &'a [crate::terrain_pipeline::TerrainDrawCall],
) -> Option<Caster<'a>> {
    if item.out_of_shadows() {
        return None;
    }
    match &item.draw {
        Draw::Static(d) => (item.casts_shadow && !d.is_wireframe).then_some(Caster::Mesh(d)),
        Draw::Skinned(d) => (item.casts_shadow
            && (item.blend == Blend::Opaque || d.soft_edge_only))
            .then_some(Caster::Skinned(d)),
        Draw::Terrain(n) => terrain.get(*n as usize).map(Caster::Terrain),
        // Translucent copies, cut-out sprites and particles cast nothing
        Draw::Echo(_) | Draw::Billboard(_) | Draw::Particles(_) => None,
    }
}

/// Every item that casts, with what it casts as, in list order: the order
/// both the uniform gather and the draws walk, so the slots line up.
fn shadow_casters<'a>(
    draws: &'a DrawList,
    terrain: &'a [crate::terrain_pipeline::TerrainDrawCall],
) -> impl Iterator<Item = (&'a DrawItem, Caster<'a>)> {
    draws
        .iter()
        .filter_map(move |item| Some((item, shadow_caster(item, terrain)?)))
}

/// Each shadow map layer's pass, by layer: a GPU debugger lists them by
/// these names (#1083).
pub(super) const SHADOW_PASS_LABELS: [&str; crate::shadow::SHADOW_LAYERS] = {
    let mut labels = ["Shadow cascade"; crate::shadow::SHADOW_LAYERS];
    labels[0] = "Shadow cascade 0";
    labels[1] = "Shadow cascade 1";
    labels[2] = "Shadow cascade 2";
    labels[crate::shadow::SPOT_SHADOW_LAYER] = "Spot shadow";
    labels[crate::shadow::POINT_SHADOW_LAYER] = "Point shadow";
    labels
};

/// One shadow map layer a frame draws, and the views it is drawn through:
/// one for a cascade or the spot, six for a point light's cube (#1112), each
/// face into its own square of the layer.
struct ShadowLayerDraw {
    layer: usize,
    views: Vec<ShadowView>,
}

struct ShadowView {
    view_proj: [[f32; 4]; 4],
    frustum: crate::frustum::Frustum,
    /// x, y, size in texels; `None` is the whole layer.
    viewport: Option<[u32; 3]>,
}

impl ShadowLayerDraw {
    fn whole(layer: usize, view_proj: [[f32; 4]; 4]) -> Self {
        Self {
            layer,
            views: vec![ShadowView {
                view_proj,
                frustum: crate::frustum::Frustum::from_view_projection(&view_proj),
                viewport: None,
            }],
        }
    }

    /// Whether any of the layer's views sees `caster`.
    fn sees(&self, caster: &Caster<'_>) -> bool {
        self.views.iter().any(|v| caster.visible(&v.frustum))
    }
}

/// The draw-list layer a pass is drawing, held open as a debug group
/// (#1083) until a draw of another layer or [`leave`](Self::leave).
#[derive(Default)]
struct LayerGroup(Option<(Layer, PassGroup)>);

impl LayerGroup {
    fn enter(&mut self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) {
        if self.0.as_ref().is_some_and(|(open, _)| *open == layer) {
            return;
        }
        self.leave(pass);
        self.0 = Some((layer, PassGroup::open(pass, layer.label())));
    }

    fn leave(&mut self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some((_, group)) = self.0.take() {
            group.close(pass);
        }
    }
}

/// The pipeline a pass has bound, so it sets one only when it changes.
#[derive(Default)]
struct BoundPipeline<'p>(Option<&'p wgpu::RenderPipeline>);

impl<'p> BoundPipeline<'p> {
    fn set(&mut self, pass: &mut wgpu::RenderPass<'_>, pipeline: &'p wgpu::RenderPipeline) {
        if self.0.is_none_or(|b| !std::ptr::eq(b, pipeline)) {
            pass.set_pipeline(pipeline);
            self.0 = Some(pipeline);
        }
    }
}

/// Draw one caster into a cascade, at the next slot of the shared uniforms.
fn draw_caster<'p>(
    pass: &mut wgpu::RenderPass<'_>,
    bound: &mut BoundPipeline<'p>,
    next_slot: &mut u32,
    shadow_pass: &'p crate::shadow::ShadowPass,
    woozy: Option<&'p crate::shadow::ShadowWooze>,
    caster: &Caster<'_>,
) {
    let offset = (*next_slot as u64 * SHADOW_DRAW_STRIDE) as u32;
    *next_slot += 1;
    let sp = shadow_pass;
    let (pipeline, bind) = match (caster, woozy) {
        (Caster::Mesh(d), Some(w)) if d.double_sided => (&w.pipeline_double_sided, &w.bind_group),
        (Caster::Mesh(_), Some(w)) => (&w.pipeline, &w.bind_group),
        (Caster::Mesh(d), None) if d.double_sided => {
            (&sp.shadow_pipeline_double_sided, &sp.draw_bind_group)
        }
        (Caster::Skinned(d), Some(w)) if d.double_sided => {
            (&w.skinned_pipeline_double_sided, &w.bind_group)
        }
        (Caster::Skinned(_), Some(w)) => (&w.skinned_pipeline, &w.bind_group),
        (Caster::Skinned(d), None) if d.double_sided => (
            &sp.skinned_shadow_pipeline_double_sided,
            &sp.draw_bind_group,
        ),
        (Caster::Skinned(_), None) => (&sp.skinned_shadow_pipeline, &sp.draw_bind_group),
        (Caster::Mesh(_), None) | (Caster::Terrain(_), _) => {
            (&sp.shadow_pipeline, &sp.draw_bind_group)
        }
    };
    bound.set(pass, pipeline);
    pass.set_bind_group(0, bind, &[offset]);
    let (vertex, index, count) = match caster {
        Caster::Mesh(d) => (&*d.vertex_buffer, &*d.index_buffer, d.index_count),
        Caster::Skinned(d) => {
            pass.set_bind_group(1, &*d.shadow_bone_bind_group, &[]);
            (&*d.vertex_buffer, &*d.index_buffer, d.index_count)
        }
        Caster::Terrain(d) => (&d.vertex_buffer, &d.index_buffer, d.index_count),
    };
    pass.set_vertex_buffer(0, vertex.slice(..));
    pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint32);
    pass.draw_indexed(0..count, 0, 0..1);
}

/// The pipeline [`draw_caster`] draws `caster` through, by its choices.
fn caster_pipeline_name(caster: &Caster<'_>, woozy: bool) -> &'static str {
    match (caster, woozy) {
        (Caster::Mesh(d), true) if d.double_sided => "wooze shadow double-sided",
        (Caster::Mesh(_), true) => "wooze shadow",
        (Caster::Mesh(d), false) if d.double_sided => "shadow double-sided",
        (Caster::Skinned(d), true) if d.double_sided => "wooze skinned shadow double-sided",
        (Caster::Skinned(_), true) => "wooze skinned shadow",
        (Caster::Skinned(d), false) if d.double_sided => "skinned shadow double-sided",
        (Caster::Skinned(_), false) => "skinned shadow",
        (Caster::Mesh(_), false) | (Caster::Terrain(_), _) => "shadow",
    }
}

impl SceneRenderer {
    /// Log each shadow map layer the pass drew as a pass of its own, with
    /// the casters it drew: the same selector ([`shadow_casters`]) and the
    /// same per-layer frustum test the draws made (#1084).
    fn log_shadow_layers(
        &self,
        pass: &mut crate::frame_capture::LoggedPass,
        layers: &[ShadowLayerDraw],
        grass_casts: bool,
    ) {
        pass.writes(&["shadow_map"]);
        let woozy_pass = self
            .shadow_pass
            .as_ref()
            .is_some_and(|sp| sp.wooze.is_some());
        for drawn in layers {
            let layer = drawn.layer;
            let mut logged = crate::frame_capture::LoggedPass::default();
            logged.pass.name = SHADOW_PASS_LABELS[layer].to_string();
            logged.pass.ran = true;
            logged.writes(&["shadow_map"]);
            logged.render_pass(SHADOW_PASS_LABELS[layer]);
            if grass_casts && layer < GRASS_SHADOW_CASCADES {
                logged.pass.also.push("Grass".into());
            }
            for (item, caster) in shadow_casters(&self.draws, &self.terrain.draws) {
                let woozy = item.wooze().is_some() && woozy_pass;
                let name = caster_pipeline_name(&caster, woozy);
                if !drawn.sees(&caster) {
                    logged
                        .culled
                        .push(self.logged_draw(item, Some("outside the layer"), name));
                } else if self.draws.within(item, SteppedPass::Shadow(layer)) {
                    logged.draws.push(self.logged_draw(item, None, name));
                } else {
                    logged
                        .culled
                        .push(self.logged_draw(item, Some("past the draw limit"), name));
                }
            }
            pass.passes.push(logged);
        }
    }

    /// Render depth from light perspective for cascaded shadow mapping, from
    /// the casting spot's for its layer (#504), and from the casting point
    /// light's six faces for theirs (#1112).
    pub(super) fn render_shadow_pass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        camera_pos: [f32; 3],
        log: Option<&mut crate::frame_capture::FrameLog>,
    ) {
        flint_core::scope!("render_shadow_pass");
        // The frame's description (#1084): the pass `encode_to` opened
        let mut logged = log.and_then(|l| l.named("Shadows"));
        // Off, forbidden by the scene, or no sun that gives any light (#627)
        // and no spot or point light that casts (#504, #1112): nothing to draw
        let sun_drawn = self.shadows_drawn();
        let spot = self
            .spot_shadow_drawn()
            .then(|| self.lights.shadow_spot().copied())
            .flatten();
        let point = self
            .point_shadow_drawn()
            .then(|| self.lights.shadow_point().copied())
            .flatten();
        let nothing = !sun_drawn && spot.is_none() && point.is_none();
        if let Some(pass) = logged.as_deref_mut() {
            if nothing {
                pass.drew_nothing(self.shadows_off_reason());
            }
        }
        let shadow_pass = match &mut self.shadow_pass {
            Some(sp) => sp,
            None => return,
        };

        if nothing {
            // The shaders sample the maps whatever this pass does, so a pass
            // turned off at runtime would leave the last frame's shadows
            // standing (FlintKart#55). Zero splits read as "no shadows".
            shadow_pass.write_off_uniforms(queue);
            return;
        }
        shadow_pass.shadows_written_off = false;

        let cascades = if sun_drawn {
            // Use the first directional light for shadows
            let light = &self.lights.uniforms.directional_lights[0];

            // Update cascade matrices. The sun's angular size (radians, 0 =
            // punctual) rides along as tan() for PCSS penumbra math (ADR 0057).
            let camera_inv = camera.inverse_view_projection_matrix();
            let sun_tan_angular = light.angular_size.tan().max(0.0);
            // Quality settings (FlintKart#55): how far shadows reach and how many
            // cascades cover it, 200 m and 3 unless a tier says otherwise. The
            // camera's own planes map the split depths to its frustum.
            shadow_pass.update_cascades(
                light.direction,
                camera_pos,
                camera_inv,
                camera.near,
                camera.far,
                100.0,
                sun_tan_angular,
            );
            shadow_pass.cascades()
        } else {
            shadow_pass.clear_cascades();
            0
        };
        shadow_pass.update_spot(spot.as_ref());
        let cube = shadow_pass.update_point(point.as_ref());

        // The layers drawn, each with the matrices it is drawn through: the
        // cascades, the spot's (#504), the point light's six faces (#1112).
        // Grass casts into the nearest cascades only: it takes no spot or
        // point light.
        let mut layers: Vec<ShadowLayerDraw> = (0..cascades)
            .map(|cascade| {
                ShadowLayerDraw::whole(
                    cascade,
                    shadow_pass.shadow_uniforms().cascade_view_proj[cascade],
                )
            })
            .collect();
        if spot.is_some() {
            layers.push(ShadowLayerDraw::whole(
                crate::shadow::SPOT_SHADOW_LAYER,
                shadow_pass.shadow_uniforms().spot_view_proj,
            ));
        }
        if let Some(cube) = &cube {
            layers.push(ShadowLayerDraw {
                layer: crate::shadow::POINT_SHADOW_LAYER,
                views: cube
                    .faces
                    .iter()
                    .map(|face| ShadowView {
                        view_proj: face.view_proj,
                        frustum: crate::frustum::Frustum::from_view_projection(&face.view_proj),
                        viewport: Some(face.viewport),
                    })
                    .collect(),
            });
        }

        // A replay stepping one layer's casters (#1204): those past its limit
        // are set aside from that layer alone, counted as its log lists them
        for drawn in &layers {
            let stepped = SteppedPass::Shadow(drawn.layer);
            if self.draws.limit_of(stepped).is_some() {
                let terrain = &self.terrain.draws;
                self.draws.cut_pass(stepped, |item| {
                    shadow_caster(item, terrain).is_some_and(|c| drawn.sees(&c))
                });
            }
        }

        // Write shadow uniforms
        queue.write_buffer_counted(
            &shadow_pass.shadow_uniforms_buffer,
            0,
            bytemuck::cast_slice(&[*shadow_pass.shadow_uniforms()]),
        );

        // Gather every draw's uniforms for every cascade up front, in exactly
        // the order the passes below bind them, and upload them once. Each
        // draw then binds the shared buffer at its slot's dynamic offset
        // instead of allocating a buffer + bind group of its own (which was
        // ~800 allocations a frame in a 270-entity scene and dominated the
        // CPU frame time).
        let mut draw_uniforms: Vec<ShadowDrawUniforms> = Vec::new();
        // Shallow wooze's drift, for the slots whose casters have any (#215).
        let mut draw_wooze = Vec::new();
        for drawn in &layers {
            for view in &drawn.views {
                for (item, caster) in shadow_casters(&self.draws, &self.terrain.draws) {
                    if !caster.visible(&view.frustum)
                        || !self.draws.within(item, SteppedPass::Shadow(drawn.layer))
                    {
                        continue;
                    }
                    if let Some(wooze) = item.wooze() {
                        draw_wooze.push((draw_uniforms.len() as u32, *wooze));
                    }
                    draw_uniforms.push(ShadowDrawUniforms {
                        light_view_proj: view.view_proj,
                        model: caster.model(),
                    });
                }
            }
        }
        shadow_pass.ensure_draw_capacity(device, draw_uniforms.len() as u32);
        shadow_pass.write_draw_uniforms(queue, &draw_uniforms, &draw_wooze);

        // Grass casts into the nearest two cascades, each through a view
        // buffer and bind group kept across frames (#139; they were made
        // per cascade per frame).
        let grass_casts = self.grass.drawing && self.pipelines.grass.is_some();
        if grass_casts {
            while self.grass.shadow_binds.len() < GRASS_SHADOW_CASCADES {
                let buffer = device.create_buffer_counted(&wgpu::BufferDescriptor {
                    label: Some("Grass Shadow Transform"),
                    size: std::mem::size_of::<TransformUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    layout: &self.layouts.transform_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                    label: Some("Grass Shadow Transform Bind Group"),
                });
                self.grass.shadow_binds.push((buffer, bind));
            }
            for (cascade, (buffer, _)) in self
                .grass
                .shadow_binds
                .iter()
                .enumerate()
                .take(GRASS_SHADOW_CASCADES)
            {
                // Grass shadow pipeline uses TransformUniforms (same layout as PBR)
                let uniforms = TransformUniforms {
                    view_proj: shadow_pass.shadow_uniforms().cascade_view_proj[cascade],
                    model: identity_matrix(),
                    model_inv_transpose: identity_matrix(),
                    camera_pos: [0.0; 3],
                    _pad: 0.0,
                };
                queue.write_buffer_counted(buffer, 0, bytemuck::cast_slice(&[uniforms]));
            }
        }
        let mut next_slot: u32 = 0;
        // A woozy caster draws through its own pipeline and a bind group that sees
        // its drift. If the pass was only just remade (a new resolution), a frame
        // draws plain.
        let shadow_wooze = shadow_pass.wooze.as_ref();

        // Render each layer: the cascades, the spot's, the point's. A GPU
        // debugger sees each as a pass of its own name, its casters grouped
        // by the draw list's layer and, with draw markers on, named (#1083).
        // The point's six faces are one pass, a viewport each.
        let marks = crate::gpu_labels::draws();
        let mut draw_label = DrawLabel::default();
        for drawn in &layers {
            let cascade = drawn.layer;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(SHADOW_PASS_LABELS[cascade]),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_pass.layer_views[cascade],
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
            let stepped = SteppedPass::Shadow(cascade);
            let mut bound = BoundPipeline::default();
            for view in &drawn.views {
                if let Some([x, y, size]) = view.viewport {
                    let (x, y, size) = (x as f32, y as f32, size as f32);
                    pass.set_viewport(x, y, size, size, 0.0, 1.0);
                    pass.set_scissor_rect(x as u32, y as u32, size as u32, size as u32);
                }
                // Same per-view test as the uniform gather above, so the slots
                // line up.
                let casts = |item: &DrawItem, caster: &Caster<'_>| {
                    caster.visible(&view.frustum) && self.draws.within(item, stepped)
                };
                let mut casters = shadow_casters(&self.draws, &self.terrain.draws).peekable();
                let mut group = LayerGroup::default();
                // The terrain's chunks come first in the list; grass casts after them
                while casters
                    .peek()
                    .is_some_and(|(item, _)| item.layer() == super::Layer::Terrain)
                {
                    let (item, caster) = casters.next().expect("peeked");
                    if casts(item, &caster) {
                        group.enter(&mut pass, Layer::Terrain);
                        if marks {
                            self.draws.mark(&mut pass, &mut draw_label, item);
                        }
                        draw_caster(
                            &mut pass,
                            &mut bound,
                            &mut next_slot,
                            shadow_pass,
                            None,
                            &caster,
                        );
                    }
                }

                // Render grass into shadow cascade (nearest 2 only)
                if grass_casts && cascade < GRASS_SHADOW_CASCADES {
                    if let (Some(gp), Some(render_bg), Some(instance_bg), Some(args)) = (
                        &self.pipelines.grass,
                        &self.grass.render_uniform_bind_group,
                        &self.grass.render_instance_bind_group,
                        &self.grass.draw_args_buffer,
                    ) {
                        group.leave(&mut pass);
                        let grass_group = PassGroup::open(&mut pass, "Grass");
                        let grass_shadow_bind = &self.grass.shadow_binds[cascade].1;
                        bound.set(&mut pass, &gp.shadow_pipeline);
                        pass.set_bind_group(0, grass_shadow_bind, &[]);
                        pass.set_bind_group(1, render_bg, &[]);
                        pass.set_bind_group(2, &gp.shadow_dummy_bind_group, &[]);
                        pass.set_bind_group(3, instance_bg, &[]);
                        pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            gp.blade_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint16,
                        );
                        // The count is the compute pass's (ADR 0092)
                        pass.draw_indexed_indirect(args, 0);
                        grass_group.close(&mut pass);
                    }
                }

                for (item, caster) in casters {
                    if casts(item, &caster) {
                        group.enter(&mut pass, item.layer());
                        if marks {
                            self.draws.mark(&mut pass, &mut draw_label, item);
                        }
                        let woozy = item.wooze().and(shadow_wooze);
                        draw_caster(
                            &mut pass,
                            &mut bound,
                            &mut next_slot,
                            shadow_pass,
                            woozy,
                            &caster,
                        );
                    }
                }
                group.leave(&mut pass);
            }
        }
        if let Some(pass) = logged {
            self.log_shadow_layers(pass, &layers, grass_casts);
        }
    }

    /// Write per-frame uniforms to all GPU buffers and sort transparent draw calls.
    pub(super) fn update_per_frame_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
        debug_mode_u32: u32,
        tonemapping_u32: u32,
        camera: &Camera,
    ) {
        flint_core::scope!("update_per_frame_uniforms", self.draws.len());
        // Blended draws back to front for this camera (each entity's echoes
        // keep their order)
        self.draws.sort_blended(camera_pos);

        // Cached draws' transforms are staged in the arena and go up a page
        // at a time at the end (#139); others are written at once.
        // Taken for the function, so `self`'s methods stay callable.
        let mut arena = std::mem::take(&mut self.draw_bind_cache.arena);
        let mut put =
            |target: &super::TransformTarget,
             uniforms: &TransformUniforms,
             wooze: Option<&crate::wooze_pipeline::ShallowWoozeUniforms>| {
                target.write(queue, &mut arena, uniforms);
                if let Some(wooze) = wooze {
                    target.write_wooze(&mut arena, wooze);
                }
            };

        // Update grid transform
        if let Some(grid) = &self.grid_draw {
            let uniforms = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            put(&grid.transform, &uniforms, None);
        }

        // Each item's uniforms for this camera: transforms (static draws
        // only where this camera draws them), the debug mode and tonemapping
        // in each material buffer whose flags changed (draws of the same look
        // share one, `SharedMaterial`), the billboards' camera vectors.
        let billboard_uniforms = BillboardUniforms {
            view_proj,
            camera_right: camera.right_vector(),
            _pad0: 0.0,
            camera_up: camera.up_vector(),
            _pad1: 0.0,
        };
        let transform = |model, model_inv_transpose| TransformUniforms {
            view_proj,
            model,
            model_inv_transpose,
            camera_pos,
            _pad: 0.0,
        };
        for item in self.draws.iter() {
            match &item.draw {
                Draw::Static(draw) => {
                    if !item.drawn() {
                        continue;
                    }
                    let uniforms = transform(draw.model, draw.model_inv_transpose);
                    put(&draw.transform, &uniforms, item.wooze());
                    draw.material_buffer
                        .write_flags(queue, debug_mode_u32, tonemapping_u32);
                }
                Draw::Skinned(draw) => {
                    let uniforms = transform(draw.model, draw.model_inv_transpose);
                    put(&draw.transform, &uniforms, item.wooze());
                    draw.material_buffer
                        .write_flags(queue, debug_mode_u32, tonemapping_u32);
                }
                // Echoes read their entity's transform, so only their materials' flags
                Draw::Echo(draw) => {
                    draw.material_buffer
                        .write_flags(queue, debug_mode_u32, tonemapping_u32)
                }
                Draw::Billboard(draw) => queue.write_buffer_counted(
                    &draw.billboard_buffer,
                    0,
                    bytemuck::cast_slice(&[billboard_uniforms]),
                ),
                Draw::Terrain(n) => {
                    if let Some(chunk) = self.terrain.draws.get(*n as usize) {
                        let uniforms = transform(chunk.model, chunk.model_inv_transpose);
                        queue.write_buffer_counted(
                            &chunk.transform_buffer,
                            0,
                            bytemuck::cast_slice(&[uniforms]),
                        );
                    }
                }
                // The particles' one uniform buffer is written below
                Draw::Particles(_) => {}
            }
        }

        // Grass view uniforms (its vertex shader reads only view_proj and camera_pos)
        if let Some(buf) = &self.grass.transform_buffer {
            let uniforms = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer_counted(buf, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update terrain material buffer (tonemapping flag)
        if let Some(buf) = &self.terrain.material_buffer {
            // enable_tonemapping is at byte offset 12 (texture_tile(4) + metallic(4) + roughness(4))
            queue.write_buffer_counted(buf, 12, bytemuck::cast_slice(&[tonemapping_u32]));
        }

        // Update ocean uniforms (wave phases in f64, grid follows the camera).
        // Water bodies read them too (ADR 0094), sea or no sea.
        if self.water_active() {
            self.water.upload(
                queue,
                view_proj,
                camera_pos,
                &super::water::WaterFrame {
                    time: self.ocean_time,
                    tonemapping: tonemapping_u32,
                    sky: self.sky.params(),
                    sky_active: self.sky.active(),
                },
            );
        }

        // Update wireframe overlay transforms
        for draw in &self.wireframe_overlay_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            put(&draw.transform, &uniforms, None);
        }

        // Update particle uniforms with camera vectors
        if let Some(pp) = &self.particle_buffers {
            if !self.particle_draws.is_empty() {
                let cam_right = camera.right_vector();
                let cam_up = camera.up_vector();
                let particle_uniforms = ParticleUniforms {
                    view_proj,
                    camera_right: cam_right,
                    _pad0: 0.0,
                    camera_up: cam_up,
                    _pad1: 0.0,
                };
                queue.write_buffer_counted(
                    &pp.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[particle_uniforms]),
                );
            }
        }

        // Update sprite2d uniforms with view_proj
        if let Some(sp) = &self.pipelines.sprite2d {
            if !self.sprite2d_batches.is_empty() {
                let sprite2d_uniforms = Sprite2dUniforms { view_proj };
                queue.write_buffer_counted(
                    &sp.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[sprite2d_uniforms]),
                );
            }
        }

        // Update normal arrow transforms
        for draw in &self.normal_arrow_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            put(&draw.transform, &uniforms, None);
        }

        // Update skeleton + debug overlay transforms
        for draw in self
            .skeleton_overlay_draws
            .iter()
            .chain(self.debug_overlay_draws.iter())
        {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            put(&draw.transform, &uniforms, None);
        }

        {
            flint_core::scope!("upload_transforms");
            arena.upload(queue);
        }
        self.draw_bind_cache.arena = arena;
    }

    /// Execute the main render pass: skybox → terrain → entities → grid → sprites → particles → debug.
    pub(super) fn render_main_pass<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        wireframe_only: bool,
        queue: &wgpu::Queue,
        camera: &Camera,
        phase: RenderPhase,
    ) {
        flint_core::scope!("render_main_pass");
        // Bind lights once for the entire pass (group 2 is shared)
        render_pass.set_bind_group(2, &self.lights.bind_group, &[]);

        // Render sky at the far plane: the procedural sky (driven by the
        // `sky` component) replaces the texture skybox when present.
        if phase.draws_opaque() {
            let group = PassGroup::open(render_pass, "Sky");
            self.sky.draw(
                render_pass,
                queue,
                camera,
                self.pipelines.sky.as_deref(),
                self.pipelines.skybox.as_deref(),
                &self.lights.uniforms.directional_lights[0],
                self.ocean_time,
            );
            group.close(render_pass);
        }

        if wireframe_only {
            let group = PassGroup::open(render_pass, "Wireframe");
            self.render_wireframe_only_pass(render_pass);
            group.close(render_pass);
            if phase.draws_opaque() {
                self.draw_editor_grid(render_pass);
            }
        } else {
            // The editor grid is drawn in here, after the opaque geometry.
            self.render_normal_pass(render_pass, phase);
        }

        // Normal arrows pass (always uses line pipeline)
        if self.debug_state.show_normals && phase.draws_rest() {
            let group = PassGroup::open(render_pass, "Normal arrows");
            render_pass.set_pipeline(&self.pipelines.main.line_pipeline);
            for draw in &self.normal_arrow_draws {
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            group.close(render_pass);
        }

        // Skeleton overlay pass (bone lines, drawn on top of everything — no depth test)
        if self.debug_state.show_skeleton && phase.draws_rest() {
            let group = PassGroup::open(render_pass, "Skeletons");
            render_pass.set_pipeline(&self.pipelines.main.skeleton_line_pipeline);
            render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
            for draw in &self.skeleton_overlay_draws {
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            group.close(render_pass);
        }

        // Editor/gizmo overlay (ADR 0068): same line pipeline, never gated
        // behind the skeleton toggle.
        if !self.debug_overlay_draws.is_empty() && phase.draws_rest() {
            let group = PassGroup::open(render_pass, "Gizmos");
            render_pass.set_pipeline(&self.pipelines.main.skeleton_line_pipeline);
            // The particle pass may have left its texture bind group at slot 2.
            render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
            for draw in &self.debug_overlay_draws {
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            group.close(render_pass);
        }
    }

    /// The editor grid, as an overlay (#346): depth-tested against what is
    /// already drawn (LessEqual, no bias), but never writing depth. Drawn
    /// after the opaque geometry, so the grid still shows on a floor at
    /// y = 0 and hides behind anything standing on it, while the depth that
    /// styles, SSAO, fog and the edge passes read holds only the scene. With
    /// depth writes, ASCII's edges traced every grid line.
    fn draw_editor_grid<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let Some(grid) = &self.grid_draw else {
            return;
        };
        let group = PassGroup::open(render_pass, "Editor grid");
        render_pass.set_pipeline(&self.pipelines.main.overlay_line_pipeline);
        // Billboards may have left their texture group at slot 2.
        render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
        render_pass.set_bind_group(0, &*grid.transform_bind_group, &[]);
        render_pass.set_bind_group(1, &*grid.material_bind_group, &[]);
        render_pass.set_vertex_buffer(0, grid.vertex_buffer.slice(..));
        render_pass.set_index_buffer(grid.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..grid.index_count, 0, 0..1);
        group.close(render_pass);
    }

    /// Draw the wireframe edges of every skinned draw call (opaque and
    /// transparent) through the skinned line pipeline so they follow the pose.
    fn draw_skinned_wireframes<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let Some(sp) = &self.pipelines.skinned else {
            return;
        };
        render_pass.set_pipeline(&sp.wire_line_pipeline);
        for item in self.draws.iter() {
            let draw = match &item.draw {
                Draw::Skinned(draw) => draw,
                Draw::Static(_)
                | Draw::Echo(_)
                | Draw::Billboard(_)
                | Draw::Terrain(_)
                | Draw::Particles(_) => continue,
            };
            let Some(wire_ib) = &draw.wire_index_buffer else {
                continue;
            };
            render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
            render_pass.set_bind_group(3, &*draw.bone_bind_group, &[]);
            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
            render_pass.set_index_buffer(wire_ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..draw.wire_index_count, 0, 0..1);
        }
    }

    /// Wireframe-only mode rendering: wireframe lines. The selection is
    /// outlined over the finished image (#602).
    fn render_wireframe_only_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        // Wireframe lines: the overlay pipeline (LessEqual, unbiased since #944)
        render_pass.set_pipeline(&self.pipelines.main.overlay_line_pipeline);
        for draw in &self.wireframe_overlay_draws {
            render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
            render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
            render_pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
        }

        // Also render entities that are already wireframe (rooms etc.)
        for item in self.draws.layer(Layer::OpaqueStatic) {
            let draw = match &item.draw {
                Draw::Static(draw) if draw.is_wireframe && item.drawn() => draw,
                Draw::Static(_)
                | Draw::Skinned(_)
                | Draw::Echo(_)
                | Draw::Billboard(_)
                | Draw::Terrain(_)
                | Draw::Particles(_) => continue,
            };
            render_pass.set_pipeline(&self.pipelines.main.overlay_line_pipeline);
            render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
            render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
            render_pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
        }

        // Skinned meshes: edges posed by the bone matrices
        self.draw_skinned_wireframes(render_pass);
    }

    /// Normal mode rendering: terrain → grass → (ocean) → opaque statics →
    /// skinned → billboards → grid → sprites → blended statics → blended
    /// skinned → echoes → particles → wireframe. The list's layers in their
    /// order, with what isn't a draw item between them.
    fn render_normal_pass<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        phase: RenderPhase,
    ) {
        // Terrain rendering (early in pass — fills depth buffer for occlusion)
        if phase.draws_opaque() {
            self.draw_layer(render_pass, Layer::Terrain);

            // Grass rendering (after terrain -- also writes depth)
            if let (Some(gp), Some(render_bg), Some(instance_bg), Some(args)) = (
                &self.pipelines.grass,
                &self.grass.render_uniform_bind_group,
                &self.grass.render_instance_bind_group,
                &self.grass.draw_args_buffer,
            ) {
                if self.grass.drawing && !self.draws.grass_cut() {
                    let group = PassGroup::open(render_pass, "Grass");
                    // Bind group 0 (transform): the grass's own view uniforms,
                    // so grass on a game's ground draws with no terrain loaded
                    if let Some(transform_bg) = &self.grass.transform_bind_group {
                        render_pass.set_bind_group(0, transform_bg, &[]);
                    }
                    render_pass.set_pipeline(&gp.render_pipeline);
                    render_pass.set_bind_group(1, render_bg, &[]);
                    render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
                    render_pass.set_bind_group(3, instance_bg, &[]);
                    render_pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                    render_pass.set_index_buffer(
                        gp.blade_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    // The count is the compute pass's, culled to the view (ADR 0092)
                    render_pass.draw_indexed_indirect(args, 0);
                    group.close(render_pass);
                }
            }
        } // end opaque phase (terrain + grass)

        // Ocean rendering (after all opaque — writes depth so fog applies;
        // group 3 holds the grab-pass snapshots, or dummies when disabled)
        if self.water_active() && phase.draws_ocean() {
            let group = PassGroup::open(render_pass, "Water");
            self.water.draw(
                render_pass,
                self.pipelines.ocean.as_deref(),
                &self.lights.bind_group,
            );
            group.close(render_pass);
        }

        if phase.draws_opaque() {
            self.draw_layer(render_pass, Layer::OpaqueStatic);
            self.draw_layer(render_pass, Layer::OpaqueSkinned);
            // Billboard sprites (after geometry, uses depth test)
            self.draw_layer(render_pass, Layer::Billboard);

            // After every opaque draw: an overlay, not geometry (#346)
            self.draw_editor_grid(render_pass);
        } // end opaque phase (entities + skinned + billboards + grid)

        if !phase.draws_rest() {
            return;
        }

        // 2D sprites (after billboards, instanced batched rendering)
        if let Some(sp) = &self.pipelines.sprite2d {
            if !self.sprite2d_batches.is_empty() {
                let group = PassGroup::open(render_pass, "2D sprites");
                render_pass.set_pipeline(&sp.pipeline);
                render_pass
                    .set_index_buffer(sp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.set_bind_group(0, &sp.uniform_bind_group, &[]);
                let drawn = self.draws.sprites_drawn();
                for batch in self.sprite2d_batches.iter().take(drawn) {
                    render_pass.set_bind_group(1, &batch.instance_bind_group, &[]);
                    render_pass.set_bind_group(2, &batch.texture_bind_group, &[]);
                    render_pass.draw_indexed(0..6, 0, 0..batch.instance_count);
                }
                group.close(render_pass);
            }
        }

        // Blended PBR entities, then blended skinned ones (each sorted back to
        // front, depth write off; alpha blend draws back faces then front
        // faces so a mesh cannot blend over itself)
        self.draw_layer(render_pass, Layer::BlendedStatic);
        self.draw_layer(render_pass, Layer::BlendedSkinned);
        // Living props' echoes (#228)
        self.draw_layer(render_pass, Layer::Echo);
        // Particle systems (after billboards + transparent, before wireframe overlay)
        self.draw_layer(render_pass, Layer::Particles);

        // Wireframe overlay pass (on top of solid geometry)
        if self.debug_state.mode == DebugMode::WireframeOverlay {
            let group = PassGroup::open(render_pass, "Wireframe overlay");
            render_pass.set_pipeline(&self.pipelines.main.overlay_line_pipeline);
            for draw in &self.wireframe_overlay_draws {
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            self.draw_skinned_wireframes(render_pass);
            group.close(render_pass);
        }
    }

    /// Draw one layer of the list in the main pass: what the layer sets up
    /// once, then each item this camera draws through the main pass's
    /// selector, then what it puts back.
    fn draw_layer<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>, layer: Layer) {
        let items = self.draws.layer(layer);
        if !items.iter().any(DrawItem::drawn) {
            return;
        }
        match layer {
            Layer::Terrain => {
                let (Some(tp), Some(mat_bg)) =
                    (&self.pipelines.terrain, &self.terrain.material_bind_group)
                else {
                    return;
                };
                render_pass.set_pipeline(&tp.pipeline);
                render_pass.set_bind_group(1, mat_bg, &[]);
                render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
            }
            Layer::Billboard => {
                let Some(bp) = &self.pipelines.billboard else {
                    return;
                };
                render_pass.set_pipeline(&bp.pipeline);
            }
            // Re-bind lights for group 2 (billboard binds may have displaced them)
            Layer::BlendedStatic | Layer::Echo => {
                render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
            }
            // Draws arrive pre-sorted (order-dependent blends far-to-near,
            // additive last) and all index one shared instance buffer (ADR 0068).
            Layer::Particles => {
                let (Some(_), Some(pp)) = (&self.pipelines.particle, &self.particle_buffers) else {
                    return;
                };
                render_pass
                    .set_index_buffer(pp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.set_bind_group(0, &pp.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, pp.instance_bind_group(), &[]);
            }
            Layer::OpaqueStatic | Layer::OpaqueSkinned | Layer::BlendedSkinned => {}
        }
        // A GPU debugger sees the layer as a group, and with draw markers
        // on each draw named by its entity (#1083)
        let group = PassGroup::open(render_pass, layer.label());
        let marks = crate::gpu_labels::draws();
        let mut label = DrawLabel::default();
        let mut state = MainPassState::default();
        for item in items.iter().filter(|i| i.drawn()) {
            if marks && self.main_pass_draws(item) {
                self.draws.mark(render_pass, &mut label, item);
            }
            self.draw_main(render_pass, item, &mut state);
        }
        group.close(render_pass);
        match layer {
            // Restore the light bind group at slot 2: every line/overlay
            // pipeline that follows (wireframe overlay, normal arrows,
            // skeleton, gizmos) expects it there, and wgpu validates the
            // layout at draw time.
            Layer::Particles => render_pass.set_bind_group(2, &self.lights.bind_group, &[]),
            Layer::Terrain
            | Layer::OpaqueStatic
            | Layer::OpaqueSkinned
            | Layer::Billboard
            | Layer::BlendedStatic
            | Layer::BlendedSkinned
            | Layer::Echo => {}
        }
    }

    /// Terrain chunk `n`, unless the camera can't see it.
    fn terrain_chunk_in_view(&self, n: u32) -> Option<&crate::terrain_pipeline::TerrainDrawCall> {
        chunk_in_view(&self.terrain.draws, self.camera_frustum.as_ref(), n)
    }

    /// Whether [`draw_main`](Self::draw_main) draws `item` (it passes over
    /// a terrain chunk out of view and a blended wireframe box), so a draw
    /// marker names only a draw.
    pub(super) fn main_pass_draws(&self, item: &DrawItem) -> bool {
        main_pass_draws(&self.terrain.draws, self.camera_frustum.as_ref(), item)
    }

    /// A replay's draw limit (#1089), once this camera's culls are made:
    /// the scene pass's draws past it are set aside, counted as
    /// `log_scene_draws` lists them. Nothing without one.
    pub(super) fn cut_draws_to_limit(&mut self) {
        flint_core::scope!("cut_draws_to_limit");
        let terrain = &self.terrain.draws;
        let frustum = self.camera_frustum.as_ref();
        let grass = self.grass.drawing && self.pipelines.grass.is_some();
        let sprites = if self.pipelines.sprite2d.is_some() {
            self.sprite2d_batches.len()
        } else {
            0
        };
        self.draws.cut_to_limit(
            |item| main_pass_draws(terrain, frustum, item),
            grass,
            sprites,
        );
    }

    /// The main pass's selector: one item, through the pipeline its kind,
    /// blend and vertex stage ask for.
    fn draw_main<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        item: &'a DrawItem,
        state: &mut MainPassState<'a>,
    ) {
        let main = &self.pipelines.main;
        match &item.draw {
            Draw::Terrain(n) => {
                let Some(draw) = self.terrain_chunk_in_view(*n) else {
                    return;
                };
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Static(draw) if item.blend == Blend::Opaque => {
                if draw.is_wireframe {
                    render_pass.set_pipeline(&main.line_pipeline);
                } else if let Some(woozy) =
                    item.wooze().and(self.pipelines.wooze_standard.as_deref())
                {
                    render_pass.set_pipeline(if draw.double_sided {
                        &woozy.opaque_double_sided
                    } else {
                        &woozy.opaque
                    });
                } else if draw.double_sided {
                    render_pass.set_pipeline(&main.pipeline_double_sided);
                } else {
                    render_pass.set_pipeline(&main.pipeline);
                }
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Static(draw) => {
                if draw.is_wireframe {
                    return;
                }
                let mode = item.blend.mesh_mode();
                let pipeline = match mode {
                    crate::pipeline::BlendMode::Alpha => &main.transparent_alpha_pipeline,
                    crate::pipeline::BlendMode::Additive => &main.transparent_additive_pipeline,
                    crate::pipeline::BlendMode::Multiply => &main.transparent_multiply_pipeline,
                };
                let woozy = item.wooze().and(self.pipelines.wooze_standard.as_deref());
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                if mode == crate::pipeline::BlendMode::Alpha {
                    // Back faces first so the front faces blend over them.
                    render_pass.set_pipeline(
                        woozy.map_or(&main.transparent_alpha_back_pipeline, |w| &w.alpha_back),
                    );
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
                render_pass.set_pipeline(woozy.map_or(pipeline, |w| w.blended(mode)));
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Skinned(draw) if item.blend == Blend::Opaque => {
                let Some(sp) = &self.pipelines.skinned else {
                    return;
                };
                // Rebound only when a draw needs another of the four (woozy or
                // not, double-sided or not)
                let woozy = item.wooze().and(self.pipelines.wooze_skinned.as_deref());
                let wants = (woozy.is_some(), draw.double_sided);
                if state.skinned != Some(wants) {
                    state.skinned = Some(wants);
                    render_pass.set_pipeline(match (woozy, draw.double_sided) {
                        (Some(w), true) => &w.opaque_double_sided,
                        (Some(w), false) => &w.opaque,
                        (None, true) => &sp.pipeline_double_sided,
                        (None, false) => &sp.pipeline,
                    });
                }
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                // Light bind group 2 is already set
                render_pass.set_bind_group(3, &*draw.bone_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Skinned(draw) => {
                let Some(sp) = &self.pipelines.skinned else {
                    return;
                };
                let mode = item.blend.mesh_mode();
                let pipeline = match mode {
                    crate::pipeline::BlendMode::Alpha => &sp.transparent_alpha_pipeline,
                    crate::pipeline::BlendMode::Additive => &sp.transparent_additive_pipeline,
                    crate::pipeline::BlendMode::Multiply => &sp.transparent_multiply_pipeline,
                };
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
                render_pass.set_bind_group(3, &*draw.bone_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                let woozy = item.wooze().and(self.pipelines.wooze_skinned.as_deref());
                if draw.soft_edge_only && woozy.is_none() {
                    render_pass.set_pipeline(&sp.soft_edge_pipeline);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    return;
                }
                if mode == crate::pipeline::BlendMode::Alpha {
                    render_pass.set_pipeline(
                        woozy.map_or(&sp.transparent_alpha_back_pipeline, |w| &w.alpha_back),
                    );
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
                render_pass.set_pipeline(woozy.map_or(pipeline, |w| w.blended(mode)));
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Billboard(draw) => {
                let Some(bp) = &self.pipelines.billboard else {
                    return;
                };
                render_pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                render_pass
                    .set_index_buffer(bp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..6, 0, 0..1);
            }
            // Front faces only, and in a fixed order with no depth write, so
            // they can't hide each other: drawing the back faces as well would
            // double every two-sided leaf card and muddy the crown. They test
            // against depth, so the part of a copy behind its own tree stays
            // hidden. A prop whose echoes may only add light (`echo_blend =
            // "lighten"`) draws them with a max blend instead.
            Draw::Echo(draw) => {
                let Some(sp) = &self.pipelines.skinned else {
                    return;
                };
                let lighten = item.blend == Blend::Lighten;
                if state.echo_lighten != Some(lighten) {
                    render_pass.set_pipeline(if lighten {
                        &sp.echo_lighten_pipeline
                    } else {
                        &sp.transparent_alpha_pipeline
                    });
                    state.echo_lighten = Some(lighten);
                }
                // A tree's echoes share a transform, and an echo's parts its
                // bones, so a grove rebinds those far less often than it draws.
                if state
                    .transform
                    .is_none_or(|t| !std::sync::Arc::ptr_eq(t, &draw.transform_bind_group))
                {
                    render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                    state.transform = Some(&draw.transform_bind_group);
                }
                if state
                    .bones
                    .is_none_or(|b| !std::sync::Arc::ptr_eq(b, &draw.bone_bind_group))
                {
                    render_pass.set_bind_group(3, &*draw.bone_bind_group, &[]);
                    state.bones = Some(&draw.bone_bind_group);
                }
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            Draw::Particles(n) => {
                let (Some(pipeline), Some(draw)) = (
                    &self.pipelines.particle,
                    self.particle_draws.get(*n as usize),
                ) else {
                    return;
                };
                if state.particles != Some(draw.blend) {
                    render_pass.set_pipeline(pipeline.pipeline(draw.blend));
                    state.particles = Some(draw.blend);
                }
                render_pass.set_bind_group(2, draw.texture_bind_group.as_ref(), &[]);
                let first = draw.first_instance;
                render_pass.draw_indexed(0..6, 0, first..first + draw.instance_count);
            }
        }
    }

    /// Each style id's screen bounds this frame (#385): the projected world
    /// bounds of exactly the draws the mask draws for it (terrain chunks in
    /// view, opaque and transparent models and boxes, skinned meshes as
    /// posed, billboards' quads), so a styled pixel is always inside its
    /// id's rect.
    pub(super) fn style_bounds(&self, view_proj: &[[f32; 4]; 4]) -> crate::style_mask::StyleBounds {
        use crate::style_mask::IdBounds;
        let mut bounds = crate::style_mask::StyleBounds::known();
        let ids = &self.style_mask.by_entity;
        if ids.is_empty() {
            return bounds;
        }
        let frustum = self.camera_frustum.as_ref();
        let of = |aabb: Option<([f32; 3], [f32; 3])>| match aabb {
            Some((min, max)) if frustum.is_some_and(|f| !f.aabb_visible(min, max)) => {
                IdBounds::Nowhere
            }
            Some((min, max)) => IdBounds::of_box(view_proj, min, max),
            None => IdBounds::Everywhere,
        };
        for item in self.draws.drawn() {
            let Some(id) = item.entity.as_ref().and_then(|e| ids.get(e).copied()) else {
                continue;
            };
            // What the mask draws for an id, and where
            let aabb = match &item.draw {
                Draw::Terrain(n) => match self.terrain.draws.get(*n as usize) {
                    Some(d) => Some((d.aabb_min, d.aabb_max)),
                    None => continue,
                },
                Draw::Static(d) if !d.is_wireframe => d.world_aabb.or(d.style_aabb),
                Draw::Skinned(d) => d.world_aabb,
                // Its quad under any turn of the camera (#1011)
                Draw::Billboard(d) => Some(d.world_aabb),
                // Not in the mask
                Draw::Static(_) | Draw::Echo(_) | Draw::Particles(_) => continue,
            };
            bounds.add(id, of(aabb));
        }
        bounds
    }

    /// Run post-processing: SSAO, bloom, composite HDR → sRGB.
    /// `depth_key`: who vouches for `depth_view` (the HDR chain for the
    /// scaled scene's own, the caller's id for theirs), so bind groups over
    /// it can be kept (#139, #379).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_postprocess(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey<'_>,
        target_view: &wgpu::TextureView,
        camera: &Camera,
        scopes: &mut crate::frame_capture::FrameScopes,
    ) {
        flint_core::scope!("render_postprocess");
        let pp = match &self.postprocess_pipeline {
            Some(pp) => pp,
            None => return,
        };
        let resources = match &self.postprocess_resources {
            Some(r) => r,
            None => return,
        };

        // GPU timing (ADR 0108): each effect is a scope of its own (#120),
        // so a slow frame names its pass, and a debug group of the same name
        // for GPU debuggers (#1083). All of them are in the frame's one
        // encoder (#139).
        let config = &self.postprocess_config;
        // The depth the post chain reads, in the description's names (#1084)
        let sd = self.scene_depth_id();

        // Stylize passes (#262). The frame every style reads; each stage
        // sets its own size and format.
        let ids = &self.style_mask.ids;
        // The `style` debug view shows the mask over the plain scene (#334):
        // mask_view runs alone, so no other style's output speckles its hues
        let solo = (self.debug_state.mode == DebugMode::StyleMask).then_some("mask_view");
        // and the `edges` view (#349) shows the ASCII style's edge passes
        // over the plain scene: ASCII alone, drawing its edge view
        let solo = solo.or((self.debug_state.mode == DebugMode::StyleEdges).then_some("ascii"));
        let hdr_styles = crate::stylize::solo(
            pp.styles
                .active(crate::stylize::StyleStage::Hdr, config, ids, &self.styles),
            solo,
        );
        let display_styles = crate::stylize::solo(
            pp.styles.active(
                crate::stylize::StyleStage::Display,
                config,
                ids,
                &self.styles,
            ),
            solo,
        );
        // The styles this frame runs, for its description (#1084)
        if let Some(styles) = scopes.log().and_then(|l| l.styles()) {
            for (stage, list) in [("hdr", &hdr_styles), ("display", &display_styles)] {
                styles.extend(
                    list.iter()
                        .map(|(d, _)| crate::frame_capture::StyleDescription {
                            name: d.name.to_string(),
                            stage: stage.to_string(),
                            blend: config.styles.blend(d.name),
                        }),
                );
            }
        }
        // The entity style mask (#263), drawn this frame by encode_to while
        // a style that reads it is on
        let style_mask = self.styles.any_uses_mask(config, ids).then(|| {
            resources.styles.get(
                device,
                crate::style_mask::MASK_TARGET,
                crate::style_mask::STYLE_MASK_FORMAT,
                resources.width,
                resources.height,
            )
        });
        // The style albedo (#869), drawn this frame by encode_to while a
        // style that reads it is on
        let style_albedo = self.styles.any_uses_albedo(config, ids).then(|| {
            resources.styles.get(
                device,
                crate::style_albedo::ALBEDO_TARGET,
                crate::style_albedo::ALBEDO_FORMAT,
                resources.width,
                resources.height,
            )
        });
        // Where each id's entities are (#385), for styles' scissors, and
        // with the scissor off too for what a style keeps only around its
        // entities (#930: ASCII's edge cells)
        let style_bounds = if style_mask.is_some() {
            self.style_bounds(&camera.view_projection_matrix())
        } else {
            crate::style_mask::StyleBounds::default()
        };
        let style_assets = self.asset_scope();
        let mut style_frame = (!hdr_styles.is_empty() || !display_styles.is_empty()).then(|| {
            crate::stylize::StyleFrame {
                device,
                queue,
                config,
                descriptor: hdr_styles.first().or(display_styles.first()).unwrap().0,
                styles: &self.styles,
                width: resources.width,
                height: resources.height,
                format: crate::postprocess::HDR_FORMAT,
                depth: depth_view,
                depth_size: [resources.width, resources.height],
                depth_key,
                camera: crate::stylize::StyleCamera {
                    view_proj: camera.view_projection_matrix(),
                    inv_view_proj: camera.inverse_view_projection_matrix(),
                    position: camera.position_array(),
                    near: camera.near,
                    far: camera.far,
                },
                mask: style_mask.as_ref().map(|m| &m.view),
                mask_fades: self.style_mask_fades(),
                style_ids: &self.style_mask.ids,
                style_bounds: &style_bounds,
                scissor: self.style_mask.scissor,
                edges: None,
                albedo: style_albedo,
                linear_sampler: &pp.linear_sampler,
                // held on twos under a whole-frame motion_hold (#405)
                time: self.post_time(),
                dt: self.style_dt,
                frame: self
                    .style_frame_serial
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                history_epoch: self.style_history_epoch,
                pixels_per_point: self.pixels_per_point,
                scene_dir: self.scene_dir.as_deref(),
                assets: &style_assets,
                debug_mode: self.debug_state.mode,
                binds: &resources.binds,
                targets: &resources.styles,
                snapped_ownership: None,
            }
        });

        // Shared geometry edges (#301): once, before either stage, for every
        // style on this frame that needs them
        if let Some(frame) = &mut style_frame {
            let both: Vec<_> = hdr_styles.iter().chain(&display_styles).copied().collect();
            let edges =
                pp.styles
                    .encode_edges(&both, frame, encoder, &mut |label, encoder, run| {
                        self.run_style_part(device, scopes, label, encoder, run, None, None)
                    });
            frame.edges = edges;
            scopes.note(crate::stylize::edges::EDGE_LABEL, |p| {
                p.reads(&[sd]).writes(&["style edges"]);
            });
        }

        // HDR stage: Kuwahara and friends, in linear HDR at scene resolution
        let hdr_steps = crate::stylize::plan(hdr_styles.len(), false);
        // Where each style writes, for a captured frame's taps (#1085)
        let hdr_dsts = style_dsts(
            scopes.taps.is_some(),
            &hdr_styles,
            &hdr_steps,
            crate::stylize::StyleStage::Hdr,
            "hdr_scene",
            "",
        );
        let hdr_result = match &mut style_frame {
            Some(frame) if !hdr_styles.is_empty() => crate::stylize::run_stage(
                &hdr_styles,
                &hdr_steps,
                frame,
                crate::stylize::StyleStage::Hdr,
                encoder,
                crate::stylize::StyleInput {
                    view: &resources.hdr_view,
                    id: crate::stylize::input_id::HDR,
                },
                None,
                pp.styles.blend_pass(),
                pp.styles.ownership(),
                pp.styles.fade(),
                &mut |label, encoder, run| {
                    let dst = dst_of(&hdr_dsts, label);
                    let refuse = history_refusal(scopes.knobs.is_some(), &hdr_styles, label);
                    self.run_style_part(device, scopes, label, encoder, run, dst, refuse)
                },
            ),
            _ => None,
        };
        log_style_steps(
            scopes,
            &hdr_styles,
            &hdr_steps,
            crate::stylize::StyleStage::Hdr,
            "hdr_scene",
            "",
            sd,
        );
        self.log_skipped_styles(scopes, crate::stylize::StyleStage::Hdr, &hdr_styles);
        let hdr_scene = hdr_result.as_ref().map(|(t, id)| (&t.view, *id));
        // The target the HDR stage left its result in, by the
        // description's name: the ping its last step wrote
        let hdr_result_id = match hdr_steps.last() {
            Some(&(_, slot)) if hdr_scene.is_some() => slot_id(
                crate::stylize::StyleStage::Hdr,
                slot,
                "hdr_scene",
                "hdr_scene",
            ),
            _ => "hdr_scene",
        };

        // Run SSAO if enabled
        if config.enabled && config.ssao_enabled {
            scopes.run("SSAO", encoder, &mut |encoder| {
                pp.run_ssao(
                    queue, encoder, device, resources, config, depth_view, depth_key, camera,
                )
            });
            scopes.note("SSAO", |p| {
                p.reads(&[sd]).writes(&["ssao", "ssao_blur"]);
            });
            self.tap_pass(device, scopes, encoder, "SSAO");
        } else {
            scopes.skip("SSAO", || self.post_off_reason("ssao_enabled"));
        }

        // Run volumetric (god rays) if enabled and shadows are available
        if config.enabled && config.volumetric_enabled {
            if let Some(shadow_pass) = &self.shadow_pass {
                if self.shadows_drawn() {
                    scopes.run("Volumetric", encoder, &mut |encoder| {
                        pp.run_volumetric(
                            queue,
                            encoder,
                            device,
                            resources,
                            config,
                            depth_view,
                            depth_key,
                            camera,
                            crate::postprocess::VolumetricShadow {
                                view: &shadow_pass.shadow_view,
                                sampler: &shadow_pass.shadow_sampler,
                                uniforms: &shadow_pass.shadow_uniforms_buffer,
                                lights: &self.lights.buffer,
                            },
                        )
                    });
                    scopes.note("Volumetric", |p| {
                        p.reads(&[sd, "shadow_map"])
                            .writes(&["volumetric", "volumetric_blur"]);
                    });
                    self.tap_pass(device, scopes, encoder, "Volumetric");
                }
            }
            if !self.shadows_drawn() || self.shadow_pass.is_none() {
                scopes.skip("Volumetric", || {
                    "no sun shadow map to march through this frame".to_string()
                });
            }
        } else {
            scopes.skip("Volumetric", || self.post_off_reason("volumetric_enabled"));
        }

        // Run bloom if post-processing effects are enabled. It reads the
        // unstylized scene, unless an HDR style that ran says it feeds
        // bloom (#300): then the stage's result, so the style's own bright
        // shapes glow.
        if config.enabled && config.bloom_enabled && resources.bloom_mip_count > 0 {
            let source = match hdr_scene {
                Some((view, id)) if hdr_styles.iter().any(|(d, _)| d.feeds_bloom_in(config)) => {
                    crate::stylize::StyleInput { view, id }
                }
                _ => crate::stylize::StyleInput {
                    view: &resources.hdr_view,
                    id: crate::stylize::input_id::HDR,
                },
            };
            let from_style = source.id != crate::stylize::input_id::HDR;
            // A captured frame reads each step's mip as it's written (#1085):
            // the upsample writes over the downsample's
            flint_core::scope!("encode_part", "Bloom");
            // Each step overwrites its mip (the upsample adds into what
            // the downsample just wrote), so a repeat is the same work
            let bloom_times = scopes.times("Bloom");
            if let Some(scope) = scopes.begin_times(encoder, "Bloom", bloom_times) {
                scopes.repeat(bloom_times, |scopes| {
                    if scopes.taps.is_some() {
                        pp.run_bloom(
                            queue,
                            encoder,
                            device,
                            resources,
                            config,
                            &source,
                            Some(&mut |encoder, mip, step| {
                                let id = format!("bloom_mip_{mip}");
                                self.tap_step(device, scopes, encoder, "Bloom", &id, step)
                            }),
                        );
                    } else {
                        pp.run_bloom(queue, encoder, device, resources, config, &source, None);
                    }
                });
                scopes.end(encoder, scope);
                let mips = resources.bloom_mips.len();
                scopes.note("Bloom", |p| {
                    p.reads(&[if from_style {
                        hdr_result_id
                    } else {
                        "hdr_scene"
                    }]);
                    for i in 0..mips {
                        p.writes(&[&format!("bloom_mip_{i}")]);
                    }
                });
            }
        } else if config.enabled && config.bloom_enabled {
            scopes.skip("Bloom", || "no bloom mips at this size".to_string());
        } else {
            scopes.skip("Bloom", || self.post_off_reason("bloom_enabled"));
        }

        // FXAA (ADR 0050): when active, composite renders into the lazily
        // allocated intermediate instead of the swapchain, and the FXAA pass
        // resolves intermediate → target. `encode_to` builds both before the
        // frame (#297); both Options are still checked, falling back to the
        // direct path.
        let fxaa_active = self.postprocess_config.enabled
            && self.postprocess_config.fxaa
            && pp.fxaa.is_some()
            && resources.fxaa.is_some();
        let final_target = if fxaa_active {
            &resources.fxaa.as_ref().unwrap().view
        } else {
            target_view
        };
        // Display stage (#262): with display styles on, the composite draws
        // into the stage's input at output resolution, and the last style
        // writes what the composite would have. The plan (#302) reuses the
        // input as a ping, and with FXAA on its intermediate as the other
        // texture, so the stage adds at most one full-size texture to
        // FXAA's (two without FXAA, from three styles on: in + one ping).
        let display_size = self.surface_size;
        let (composite_slot, display_steps) =
            crate::stylize::display_plan(display_styles.len(), fxaa_active);
        let display_in = (!display_styles.is_empty()).then(|| {
            resources.styles.get(
                device,
                "style display in",
                pp.surface_format,
                display_size.0,
                display_size.1,
            )
        });
        let composite_target = match (&display_in, composite_slot) {
            (Some(input), crate::stylize::Slot::Input) => &input.view,
            _ => final_target,
        };
        // The scene the composite reads: the HDR styles' result, or the
        // HDR buffer
        let scene = match hdr_scene {
            Some((view, id)) => crate::stylize::StyleInput { view, id },
            None => crate::stylize::StyleInput {
                view: &resources.hdr_view,
                id: crate::stylize::input_id::HDR,
            },
        };
        // Composite: HDR + bloom + SSAO + volumetric + fog → tonemapped sRGB surface
        // (always runs — this is what converts Rgba16Float → surface format)
        let composite_timed = !fxaa_active && scopes.timer.is_some();
        // It writes its target whole, so a repeat is the same work (#1317)
        let composite_times = scopes.times("Composite");
        if composite_times == 0 {
            scopes.skip("Composite", || {
                crate::frame_capture::SKIPPED_BY_KNOB.to_string()
            });
        } else if let Some(log) = scopes.log() {
            let pass = log.ran("Composite", composite_timed);
            pass.reads(&[hdr_result_id, sd]);
            if config.bloom_enabled && !resources.bloom_mips.is_empty() {
                pass.reads(&["bloom_mip_0"]);
            }
            if config.ssao_enabled {
                pass.reads(&["ssao_blur"]);
            }
            if config.volumetric_enabled {
                pass.reads(&["volumetric_blur"]);
            }
            pass.writes(&[match (&display_in, composite_slot, fxaa_active) {
                (Some(_), crate::stylize::Slot::Input, _) => "style display in",
                (_, _, true) => "fxaa_in",
                _ => "surface",
            }]);
        }
        pp.composite(
            queue,
            encoder,
            device,
            resources,
            &self.postprocess_config,
            composite_target,
            depth_view,
            depth_key,
            camera,
            // Same f64 phase wrap as the ocean upload — keeps mode animation
            // precise over long sessions. Headless render leaves this 0.0.
            // Held on twos under a whole-frame motion_hold (#405).
            self.post_time(),
            self.ground_fog.as_ref(),
            &scene,
            // The fog veil (#513), while this frame drew it and the
            // composite reads the HDR buffer it was drawn over (an HDR
            // style's result isn't that picture)
            resources
                .fog_veil
                .as_ref()
                .filter(|_| self.fog_veil.drawn && hdr_scene.is_none()),
            if fxaa_active {
                None
            } else {
                scopes.timer.as_mut()
            },
            composite_times,
        );
        if composite_times > 0 {
            self.tap_pass(device, scopes, encoder, "Composite");
        }

        if let (Some(frame), Some(input)) = (&mut style_frame, &display_in) {
            let display_dsts = style_dsts(
                scopes.taps.is_some(),
                &display_styles,
                &display_steps,
                crate::stylize::StyleStage::Display,
                "style display in",
                if fxaa_active { "fxaa_in" } else { "surface" },
            );
            frame.width = input.width;
            frame.height = input.height;
            frame.format = pp.surface_format;
            crate::stylize::run_stage(
                &display_styles,
                &display_steps,
                frame,
                crate::stylize::StyleStage::Display,
                encoder,
                crate::stylize::StyleInput {
                    view: &input.view,
                    id: crate::stylize::input_id::DISPLAY_IN,
                },
                // Read back only when it's FXAA's intermediate
                Some(crate::stylize::StyleInput {
                    view: final_target,
                    id: crate::stylize::input_id::FXAA_IN,
                }),
                pp.styles.blend_pass(),
                pp.styles.ownership(),
                pp.styles.fade(),
                &mut |label, encoder, run| {
                    let dst = dst_of(&display_dsts, label);
                    let refuse = history_refusal(scopes.knobs.is_some(), &display_styles, label);
                    self.run_style_part(device, scopes, label, encoder, run, dst, refuse)
                },
            );
            log_style_steps(
                scopes,
                &display_styles,
                &display_steps,
                crate::stylize::StyleStage::Display,
                "style display in",
                if fxaa_active { "fxaa_in" } else { "surface" },
                sd,
            );
        }
        self.log_skipped_styles(scopes, crate::stylize::StyleStage::Display, &display_styles);

        if fxaa_active {
            let timed = scopes.timer.is_some();
            let fxaa_times = scopes.times("FXAA");
            if fxaa_times == 0 {
                scopes.skip("FXAA", || crate::frame_capture::SKIPPED_BY_KNOB.to_string());
            } else {
                if let Some(log) = scopes.log() {
                    log.ran("FXAA", timed)
                        .reads(&["fxaa_in"])
                        .writes(&["surface"]);
                }
                pp.run_fxaa(
                    queue,
                    encoder,
                    device,
                    resources,
                    target_view,
                    scopes.timer.as_mut(),
                    fxaa_times,
                );
                self.tap_pass(device, scopes, encoder, "FXAA");
            }
        } else {
            scopes.skip("FXAA", || self.post_off_reason("fxaa"));
        }
    }

    /// Log each style that is on but left out of this frame's chain, and
    /// why (#399's blend 0, #406's reveal 0).
    fn log_skipped_styles(
        &self,
        scopes: &mut crate::frame_capture::FrameScopes,
        stage: crate::stylize::StyleStage,
        ran: &[(
            &crate::stylize::StyleDescriptor,
            &dyn crate::stylize::StylePass,
        )],
    ) {
        if scopes.log.is_none() {
            return;
        }
        let config = &self.postprocess_config;
        for d in self.styles.iter() {
            if d.stage != stage || !d.is_on(config, &self.style_mask.ids) {
                continue;
            }
            if ran.iter().any(|(r, _)| std::ptr::eq(*r, d)) {
                continue;
            }
            let why = if !config.enabled {
                "post-processing is off".to_string()
            } else if config.styles.blend(d.name) <= 0.0 {
                "blend 0: the chain skips it, the frame is the one without it".to_string()
            } else if !config.styles.shows_in(d.name, config) {
                "not shown this frame (reveal 0, or a tear with no tear)".to_string()
            } else {
                "on but not built yet".to_string()
            };
            scopes.skip(d.label, || why);
        }
    }
}

/// Log what each style of one stage read and wrote, from the plan it ran
/// by (#1084): a step's source and destination slots as target ids.
fn log_style_steps(
    scopes: &mut crate::frame_capture::FrameScopes,
    styles: &[(
        &crate::stylize::StyleDescriptor,
        &dyn crate::stylize::StylePass,
    )],
    steps: &[(crate::stylize::Slot, crate::stylize::Slot)],
    stage: crate::stylize::StyleStage,
    input: &str,
    last: &str,
    sd: &str,
) {
    if scopes.log.is_none() {
        return;
    }
    for ((d, _), (src, dst)) in styles.iter().zip(steps.iter().copied()) {
        let (src, dst) = (
            slot_id(stage, src, input, last),
            slot_id(stage, dst, input, last),
        );
        scopes.note(d.label, |p| {
            p.reads(&[&src, sd]).writes(&[&dst]);
        });
    }
}

/// Each style of one stage with the target its step writes (#1085), by the
/// description's names: what a captured frame reads as the style ends.
/// Empty, and nothing allocated, unless the frame is `capturing`.
fn style_dsts(
    capturing: bool,
    styles: &[(
        &crate::stylize::StyleDescriptor,
        &dyn crate::stylize::StylePass,
    )],
    steps: &[(crate::stylize::Slot, crate::stylize::Slot)],
    stage: crate::stylize::StyleStage,
    input: &'static str,
    last: &'static str,
) -> Vec<(&'static str, &'static str)> {
    if !capturing {
        return Vec::new();
    }
    styles
        .iter()
        .zip(steps.iter().copied())
        .map(|((d, _), (_, dst))| (d.label, slot_id(stage, dst, input, last)))
        .collect()
}

/// The target `label`'s style writes, from [`style_dsts`].
fn dst_of(dsts: &[(&'static str, &'static str)], label: &str) -> Option<&'static str> {
    dsts.iter().find(|(l, _)| *l == label).map(|(_, d)| *d)
}

/// Why the style drawn as `label` draws once whatever a pass knob's repeat
/// asks (#1317): it keeps history from frame to frame. Asked only while a
/// host has knobs on.
fn history_refusal(
    knobs_on: bool,
    styles: &[(
        &crate::stylize::StyleDescriptor,
        &dyn crate::stylize::StylePass,
    )],
    label: &str,
) -> Option<&'static str> {
    (knobs_on
        && styles
            .iter()
            .any(|(d, p)| d.label == label && p.keeps_history()))
    .then_some(crate::pass_knobs::KEEPS_HISTORY)
}

/// A style stage's slot as the description names its target: the stage's
/// input, one of its two pings (`stylize`'s `HDR_PINGS` / `DISPLAY_PINGS`),
/// or where its last style writes.
fn slot_id<'a>(
    stage: crate::stylize::StyleStage,
    slot: crate::stylize::Slot,
    input: &'a str,
    last: &'a str,
) -> &'a str {
    let pings = match stage {
        crate::stylize::StyleStage::Hdr => ["style hdr ping 0", "style hdr ping 1"],
        crate::stylize::StyleStage::Display => ["style display ping 0", "style display ping 1"],
    };
    match slot {
        crate::stylize::Slot::Input => input,
        crate::stylize::Slot::Ping(n) => pings[n as usize & 1],
        crate::stylize::Slot::Final => last,
    }
}

/// Terrain chunk `n` of `terrain`, unless `frustum` can't see it.
fn chunk_in_view<'a>(
    terrain: &'a [crate::terrain_pipeline::TerrainDrawCall],
    frustum: Option<&crate::frustum::Frustum>,
    n: u32,
) -> Option<&'a crate::terrain_pipeline::TerrainDrawCall> {
    let draw = terrain.get(n as usize)?;
    let hidden = frustum.is_some_and(|f| !f.aabb_visible(draw.aabb_min, draw.aabb_max));
    (!hidden).then_some(draw)
}

/// [`SceneRenderer::main_pass_draws`] from the fields it reads, so a walk
/// that changes the draw list can ask it.
fn main_pass_draws(
    terrain: &[crate::terrain_pipeline::TerrainDrawCall],
    frustum: Option<&crate::frustum::Frustum>,
    item: &DrawItem,
) -> bool {
    match &item.draw {
        Draw::Terrain(n) => chunk_in_view(terrain, frustum, *n).is_some(),
        Draw::Static(d) => item.blend == Blend::Opaque || !d.is_wireframe,
        Draw::Skinned(_) | Draw::Echo(_) | Draw::Billboard(_) | Draw::Particles(_) => true,
    }
}
