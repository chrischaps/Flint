//! Per-entity styles' state, owned by [`StyleMaskState`] (ADR 0118, #736):
//! which styles entities ask for and the id each styled entity has in the
//! mask (#263), how far each is drawn into its style (#278) and which
//! follow the tear (#397) or the screen field (#859), the frame's field,
//! whether the last mask drew a veil (#290), the scissor switch (#385), and
//! the mask and albedo passes (#869).
//!
//! What the passes draw is gathered here too, by `SceneRenderer`'s
//! `encode_style_mask` and `encode_style_albedo`: each is a selector over
//! the frame's one draw list (#737), and the state holds no draws of its own.

use super::{Blend, Draw, DrawItem, SceneRenderer, SteppedPass};
use crate::camera::Camera;
use crate::postprocess::PostProcessConfig;
use crate::style_albedo::StyleAlbedoPass;
use crate::style_mask::{BlendSource, StyleIds, StyleMaskPass};
use flint_core::style_field::StyleField;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Who is styled and how, and the passes that draw them into the mask
/// and the albedo.
pub(crate) struct StyleMaskState {
    /// The passes, at the view's size; a comic panel keeps its own.
    passes: StylePasses,
    /// Style names entities ask for, numbered for the mask.
    pub(crate) ids: StyleIds,
    /// Styled entities -> their mask id, rebuilt each `update_from_world`
    /// while the mask is wanted (empty otherwise).
    pub(crate) by_entity: HashMap<EntityId, u16>,
    /// Whether per-entity styles scissor their passes to their entities'
    /// screen bounds (#385); `FLINT_STYLE_SCISSOR=0` starts it off.
    pub(crate) scissor: bool,
    /// The blend of each styled entity drawn at less than 1 (#278).
    pub(crate) blend_by_entity: HashMap<EntityId, f32>,
    /// The styled entities whose blend follows the reality tear (#397) or
    /// the screen field (#859); one at its own blend isn't listed.
    pub(crate) blend_sources: HashMap<EntityId, BlendSource>,
    /// The frame's screen field (#859), from `FrameInputs::style_field`:
    /// empty unless the host filled one this frame.
    pub(crate) field: StyleField,
    /// Whether the last style mask drew a veil over its entities (#290):
    /// something unstyled in front, so the mask's blend is below 1 there
    /// even with every entity fully styled (#396 cross-fades by it).
    veiled: bool,
}

/// The entity style mask's pass (#263) and the style albedo's (#869), each
/// made the first frame that needs it. A comic panel's `ViewSlot` holds
/// its own pair and swaps it in around its draw.
#[derive(Default)]
pub(crate) struct StylePasses {
    pub(crate) mask: Option<StyleMaskPass>,
    pub(crate) albedo: Option<StyleAlbedoPass>,
}

impl StyleMaskState {
    pub(crate) fn new() -> Self {
        Self {
            passes: StylePasses::default(),
            ids: Default::default(),
            by_entity: HashMap::new(),
            scissor: crate::stylize::scissor_enabled(),
            blend_by_entity: HashMap::new(),
            blend_sources: HashMap::new(),
            field: StyleField::default(),
            veiled: false,
        }
    }

    /// Ids for every styled entity: they say which styles are on for
    /// entities alone (#266) as well as what the mask holds (#263).
    pub(crate) fn resolve(
        &mut self,
        world: &FlintWorld,
        styles: &crate::stylize::StyleRegistry,
        config: &PostProcessConfig,
    ) {
        crate::style_mask::resolve_blended(
            world,
            styles,
            config,
            &mut self.ids,
            &mut self.by_entity,
            &mut self.blend_by_entity,
            &mut self.blend_sources,
        );
    }

    /// No styled entities: post-processing is off.
    pub(crate) fn clear(&mut self) {
        self.ids.set(Vec::new());
        self.by_entity.clear();
        self.blend_by_entity.clear();
        self.blend_sources.clear();
    }

    /// Whether some pixel of the last style mask may hold a blend below 1
    /// (#396). See [`SceneRenderer::style_mask_fades`].
    pub(crate) fn fades(&self, config: &PostProcessConfig) -> bool {
        let tear = crate::postprocess::tear_active(config);
        !self.blend_by_entity.is_empty()
            || self.veiled
            || self.blend_sources.values().any(|s| match s {
                BlendSource::Own => false,
                BlendSource::Tear => tear,
                BlendSource::Field => !self.field.is_empty(),
            })
    }

    /// The mask's per-draw uniform bytes and how many styled draws the last
    /// frame masked (#311); `None` before a frame needed the mask.
    pub(crate) fn draw_stats(&self) -> Option<(u64, usize)> {
        self.passes
            .mask
            .as_ref()
            .map(|p| (p.draw_buffer_bytes(), p.last_draws()))
    }

    /// Trade this view's passes for a panel's (`panels::ViewSlot`).
    pub(crate) fn swap_passes(&mut self, other: &mut StylePasses) {
        std::mem::swap(&mut self.passes, other);
    }
}

impl SceneRenderer {
    /// Record the style mask pass over this frame's visible draws of
    /// styled entities: terrain chunks, opaque static, procedural and
    /// skinned draws and billboards (#1011), then transparents over them
    /// (#310). The mask is cleared even with none, so a style never reads
    /// a stale frame.
    pub(super) fn encode_style_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey<'_>,
    ) {
        flint_core::scope!("encode_style_mask");
        self.style_mask.veiled = false;
        // A replay stepping the mask (#1204): its styled draws past the limit
        let (state, terrain, frustum) = (
            &self.style_mask,
            &self.terrain.draws,
            self.camera_frustum.as_ref(),
        );
        self.draws.cut_pass(SteppedPass::StyleMask, |item| {
            item.drawn() && mask_draw(item, state, terrain, frustum).is_some()
        });
        let Some(resources) = &self.postprocess_resources else {
            return;
        };
        let Some(skinned) = &self.pipelines.skinned else {
            return;
        };
        let pass = self.style_mask.passes.mask.get_or_insert_with(|| {
            crate::style_mask::StyleMaskPass::new(
                device,
                &crate::style_mask::MaskLayouts {
                    transform: &self.layouts.transform_bind_group_layout,
                    material: &self.layouts.material_bind_group_layout,
                    light: &self.layouts.light_bind_group_layout,
                    bones: &skinned.bone_bind_group_layout,
                },
            )
        });
        // Under MSAA the mask is drawn 4x against the scene's own samples
        // and resolved, so a feature thinner than a pixel keeps the pixels
        // the scene's samples gave it (#306).
        let msaa_depth = match self.msaa.depth_at(resources.width, resources.height) {
            Some(view) if self.pipelines.samples() == crate::style_mask::MSAA_SAMPLES => {
                pass.ensure_msaa(
                    device,
                    &self.layouts.transform_bind_group_layout,
                    &skinned.bone_bind_group_layout,
                );
                Some(crate::style_mask::MsaaDepth { view })
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
        let ids = &self.style_mask.by_entity;
        let id_of = |e: &Option<flint_core::EntityId>| e.as_ref().and_then(|e| ids.get(e).copied());
        let frustum = self.camera_frustum.as_ref();
        let mut draws = Vec::new();
        if !ids.is_empty() {
            draws.extend(
                self.draws
                    .drawn()
                    .filter(|item| self.draws.within(item, SteppedPass::StyleMask))
                    .filter_map(|item| {
                        mask_draw(item, &self.style_mask, &self.terrain.draws, frustum)
                    }),
            );
        }
        for d in &mut draws {
            d.woozy = self.masks_woozy(d);
        }
        if draws.iter().any(|d| d.woozy) {
            if let Some(pass) = &mut self.style_mask.passes.mask {
                pass.ensure_wooze(
                    device,
                    &self.layouts.transform_bind_group_layout,
                    &skinned.bone_bind_group_layout,
                );
            }
        }
        // A styled billboard (#1011): the sprite's layouts and quad are the
        // billboard pipeline's, which drew it
        if let (Some(pass), Some(bp)) = (
            &mut self.style_mask.passes.mask,
            self.pipelines.billboard.as_deref(),
        ) {
            if draws.iter().any(|d| d.is_billboard()) {
                pass.ensure_billboard(
                    device,
                    &bp.billboard_bind_group_layout,
                    &bp.texture_bind_group_layout,
                    &bp.quad_index_buffer,
                );
            }
        }
        // The veil's selector (#290): what's in front of them, every item
        // that blends and isn't styled itself, drawn as its coverage
        let mut veil = crate::style_mask::Veil {
            light_bind_group: &self.lights.bind_group,
            meshes: Vec::new(),
            particles: None,
        };
        if !draws.is_empty() {
            let mut particles = false;
            let veils =
                |item: &&super::DrawItem| item.blend.blends() && id_of(&item.entity).is_none();
            for item in self.draws.drawn().filter(veils) {
                // Through the vertex stage the main pass drew it with
                // (#1241), so it covers where it drifted to
                let woozy = self
                    .main_pipe(item)
                    .is_some_and(super::render_passes::MainPipe::woozy);
                match &item.draw {
                    Draw::Static(d) if !d.is_wireframe => {
                        veil.meshes.push(crate::style_mask::VeilMesh {
                            vertex_buffer: &d.vertex_buffer,
                            index_buffer: &d.index_buffer,
                            index_count: d.index_count,
                            transform_bind_group: &d.transform_bind_group,
                            material_bind_group: &d.material_bind_group,
                            bones: None,
                            echo: false,
                            woozy,
                        })
                    }
                    Draw::Skinned(d) => veil.meshes.push(crate::style_mask::VeilMesh {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        material_bind_group: &d.material_bind_group,
                        bones: Some(&d.bone_bind_group),
                        echo: false,
                        woozy,
                    }),
                    // A living prop's echoes (#228, #737), translucent and
                    // depth-tested with no depth write, each by its alpha: a
                    // lightening echo (`echo_blend = "lighten"`) too, since
                    // the veil can't know how much light it adds there
                    Draw::Echo(d) => veil.meshes.push(crate::style_mask::VeilMesh {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        material_bind_group: &d.material_bind_group,
                        bones: Some(&d.bone_bind_group),
                        echo: true,
                        woozy: false,
                    }),
                    // Every particle draw goes in, as the upload sorted them
                    Draw::Particles(_) => particles = true,
                    // Billboards and terrain don't blend (a billboard's alpha
                    // cuts it out, and what's kept writes depth, so the mask's
                    // own depth test leaves it out); a wireframe isn't a surface
                    Draw::Static(_) | Draw::Billboard(_) | Draw::Terrain(_) => {}
                }
            }
            veil.particles = match (&self.pipelines.particle, &self.particle_buffers) {
                (Some(pipeline), Some(buffers)) if particles => {
                    Some(crate::style_mask::VeilParticles {
                        pipeline,
                        buffers,
                        draws: &self.particle_draws,
                    })
                }
                _ => None,
            };
        }
        if veil.meshes.iter().any(|m| m.woozy) {
            if let Some(pass) = &mut self.style_mask.passes.mask {
                pass.ensure_veil_wooze(
                    device,
                    &crate::style_mask::MaskLayouts {
                        transform: &self.layouts.transform_bind_group_layout,
                        material: &self.layouts.material_bind_group_layout,
                        light: &self.layouts.light_bind_group_layout,
                        bones: &skinned.bone_bind_group_layout,
                    },
                );
            }
        }
        let veiled = !veil.is_empty();
        // The frame's tear (#397, #746), for the draws whose blend follows
        // it: the same config and post time the styles that follow it read
        let config = &self.postprocess_config;
        let tear = crate::style_mask::Tear {
            on: crate::postprocess::tear_active(config),
            params: crate::postprocess::tear_params(config, self.post_time()),
        };
        if let Some(pass) = &mut self.style_mask.passes.mask {
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
                tear,
                &self.style_mask.field,
            );
        }
        self.style_mask.veiled = veiled;
    }

    /// Record the style albedo (#869): the frame's opaque static,
    /// procedural and skinned draws once more, base colour only, against
    /// the depth the scene drew (its samples under MSAA, resolved), for the
    /// styles that divide the picture by it. Woozy draws (#215) move their
    /// vertices in a stage of their own and are left out, as are terrain,
    /// grass and the ocean: their pixels read as no mesh.
    pub(super) fn encode_style_albedo(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
    ) {
        flint_core::scope!("encode_style_albedo");
        let Some(resources) = &self.postprocess_resources else {
            return;
        };
        let Some(skinned) = &self.pipelines.skinned else {
            return;
        };
        let target = resources.styles.get(
            device,
            crate::style_albedo::ALBEDO_TARGET,
            crate::style_albedo::ALBEDO_FORMAT,
            resources.width,
            resources.height,
        );
        // The scene's own samples while it drew 4x at this size
        let (scene_depth, samples) = match self.msaa.depth_at(resources.width, resources.height) {
            Some(view) if self.pipelines.samples() > 1 => (view, self.pipelines.samples()),
            _ => (depth_view, 1),
        };
        // A replay stepping the albedo (#1204): its meshes past the limit
        self.draws.cut_pass(SteppedPass::StyleAlbedo, |item| {
            item.drawn() && albedo_mesh(item).is_some()
        });
        let meshes: Vec<crate::style_albedo::AlbedoMesh<'_>> = self
            .draws
            .drawn()
            .filter(|item| self.draws.within(item, SteppedPass::StyleAlbedo))
            .filter_map(albedo_mesh)
            .collect();
        let layouts = crate::style_albedo::AlbedoLayouts {
            transform: &self.layouts.transform_bind_group_layout,
            material: &self.layouts.material_bind_group_layout,
            light: &self.layouts.light_bind_group_layout,
            bones: &skinned.bone_bind_group_layout,
        };
        let pass = self
            .style_mask
            .passes
            .albedo
            .get_or_insert_with(|| crate::style_albedo::StyleAlbedoPass::new(device));
        pass.encode(
            device,
            encoder,
            &layouts,
            &target,
            scene_depth,
            samples,
            &self.lights.bind_group,
            &meshes,
        );
    }

    /// The style ids the mask uses: which number each `style_mask` style
    /// name has this frame.
    pub fn style_ids(&self) -> &crate::style_mask::StyleIds {
        &self.style_mask.ids
    }

    /// The style mask id of each styled entity, as of the last
    /// [`update_from_world`](Self::update_from_world) (empty while no
    /// style reads the mask).
    pub fn styled_entities(&self) -> &HashMap<flint_core::EntityId, u16> {
        &self.style_mask.by_entity
    }

    /// How much of its style each styled entity drawn at less than 1 is
    /// drawn with (#278); an entity missing here is fully styled.
    pub fn styled_blends(&self) -> &HashMap<flint_core::EntityId, f32> {
        &self.style_mask.blend_by_entity
    }

    /// Whether some pixel of the last style mask may hold a blend below 1
    /// (#396): an entity drawn part-way into its style (#278), or a veil
    /// over a styled one (#290). While neither, the framework's
    /// cross-fade is skipped.
    pub fn style_mask_fades(&self) -> bool {
        self.style_mask.fades(&self.postprocess_config)
    }

    /// The entity style mask's per-draw uniform bytes and how many styled
    /// draws the last frame masked (#311); `None` before a frame needed
    /// the mask.
    pub fn style_mask_draws(&self) -> Option<(u64, usize)> {
        self.style_mask.draw_stats()
    }

    /// Whether per-entity styles draw only inside their entities' screen
    /// bounds (#385). On by default (`FLINT_STYLE_SCISSOR=0` starts it
    /// off); the frame is the same either way, only cheaper with it on.
    pub fn set_style_scissor(&mut self, on: bool) {
        self.style_mask.scissor = on;
    }

    pub fn style_scissor(&self) -> bool {
        self.style_mask.scissor
    }

    /// Where each style id's entities are on screen under `camera`, from
    /// the draws of the last [`update_from_world`](Self::update_from_world)
    /// (#385): what the per-entity style scissor reads.
    pub fn style_screen_bounds(&self, camera: &Camera) -> crate::style_mask::StyleBounds {
        self.style_bounds(&camera.view_projection_matrix())
    }

    /// Whether this frame draws the entity style mask: post-processing is
    /// up and a style that reads it is on (#263).
    pub(super) fn style_mask_wanted(&self) -> bool {
        self.postprocess_resources.is_some()
            && self
                .styles
                .any_uses_mask(&self.postprocess_config, &self.style_mask.ids)
    }

    /// Log the style mask's styled draws (#1204): the same selector
    /// ([`mask_draw`]) over the same drawn items, with those a replay's
    /// limit set aside beside them. The veil drawn with them is `also`.
    pub(super) fn log_style_mask_draws(&self, pass: &mut crate::frame_capture::LoggedPass) {
        let frustum = self.camera_frustum.as_ref();
        if !self.style_mask.by_entity.is_empty() {
            for item in self.draws.drawn() {
                let Some(d) = mask_draw(item, &self.style_mask, &self.terrain.draws, frustum)
                else {
                    continue;
                };
                let name = match (d.is_billboard(), d.bones().is_some(), self.masks_woozy(&d)) {
                    (true, _, _) => "style mask billboard",
                    (false, false, false) => "style mask",
                    (false, true, false) => "style mask skinned",
                    (false, false, true) => "style mask woozy",
                    (false, true, true) => "style mask woozy skinned",
                };
                if self.draws.within(item, SteppedPass::StyleMask) {
                    pass.draws.push(self.logged_draw(item, None, name));
                } else {
                    pass.culled
                        .push(self.logged_draw(item, Some("past the draw limit"), name));
                }
            }
        }
        if self.style_mask.veiled {
            pass.pass.also.push("Veil".into());
        }
    }

    /// Whether the mask draws `d` through the woozy vertex stages (#737):
    /// it drifts, and the main pass drew it through its own (they're made
    /// on its first woozy draw), so the two put each vertex alike.
    fn masks_woozy(&self, d: &crate::style_mask::MaskDraw<'_>) -> bool {
        d.woozy
            && !d.is_billboard()
            && if d.bones().is_some() {
                self.pipelines.wooze_skinned.is_some()
            } else {
                self.pipelines.wooze_standard.is_some()
            }
    }

    /// Log the style albedo's meshes (#1204), as
    /// [`log_style_mask_draws`](Self::log_style_mask_draws) logs the mask's.
    pub(super) fn log_style_albedo_draws(&self, pass: &mut crate::frame_capture::LoggedPass) {
        for item in self.draws.drawn() {
            let Some(m) = albedo_mesh(item) else {
                continue;
            };
            let name = if m.bones.is_some() {
                "style albedo skinned"
            } else {
                "style albedo"
            };
            if self.draws.within(item, SteppedPass::StyleAlbedo) {
                pass.draws.push(self.logged_draw(item, None, name));
            } else {
                pass.culled
                    .push(self.logged_draw(item, Some("past the draw limit"), name));
            }
        }
    }
}

/// The mask's selector: what it draws `item` as, if anything. Every item of
/// a styled entity this camera draws, in list order. Terrain comes first
/// (#310), and the props on it after: each fragment's depth test decides
/// who owns a pixel anyway. Styled blended meshes come last, back to front
/// as the scene drew them (#310): they write no depth, so the depth test
/// keeps them behind the opaques in front of them and nothing else, and
/// where one shows, it is the nearest styled surface and its id replaces
/// what's behind. A styled glass is a lens, drawing what's seen through it
/// in its style. A styled billboard is opaque too (#1011): its quad, cut
/// out by its alpha at 0.5 as the main pass cut it. Whether the camera
/// draws it is the caller's to ask.
fn mask_draw<'a>(
    item: &'a DrawItem,
    state: &StyleMaskState,
    terrain: &'a [crate::terrain_pipeline::TerrainDrawCall],
    frustum: Option<&crate::frustum::Frustum>,
) -> Option<crate::style_mask::MaskDraw<'a>> {
    use crate::style_mask::MaskShape;
    let entity = item.entity.as_ref()?;
    let id = state.by_entity.get(entity).copied()?;
    let shape = match &item.draw {
        Draw::Terrain(n) => {
            let d = terrain.get(*n as usize)?;
            if frustum.is_some_and(|f| !f.aabb_visible(d.aabb_min, d.aabb_max)) {
                return None;
            }
            MaskShape::Mesh {
                vertex_buffer: &d.vertex_buffer,
                index_buffer: &d.index_buffer,
                index_count: d.index_count,
                transform_bind_group: &d.transform_bind_group,
                bones: None,
            }
        }
        Draw::Static(d) if !d.is_wireframe => MaskShape::Mesh {
            vertex_buffer: &d.vertex_buffer,
            index_buffer: &d.index_buffer,
            index_count: d.index_count,
            transform_bind_group: &d.transform_bind_group,
            bones: None,
        },
        Draw::Skinned(d) => MaskShape::Mesh {
            vertex_buffer: &d.vertex_buffer,
            index_buffer: &d.index_buffer,
            index_count: d.index_count,
            transform_bind_group: &d.transform_bind_group,
            bones: Some(&d.bone_bind_group),
        },
        Draw::Static(_) => return None,
        // A styled billboard (#1011): its quad, cut out by its alpha as the
        // scene drew it, with the sprite's own bind groups
        Draw::Billboard(d) => MaskShape::Billboard {
            billboard_bind_group: &d.billboard_bind_group,
            texture_bind_group: &d.texture_bind_group,
        },
        // A styled entity's echoes and particles stay unmasked
        Draw::Echo(_) | Draw::Particles(_) => return None,
    };
    Some(crate::style_mask::MaskDraw {
        shape,
        id,
        blend: state.blend_by_entity.get(entity).copied().unwrap_or(1.0),
        source: state.blend_sources.get(entity).copied().unwrap_or_default(),
        woozy: item.wooze().is_some(),
    })
}

/// The albedo's selector: opaque meshes with a plain vertex stage; terrain
/// and billboards read as no mesh. A billboard is never lit (its fragment
/// stage returns the texture's colour), so where it draws the picture is
/// already its albedo, which is what a reader falls back on there: a
/// styled billboard needs no albedo draw (#1011). Whether the camera draws
/// it is the caller's to ask.
fn albedo_mesh(item: &DrawItem) -> Option<crate::style_albedo::AlbedoMesh<'_>> {
    if item.blend != Blend::Opaque || item.wooze().is_some() {
        return None;
    }
    let (vertex_buffer, index_buffer, index_count, t, m, bones) = match &item.draw {
        Draw::Static(d) if !d.is_wireframe => (
            &d.vertex_buffer,
            &d.index_buffer,
            d.index_count,
            &d.transform_bind_group,
            &d.material_bind_group,
            None,
        ),
        Draw::Skinned(d) => (
            &d.vertex_buffer,
            &d.index_buffer,
            d.index_count,
            &d.transform_bind_group,
            &d.material_bind_group,
            Some(&*d.bone_bind_group),
        ),
        Draw::Static(_)
        | Draw::Terrain(_)
        | Draw::Billboard(_)
        | Draw::Echo(_)
        | Draw::Particles(_) => return None,
    };
    Some(crate::style_albedo::AlbedoMesh {
        vertex_buffer,
        index_buffer,
        index_count,
        transform_bind_group: t,
        material_bind_group: m,
        bones,
    })
}
