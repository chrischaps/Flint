//! Per-entity styles' state, owned by [`StyleMaskState`] (ADR 0118, #736):
//! which styles entities ask for and the id each styled entity has in the
//! mask (#263), how far each is drawn into its style (#278) and which
//! follow the tear (#397), whether the last mask drew a veil (#290), the
//! scissor switch (#385), and the mask and albedo passes (#869).
//!
//! What the passes draw is gathered here too, by `SceneRenderer`'s
//! `encode_style_mask` and `encode_style_albedo`, from the renderer's draw
//! lists as they stand: the state never holds a draw list of its own, so
//! step 5 (#737) can hand both one list of draw items instead.

use super::SceneRenderer;
use crate::camera::Camera;
use crate::postprocess::PostProcessConfig;
use crate::style_albedo::StyleAlbedoPass;
use crate::style_mask::{StyleIds, StyleMaskPass};
use crate::terrain_pipeline::TerrainDrawCall;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{HashMap, HashSet};

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
    /// The styled entities whose blend follows the reality tear (#397).
    pub(crate) torn_entities: HashSet<EntityId>,
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
            torn_entities: HashSet::new(),
            veiled: false,
        }
    }

    /// Ids for every styled entity: they say which styles are on for
    /// entities alone (#266) as well as what the mask holds (#263).
    pub(crate) fn resolve(&mut self, world: &FlintWorld, config: &PostProcessConfig) {
        crate::style_mask::resolve_blended(
            world,
            config,
            &mut self.ids,
            &mut self.by_entity,
            &mut self.blend_by_entity,
            &mut self.torn_entities,
        );
    }

    /// No styled entities: post-processing is off.
    pub(crate) fn clear(&mut self) {
        self.ids.set(Vec::new());
        self.by_entity.clear();
        self.blend_by_entity.clear();
        self.torn_entities.clear();
    }

    /// Whether some pixel of the last style mask may hold a blend below 1
    /// (#396). See [`SceneRenderer::style_mask_fades`].
    pub(crate) fn fades(&self, config: &PostProcessConfig) -> bool {
        !self.blend_by_entity.is_empty()
            || self.veiled
            || (!self.torn_entities.is_empty() && crate::postprocess::tear_active(config))
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
    /// skinned draws, then transparents over them (#310). The mask is
    /// cleared even with none, so a style never reads a stale frame.
    pub(super) fn encode_style_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey,
    ) {
        self.style_mask.veiled = false;
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
            Some((view, id)) if self.pipelines.samples() == crate::style_mask::MSAA_SAMPLES => {
                pass.ensure_msaa(
                    device,
                    &self.layouts.transform_bind_group_layout,
                    &skinned.bone_bind_group_layout,
                );
                Some(crate::style_mask::MsaaDepth { view, id })
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
        let blends = &self.style_mask.blend_by_entity;
        let torn = &self.style_mask.torn_entities;
        let torn_of =
            |e: &Option<flint_core::EntityId>| e.as_ref().is_some_and(|e| torn.contains(e));
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
            if let Some(id) = id_of(&self.terrain.entity) {
                let blend = blend_of(&self.terrain.entity);
                let torn = torn_of(&self.terrain.entity);
                let frustum = self.camera_frustum.as_ref();
                let visible = |d: &&TerrainDrawCall| {
                    frustum.is_none_or(|f| f.aabb_visible(d.aabb_min, d.aabb_max))
                };
                draws.extend(self.terrain.draws.iter().filter(visible).map(|d| {
                    crate::style_mask::MaskDraw {
                        vertex_buffer: &d.vertex_buffer,
                        index_buffer: &d.index_buffer,
                        index_count: d.index_count,
                        transform_bind_group: &d.transform_bind_group,
                        bones: None,
                        id,
                        blend,
                        torn,
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
                            torn: torn_of(&d.entity_id),
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
                    torn: torn_of(&d.entity_id),
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
                            torn: torn_of(&d.entity_id),
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
                    torn: torn_of(&d.entity_id),
                })
            }));
        }
        // What's in front of them (#290): transparents that aren't styled
        // themselves, and every particle
        let mut veil = crate::style_mask::Veil {
            light_bind_group: &self.lights.bind_group,
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
            veil.particles = match (&self.pipelines.particle, &self.particle_buffers) {
                (Some(pipeline), Some(buffers)) => Some(crate::style_mask::VeilParticles {
                    pipeline,
                    buffers,
                    draws: &self.particle_draws,
                }),
                _ => None,
            };
        }
        let veiled = !veil.is_empty();
        // The tear the composite draws this frame (#397), for the draws
        // whose blend follows it: the same config and post time
        let config = &self.postprocess_config;
        let tear = crate::style_mask::Tear {
            on: crate::postprocess::tear_active(config),
            mode_params: config.mode_params,
            mix: config.mode_mix,
            time: self.post_time(),
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
            Some((view, _)) if self.pipelines.samples() > 1 => (view, self.pipelines.samples()),
            _ => (depth_view, 1),
        };
        let mut meshes: Vec<crate::style_albedo::AlbedoMesh<'_>> = self
            .entity_draws
            .iter()
            .filter(|d| !d.is_wireframe && d.wooze.is_none())
            .map(|d| crate::style_albedo::AlbedoMesh {
                vertex_buffer: &d.vertex_buffer,
                index_buffer: &d.index_buffer,
                index_count: d.index_count,
                transform_bind_group: &d.transform_bind_group,
                material_bind_group: &d.material_bind_group,
                bones: None,
            })
            .collect();
        meshes.extend(
            self.skinned_entity_draws
                .iter()
                .filter(|d| d.wooze.is_none())
                .map(|d| crate::style_albedo::AlbedoMesh {
                    vertex_buffer: &d.vertex_buffer,
                    index_buffer: &d.index_buffer,
                    index_count: d.index_count,
                    transform_bind_group: &d.transform_bind_group,
                    material_bind_group: &d.material_bind_group,
                    bones: Some(&d.bone_bind_group),
                }),
        );
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
            && crate::stylize::any_uses_mask(&self.postprocess_config, &self.style_mask.ids)
    }
}
