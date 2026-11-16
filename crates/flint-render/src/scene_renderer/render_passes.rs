//! Render pass helpers for `render_to()`.
//!
//! Each method handles one stage of the multi-pass rendering pipeline.

use super::helpers::identity_matrix;
use super::{Blend, Draw, DrawItem, DrawList, Layer, SceneRenderer};
use crate::billboard_pipeline::BillboardUniforms;
use crate::camera::Camera;
use crate::debug::DebugMode;
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
    if item.lod_culled() {
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

impl SceneRenderer {
    /// Render depth from light perspective for cascaded shadow mapping, and
    /// from the casting spot's for its layer (#504).
    #[profiling::function]
    pub(super) fn render_shadow_pass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        camera_pos: [f32; 3],
    ) {
        // Off, forbidden by the scene, or no sun that gives any light (#627)
        // and no spot that casts (#504): nothing to draw
        let sun_drawn = self.shadows_drawn();
        let spot = self
            .spot_shadow_drawn()
            .then(|| self.lights.shadow_spot().copied())
            .flatten();
        let shadow_pass = match &mut self.shadow_pass {
            Some(sp) => sp,
            None => return,
        };

        if !sun_drawn && spot.is_none() {
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

        // The layers drawn, each with the matrix it is drawn through: the
        // cascades, then the spot's (#504). Grass casts into the nearest
        // cascades only: it takes no spot light.
        let mut layers: Vec<(usize, [[f32; 4]; 4])> = (0..cascades)
            .map(|cascade| {
                (
                    cascade,
                    shadow_pass.shadow_uniforms().cascade_view_proj[cascade],
                )
            })
            .collect();
        if spot.is_some() {
            layers.push((
                crate::shadow::SPOT_SHADOW_LAYER,
                shadow_pass.shadow_uniforms().spot_view_proj,
            ));
        }

        // Write shadow uniforms
        queue.write_buffer(
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
        for &(_, cascade_vp) in &layers {
            let cascade_frustum = crate::frustum::Frustum::from_view_projection(&cascade_vp);
            for (item, caster) in shadow_casters(&self.draws, &self.terrain.draws) {
                if !caster.visible(&cascade_frustum) {
                    continue;
                }
                if let Some(wooze) = item.wooze() {
                    draw_wooze.push((draw_uniforms.len() as u32, *wooze));
                }
                draw_uniforms.push(ShadowDrawUniforms {
                    light_view_proj: cascade_vp,
                    model: caster.model(),
                });
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
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Grass Shadow Transform"),
                    size: std::mem::size_of::<TransformUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&[uniforms]));
            }
        }
        let mut next_slot: u32 = 0;
        // A woozy caster draws through its own pipeline and a bind group that sees
        // its drift. If the pass was only just remade (a new resolution), a frame
        // draws plain.
        let shadow_wooze = shadow_pass.wooze.as_ref();

        // Render each layer: the cascades, then the spot's
        for &(cascade, cascade_vp) in &layers {
            let label = if cascade == crate::shadow::SPOT_SHADOW_LAYER {
                "Spot Shadow Pass"
            } else {
                "Shadow Cascade Pass"
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
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
            // Same per-cascade test as the uniform gather above, so the slots
            // line up.
            let cascade_frustum = crate::frustum::Frustum::from_view_projection(&cascade_vp);
            let mut bound = BoundPipeline::default();
            let mut casters = shadow_casters(&self.draws, &self.terrain.draws).peekable();
            // The terrain's chunks come first in the list; grass casts after them
            while casters
                .peek()
                .is_some_and(|(item, _)| item.layer() == super::Layer::Terrain)
            {
                let (_, caster) = casters.next().expect("peeked");
                if caster.visible(&cascade_frustum) {
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
                }
            }

            for (item, caster) in casters {
                if caster.visible(&cascade_frustum) {
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
                Draw::Billboard(draw) => queue.write_buffer(
                    &draw.billboard_buffer,
                    0,
                    bytemuck::cast_slice(&[billboard_uniforms]),
                ),
                Draw::Terrain(n) => {
                    if let Some(chunk) = self.terrain.draws.get(*n as usize) {
                        let uniforms = transform(chunk.model, chunk.model_inv_transpose);
                        queue.write_buffer(
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
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update terrain material buffer (tonemapping flag)
        if let Some(buf) = &self.terrain.material_buffer {
            // enable_tonemapping is at byte offset 12 (texture_tile(4) + metallic(4) + roughness(4))
            queue.write_buffer(buf, 12, bytemuck::cast_slice(&[tonemapping_u32]));
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
                queue.write_buffer(
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
                queue.write_buffer(
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

        arena.upload(queue);
        self.draw_bind_cache.arena = arena;
    }

    /// Execute the main render pass: skybox → terrain → entities → grid → sprites → particles → debug.
    #[profiling::function]
    pub(super) fn render_main_pass<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        wireframe_only: bool,
        queue: &wgpu::Queue,
        camera: &Camera,
        phase: RenderPhase,
    ) {
        // Bind lights once for the entire pass (group 2 is shared)
        render_pass.set_bind_group(2, &self.lights.bind_group, &[]);

        // Render sky at the far plane: the procedural sky (driven by the
        // `sky` component) replaces the texture skybox when present.
        if phase.draws_opaque() {
            self.sky.draw(
                render_pass,
                queue,
                camera,
                self.pipelines.sky.as_deref(),
                self.pipelines.skybox.as_deref(),
                &self.lights.uniforms.directional_lights[0],
                self.ocean_time,
            );
        }

        if wireframe_only {
            self.render_wireframe_only_pass(render_pass);
            if phase.draws_opaque() {
                self.draw_editor_grid(render_pass);
            }
        } else {
            // The editor grid is drawn in here, after the opaque geometry.
            self.render_normal_pass(render_pass, phase);
        }

        // Normal arrows pass (always uses line pipeline)
        if self.debug_state.show_normals && phase.draws_rest() {
            render_pass.set_pipeline(&self.pipelines.main.line_pipeline);
            for draw in &self.normal_arrow_draws {
                render_pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &*draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // Skeleton overlay pass (bone lines, drawn on top of everything — no depth test)
        if self.debug_state.show_skeleton && phase.draws_rest() {
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
        }

        // Editor/gizmo overlay (ADR 0068): same line pipeline, never gated
        // behind the skeleton toggle.
        if !self.debug_overlay_draws.is_empty() && phase.draws_rest() {
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
        render_pass.set_pipeline(&self.pipelines.main.overlay_line_pipeline);
        // Billboards may have left their texture group at slot 2.
        render_pass.set_bind_group(2, &self.lights.bind_group, &[]);
        render_pass.set_bind_group(0, &*grid.transform_bind_group, &[]);
        render_pass.set_bind_group(1, &*grid.material_bind_group, &[]);
        render_pass.set_vertex_buffer(0, grid.vertex_buffer.slice(..));
        render_pass.set_index_buffer(grid.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..grid.index_count, 0, 0..1);
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
                if self.grass.drawing {
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
                }
            }
        } // end opaque phase (terrain + grass)

        // Ocean rendering (after all opaque — writes depth so fog applies;
        // group 3 holds the grab-pass snapshots, or dummies when disabled)
        if self.water_active() && phase.draws_ocean() {
            self.water.draw(
                render_pass,
                self.pipelines.ocean.as_deref(),
                &self.lights.bind_group,
            );
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
                render_pass.set_pipeline(&sp.pipeline);
                render_pass
                    .set_index_buffer(sp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.set_bind_group(0, &sp.uniform_bind_group, &[]);
                for batch in &self.sprite2d_batches {
                    render_pass.set_bind_group(1, &batch.instance_bind_group, &[]);
                    render_pass.set_bind_group(2, &batch.texture_bind_group, &[]);
                    render_pass.draw_indexed(0..6, 0, 0..batch.instance_count);
                }
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
        let mut state = MainPassState::default();
        for item in items.iter().filter(|i| i.drawn()) {
            self.draw_main(render_pass, item, &mut state);
        }
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
                let Some(draw) = self.terrain.draws.get(*n as usize) else {
                    return;
                };
                if let Some(ref frustum) = self.camera_frustum {
                    if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                        return;
                    }
                }
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
    /// posed), so a styled pixel is always inside its id's rect.
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
                // Not in the mask (a styled billboard is #1011)
                Draw::Static(_) | Draw::Echo(_) | Draw::Billboard(_) | Draw::Particles(_) => {
                    continue
                }
            };
            bounds.add(id, of(aabb));
        }
        bounds
    }

    /// Run post-processing: SSAO, bloom, composite HDR → sRGB.
    #[profiling::function]
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
        depth_key: crate::bind_cache::DepthKey,
        target_view: &wgpu::TextureView,
        camera: &Camera,
        mut timer: Option<&mut crate::gpu_timer::GpuTimer>,
    ) {
        let pp = match &self.postprocess_pipeline {
            Some(pp) => pp,
            None => return,
        };
        let resources = match &self.postprocess_resources {
            Some(r) => r,
            None => return,
        };

        // GPU timing (ADR 0108): each effect is a scope of its own (#120),
        // so a slow frame names its pass. All of them are in the frame's
        // one encoder (#139).
        let mut scoped = |label: &'static str,
                          encoder: &mut wgpu::CommandEncoder,
                          run: &mut dyn FnMut(&mut wgpu::CommandEncoder)| {
            let scope = timer.as_deref_mut().map(|t| t.begin(encoder, label));
            run(encoder);
            if let (Some(t), Some(scope)) = (timer.as_deref_mut(), scope) {
                t.end(encoder, scope);
            }
        };
        let config = &self.postprocess_config;

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
                .active(crate::stylize::StyleStage::Hdr, config, ids),
            solo,
        );
        let display_styles = crate::stylize::solo(
            pp.styles
                .active(crate::stylize::StyleStage::Display, config, ids),
            solo,
        );
        // The entity style mask (#263), drawn this frame by encode_to while
        // a style that reads it is on
        let style_mask = crate::stylize::any_uses_mask(config, ids).then(|| {
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
        let style_albedo = crate::stylize::any_uses_albedo(config, ids).then(|| {
            resources.styles.get(
                device,
                crate::style_albedo::ALBEDO_TARGET,
                crate::style_albedo::ALBEDO_FORMAT,
                resources.width,
                resources.height,
            )
        });
        // Where each id's entities are (#385), for styles' scissors
        let style_bounds = if style_mask.is_some() && self.style_mask.scissor {
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
            }
        });

        // Shared geometry edges (#301): once, before either stage, for every
        // style on this frame that needs them
        if let Some(frame) = &mut style_frame {
            let both: Vec<_> = hdr_styles.iter().chain(&display_styles).copied().collect();
            frame.edges =
                pp.styles
                    .encode_edges(&both, frame, encoder, &mut |label, encoder, run| {
                        scoped(label, encoder, run)
                    });
        }

        // HDR stage: Kuwahara and friends, in linear HDR at scene resolution
        let hdr_result = match &mut style_frame {
            Some(frame) if !hdr_styles.is_empty() => crate::stylize::run_stage(
                &hdr_styles,
                &crate::stylize::plan(hdr_styles.len(), false),
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
                &mut |label, encoder, run| scoped(label, encoder, run),
            ),
            _ => None,
        };
        let hdr_scene = hdr_result.as_ref().map(|(t, id)| (&t.view, *id));

        // Run SSAO if enabled
        if config.enabled && config.ssao_enabled {
            scoped("SSAO", encoder, &mut |encoder| {
                pp.run_ssao(
                    queue, encoder, device, resources, config, depth_view, depth_key, camera,
                )
            });
        }

        // Run volumetric (god rays) if enabled and shadows are available
        if config.enabled && config.volumetric_enabled {
            if let Some(shadow_pass) = &self.shadow_pass {
                if self.shadows_drawn() {
                    scoped("Volumetric", encoder, &mut |encoder| {
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
                                id: shadow_pass.id,
                                view: &shadow_pass.shadow_view,
                                sampler: &shadow_pass.shadow_sampler,
                                uniforms: &shadow_pass.shadow_uniforms_buffer,
                                lights: &self.lights.buffer,
                            },
                        )
                    });
                }
            }
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
            scoped("Bloom", encoder, &mut |encoder| {
                pp.run_bloom(queue, encoder, device, resources, config, &source)
            });
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
                timer.as_deref_mut()
            },
        );

        if let (Some(frame), Some(input)) = (&mut style_frame, &display_in) {
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
                    let scope = timer.as_deref_mut().map(|t| t.begin(encoder, label));
                    run(encoder);
                    if let (Some(t), Some(scope)) = (timer.as_deref_mut(), scope) {
                        t.end(encoder, scope);
                    }
                },
            );
        }

        if fxaa_active {
            pp.run_fxaa(queue, encoder, device, resources, target_view, timer);
        }
    }
}
