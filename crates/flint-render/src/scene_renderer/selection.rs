//! The selection outline's half that walks the frame's draw lists (#602):
//! which draws are the selection, and the mask pass over them. The
//! pipelines, the mask and the outline pass are `crate::selection_outline`.

use super::SceneRenderer;
use crate::selection_outline::{OutlineLayouts, SelectionOutline};
use flint_core::EntityId;
use std::collections::HashSet;

impl SceneRenderer {
    /// Set the entity the editor outlines (#602), or None to clear. The
    /// outline goes round it and everything under it in the hierarchy, so
    /// a parent with no mesh of its own, or a glTF model whose nodes were
    /// expanded into child entities, is outlined whole.
    pub fn set_selected_entity(&mut self, id: Option<EntityId>) {
        self.selected_entity = id;
    }

    /// The entity set with [`set_selected_entity`](Self::set_selected_entity).
    pub fn selected_entity(&self) -> Option<EntityId> {
        self.selected_entity
    }

    /// The selected entity and its descendants, as the last
    /// `update_from_world` saw the hierarchy. Empty with nothing selected.
    pub(super) fn selection_set(&self) -> HashSet<EntityId> {
        let mut set = HashSet::new();
        let Some(root) = self.selected_entity else {
            return set;
        };
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if set.insert(id) {
                if let Some(children) = self.entity_children.get(&id) {
                    stack.extend(children.iter().copied());
                }
            }
        }
        set
    }

    /// Draw the selection into its mask against the frame's single-sample
    /// `depth_view` (scene resolution). Records how many draws it made, so
    /// the outline pass is skipped when the selection drew nothing.
    pub(super) fn encode_selection_mask(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        selection: &HashSet<EntityId>,
    ) {
        let surface_format = self.surface_format;
        let size = self.scene_size();
        let outline =
            self.selection_outline.get_or_insert_with(|| {
                SelectionOutline::new(
                    device,
                    &OutlineLayouts {
                        transform: &self.layouts.transform_bind_group_layout,
                        bones: self
                            .pipelines
                            .skinned
                            .as_ref()
                            .map(|s| &s.bone_bind_group_layout),
                        billboard: self.pipelines.billboard.as_ref().map(|b| {
                            (&b.billboard_bind_group_layout, &b.texture_bind_group_layout)
                        }),
                    },
                    surface_format,
                )
            });
        outline.last_draws.set(0);
        outline.ensure_mask(device, size);
        let outline = &*outline;
        let mask_view = outline.mask_view();
        // One bind group a frame over the depth, and only while something
        // is selected: the editor's, never a game's
        let depth_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Selection Mask Depth"),
            layout: &outline.depth_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(depth_view),
            }],
        });
        let mine = |id: Option<EntityId>| id.is_some_and(|id| selection.contains(&id));
        let mut draws = 0usize;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Selection Mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask_view,
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
            let p = &outline.mask;

            // Meshes: opaque, transparent, wireframe (as lines), woozy
            for draw in self.entity_draws.iter().chain(&self.transparent_draws) {
                if !mine(draw.entity_id) {
                    continue;
                }
                let pipeline = if draw.is_wireframe {
                    &p.static_lines
                } else if draw.wooze.is_some() && self.pipelines.wooze_standard.is_some() {
                    &p.woozy_tris
                } else {
                    &p.static_tris
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                pass.set_bind_group(1, &depth_bind, &[]);
                pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.index_count, 0, 0..1);
                draws += 1;
            }

            // Terrain, when the terrain's entity is the selection or under it
            if mine(self.terrain.entity) {
                pass.set_pipeline(&p.static_tris);
                pass.set_bind_group(1, &depth_bind, &[]);
                for draw in &self.terrain.draws {
                    if let Some(frustum) = &self.camera_frustum {
                        if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                            continue;
                        }
                    }
                    pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    draws += 1;
                }
            }

            // Skinned meshes, posed by this frame's bones
            if let (Some(skinned), Some(woozy_skinned)) = (&p.skinned, &p.woozy_skinned) {
                for draw in self
                    .skinned_entity_draws
                    .iter()
                    .chain(&self.transparent_skinned_draws)
                {
                    if !mine(draw.entity_id) {
                        continue;
                    }
                    let woozy = draw.wooze.is_some() && self.pipelines.wooze_skinned.is_some();
                    pass.set_pipeline(if woozy { woozy_skinned } else { skinned });
                    pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                    pass.set_bind_group(1, &depth_bind, &[]);
                    pass.set_bind_group(2, &*draw.bone_bind_group, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    draws += 1;
                }
            }

            // Billboards, cut out by their sprite's alpha
            if let (Some(pipeline), Some(bp)) = (&p.billboard, &self.pipelines.billboard) {
                for draw in &self.billboard_draws {
                    if !mine(draw.entity_id) {
                        continue;
                    }
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                    pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                    pass.set_bind_group(2, &depth_bind, &[]);
                    pass.set_index_buffer(
                        bp.quad_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..6, 0, 0..1);
                    draws += 1;
                }
            }
        }
        if let Some(outline) = &self.selection_outline {
            outline.last_draws.set(draws);
        }
    }

    /// Ink the outline over the finished image in `target_view`, which is
    /// in the surface's format (the pass is built for it, as the composite
    /// is).
    pub(super) fn encode_selection_outline(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
    ) {
        let Some(outline) = &self.selection_outline else {
            return;
        };
        outline.encode_outline(
            queue,
            encoder,
            target_view,
            self.surface_size,
            self.pixels_per_point,
        );
    }
}
