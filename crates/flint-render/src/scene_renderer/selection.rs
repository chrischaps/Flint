//! The selection outline's half that walks the frame's draw lists (#602):
//! which draws are the selection, and the mask pass over them. The
//! pipelines, the mask and the outline pass are `crate::selection_outline`.

use super::{Draw, SceneRenderer};
use crate::gpu_counts::CountedDevice;
use crate::selection_outline::{OutlineLayouts, SelectionOutline};
use flint_core::EntityId;
use std::collections::HashSet;

/// What the selection mask draws an item through (#1151): the one choice
/// the encode binds by and the frame's description names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionPipe {
    /// A mesh or a terrain chunk.
    Tris,
    /// A wireframe box.
    Lines,
    WoozyTris,
    Skinned,
    WoozySkinned,
    Billboard,
}

impl SelectionPipe {
    fn name(self) -> &'static str {
        match self {
            SelectionPipe::Tris => "selection mask",
            SelectionPipe::Lines => "selection mask lines",
            SelectionPipe::WoozyTris => "selection mask woozy",
            SelectionPipe::Skinned => "selection mask skinned",
            SelectionPipe::WoozySkinned => "selection mask woozy skinned",
            SelectionPipe::Billboard => "selection mask billboard",
        }
    }

    /// Its pipeline; `None` for one the mask couldn't make (skinned without
    /// the skinned layouts, a billboard without the billboard's).
    fn pipeline(
        self,
        p: &crate::selection_outline::MaskPipelines,
    ) -> Option<&wgpu::RenderPipeline> {
        match self {
            SelectionPipe::Tris => Some(&p.static_tris),
            SelectionPipe::Lines => Some(&p.static_lines),
            SelectionPipe::WoozyTris => Some(&p.woozy_tris),
            SelectionPipe::Skinned => p.skinned.as_ref(),
            SelectionPipe::WoozySkinned => p.woozy_skinned.as_ref(),
            SelectionPipe::Billboard => p.billboard.as_ref(),
        }
    }
}

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
                        billboard: self.pipelines.billboard.as_deref().map(|b| {
                            (&b.billboard_bind_group_layout, &b.texture_bind_group_layout)
                        }),
                    },
                    surface_format,
                )
            });
        outline.last_draws.set(0);
        outline.ensure_mask(device, size);
        let Some(outline) = self.selection_outline.as_ref() else {
            return;
        };
        let mask_view = outline.mask_view();
        // One bind group a frame over the depth, and only while something
        // is selected: the editor's, never a game's
        let depth_bind = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            label: Some("Selection Mask Depth"),
            layout: &outline.depth_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(depth_view),
            }],
        });
        let mut draws = 0usize;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Selection Mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask_view,
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
            });
            // Every item of the selection this camera draws, through the
            // mask's pipeline for its kind (`selection_draws`). The mask
            // keeps the most any draw wrote, so order is free.
            for (item, _, pipeline) in self.selection_draws(&outline.mask, selection) {
                pass.set_pipeline(pipeline);
                match &item.draw {
                    Draw::Static(draw) => {
                        pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                        pass.set_bind_group(1, &depth_bind, &[]);
                        pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                    Draw::Terrain(n) => {
                        let Some(draw) = self.terrain.draws.get(*n as usize) else {
                            continue;
                        };
                        pass.set_bind_group(1, &depth_bind, &[]);
                        pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                    Draw::Skinned(draw) => {
                        pass.set_bind_group(0, &*draw.transform_bind_group, &[]);
                        pass.set_bind_group(1, &depth_bind, &[]);
                        pass.set_bind_group(2, &*draw.bone_bind_group, &[]);
                        pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                    Draw::Billboard(draw) => {
                        // selection_draws kept it only with the quad
                        let Some(bp) = &self.pipelines.billboard else {
                            continue;
                        };
                        pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                        pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                        pass.set_bind_group(2, &depth_bind, &[]);
                        pass.set_index_buffer(
                            bp.quad_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..6, 0, 0..1);
                    }
                    // selection_draws keeps neither
                    Draw::Echo(_) | Draw::Particles(_) => continue,
                }
                draws += 1;
            }
        }
        if let Some(outline) = &self.selection_outline {
            outline.last_draws.set(draws);
        }
    }

    /// The selection mask's draws (#602, #1151): every item of `selection`
    /// this camera draws, with what it's drawn as and the mask pipeline
    /// that is: meshes (wireframes as lines, woozy ones through their own
    /// stage), terrain chunks in view, skinned meshes as posed, billboards
    /// cut out by their alpha. A translucent copy or a particle isn't the
    /// thing picked. The encode and the description both walk this.
    fn selection_draws<'a>(
        &'a self,
        mask: &'a crate::selection_outline::MaskPipelines,
        selection: &'a HashSet<EntityId>,
    ) -> impl Iterator<Item = (&'a super::DrawItem, SelectionPipe, &'a wgpu::RenderPipeline)> + 'a
    {
        let frustum = self.camera_frustum.as_ref();
        self.draws
            .drawn()
            .filter(|i| i.entity.is_some_and(|id| selection.contains(&id)))
            .filter_map(move |item| {
                let pipe = match &item.draw {
                    Draw::Static(d) if d.is_wireframe => SelectionPipe::Lines,
                    Draw::Static(_)
                        if item.wooze().is_some() && self.pipelines.wooze_standard.is_some() =>
                    {
                        SelectionPipe::WoozyTris
                    }
                    Draw::Static(_) => SelectionPipe::Tris,
                    Draw::Terrain(n) => {
                        let d = self.terrain.draws.get(*n as usize)?;
                        if frustum.is_some_and(|f| !f.aabb_visible(d.aabb_min, d.aabb_max)) {
                            return None;
                        }
                        SelectionPipe::Tris
                    }
                    Draw::Skinned(_)
                        if item.wooze().is_some() && self.pipelines.wooze_skinned.is_some() =>
                    {
                        SelectionPipe::WoozySkinned
                    }
                    Draw::Skinned(_) => SelectionPipe::Skinned,
                    Draw::Billboard(_) => {
                        self.pipelines.billboard.as_ref()?;
                        SelectionPipe::Billboard
                    }
                    Draw::Echo(_) | Draw::Particles(_) => return None,
                };
                Some((item, pipe, pipe.pipeline(mask)?))
            })
    }

    /// Log the selection mask's draws (#1151) into the description's pass.
    pub(super) fn log_selection_draws(
        &self,
        pass: &mut crate::frame_capture::LoggedPass,
        selection: &HashSet<EntityId>,
    ) {
        let Some(outline) = &self.selection_outline else {
            return;
        };
        for (item, pipe, _) in self.selection_draws(&outline.mask, selection) {
            pass.draws.push(self.logged_draw(item, None, pipe.name()));
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
