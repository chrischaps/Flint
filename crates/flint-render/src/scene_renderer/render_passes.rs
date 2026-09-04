//! Render pass helpers for `render_to()`.
//!
//! Each method handles one stage of the multi-pass rendering pipeline.

use super::helpers::identity_matrix;
use super::SceneRenderer;
use crate::billboard_pipeline::BillboardUniforms;
use crate::camera::{mat4_inverse, mat4_mul, Camera};
use crate::debug::DebugMode;
use crate::grass_pipeline::{GrassComputeUniforms, GrassRenderUniforms, BLADE_INDEX_COUNT};
use crate::particle_pipeline::ParticleUniforms;
use crate::pipeline::TransformUniforms;
use crate::shadow::{ShadowDrawUniforms, CASCADE_COUNT};
use crate::skybox_pipeline::SkyboxUniforms;
use crate::sprite2d_pipeline::Sprite2dUniforms;
use wgpu::util::DeviceExt;

/// Which slice of the main pass to render. `All` is the classic single-pass
/// path; Pre/PostOcean split around the refraction grab (opaque scene is
/// snapshotted between them so the ocean can sample it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenderPhase {
    All,
    PreOcean,
    PostOcean,
}

impl RenderPhase {
    fn draws_opaque(self) -> bool {
        self != RenderPhase::PostOcean
    }
    fn draws_ocean_and_after(self) -> bool {
        self != RenderPhase::PreOcean
    }
}

impl SceneRenderer {
    /// Render depth from light perspective for cascaded shadow mapping.
    pub(super) fn render_shadow_pass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &Camera,
        camera_pos: [f32; 3],
    ) {
        let shadow_pass = match &mut self.shadow_pass {
            Some(sp) => sp,
            None => return,
        };

        if !shadow_pass.enabled || self.light_uniforms.directional_count == 0 {
            return;
        }

        // Use the first directional light for shadows
        let light = &self.light_uniforms.directional_lights[0];

        // Update cascade matrices. The sun's angular size (radians, 0 =
        // punctual) rides along as tan() for PCSS penumbra math (ADR 0057).
        let camera_inv = camera.inverse_view_projection_matrix();
        let sun_tan_angular = light.angular_size.tan().max(0.0);
        shadow_pass.update_cascades(
            light.direction,
            camera_pos,
            camera_inv,
            0.1,
            200.0,
            100.0,
            sun_tan_angular,
        );

        // Write shadow uniforms
        queue.write_buffer(
            &shadow_pass.shadow_uniforms_buffer,
            0,
            bytemuck::cast_slice(&[*shadow_pass.shadow_uniforms()]),
        );

        // Render each cascade
        for cascade in 0..CASCADE_COUNT {
            let cascade_vp = shadow_pass.shadow_uniforms().cascade_view_proj[cascade];

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(&format!("Shadow Cascade {} Encoder", cascade)),
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&format!("Shadow Cascade {} Pass", cascade)),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &shadow_pass.cascade_views[cascade],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_pipeline(&shadow_pass.shadow_pipeline);

                // Render each solid entity
                for draw in &self.entity_draws {
                    if draw.is_wireframe {
                        continue;
                    }

                    let shadow_uniforms = ShadowDrawUniforms {
                        light_view_proj: cascade_vp,
                        model: draw.model,
                    };

                    let shadow_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Shadow Draw Uniform"),
                            contents: bytemuck::cast_slice(&[shadow_uniforms]),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &shadow_pass.shadow_bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: shadow_buffer.as_entire_binding(),
                        }],
                        label: Some("Shadow Draw Bind Group"),
                    });

                    pass.set_bind_group(0, &shadow_bind, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }

                // Render terrain chunks into shadow map (frustum cull per cascade)
                let cascade_frustum = crate::frustum::Frustum::from_view_projection(&cascade_vp);
                for draw in &self.terrain_draws {
                    if !cascade_frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                        continue;
                    }
                    let shadow_uniforms = ShadowDrawUniforms {
                        light_view_proj: cascade_vp,
                        model: draw.model,
                    };

                    let shadow_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Terrain Shadow Draw Uniform"),
                            contents: bytemuck::cast_slice(&[shadow_uniforms]),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &shadow_pass.shadow_bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: shadow_buffer.as_entire_binding(),
                        }],
                        label: Some("Terrain Shadow Draw Bind Group"),
                    });

                    pass.set_bind_group(0, &shadow_bind, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }

                // Render grass into shadow cascade (nearest 2 only)
                if cascade < 2 {
                    if let (Some(gp), Some(render_bg), Some(instance_bg)) = (
                        &self.grass_pipeline,
                        &self.grass_render_uniform_bind_group,
                        &self.grass_render_instance_bind_group,
                    ) {
                        if self.grass_instance_count > 0 {
                            // Grass shadow pipeline uses TransformUniforms (same layout as PBR)
                            let grass_shadow_uniforms = TransformUniforms {
                                view_proj: cascade_vp,
                                model: identity_matrix(),
                                model_inv_transpose: identity_matrix(),
                                camera_pos: [0.0; 3],
                                _pad: 0.0,
                            };
                            let grass_shadow_buffer =
                                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("Grass Shadow Transform"),
                                    contents: bytemuck::cast_slice(&[grass_shadow_uniforms]),
                                    usage: wgpu::BufferUsages::UNIFORM,
                                });
                            let grass_shadow_bind =
                                device.create_bind_group(&wgpu::BindGroupDescriptor {
                                    layout: &self.pipeline.transform_bind_group_layout,
                                    entries: &[wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: grass_shadow_buffer.as_entire_binding(),
                                    }],
                                    label: Some("Grass Shadow Transform Bind Group"),
                                });

                            pass.set_pipeline(&gp.shadow_pipeline);
                            pass.set_bind_group(0, &grass_shadow_bind, &[]);
                            pass.set_bind_group(1, render_bg, &[]);
                            pass.set_bind_group(2, &gp.shadow_dummy_bind_group, &[]);
                            pass.set_bind_group(3, instance_bg, &[]);
                            pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                gp.blade_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            pass.draw_indexed(
                                0..BLADE_INDEX_COUNT,
                                0,
                                0..self.grass_instance_count,
                            );
                        }
                    }
                }

                // Render transparent entities into shadow map (skip very transparent)
                for draw in &self.transparent_draws {
                    if draw.is_wireframe {
                        continue;
                    }

                    let shadow_uniforms = ShadowDrawUniforms {
                        light_view_proj: cascade_vp,
                        model: draw.model,
                    };

                    let shadow_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Transparent Shadow Draw Uniform"),
                            contents: bytemuck::cast_slice(&[shadow_uniforms]),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &shadow_pass.shadow_bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: shadow_buffer.as_entire_binding(),
                        }],
                        label: Some("Transparent Shadow Draw Bind Group"),
                    });

                    pass.set_bind_group(0, &shadow_bind, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }

                // Render skinned entities into shadow map
                pass.set_pipeline(&shadow_pass.skinned_shadow_pipeline);

                for draw in &self.skinned_entity_draws {
                    let shadow_uniforms = ShadowDrawUniforms {
                        light_view_proj: cascade_vp,
                        model: draw.model,
                    };

                    let shadow_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Skinned Shadow Draw Uniform"),
                            contents: bytemuck::cast_slice(&[shadow_uniforms]),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &shadow_pass.shadow_bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: shadow_buffer.as_entire_binding(),
                        }],
                        label: Some("Skinned Shadow Draw Bind Group"),
                    });

                    pass.set_bind_group(0, &shadow_bind, &[]);
                    pass.set_bind_group(1, &draw.bone_bind_group, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }

            queue.submit(std::iter::once(encoder.finish()));
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
        // Update grid transform
        if let Some(grid) = &self.grid_draw {
            let uniforms = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&grid.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update entity transforms and write debug_mode + tonemapping into each material buffer
        for draw in &self.entity_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            // Write debug_mode at byte offset 28 in the material buffer
            // Layout: base_color(16) + metallic(4) + roughness(4) + use_vertex_color(4) = 28
            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );

            // Write enable_tonemapping at byte offset 32
            // Layout: ... + debug_mode(4) = 32
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update skinned entity transforms
        for draw in &self.skinned_entity_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update transparent entity transforms
        for draw in &self.transparent_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update transparent skinned entity transforms
        for draw in &self.transparent_skinned_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update terrain chunk transforms
        for draw in &self.terrain_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update terrain material buffer (tonemapping flag)
        if let Some(buf) = &self.terrain_material_buffer {
            // enable_tonemapping is at byte offset 12 (texture_tile(4) + metallic(4) + roughness(4))
            queue.write_buffer(buf, 12, bytemuck::cast_slice(&[tonemapping_u32]));
        }

        // Update ocean uniforms (wave phases in f64, grid follows the camera)
        if self.ocean_active {
            if let (Some(spectrum), Some(ubuf), Some(tbuf)) = (
                &self.ocean_spectrum,
                &self.ocean_uniform_buffer,
                &self.ocean_transform_buffer,
            ) {
                let transform = TransformUniforms {
                    view_proj,
                    model: identity_matrix(),
                    model_inv_transpose: identity_matrix(),
                    camera_pos,
                    _pad: 0.0,
                };
                queue.write_buffer(tbuf, 0, bytemuck::cast_slice(&[transform]));

                let v = &self.ocean_visuals;
                // Snap the grid to inner-cell multiples so wave sampling
                // positions stay world-anchored while the mesh follows us.
                let q = v.snap_quantum();
                let grid_offset = [
                    (camera_pos[0] / q).floor() * q,
                    (camera_pos[2] / q).floor() * q,
                ];
                let uniforms = crate::ocean_pipeline::OceanUniformsGpu {
                    waves: spectrum.to_gpu(self.ocean_time),
                    deep_color: v.deep_color,
                    shallow_color: v.shallow_color,
                    foam_color: v.foam_color,
                    sss_color: v.sss_color,
                    grid_offset,
                    num_waves: spectrum.waves.len() as u32,
                    enable_tonemapping: tonemapping_u32,
                    grid_scale: v.grid_scale,
                    fade_start: v.fade_start,
                    fade_end: v.fade_end,
                    foam_threshold: v.foam_threshold,
                    foam_noise_scale: v.foam_noise_scale,
                    ramp_steps: v.ramp_steps,
                    specular_strength: v.specular_strength,
                    time: (self.ocean_time % 100_000.0) as f32,
                    absorption_color: v.absorption_color,
                    turbidity: v.turbidity,
                    refraction_strength: v.refraction_strength,
                    camera_near: self.ocean_camera_near_far.0,
                    camera_far: self.ocean_camera_near_far.1,
                    grab_enabled: u32::from(self.ocean_grab_this_frame),
                    fog_color: {
                        let c = self.postprocess_config.fog_color;
                        let strength = if self.postprocess_config.fog_enabled {
                            1.0
                        } else {
                            0.0
                        };
                        [c[0], c[1], c[2], strength]
                    },
                    sky_zenith: self.sky_params.zenith_color,
                    sky_horizon: self.sky_params.horizon_color,
                    sky_haze: self.sky_params.haze_color,
                    // Reflections need a procedural sky to reflect; scenes
                    // with a texture skybox (or none) keep the old look.
                    sky_reflection_strength: if self.sky_active {
                        v.sky_reflection_strength
                    } else {
                        0.0
                    },
                    // Contact foam is off (strength 0) without a hull.
                    splash_strength: if self.ocean_contact.is_some() {
                        v.splash_strength
                    } else {
                        0.0
                    },
                    splash_width: v.splash_width,
                    splash_flicker_speed: v.splash_flicker_speed,
                    hull_a: self
                        .ocean_contact
                        .map(|(a, _)| a)
                        .unwrap_or([0.0, 0.0, 1.0, 0.0]),
                    hull_b: {
                        let h = self.ocean_contact.map(|(_, h)| h).unwrap_or([1.0, 1.0]);
                        [h[0], h[1], v.splash_baseline, v.splash_noise_scale]
                    },
                    hull_c: {
                        let hv = self.ocean_contact_vel;
                        [hv[0], hv[1], hv[2], v.splash_response]
                    },
                    band_wobble: v.band_wobble,
                    band_dither: v.band_dither,
                    band_dither_scale: v.band_dither_scale,
                    rain_ripple: v.rain_ripple,
                    foam_glow: [
                        v.foam_glow_color[0],
                        v.foam_glow_color[1],
                        v.foam_glow_color[2],
                        v.foam_glow,
                    ],
                    // Reality tear rides the shared post-process fields: the
                    // remap happens in-shader AFTER the TOD palette lands in
                    // these uniforms, so it can never race time_of_day.rhai.
                    mode: self.postprocess_config.render_mode,
                    mode_mix: self.postprocess_config.mode_mix,
                    _pad_mode: [0.0; 2],
                };
                queue.write_buffer(ubuf, 0, bytemuck::cast_slice(&[uniforms]));
            }
        }

        // Sort transparent draws back-to-front (descending distance from camera)
        {
            let cam = camera_pos;
            for draw in &mut self.transparent_draws {
                let dx = draw.model[3][0] - cam[0];
                let dy = draw.model[3][1] - cam[1];
                let dz = draw.model[3][2] - cam[2];
                draw.sort_depth = dx * dx + dy * dy + dz * dz;
            }
            self.transparent_draws.sort_by(|a, b| {
                b.sort_depth
                    .partial_cmp(&a.sort_depth)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            for draw in &mut self.transparent_skinned_draws {
                let dx = draw.model[3][0] - cam[0];
                let dy = draw.model[3][1] - cam[1];
                let dz = draw.model[3][2] - cam[2];
                draw.sort_depth = dx * dx + dy * dy + dz * dz;
            }
            self.transparent_skinned_draws.sort_by(|a, b| {
                b.sort_depth
                    .partial_cmp(&a.sort_depth)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
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
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update billboard uniforms with camera vectors
        {
            let cam_right = camera.right_vector();
            let cam_up = camera.up_vector();
            let billboard_uniforms = BillboardUniforms {
                view_proj,
                camera_right: cam_right,
                _pad0: 0.0,
                camera_up: cam_up,
                _pad1: 0.0,
            };
            for draw in &self.billboard_draws {
                queue.write_buffer(
                    &draw.billboard_buffer,
                    0,
                    bytemuck::cast_slice(&[billboard_uniforms]),
                );
            }
        }

        // Update particle uniforms with camera vectors
        if let Some(pp) = &self.particle_pipeline {
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
        if let Some(sp) = &self.sprite2d_pipeline {
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
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
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
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }
    }

    /// Execute the main render pass: skybox → grid → terrain → entities → sprites → particles → debug.
    pub(super) fn render_main_pass<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        wireframe_only: bool,
        queue: &wgpu::Queue,
        camera: &Camera,
        phase: RenderPhase,
    ) {
        // Bind lights once for the entire pass (group 2 is shared)
        render_pass.set_bind_group(2, &self.light_bind_group, &[]);

        // Render sky at the far plane: the procedural sky (driven by the
        // `sky` component) replaces the texture skybox when present.
        let mut sky_drawn = false;
        if self.sky_active && phase.draws_opaque() {
            if let (Some(sp), Some(ub), Some(ubg)) = (
                &self.sky_pipeline,
                &self.sky_uniform_buffer,
                &self.sky_uniform_bind_group,
            ) {
                let view = camera.view_matrix();
                let view_rot_only = [view[0], view[1], view[2], [0.0, 0.0, 0.0, 1.0]];
                let proj = camera.projection_matrix();
                let vp = mat4_mul(&proj, &view_rot_only);
                let inv_vp = mat4_inverse(&vp);

                // Sun disc = the scene's first directional light, so the
                // drawn sun and the lighting can never disagree.
                let sun = &self.light_uniforms.directional_lights[0];
                let p = &self.sky_params;
                let uniforms = crate::sky_pipeline::SkyUniformsGpu {
                    inv_view_proj: inv_vp,
                    sun_dir: sun.direction,
                    sun_disc_size: p.sun_disc_size,
                    sun_color: [
                        sun.color[0] * sun.intensity,
                        sun.color[1] * sun.intensity,
                        sun.color[2] * sun.intensity,
                        1.0,
                    ],
                    zenith_color: p.zenith_color,
                    horizon_color: p.horizon_color,
                    haze_color: p.haze_color,
                    cloud_tint: p.cloud_tint,
                    sun_glow: p.sun_glow,
                    star_opacity: p.star_opacity,
                    cloud_coverage: p.cloud_coverage,
                    cloud_density: p.cloud_density,
                    cloud_scale: p.cloud_scale,
                    cloud_drift_x: p.cloud_drift_x,
                    cloud_drift_y: p.cloud_drift_y,
                    time: (self.ocean_time % 100_000.0) as f32,
                };
                queue.write_buffer(ub, 0, bytemuck::cast_slice(&[uniforms]));

                render_pass.set_pipeline(&sp.pipeline);
                render_pass.set_bind_group(0, ubg, &[]);
                render_pass.draw(0..3, 0..1);
                sky_drawn = true;
            }
        }

        // Render texture skybox (before everything else, at the far plane)
        if !sky_drawn && phase.draws_opaque() {
            if let (Some(sp), Some(ub), Some(ubg), Some(tbg)) = (
                &self.skybox_pipeline,
                &self.skybox_uniform_buffer,
                &self.skybox_uniform_bind_group,
                &self.skybox_texture_bind_group,
            ) {
                // Build view matrix with translation stripped (rotation only)
                let view = camera.view_matrix();
                let view_rot_only = [
                    view[0],
                    view[1],
                    view[2],
                    [0.0, 0.0, 0.0, 1.0], // zero out translation column
                ];
                let proj = camera.projection_matrix();
                let vp = mat4_mul(&proj, &view_rot_only);
                let inv_vp = mat4_inverse(&vp);

                queue.write_buffer(
                    ub,
                    0,
                    bytemuck::cast_slice(&[SkyboxUniforms {
                        inv_view_proj: inv_vp,
                    }]),
                );

                render_pass.set_pipeline(&sp.pipeline);
                render_pass.set_bind_group(0, ubg, &[]);
                render_pass.set_bind_group(1, tbg, &[]);
                render_pass.draw(0..3, 0..1);
            }
        }

        // Render grid
        if phase.draws_opaque() {
            if let Some(grid) = &self.grid_draw {
                render_pass.set_pipeline(&self.pipeline.line_pipeline);
                render_pass.set_bind_group(0, &grid.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &grid.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, grid.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(grid.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..grid.index_count, 0, 0..1);
            }
        }

        // In WireframeOnly mode: depth prepass masks outline interior,
        // then outline, then wireframe lines on top.
        if wireframe_only {
            self.render_wireframe_only_pass(render_pass);
        } else {
            self.render_normal_pass(render_pass, phase);
        }

        // Normal arrows pass (always uses line pipeline)
        if self.debug_state.show_normals && phase.draws_ocean_and_after() {
            render_pass.set_pipeline(&self.pipeline.line_pipeline);
            for draw in &self.normal_arrow_draws {
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // Skeleton overlay pass (bone lines, drawn on top of everything — no depth test)
        if self.debug_state.show_skeleton && phase.draws_ocean_and_after() {
            render_pass.set_pipeline(&self.pipeline.skeleton_line_pipeline);
            render_pass.set_bind_group(2, &self.light_bind_group, &[]);
            for draw in &self.skeleton_overlay_draws {
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // Editor/gizmo overlay (ADR 0068): same line pipeline, never gated
        // behind the skeleton toggle.
        if !self.debug_overlay_draws.is_empty() && phase.draws_ocean_and_after() {
            render_pass.set_pipeline(&self.pipeline.skeleton_line_pipeline);
            // The particle pass may have left its texture bind group at slot 2.
            render_pass.set_bind_group(2, &self.light_bind_group, &[]);
            for draw in &self.debug_overlay_draws {
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }
    }

    /// Draw the wireframe edges of every skinned draw call (opaque and
    /// transparent) through the skinned line pipeline so they follow the pose.
    fn draw_skinned_wireframes<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let Some(sp) = &self.skinned_pipeline else {
            return;
        };
        render_pass.set_pipeline(&sp.wire_line_pipeline);
        for draw in self
            .skinned_entity_draws
            .iter()
            .chain(self.transparent_skinned_draws.iter())
        {
            let Some(wire_ib) = &draw.wire_index_buffer else {
                continue;
            };
            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
            render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
            render_pass.set_index_buffer(wire_ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..draw.wire_index_count, 0, 0..1);
        }
    }

    /// Wireframe-only mode rendering: depth prepass + outline + wireframe lines.
    fn render_wireframe_only_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if let Some(sel_id) = self.selected_entity {
            // Step 1: Depth prepass — write front-face depth (no color)
            // so the outline interior is blocked by the depth buffer.
            render_pass.set_pipeline(&self.pipeline.depth_prepass_pipeline);
            for draw in self
                .entity_draws
                .iter()
                .chain(self.transparent_draws.iter())
            {
                if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }

            if let Some(sp) = &self.skinned_pipeline {
                render_pass.set_pipeline(&sp.depth_prepass_pipeline);
                for draw in self
                    .skinned_entity_draws
                    .iter()
                    .chain(self.transparent_skinned_draws.iter())
                {
                    if draw.entity_id == Some(sel_id) {
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }

            // Step 2: Outline — back faces pushed out, only visible at silhouette
            render_pass.set_pipeline(&self.pipeline.outline_pipeline);
            for draw in self
                .entity_draws
                .iter()
                .chain(self.transparent_draws.iter())
            {
                if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }

            if let Some(sp) = &self.skinned_pipeline {
                render_pass.set_pipeline(&sp.outline_pipeline);
                for draw in self
                    .skinned_entity_draws
                    .iter()
                    .chain(self.transparent_skinned_draws.iter())
                {
                    if draw.entity_id == Some(sel_id) {
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }
        }

        // Step 3: Wireframe lines — use overlay pipeline (LessEqual + bias)
        // so lines draw on top of the depth prepass.
        render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
        for draw in &self.wireframe_overlay_draws {
            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
            render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
            render_pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
        }

        // Also render entities that are already wireframe (rooms etc.)
        for draw in &self.entity_draws {
            if draw.is_wireframe {
                render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // Skinned meshes: edges posed by the bone matrices
        self.draw_skinned_wireframes(render_pass);
    }

    /// Normal mode rendering: terrain → outlines → entities → skinned → billboards → sprites → transparent → particles → wireframe.
    fn render_normal_pass<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        phase: RenderPhase,
    ) {
        // Terrain rendering (early in pass — fills depth buffer for occlusion)
        if phase.draws_opaque() {
            if let (Some(tp), Some(mat_bg)) =
                (&self.terrain_pipeline, &self.terrain_material_bind_group)
            {
                if !self.terrain_draws.is_empty() {
                    render_pass.set_pipeline(&tp.pipeline);
                    render_pass.set_bind_group(1, mat_bg, &[]);
                    render_pass.set_bind_group(2, &self.light_bind_group, &[]);

                    for draw in &self.terrain_draws {
                        if let Some(ref frustum) = self.camera_frustum {
                            if !frustum.aabb_visible(draw.aabb_min, draw.aabb_max) {
                                continue;
                            }
                        }
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }

            // Grass rendering (after terrain -- also writes depth)
            if let (Some(gp), Some(render_bg), Some(instance_bg)) = (
                &self.grass_pipeline,
                &self.grass_render_uniform_bind_group,
                &self.grass_render_instance_bind_group,
            ) {
                if self.grass_instance_count > 0 {
                    // Explicitly set bind group 0 (transform) — cannot rely on terrain having set it
                    if let Some(first_terrain) = self.terrain_draws.first() {
                        render_pass.set_bind_group(0, &first_terrain.transform_bind_group, &[]);
                    }
                    render_pass.set_pipeline(&gp.render_pipeline);
                    render_pass.set_bind_group(1, render_bg, &[]);
                    render_pass.set_bind_group(2, &self.light_bind_group, &[]);
                    render_pass.set_bind_group(3, instance_bg, &[]);
                    render_pass.set_vertex_buffer(0, gp.blade_vertex_buffer.slice(..));
                    render_pass.set_index_buffer(
                        gp.blade_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    render_pass.draw_indexed(0..BLADE_INDEX_COUNT, 0, 0..self.grass_instance_count);
                }
            }
        } // end opaque phase (terrain + grass)

        // Ocean rendering (after all opaque — writes depth so fog applies;
        // group 3 holds the grab-pass snapshots, or dummies when disabled)
        if self.ocean_active && phase.draws_ocean_and_after() {
            let grab_bg = if self.ocean_grab_this_frame {
                self.ocean_grab_bind_group.as_ref()
            } else {
                self.ocean_grab_dummy_bind_group.as_ref()
            };
            if let (Some(op), Some(ocean_bg), Some(transform_bg), Some(grab_bg)) = (
                &self.ocean_pipeline,
                &self.ocean_uniform_bind_group,
                &self.ocean_transform_bind_group,
                grab_bg,
            ) {
                render_pass.set_pipeline(&op.pipeline);
                render_pass.set_bind_group(0, transform_bg, &[]);
                render_pass.set_bind_group(1, ocean_bg, &[]);
                render_pass.set_bind_group(2, &self.light_bind_group, &[]);
                render_pass.set_bind_group(3, grab_bg, &[]);
                render_pass.set_vertex_buffer(0, op.grid_vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(op.grid_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..op.grid_index_count, 0, 0..1);
            }
        }

        if phase.draws_opaque() {
            // Outline pass for selected standard entities (before normal rendering)
            if let Some(sel_id) = self.selected_entity {
                render_pass.set_pipeline(&self.pipeline.outline_pipeline);
                for draw in self
                    .entity_draws
                    .iter()
                    .chain(self.transparent_draws.iter())
                {
                    if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }

            // Normal entity rendering
            for draw in &self.entity_draws {
                if draw.is_wireframe {
                    render_pass.set_pipeline(&self.pipeline.line_pipeline);
                } else {
                    render_pass.set_pipeline(&self.pipeline.pipeline);
                }
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }

            // Outline pass for selected skinned entities
            if let Some(sel_id) = self.selected_entity {
                if let Some(sp) = &self.skinned_pipeline {
                    render_pass.set_pipeline(&sp.outline_pipeline);
                    for draw in self
                        .skinned_entity_draws
                        .iter()
                        .chain(self.transparent_skinned_draws.iter())
                    {
                        if draw.entity_id == Some(sel_id) {
                            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                            render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            render_pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }
                }
            }

            // Render skinned entities
            if let Some(sp) = &self.skinned_pipeline {
                if !self.skinned_entity_draws.is_empty() {
                    render_pass.set_pipeline(&sp.pipeline);
                    for draw in &self.skinned_entity_draws {
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                        // Light bind group 2 is already set
                        render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }

            // Outline pass for selected billboards
            if let Some(sel_id) = self.selected_entity {
                if let Some(bp) = &self.billboard_pipeline {
                    render_pass.set_pipeline(&bp.outline_pipeline);
                    for draw in &self.billboard_draws {
                        if draw.entity_id == Some(sel_id) {
                            render_pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                            render_pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                            render_pass.set_index_buffer(
                                bp.quad_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..6, 0, 0..1);
                        }
                    }
                }
            }

            // Billboard sprites (after geometry, uses depth test)
            if let Some(bp) = &self.billboard_pipeline {
                if !self.billboard_draws.is_empty() {
                    render_pass.set_pipeline(&bp.pipeline);
                    for draw in &self.billboard_draws {
                        render_pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                        render_pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                        render_pass.set_index_buffer(
                            bp.quad_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..6, 0, 0..1);
                    }
                }
            }
        } // end opaque phase (outlines + entities + skinned + billboards)

        if !phase.draws_ocean_and_after() {
            return;
        }

        // 2D sprites (after billboards, instanced batched rendering)
        if let Some(sp) = &self.sprite2d_pipeline {
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

        // Transparent PBR entities (sorted back-to-front, depth write OFF; alpha
        // blend draws back faces then front faces so a mesh cannot blend over itself)
        if !self.transparent_draws.is_empty() {
            // Re-bind lights for group 2 (may have been displaced by billboard binds)
            render_pass.set_bind_group(2, &self.light_bind_group, &[]);

            for draw in &self.transparent_draws {
                if draw.is_wireframe {
                    continue;
                }
                let pipeline = match draw.blend_mode {
                    crate::pipeline::BlendMode::Alpha => &self.pipeline.transparent_alpha_pipeline,
                    crate::pipeline::BlendMode::Additive => {
                        &self.pipeline.transparent_additive_pipeline
                    }
                    crate::pipeline::BlendMode::Multiply => {
                        &self.pipeline.transparent_multiply_pipeline
                    }
                };
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                if draw.blend_mode == crate::pipeline::BlendMode::Alpha {
                    // Back faces first so the front faces blend over them.
                    render_pass.set_pipeline(&self.pipeline.transparent_alpha_back_pipeline);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
                render_pass.set_pipeline(pipeline);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // Transparent skinned entities (sorted back-to-front, depth write OFF)
        if let Some(sp) = &self.skinned_pipeline {
            if !self.transparent_skinned_draws.is_empty() {
                for draw in &self.transparent_skinned_draws {
                    let pipeline = match draw.blend_mode {
                        crate::pipeline::BlendMode::Alpha => &sp.transparent_alpha_pipeline,
                        crate::pipeline::BlendMode::Additive => &sp.transparent_additive_pipeline,
                        crate::pipeline::BlendMode::Multiply => &sp.transparent_multiply_pipeline,
                    };
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                    render_pass.set_bind_group(2, &self.light_bind_group, &[]);
                    render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    if draw.blend_mode == crate::pipeline::BlendMode::Alpha {
                        render_pass.set_pipeline(&sp.transparent_alpha_back_pipeline);
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                    render_pass.set_pipeline(pipeline);
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }
        }

        // Particle systems (after billboards + transparent, before wireframe overlay).
        // Draws arrive pre-sorted (order-dependent blends far-to-near, additive
        // last) and all index one shared instance buffer (ADR 0068).
        if let Some(pp) = &self.particle_pipeline {
            if !self.particle_draws.is_empty() {
                render_pass
                    .set_index_buffer(pp.quad_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.set_bind_group(0, &pp.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, pp.instance_bind_group(), &[]);

                let mut current: Option<flint_particles::ParticleBlendMode> = None;
                for draw in &self.particle_draws {
                    if current != Some(draw.blend) {
                        render_pass.set_pipeline(pp.pipeline(draw.blend));
                        current = Some(draw.blend);
                    }
                    render_pass.set_bind_group(2, draw.texture_bind_group.as_ref(), &[]);
                    let first = draw.first_instance;
                    render_pass.draw_indexed(0..6, 0, first..first + draw.instance_count);
                }
            }
        }

        // Wireframe overlay pass (on top of solid geometry)
        if self.debug_state.mode == DebugMode::WireframeOverlay {
            render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
            for draw in &self.wireframe_overlay_draws {
                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
            self.draw_skinned_wireframes(render_pass);
        }
    }

    /// Run post-processing: SSAO, bloom, composite HDR → sRGB.
    pub(super) fn render_postprocess(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth_view: &wgpu::TextureView,
        target_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        let pp = match &self.postprocess_pipeline {
            Some(pp) => pp,
            None => return,
        };
        let resources = match &self.postprocess_resources {
            Some(r) => r,
            None => return,
        };

        // Run Kuwahara filter if enabled
        if self.postprocess_config.enabled && self.postprocess_config.kuwahara_enabled {
            pp.run_kuwahara(device, queue, resources, &self.postprocess_config);
        }

        // Run SSAO if enabled
        if self.postprocess_config.enabled && self.postprocess_config.ssao_enabled {
            pp.run_ssao(
                device,
                queue,
                resources,
                &self.postprocess_config,
                depth_view,
                camera,
            );
        }

        // Run volumetric (god rays) if enabled and shadows are available
        if self.postprocess_config.enabled && self.postprocess_config.volumetric_enabled {
            if let Some(shadow_pass) = &self.shadow_pass {
                if shadow_pass.enabled {
                    pp.run_volumetric(
                        device,
                        queue,
                        resources,
                        &self.postprocess_config,
                        depth_view,
                        camera,
                        &shadow_pass.shadow_view,
                        &shadow_pass.shadow_sampler,
                        &shadow_pass.shadow_uniforms_buffer,
                        &self.light_buffer,
                    );
                }
            }
        }

        // Run bloom if post-processing effects are enabled
        if self.postprocess_config.enabled
            && self.postprocess_config.bloom_enabled
            && resources.bloom_mip_count > 0
        {
            pp.run_bloom(device, queue, resources, &self.postprocess_config);
        }

        // FXAA (ADR 0050): when active, composite renders into the lazily
        // allocated intermediate instead of the swapchain, and the FXAA pass
        // resolves intermediate → target. Falls back to the direct path if
        // ensure_fxaa_resources hasn't run (both Options are checked).
        let fxaa_active = self.postprocess_config.enabled
            && self.postprocess_config.fxaa_enabled
            && pp.fxaa.is_some()
            && resources.fxaa.is_some();
        let composite_target = if fxaa_active {
            &resources.fxaa.as_ref().unwrap().view
        } else {
            target_view
        };

        // Composite: HDR + bloom + SSAO + volumetric + fog → tonemapped sRGB surface
        // (always runs — this is what converts Rgba16Float → surface format)
        pp.composite(
            device,
            queue,
            resources,
            &self.postprocess_config,
            composite_target,
            depth_view,
            camera,
            // Same f64 phase wrap as the ocean upload — keeps mode animation
            // precise over long sessions. Headless render leaves this 0.0.
            (self.ocean_time % 100_000.0) as f32,
        );

        if fxaa_active {
            pp.run_fxaa(device, queue, resources, target_view);
        }
    }

    /// Dispatch the grass compute shader to scatter instances.
    /// Call before render_main_pass, after update_per_frame_uniforms.
    pub fn dispatch_grass_compute(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        time: f32,
    ) {
        let config = match &self.grass_config {
            Some(c) if c.enabled => c.clone(),
            _ => return,
        };

        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let compute_uniform_bg = match &self.grass_compute_uniform_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_texture_bg = match &self.grass_compute_texture_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_storage_bg = match &self.grass_compute_storage_bind_group {
            Some(bg) => bg,
            None => return,
        };

        // Reset atomic counter to 0
        if let Some(counter_buf) = &self.grass_counter_buffer {
            queue.write_buffer(counter_buf, 0, &[0u8; 4]);
        }

        // Update compute uniforms
        let uniforms = GrassComputeUniforms {
            camera_pos: [camera.position.x, camera.position.y, camera.position.z],
            time,
            terrain_offset: self.grass_terrain_offset,
            density: config.density,
            terrain_width: self.grass_terrain_width,
            terrain_depth: self.grass_terrain_depth,
            height_scale: self.grass_terrain_height_scale,
            max_distance: config.max_distance,
            fade_start: config.fade_start,
            density_threshold: config.density_threshold,
            density_layer: config.density_layer,
            blade_height: config.blade_height,
            height_variation: config.height_variation,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };

        if let Some(buf) = &self.grass_compute_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update render uniforms (wind, colors, etc.)
        let wind_dir = {
            let d = config.wind_direction;
            let len = (d[0] * d[0] + d[2] * d[2]).sqrt().max(0.001);
            [d[0] / len, d[1], d[2] / len]
        };

        let render_uniforms = GrassRenderUniforms {
            wind_direction: wind_dir,
            wind_speed: config.wind_speed,
            wind_strength: config.wind_strength,
            time,
            bend_radius: config.bend_radius,
            bend_strength: config.bend_strength,
            color_base: config.color_base,
            blade_width: config.blade_width,
            color_tip: config.color_tip,
            blade_height: config.blade_height,
            color_dry: config.color_dry,
            dry_amount: config.dry_amount,
            entity_count: self.grass_entity_count,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };

        if let Some(buf) = &self.grass_render_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[render_uniforms]));
        }

        // Compute dispatch
        let spacing = 1.0 / config.density.sqrt();
        let grid_x = (self.grass_terrain_width / spacing).ceil() as u32;
        let grid_z = (self.grass_terrain_depth / spacing).ceil() as u32;
        let workgroups_x = (grid_x + 7) / 8;
        let workgroups_z = (grid_z + 7) / 8;

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Grass Compute Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&grass_pipeline.compute_pipeline);
            pass.set_bind_group(0, compute_uniform_bg, &[]);
            pass.set_bind_group(1, compute_texture_bg, &[]);
            pass.set_bind_group(2, compute_storage_bg, &[]);
            pass.dispatch_workgroups(workgroups_x, workgroups_z, 1);
        }

        // Copy counter to staging buffer for CPU readback
        if let (Some(counter), Some(staging)) =
            (&self.grass_counter_buffer, &self.grass_staging_buffer)
        {
            encoder.copy_buffer_to_buffer(counter, 0, staging, 0, 4);
        }
    }

    /// Read back the grass instance count from the staging buffer.
    /// Call after submitting the command buffer from the previous frame.
    pub fn read_grass_instance_count(&mut self, device: &wgpu::Device) {
        let staging = match &self.grass_staging_buffer {
            Some(s) => s,
            None => return,
        };

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::Maintain::Wait);

        if let Ok(Ok(())) = rx.recv() {
            let data = slice.get_mapped_range();
            let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            self.grass_instance_count = count.min(self.grass_max_instances);
            drop(data);
            staging.unmap();
        }
    }
}
