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
//! ## How the mask is drawn
//!
//! The ASCII spike (#261) measured the two ways to write it and chose a
//! redraw over a new attachment on every scene pipeline. After the scene
//! pass (and the MSAA depth resolve), each visible opaque draw of a styled
//! entity is drawn again into one `R16Uint` target at scene resolution,
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
//! Particles, sprites, grass, terrain and water stay unmasked: they write
//! no mask (and, but for terrain, no depth either).
//!
//! [`StyleDescriptor::uses_mask`]: crate::stylize::StyleDescriptor::uses_mask

use bytemuck::{Pod, Zeroable};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{HashMap, HashSet};

/// The mask's format: one style id a pixel, 0 where no style drew.
pub const STYLE_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Uint;

/// The id of a pixel no style drew.
pub const NO_STYLE: u16 = 0;

/// The component an entity names its style in.
pub const STYLE_MASK_COMPONENT: &str = "style_mask";

/// The name of the mask in the post chain's style targets.
pub(crate) const MASK_TARGET: &str = "style mask";

/// How far behind the scene's depth a mask fragment may be, in multiples
/// of its own depth slope (`fwidth(z)`). The spike's value: it absorbs the
/// sample-0 resolve under MSAA without letting a hidden face through.
pub const SLOPE_TOLERANCE: f32 = 2.0;

/// Masked draws one frame can hold; any more aren't masked (logged once).
pub const MAX_MASK_DRAWS: usize = 4096;

/// The dynamic-offset stride of the per-draw uniforms: the alignment
/// wgpu's default limits ask for.
const DRAW_STRIDE: u64 = 256;

// ---------------------------------------------------------------------------
// Style names -> ids
// ---------------------------------------------------------------------------

/// The styles entities ask for, numbered from 1 in name order. Ids change
/// only when the set of names does, and are the same for the same scene
/// on every run (no entity id or hash order goes into them).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StyleIds {
    names: Vec<String>,
}

impl StyleIds {
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
        names.sort_unstable();
        names.dedup();
        names.truncate(u16::MAX as usize - 1);
        if self.names.len() == names.len()
            && self.names.iter().zip(names.iter()).all(|(a, b)| a == b)
        {
            return false;
        }
        self.names = names.iter().map(|n| n.to_string()).collect();
        true
    }
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

/// Find every styled entity's id: those with a `style_mask` and their
/// descendants, the nearest `style_mask` winning. Fills `out` (cleared
/// first) with the non-zero ids only and syncs `ids` with the names used.
pub(crate) fn resolve(world: &FlintWorld, ids: &mut StyleIds, out: &mut HashMap<EntityId, u16>) {
    out.clear();
    let roots: &HashSet<EntityId> = world.entities_with_component(STYLE_MASK_COMPONENT);
    if roots.is_empty() {
        ids.sync(&mut Vec::new());
        return;
    }
    let mut names: Vec<&str> = roots
        .iter()
        .filter_map(|&e| requested_style(world, e))
        .collect();
    ids.sync(&mut names);
    let mut stack = Vec::new();
    for &root in roots {
        let Some(id) = requested_style(world, root).and_then(|n| ids.id(n)) else {
            continue; // an opt-out: its subtree stays unstyled
        };
        stack.clear();
        stack.push(root);
        while let Some(e) = stack.pop() {
            out.insert(e, id);
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
    _pad: [f32; 2],
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
}

/// The mask's pipelines and its per-draw uniform buffer, made the first
/// time a frame needs the mask.
pub struct StyleMaskPass {
    static_pipeline: wgpu::RenderPipeline,
    skinned_pipeline: wgpu::RenderPipeline,
    draw_bgl: wgpu::BindGroupLayout,
    draw_buffer: wgpu::Buffer,
    /// The uniform bytes, kept so a frame reuses the allocation.
    scratch: Vec<u8>,
    warned_overflow: bool,
}

impl StyleMaskPass {
    pub fn new(
        device: &wgpu::Device,
        transform_bgl: &wgpu::BindGroupLayout,
        bone_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
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
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Style Mask Draws"),
            size: DRAW_STRIDE * MAX_MASK_DRAWS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            static_pipeline,
            skinned_pipeline,
            draw_bgl,
            draw_buffer,
            scratch: Vec::new(),
            warned_overflow: false,
        }
    }

    /// The bind group of the per-draw uniforms and `scene_depth`: kept in
    /// `binds` while the chain owns the depth, else made for this frame
    /// (once a frame, never per draw).
    fn draw_bind(
        &self,
        device: &wgpu::Device,
        binds: &crate::bind_cache::BindCache,
        scene_depth: &wgpu::TextureView,
        depth_owned: bool,
    ) -> std::sync::Arc<wgpu::BindGroup> {
        binds.get_if(depth_owned, "Style Mask Draw BG", 0, || {
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
                ],
            })
        })
    }

    /// Clear `mask` and draw `draws` into it, each tested against
    /// `scene_depth` (single-sample, the mask's size).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        binds: &crate::bind_cache::BindCache,
        mask: &wgpu::TextureView,
        scene_depth: &wgpu::TextureView,
        depth_owned: bool,
        draws: &[MaskDraw<'_>],
    ) {
        if draws.len() > MAX_MASK_DRAWS && !self.warned_overflow {
            self.warned_overflow = true;
            tracing::warn!(
                "style mask: {} styled draws, only the first {MAX_MASK_DRAWS} are masked",
                draws.len()
            );
        }
        let draws = &draws[..draws.len().min(MAX_MASK_DRAWS)];
        self.scratch.clear();
        self.scratch.resize(draws.len() * DRAW_STRIDE as usize, 0);
        for (i, d) in draws.iter().enumerate() {
            let at = i * DRAW_STRIDE as usize;
            let uniform = MaskDrawUniform {
                id: u32::from(d.id),
                slope_tolerance: SLOPE_TOLERANCE,
                _pad: [0.0; 2],
            };
            self.scratch[at..at + std::mem::size_of::<MaskDrawUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&uniform));
        }
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, &self.scratch);
        }
        let draw_bg =
            (!draws.is_empty()).then(|| self.draw_bind(device, binds, scene_depth, depth_owned));

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
        resolve(&world, &mut ids, &mut out);
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
}
