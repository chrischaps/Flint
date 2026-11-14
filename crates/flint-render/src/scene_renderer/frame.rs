//! The frame (ADR 0118, #736): what a host calls each frame to draw the
//! scene, and the order the frame's work is recorded in.
//!
//! - `render` (a window) and `render_to` (any target, with its own submit)
//!   both go through `encode_to`, which records the whole frame into the
//!   caller's encoder: the grass compute, the shadow cascades, the
//!   per-frame uniforms, the scene pass (split around the ocean's
//!   refraction grab when water draws, with MSAA's depth resolved after
//!   each opaque phase), the style mask and albedo, the post chain, the
//!   depth upscale and the world lines (#139, one submission a frame).
//! - `encode_again_to` draws the same frame into another target.
//! - `frame_submitted` runs after the host's submit: the grass count and GPU
//!   timer read-backs.
//!
//! The subsystems own their state (`msaa`, `sky`, `lights`, `terrain`,
//! `style_mask`, `water`, `skinning`, `grass`) and this file orchestrates
//! them. The per-frame inputs hosts set by hand today (`ocean_time`,
//! `grass_time`, `style_dt`, `pixels_per_point`, `set_boil`,
//! `set_shallow_wooze`, `set_motion_time`, world lines) are what #739's
//! `prepare(&world, &FrameInputs)` gathers, and it lives here beside
//! `encode_to`.

use super::*;

impl SceneRenderer {
    // ── Render entry points ──

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), crate::SurfaceError> {
        if self.device_lost {
            return Ok(());
        }
        self.render_to(
            &context.device,
            &context.queue,
            context.depth_target(),
            camera,
            view,
        );
        Ok(())
    }

    // ── Slim render_to: orchestrates shadow → uniforms → main pass → postprocess ──

    /// Render the scene to an arbitrary texture view with explicit device/queue/depth
    pub fn render_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Frame Encoder"),
        });
        self.encode_to(device, queue, &mut encoder, depth, camera, target_view);
        queue.submit(std::iter::once(encoder.finish()));
        self.frame_submitted();
    }

    /// [`render_to`](Self::render_to) into `encoder`, for a host that
    /// draws more into the frame (its HUD) and submits once (#139). On the
    /// web each submission crosses into JS; the frame used to make twenty.
    ///
    /// The frame's uniforms are written to `queue` now and reach the GPU
    /// with the next submission, so encode one camera per submission: a
    /// second camera's uniforms would overwrite this one's. After
    /// submitting, call [`frame_submitted`](Self::frame_submitted).
    #[profiling::function]
    ///
    /// `depth` is the caller's depth buffer: a `&TextureView`, or a
    /// [`DepthTarget`](crate::bind_cache::DepthTarget) with an id
    /// ([`RenderContext::depth_target`](crate::RenderContext::depth_target))
    /// so the bind groups over it are kept from frame to frame (#379).
    pub fn encode_to<'d>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth: impl Into<crate::bind_cache::DepthTarget<'d>>,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let depth = depth.into();
        let depth_view = depth.view;
        // A style switched on by any route (scene load, F4, a script's
        // config) is built before the frame that first needs it (#262).
        // When nothing new is on this is a walk over the registry.
        self.sync_debug_styles();
        // A tear is a script's, set every frame, maybe after the world was
        // read: a prop whose blend follows it (#397) is off while the
        // config this frame draws has none
        self.style_mask
            .ids
            .set_tear(crate::postprocess::tear_active(&self.postprocess_config));
        self.ensure_style_resources(device);
        // FXAA likewise (#297): a host that only sets the config (the
        // viewer's hot reload and scene/default toggle, a script's post
        // override) drew none until a resize. Two `Option` checks when built.
        self.ensure_fxaa_resources(device);
        if self.device_lost {
            return;
        }
        // Taken for the frame, so the phases can be scoped while self is
        // borrowed; put back at the end (ADR 0108).
        let mut gpu_timer = self.gpu_timer.take();
        // Below render scale 1 the scene draws against a depth buffer of
        // its own size, copied into the caller's once the frame is drawn
        // (#122). Every scene pass and effect below sees that one.
        let surface_depth_view = depth_view;
        let scaled_depth = self.scaled_depth.clone();
        let depth_view = scaled_depth.as_ref().map_or(depth_view, |d| &d.view);
        // Who vouches for that view, so bind groups over it can be kept:
        // the chain for its own, the caller's id for the caller's (#379).
        let depth_key = if scaled_depth.is_some() {
            crate::bind_cache::DepthKey::Owned
        } else {
            crate::bind_cache::DepthKey::caller(depth.id)
        };
        let clear_color = wgpu::Color {
            r: self.clear_color[0] as f64,
            g: self.clear_color[1] as f64,
            b: self.clear_color[2] as f64,
            a: self.clear_color[3] as f64,
        };
        let view_proj = camera.view_projection_matrix();
        self.camera_frustum = Some(crate::frustum::Frustum::from_view_projection(&view_proj));

        // Draws an earlier render_to culled (another camera, same extract)
        // come back for the shadow pass and this camera's test.
        self.entity_draws.append(&mut self.culled_entity_draws);
        self.transparent_draws
            .append(&mut self.culled_transparent_draws);

        // Pick each model's level of detail for this camera and drop what is
        // past its draw distance (#56), before the shadow pass sees them.
        self.cull_draws_to_lod(camera.position_array());

        // Count visible terrain chunks for stats
        self.terrain.visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain
                .draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain.draws.len() as u32
        };

        let camera_pos = camera.position_array();
        let debug_mode_u32 = self.debug_state.mode.as_u32();
        let wireframe_only = self.debug_state.mode == DebugMode::WireframeOnly;

        // Update light uniforms
        self.lights.upload(queue);

        // Grass compute (ADR 0092): scatters this camera's blades, culled to
        // its view, and counts them into the indirect draw arguments. It goes
        // before the shadow pass so the cascades cast this frame's blades.
        // Nothing waits on it: the draws read the count on the GPU, and the
        // stats get it a frame or so late.
        self.grass.drawing = false;
        if self.grass.needs_realloc {
            if let Some(config) = self.grass.config.clone() {
                self.reload_grass_config(device, config);
            }
        }
        if self.grass.config.as_ref().is_some_and(|c| c.enabled) {
            let grass_time = self.grass_time;
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Grass"));
            self.dispatch_grass_compute(device, queue, encoder, camera, grass_time);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
            // Its count is read back once the frame is submitted
            self.grass.readback_due = true;
        } else {
            self.grass.instance_count = 0;
        }

        // Shadow pass: render depth from light perspective. It sees every
        // draw (culled per cascade), since a caster outside the view can
        // still shadow what's in it.
        let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Shadows"));
        self.render_shadow_pass(device, queue, encoder, camera, camera_pos);
        if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
            timer.end(encoder, scope);
        }

        // Set aside draws outside the camera frustum for the rest of the frame.
        self.cull_draws_to_camera();

        // All scene pipelines target Rgba16Float, so we always render to the HDR
        // buffer and composite to sRGB.
        let has_postprocess =
            self.postprocess_pipeline.is_some() && self.postprocess_resources.is_some();

        // Shader-side tonemapping is always OFF when compositing through the HDR
        // buffer (the composite pass handles ACES + gamma).
        let tonemapping_u32: u32 = if !has_postprocess && self.tonemapping_enabled {
            1
        } else {
            0
        };

        // Grab-pass refraction runs only with an active ocean + postprocess
        // (the HDR buffer is what gets snapshotted) in normal render mode.
        // Water bodies refract through the same grab (ADR 0094).
        self.water.grab_this_frame = self.water_active()
            && has_postprocess
            && !wireframe_only
            && self.pipelines.ocean.is_some();
        self.water.camera_near_far = (camera.near, camera.far);
        if self.water.grab_this_frame {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.water
                .ensure_grab(device, self.pipelines.ocean.as_deref(), w, h);
            if self.water.grab.bind_group.is_none() {
                self.water.grab_this_frame = false;
            }
        }

        // Update all per-frame uniforms and sort transparent draws
        self.update_per_frame_uniforms(
            queue,
            view_proj,
            camera_pos,
            debug_mode_u32,
            tonemapping_u32,
            camera,
        );

        // MSAA (ADR 0058): scene passes draw into MSAA color/depth and
        // resolve color into the HDR buffer; depth resolves (sample 0) into
        // the caller's depth view after each opaque phase so every existing
        // depth consumer stays single-sample. Requires the HDR chain (`build`
        // always makes it).
        let msaa_active = self.pipelines.samples() > 1 && has_postprocess;
        if msaa_active {
            let (w, h) = {
                let res = self.postprocess_resources.as_ref().unwrap();
                (res.width, res.height)
            };
            self.msaa.prepare(device, self.pipelines.samples(), w, h);
        }

        // Choose render target: HDR buffer or direct to surface
        let scene_target_view = if has_postprocess {
            &self.postprocess_resources.as_ref().unwrap().hdr_view
        } else {
            target_view
        };

        // Scene-pass attachments: MSAA views with a color resolve when
        // active, the plain single-sample pair otherwise.
        let (pass_color_view, pass_resolve_target, pass_depth_view) = if msaa_active {
            (
                self.msaa.color_view().unwrap(),
                Some(scene_target_view),
                self.msaa.depth_view().unwrap(),
            )
        } else {
            (scene_target_view, None, depth_view)
        };

        let scene_scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Scene"));

        if !self.water.grab_this_frame {
            // Single-pass path (unchanged for scenes without an ocean).
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pass_color_view,
                    resolve_target: pass_resolve_target,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: pass_depth_view,
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
            self.render_main_pass(
                &mut render_pass,
                wireframe_only,
                queue,
                camera,
                RenderPhase::All,
            );
            drop(render_pass);
            if msaa_active {
                self.msaa.resolve_depth_into(device, encoder, depth_view);
            }
        } else {
            // ── Grab-pass split: opaque scene → snapshot → ocean + rest ──
            // Pass A: sky + all opaque geometry (including the legs).
            {
                let mut pass_a = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass A (pre-ocean)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: pass_color_view,
                        resolve_target: pass_resolve_target,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear_color),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: pass_depth_view,
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
                self.render_main_pass(
                    &mut pass_a,
                    wireframe_only,
                    queue,
                    camera,
                    RenderPhase::PreOcean,
                );
            }

            // With MSAA, the grab blit below reads the caller's single-sample
            // depth view — resolve the opaque depth into it first (ADR 0058).
            if msaa_active {
                self.msaa.resolve_depth_into(device, encoder, depth_view);
            }

            // Blit: snapshot opaque color + depth into sampleable copies.
            {
                let op = self.pipelines.ocean.as_deref().unwrap();
                // The grab runs only into the HDR chain, so the bind group
                // is the chain's to keep while the depth is its too (#139).
                let resources = self.postprocess_resources.as_ref().unwrap();
                let blit_bind_group =
                    resources
                        .binds
                        .get_over(depth_key, "ocean grab blit", 0, || {
                            device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &op.blit_bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(
                                            scene_target_view,
                                        ),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::TextureView(depth_view),
                                    },
                                ],
                                label: Some("Grab Blit Bind Group"),
                            })
                        });
                let grab_color_view = &self.water.grab.color.as_ref().unwrap().1;
                let grab_depth_view = &self.water.grab.depth.as_ref().unwrap().1;
                let mut blit_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Ocean Grab Blit"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_color_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: grab_depth_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                blit_pass.set_pipeline(&op.blit_pipeline);
                blit_pass.set_bind_group(0, &*blit_bind_group, &[]);
                blit_pass.draw(0..3, 0..1);
            }

            // Pass B: ocean (sampling the snapshots) + transparents/particles.
            {
                let mut pass_b = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass B (ocean + transparents)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: pass_color_view,
                        resolve_target: pass_resolve_target,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: pass_depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                self.render_main_pass(
                    &mut pass_b,
                    wireframe_only,
                    queue,
                    camera,
                    RenderPhase::PostOcean,
                );
            }

            // Final depth resolve so postprocess (SSAO/DoF/fog/volumetric)
            // sees the complete frame's depth (ADR 0058).
            if msaa_active {
                self.msaa.resolve_depth_into(device, encoder, depth_view);
            }
        }

        if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scene_scope) {
            timer.end(encoder, scope);
        }

        // The entity style mask (#263): the styled entities' ids, drawn
        // against the frame's single-sample depth, while a style reads it.
        if has_postprocess && self.style_mask_wanted() {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Style mask"));
            self.encode_style_mask(device, queue, encoder, depth_view, depth_key);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // The style albedo (#869): the opaque meshes' own colour, while a
        // style divides the picture by it
        if has_postprocess
            && crate::stylize::any_uses_albedo(&self.postprocess_config, &self.style_mask.ids)
        {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "Style albedo"));
            self.encode_style_albedo(device, encoder, depth_view);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // The editor's selection (#602): its draws into a mask, against the
        // same depth. Nothing selected, nothing drawn.
        let selection = self.selection_set();
        let outlining = !selection.is_empty();
        if outlining {
            let scope = gpu_timer
                .as_mut()
                .map(|t| t.begin(encoder, "Selection mask"));
            self.encode_selection_mask(device, encoder, depth_view, &selection);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // Composite: always needed to convert HDR → sRGB surface
        if has_postprocess {
            self.render_postprocess(
                device,
                queue,
                encoder,
                depth_view,
                depth_key,
                target_view,
                camera,
                gpu_timer.as_mut(),
            );
        }

        // The scaled scene's depth, into the surface-sized view the overlays
        // below and the caller's own (world text, gizmos) test against.
        if let (Some(_), Some(upscale)) = (&scaled_depth, &self.depth_upscale) {
            upscale.record(encoder, surface_depth_view);
        }

        // The selection outline, over the finished image: after every
        // style and the tonemap, so its colour is the one asked for (#602)
        if outlining {
            let scope = gpu_timer
                .as_mut()
                .map(|t| t.begin(encoder, "Selection outline"));
            self.encode_selection_outline(queue, encoder, target_view);
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }

        // World lines (ADR 0083) go over the finished image, tested against
        // the frame's single-sample depth. Without the HDR chain an MSAA depth
        // is never resolved, so they're skipped there.
        if !self.world_lines.is_empty() && (has_postprocess || self.pipelines.samples() == 1) {
            let scope = gpu_timer.as_mut().map(|t| t.begin(encoder, "World lines"));
            self.render_world_lines(
                device,
                queue,
                encoder,
                surface_depth_view,
                target_view,
                camera,
            );
            if let (Some(timer), Some(scope)) = (gpu_timer.as_mut(), scope) {
                timer.end(encoder, scope);
            }
        }
        self.gpu_timer = gpu_timer;
    }

    /// [`encode_to`](Self::encode_to) a second time in a frame, for the other
    /// half of a split view (#238), with the GPU timer left out: its scopes are
    /// the first drawing's. Submit the first drawing before calling this, since
    /// what changed between the two (bones, the boil) reaches the GPU through
    /// the queue's writes, and those land with the next submission.
    pub fn encode_again_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        let timer = self.gpu_timer.take();
        self.encode_to(device, queue, encoder, depth_view, camera, target_view);
        self.gpu_timer = timer;
    }

    /// Start what waits on the frame [`encode_to`](Self::encode_to)
    /// recorded: the grass count's read-back and, once
    /// [`encode_gpu_timings`](Self::encode_gpu_timings) resolved them, the
    /// GPU timer's. Call once that frame is submitted.
    pub fn frame_submitted(&mut self) {
        if std::mem::take(&mut self.grass.readback_due) {
            if let Some(readback) = &mut self.grass.count_readback {
                readback.start_map();
            }
        }
        if let Some(timer) = &mut self.gpu_timer {
            timer.start_readback();
        }
    }

    fn render_world_lines(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        if self.postprocess_resources.is_none() {
            return;
        }
        // Drawn into the surface, whatever the scene's render scale
        let viewport = [self.surface_size.0 as f32, self.surface_size.1 as f32];
        let format = self.surface_format;
        let pipeline = self.world_line_pipeline.get_or_insert_with(|| {
            crate::world_line_pipeline::WorldLinePipeline::new(device, format)
        });
        let view = crate::world_line_pipeline::LineView {
            view_proj: camera.view_projection_matrix(),
            eye: camera.position_array(),
            viewport,
            px_per_point: self.world_line_px_per_point,
            linear_output: format.is_srgb(),
        };
        pipeline.draw(
            device,
            queue,
            encoder,
            target_view,
            depth_view,
            &self.world_lines,
            &view,
        );
    }
}
