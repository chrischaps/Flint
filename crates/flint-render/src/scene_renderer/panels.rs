//! Drawing a page of comic panels (#880, ADR 0120). The layout, its
//! rectangles, cameras and the compositor are `crate::panels`.
//!
//! Each panel is the whole frame drawn again, by [`SceneRenderer::encode_to`],
//! with the panel's camera, into a picture of the panel's size. What the
//! frame keeps at its size (the HDR chain and the style targets in
//! `PostProcessResources`, the MSAA targets, the ocean's grab, the scaled
//! depth, the style mask's and albedo's passes, the selection mask) is a
//! [`ViewSlot`] each panel holds its own of and swaps in around its draw, so
//! a style sees the panel's size, depth and camera and keeps its history per
//! panel, and nothing is remade from frame to frame.
//!
//! The frame's uniforms are written through the queue and land with the
//! next submission, so each panel but the last is submitted on its own
//! encoder: N panels are N submissions, and a scene with no `[panels]` is
//! still one. The last panel and the page go into the caller's encoder.

use super::SceneRenderer;
use crate::camera::Camera;
use crate::panels::{panel_camera, PanelCompositor, PanelLayout, PanelRect};
use crate::postprocess::{PostProcessConfig, PostProcessResources};
use std::collections::HashMap;

/// What a view of the frame keeps at its own size: the renderer's fields
/// [`SceneRenderer::swap_view`] trades with a panel's.
#[derive(Default)]
pub(crate) struct ViewSlot {
    postprocess_resources: Option<PostProcessResources>,
    msaa_color: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    msaa_depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    msaa_depth_id: u64,
    ocean_grab_color: Option<(wgpu::Texture, wgpu::TextureView)>,
    ocean_grab_depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    ocean_grab_size: (u32, u32),
    ocean_grab_bind_group: Option<wgpu::BindGroup>,
    scaled_depth: Option<std::sync::Arc<crate::render_scale::ScaledDepth>>,
    depth_upscale: Option<crate::render_scale::DepthUpscale>,
    surface_size: (u32, u32),
    style_mask_pass: Option<crate::style_mask::StyleMaskPass>,
    style_albedo_pass: Option<crate::style_albedo::StyleAlbedoPass>,
    selection_outline: Option<crate::selection_outline::SelectionOutline>,
}

/// One panel's picture, its depth and its view of the frame.
struct PanelView {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    msaa: u32,
    scale: f32,
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    depth_id: u64,
    picture_bind: wgpu::BindGroup,
    slot: ViewSlot,
}

/// The renderer's panels: the layout, each panel's look resolved, and the
/// views kept by panel name.
#[derive(Default)]
pub(crate) struct PanelState {
    layout: Option<PanelLayout>,
    /// Each panel's own config, in the layout's order; `None` for a panel
    /// that draws with the frame's
    posts: Vec<Option<PostProcessConfig>>,
    /// The scene's `[post_process]` as written, which a panel's look is laid
    /// over
    base: Option<flint_scene::PostProcessDef>,
    /// Kept while the scene lasts, so a cut back to a layout is free
    views: HashMap<String, PanelView>,
    compositor: Option<PanelCompositor>,
    /// How many submissions the last page took (one a panel)
    pub(crate) submissions: usize,
}

impl SceneRenderer {
    /// Lay the frame out as `layout`'s panels, or as one view with `None`
    /// (#880). `base` is the scene's `[post_process]` as written, which a
    /// panel's `look` and `post_process` are laid over. A host calls this at
    /// each scene load, which also forgets the last scene's panel views.
    pub fn set_panels(
        &mut self,
        layout: Option<PanelLayout>,
        base: Option<&flint_scene::PostProcessDef>,
    ) {
        self.panel_state.base = base.cloned();
        self.panel_state.views.clear();
        self.set_panel_layout(layout);
    }

    /// Change the layout mid-scene (a script's cut): the scene's base and
    /// the views kept for each panel name stay, so cutting back to a layout
    /// draws at once.
    pub fn set_panel_layout(&mut self, layout: Option<PanelLayout>) {
        let scope = self.asset_scope();
        let base = self.panel_state.base.clone();
        let posts = layout.as_ref().map_or_else(Vec::new, |l| {
            l.panels
                .iter()
                .map(|spec| {
                    if !spec.has_own_look() {
                        return None;
                    }
                    match resolve_panel_post(base.as_ref(), spec, &scope) {
                        Ok(config) => Some(config),
                        Err(e) => {
                            tracing::warn!(
                                "[panels.panel.{}]: {e} (drawn with the frame's look)",
                                spec.name
                            );
                            None
                        }
                    }
                })
                .collect()
        });
        self.panel_state.layout = layout;
        self.panel_state.posts = posts;
    }

    /// The layout the frame is drawn as, if it has panels.
    pub fn panel_layout(&self) -> Option<&PanelLayout> {
        self.panel_state.layout.as_ref()
    }

    /// Submissions the last frame of panels took (one a panel), for tests.
    pub fn panel_submissions(&self) -> usize {
        self.panel_state.submissions
    }

    /// Panel views kept, for tests: one a panel name drawn this scene.
    pub fn panel_views_kept(&self) -> usize {
        self.panel_state.views.len()
    }

    /// The frame, whatever its layout: [`encode_to`](Self::encode_to) for
    /// one view, else each panel's camera into its picture and the page
    /// laid over `target_view` (#880). `world` finds what a panel follows;
    /// without it a following panel looks from the world's origin.
    ///
    /// A page of N panels submits N - 1 of them itself (each panel's
    /// uniforms must land before its draw), so the caller's encoder holds
    /// the last panel, the page and whatever the caller adds; it submits
    /// that as always and calls [`frame_submitted`](Self::frame_submitted).
    #[allow(clippy::too_many_arguments)]
    pub fn encode_frame_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
        world: Option<&flint_ecs::FlintWorld>,
    ) {
        if self.panel_state.layout.is_none() {
            self.encode_to(device, queue, encoder, depth, camera, target_view);
            return;
        }
        self.encode_panels(device, queue, encoder, camera, target_view, world);
    }

    /// [`encode_frame_to`](Self::encode_frame_to) with its own submission,
    /// as [`render_to`](Self::render_to) is `encode_to`'s.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
        world: Option<&flint_ecs::FlintWorld>,
    ) {
        if self.device_lost {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Frame Encoder"),
        });
        self.encode_frame_to(
            device,
            queue,
            &mut encoder,
            depth,
            camera,
            target_view,
            world,
        );
        queue.submit(std::iter::once(encoder.finish()));
        self.frame_submitted();
    }

    fn encode_panels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        target_view: &wgpu::TextureView,
        world: Option<&flint_ecs::FlintWorld>,
    ) {
        if self.device_lost {
            return;
        }
        let Some(layout) = self.panel_state.layout.clone() else {
            return;
        };
        let frame_size = self.surface_size;
        let format = self.surface_format;
        let msaa = self.pipelines.samples();
        let rects = layout.rects(frame_size.0, frame_size.1, self.pixels_per_point);
        let compositor = match self.panel_state.compositor.take() {
            Some(c) if c.format() == format => c,
            _ => PanelCompositor::new(device, format),
        };
        let scale = self.render_scale;
        let mut views = std::mem::take(&mut self.panel_state.views);
        let posts = std::mem::take(&mut self.panel_state.posts);
        let last = layout.panels.len() - 1;
        let mut submissions = 0;
        for (i, (spec, rect)) in layout.panels.iter().zip(&rects).enumerate() {
            let size = rect.size();
            let fits = views.get(&spec.name).is_some_and(|v| {
                v.size == size && v.format == format && v.msaa == msaa && v.scale == scale
            });
            if !fits {
                let view = self.new_panel_view(device, &compositor, size, format);
                views.insert(spec.name.clone(), view);
            }
            let view = views.get_mut(&spec.name).expect("made above");
            let followed = spec.follow.as_deref().and_then(|name| {
                let world = world?;
                let found = world
                    .get_id(name)
                    .and_then(|id| world.get_world_position(id));
                if found.is_none() {
                    tracing::warn!(
                        "[panels.panel.{}] follow = \"{name}\": no such entity",
                        spec.name
                    );
                }
                found
            });
            let cam = panel_camera(spec, camera, followed, size.0 as f32 / size.1 as f32);

            // The panel's look in place of the frame's, under the same caps
            // and on the same boil
            let own = posts.get(i).and_then(Option::as_ref).map(|config| {
                let mut config = crate::quality::mask_post_process(config, &self.quality);
                config.boil = self.postprocess_config.boil;
                config
            });
            let frame_config = own.map(|c| std::mem::replace(&mut self.postprocess_config, c));
            self.swap_view(&mut view.slot);
            let depth = crate::bind_cache::DepthTarget::new(&view.depth_view, view.depth_id);
            if i == last {
                self.encode_to(device, queue, encoder, depth, &cam, &view.color_view);
            } else {
                // Its uniforms land with this submission, before the next
                // panel writes its own; the GPU timer's scopes are the last
                // panel's, in the caller's encoder
                let mut own_encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Panel Encoder"),
                    });
                let timer = self.gpu_timer.take();
                self.encode_to(
                    device,
                    queue,
                    &mut own_encoder,
                    depth,
                    &cam,
                    &view.color_view,
                );
                self.gpu_timer = timer;
                queue.submit(std::iter::once(own_encoder.finish()));
            }
            submissions += 1;
            self.swap_view(&mut view.slot);
            if let Some(config) = frame_config {
                self.postprocess_config = config;
            }
        }
        let pictures: Vec<(PanelRect, &wgpu::BindGroup)> = layout
            .panels
            .iter()
            .zip(&rects)
            .map(|(spec, rect)| (*rect, &views[&spec.name].picture_bind))
            .collect();
        compositor.encode(queue, encoder, target_view, frame_size, &layout, &pictures);
        self.panel_state.compositor = Some(compositor);
        self.panel_state.views = views;
        self.panel_state.posts = posts;
        self.panel_state.submissions = submissions;
    }

    /// A panel's picture, depth and view of the frame at `size`, the view's
    /// HDR chain built by the same [`resize_postprocess`](Self::resize_postprocess)
    /// the window's is.
    fn new_panel_view(
        &mut self,
        device: &wgpu::Device,
        compositor: &PanelCompositor,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> PanelView {
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Panel Picture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Panel Depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let picture_bind = compositor.picture_bind(device, &color_view);
        let mut slot = ViewSlot::default();
        self.swap_view(&mut slot);
        self.resize_postprocess(device, size.0, size.1);
        self.swap_view(&mut slot);
        PanelView {
            size,
            format,
            msaa: self.pipelines.samples(),
            scale: self.render_scale,
            _color: color,
            color_view,
            _depth: depth,
            depth_view,
            depth_id: crate::bind_cache::new_depth_id(),
            picture_bind,
            slot,
        }
    }

    /// Trade the renderer's view of the frame for `slot`'s.
    fn swap_view(&mut self, slot: &mut ViewSlot) {
        use std::mem::swap;
        swap(
            &mut self.postprocess_resources,
            &mut slot.postprocess_resources,
        );
        swap(&mut self.msaa_color, &mut slot.msaa_color);
        swap(&mut self.msaa_depth, &mut slot.msaa_depth);
        swap(&mut self.msaa_depth_id, &mut slot.msaa_depth_id);
        swap(&mut self.ocean_grab_color, &mut slot.ocean_grab_color);
        swap(&mut self.ocean_grab_depth, &mut slot.ocean_grab_depth);
        swap(&mut self.ocean_grab_size, &mut slot.ocean_grab_size);
        swap(
            &mut self.ocean_grab_bind_group,
            &mut slot.ocean_grab_bind_group,
        );
        swap(&mut self.scaled_depth, &mut slot.scaled_depth);
        swap(&mut self.depth_upscale, &mut slot.depth_upscale);
        swap(&mut self.surface_size, &mut slot.surface_size);
        swap(&mut self.style_mask_pass, &mut slot.style_mask_pass);
        swap(&mut self.style_albedo_pass, &mut slot.style_albedo_pass);
        swap(&mut self.selection_outline, &mut slot.selection_outline);
    }
}

/// A panel's own post-processing: the scene's `[post_process]` (with the
/// look it names), the panel's look laid over that as `--look` lays one,
/// then the panel's `post_process` keys.
fn resolve_panel_post(
    base: Option<&flint_scene::PostProcessDef>,
    spec: &crate::panels::PanelSpec,
    scope: &flint_core::project::AssetScope,
) -> Result<PostProcessConfig, String> {
    let mut def = match base {
        Some(def) => crate::look::resolve_in(def, def.look.as_deref(), scope)?,
        None => flint_scene::PostProcessDef::from_authored(toml::Table::new())
            .map_err(|e| format!("[post_process]: {e}"))?,
    };
    if let Some(name) = &spec.look {
        def = crate::look::find_in(name, scope)?.apply(Some(&def))?;
    }
    if let Some(keys) = &spec.post_process {
        def = crate::look::overlay(Some(&def), keys)?;
    }
    Ok(crate::postprocess::post_process_config_from_def(&def))
}
