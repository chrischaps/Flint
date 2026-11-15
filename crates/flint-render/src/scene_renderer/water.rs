//! The sea and the game's lakes and rivers, owned by [`WaterRenderer`]
//! (ADR 0118, #736): the ocean's uniforms and transform, the wave spectrum
//! and look a scene's `ocean` gives, the contact-foam hull, water bodies
//! (ADR 0094), and the refraction grab the water reads (an opaque-scene
//! snapshot, kept per view so a comic panel has its own). The pipelines are
//! in `ScenePipelines`; the host's clock is still `SceneRenderer::ocean_time`.

use super::helpers::identity_matrix;
use crate::ocean_pipeline::OceanPipeline;
use crate::pipeline::TransformUniforms;
use crate::postprocess::HDR_FORMAT;
use crate::sky_pipeline::SkyParams;
use flint_core::components as comp;
use flint_ecs::FlintWorld;

/// The water's GPU resources and what the scene says about it.
pub(crate) struct WaterRenderer {
    uniform_buffer: Option<wgpu::Buffer>,
    uniform_bind_group: Option<wgpu::BindGroup>,
    transform_buffer: Option<wgpu::Buffer>,
    transform_bind_group: Option<wgpu::BindGroup>,
    /// The sea's own body block (group 1, binding 1): its colours, full
    /// waves, no flow (ADR 0094).
    sea_body_buffer: Option<wgpu::Buffer>,
    /// Water bodies a game set at run time, by its id (ADR 0094).
    bodies: std::collections::BTreeMap<String, crate::water_body::GpuWaterBody>,
    /// Water bodies from `water_body` components, by entity name, with the
    /// component they were built from (rebuilt only when it changes). `None`
    /// when the component didn't describe a mesh; the warning said why.
    scene_bodies:
        std::collections::BTreeMap<String, (toml::Value, Option<crate::water_body::GpuWaterBody>)>,
    spectrum: Option<flint_geometry::ocean::WaveSpectrum>,
    visuals: crate::ocean_pipeline::OceanVisuals,
    /// The scene has a sea this frame (and its pipeline built).
    ocean_active: bool,
    /// Contact-foam hull: ([x, z, cos_yaw, sin_yaw], [half_x, half_z]).
    /// None when the scene has no `ocean_contact` entity.
    contact: Option<([f32; 4], [f32; 2])>,
    /// (ocean_time, hull position) of the previous frame, for velocity.
    contact_prev: Option<(f64, [f32; 3])>,
    /// Smoothed hull velocity fed to the contact-foam churn term.
    contact_vel: [f32; 3],
    /// Grab pass (refraction): opaque scene color+depth snapshots, at the
    /// view's size.
    pub(crate) grab: OceanGrab,
    grab_dummy_bind_group: Option<wgpu::BindGroup>,
    /// The grab runs this frame: water, the HDR chain, normal shading.
    pub(crate) grab_this_frame: bool,
    /// The camera's near and far planes, which the water linearises the
    /// grabbed depth with.
    pub(crate) camera_near_far: (f32, f32),
}

/// The refraction grab at one view's size. A comic panel's `ViewSlot`
/// holds its own and swaps it in around its draw.
#[derive(Default)]
pub(crate) struct OceanGrab {
    pub(crate) color: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub(crate) depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    size: (u32, u32),
    pub(crate) bind_group: Option<wgpu::BindGroup>,
}

/// What the water's uniforms read from the rest of the frame.
pub(crate) struct WaterFrame<'a> {
    /// The wave clock (`SceneRenderer::ocean_time`).
    pub(crate) time: f64,
    pub(crate) tonemapping: u32,
    /// The sky's colours, which the water reflects when the sky is active.
    pub(crate) sky: &'a SkyParams,
    pub(crate) sky_active: bool,
    /// The reality tear's mode and mix, from the post-process config.
    pub(crate) render_mode: u32,
    pub(crate) mode_mix: f32,
}

impl WaterRenderer {
    /// The ocean's uniform/transform buffers and bind groups, when its
    /// pipeline built (`ScenePipelines` leaves it out when its shader fails:
    /// no ocean rather than a crashed renderer).
    pub(crate) fn new(
        device: &wgpu::Device,
        pipeline: Option<&crate::ocean_pipeline::OceanPipeline>,
        transform_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let Some(pipeline) = pipeline else {
            return Self::without_gpu(None, None, None, None, None);
        };

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Uniform Buffer"),
            size: std::mem::size_of::<crate::ocean_pipeline::OceanUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sea_body_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Sea Body Buffer"),
            size: std::mem::size_of::<crate::water_body::WaterBodyUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.ocean_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sea_body_buffer.as_entire_binding(),
                },
            ],
            label: Some("Ocean Uniform Bind Group"),
        });

        let transform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean Transform Buffer"),
            size: std::mem::size_of::<TransformUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: transform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: transform_buffer.as_entire_binding(),
            }],
            label: Some("Ocean Transform Bind Group"),
        });

        Self::without_gpu(
            Some(uniform_buffer),
            Some(uniform_bind_group),
            Some(transform_buffer),
            Some(transform_bind_group),
            Some(sea_body_buffer),
        )
    }

    /// Everything but the GPU resources, empty.
    fn without_gpu(
        uniform_buffer: Option<wgpu::Buffer>,
        uniform_bind_group: Option<wgpu::BindGroup>,
        transform_buffer: Option<wgpu::Buffer>,
        transform_bind_group: Option<wgpu::BindGroup>,
        sea_body_buffer: Option<wgpu::Buffer>,
    ) -> Self {
        Self {
            uniform_buffer,
            uniform_bind_group,
            transform_buffer,
            transform_bind_group,
            sea_body_buffer,
            bodies: std::collections::BTreeMap::new(),
            scene_bodies: std::collections::BTreeMap::new(),
            spectrum: None,
            visuals: crate::ocean_pipeline::OceanVisuals::default(),
            ocean_active: false,
            contact: None,
            contact_prev: None,
            contact_vel: [0.0; 3],
            grab: OceanGrab::default(),
            grab_dummy_bind_group: None,
            grab_this_frame: false,
            camera_near_far: (0.1, 1000.0),
        }
    }

    /// Trade this view's grab for a panel's (`panels::ViewSlot`).
    pub(crate) fn swap_grab(&mut self, other: &mut OceanGrab) {
        std::mem::swap(&mut self.grab, other);
    }

    /// Create the placeholder group-3 bind group (1x1 textures) so the ocean
    /// can always bind group 3 even when the grab pass didn't run.
    pub(crate) fn ensure_grab_dummy(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
    ) {
        if self.grab_dummy_bind_group.is_some() {
            return;
        }
        let Some(op) = pipeline else {
            return;
        };
        let dummy_color = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            HDR_FORMAT,
            "Ocean Grab Dummy Color",
        );
        let dummy_depth = crate::ocean_pipeline::create_dummy_grab_texture(
            device,
            wgpu::TextureFormat::R32Float,
            "Ocean Grab Dummy Depth",
        );
        let cv = dummy_color.create_view(&Default::default());
        let dv = dummy_depth.create_view(&Default::default());
        self.grab_dummy_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &op.grab_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cv),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dv),
                },
            ],
            label: Some("Ocean Grab Dummy Bind Group"),
        }));
    }

    /// Ensure grab-pass textures + bind groups exist at the given size.
    /// Called only on the ocean+postprocess path; cheap when size unchanged.
    pub(crate) fn ensure_grab(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        width: u32,
        height: u32,
    ) {
        self.ensure_grab_dummy(device, pipeline);
        let Some(op) = pipeline else {
            return;
        };

        if self.grab.size == (width, height) && self.grab.bind_group.is_some() {
            return;
        }

        let make = |format: wgpu::TextureFormat, label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let color = make(HDR_FORMAT, "Ocean Grab Color");
        let depth = make(wgpu::TextureFormat::R32Float, "Ocean Grab Depth");

        self.grab.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &op.grab_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color.1),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth.1),
                },
            ],
            label: Some("Ocean Grab Bind Group"),
        }));
        self.grab.color = Some(color);
        self.grab.depth = Some(depth);
        self.grab.size = (width, height);
    }

    /// Extract the first `ocean` component from the world. The wave spectrum
    /// is regenerated only when simulation params change (cheap: N sin/cos
    /// coefficient sets), so live-tweaking any field takes effect next frame.
    pub(crate) fn extract_ocean(&mut self, world: &FlintWorld, has_pipeline: bool) {
        self.ocean_active = false;
        for &entity_id in world.entities_with_component(flint_geometry::components::OCEAN) {
            let ocean_comp = world
                .get_components(entity_id)
                .and_then(|components| components.get(flint_geometry::components::OCEAN).cloned());
            let Some(ocean_comp) = ocean_comp else {
                continue;
            };

            let params = flint_geometry::ocean::OceanParams::from_component(&ocean_comp);
            let needs_regen = self
                .spectrum
                .as_ref()
                .map(|s| s.params != params)
                .unwrap_or(true);
            if needs_regen {
                self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(&params));
            }
            self.visuals = crate::ocean_pipeline::OceanVisuals::from_component(&ocean_comp);
            self.ocean_active = has_pipeline;
            break; // one ocean per scene
        }
    }

    /// Whether any water draws this frame: the sea, or a body.
    pub(crate) fn any_active(&self, has_pipeline: bool) -> bool {
        self.ocean_active || self.has_bodies(has_pipeline)
    }

    fn has_bodies(&self, has_pipeline: bool) -> bool {
        has_pipeline
            && (!self.bodies.is_empty() || self.scene_bodies.values().any(|(_, b)| b.is_some()))
    }

    /// Every body to draw this frame: the scene's, then the game's.
    pub(crate) fn bodies_to_draw(&self) -> impl Iterator<Item = &crate::water_body::GpuWaterBody> {
        self.scene_bodies
            .values()
            .filter_map(|(_, b)| b.as_ref())
            .chain(self.bodies.values())
    }

    fn build_body(
        &self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        label: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<crate::water_body::GpuWaterBody, String> {
        let (Some(op), Some(ocean_buf)) = (pipeline, &self.uniform_buffer) else {
            return Err("this device has no ocean pipeline".into());
        };
        mesh.validate()?;
        if mesh.indices.is_empty() {
            return Err("water body has no triangles".into());
        }
        Ok(crate::water_body::GpuWaterBody::new(
            device,
            &op.ocean_bind_group_layout,
            ocean_buf,
            label,
            mesh,
            look,
        ))
    }

    /// Build the bodies `water_body` components describe. A body is rebuilt
    /// only when its component changes, and dropped with its entity.
    pub(crate) fn extract_bodies(
        &mut self,
        world: &FlintWorld,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
    ) {
        let mut seen = std::collections::BTreeSet::new();
        for &entity_id in world.entities_with_component(flint_geometry::components::WATER_BODY) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(value) = components.get(flint_geometry::components::WATER_BODY) else {
                continue;
            };
            let name = world
                .get_name(entity_id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{entity_id:?}"));
            seen.insert(name.clone());
            if self
                .scene_bodies
                .get(&name)
                .is_some_and(|(v, _)| v == value)
            {
                continue;
            }
            let look = crate::water_body::WaterBodyLook::from_component(value);
            let body = crate::water_body::mesh_from_component(value)
                .and_then(|mesh| self.build_body(device, pipeline, &name, &mesh, look));
            let body = match body {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("water_body `{name}`: {e}");
                    None
                }
            };
            self.scene_bodies.insert(name, (value.clone(), body));
        }
        self.scene_bodies.retain(|name, _| seen.contains(name));

        // Bodies take the sea's look and waves; a scene with bodies and no
        // ocean gets the default ocean's.
        if !self.ocean_active && self.has_bodies(pipeline.is_some()) && self.spectrum.is_none() {
            self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(
                &flint_geometry::ocean::OceanParams::default(),
            ));
        }
    }

    /// Draw water on a game's own mesh (ADR 0094): a lake at its level, a
    /// river down a valley. It shades as the sea does, with the body's own
    /// tint, wave scale and flow, and the sea's (or the default ocean's)
    /// foam, refraction and reflection. Replaces any body set with this
    /// `id`. Errs when the mesh's indices are bad or it has no triangles.
    pub(crate) fn set_body(
        &mut self,
        device: &wgpu::Device,
        pipeline: Option<&OceanPipeline>,
        id: &str,
        mesh: &crate::water_body::WaterBodyMesh,
        look: crate::water_body::WaterBodyLook,
    ) -> Result<(), String> {
        let body = self.build_body(device, pipeline, id, mesh, look)?;
        self.bodies.insert(id.to_string(), body);
        if !self.ocean_active && self.spectrum.is_none() {
            self.spectrum = Some(flint_geometry::ocean::WaveSpectrum::generate(
                &flint_geometry::ocean::OceanParams::default(),
            ));
        }
        Ok(())
    }

    /// Change a body's look and keep its mesh. Returns whether it existed.
    pub(crate) fn set_body_look(
        &mut self,
        id: &str,
        look: crate::water_body::WaterBodyLook,
    ) -> bool {
        match self.bodies.get_mut(id) {
            Some(b) => {
                b.look = look;
                true
            }
            None => false,
        }
    }

    /// Remove the body set with `id`. Returns whether it existed.
    pub(crate) fn remove_body(&mut self, id: &str) -> bool {
        self.bodies.remove(id).is_some()
    }

    /// Remove every body a game set (the scene's own stay).
    pub(crate) fn clear_bodies(&mut self) {
        self.bodies.clear();
    }

    /// Extract the first `ocean_contact` entity: hull center/yaw/extents for
    /// the splash ring, plus hull velocity differentiated across frames
    /// (drift + heave both count as impact against the moving water).
    pub(crate) fn extract_contact(&mut self, world: &FlintWorld, time: f64) {
        use flint_core::toml_util::{toml_f32, toml_vec3};
        self.contact = None;
        for &entity_id in world.entities_with_component(flint_geometry::components::OCEAN_CONTACT) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };
            let Some(contact) = components.get(flint_geometry::components::OCEAN_CONTACT) else {
                continue;
            };
            let Some(tf) = components.get(comp::TRANSFORM) else {
                continue;
            };

            let pos = tf.get("position").and_then(toml_vec3).unwrap_or([0.0; 3]);
            let rot = tf.get("rotation").and_then(toml_vec3).unwrap_or([0.0; 3]);
            let yaw = rot[1].to_radians(); // engine Euler is degrees; Y = yaw
            let half_x = contact.get("half_x").and_then(toml_f32).unwrap_or(1.0);
            let half_z = contact.get("half_z").and_then(toml_f32).unwrap_or(1.0);

            self.contact = Some((
                [pos[0], pos[2], yaw.cos(), yaw.sin()],
                [half_x.max(0.01), half_z.max(0.01)],
            ));

            // Hull velocity on the same clock as the waves. Clamped against
            // teleports/scene loads, lightly smoothed against frame jitter.
            if let Some((prev_t, prev_pos)) = self.contact_prev {
                let dt = (time - prev_t) as f32;
                if dt > 1e-4 {
                    let k = (10.0 * dt).min(1.0);
                    for i in 0..3 {
                        let v = ((pos[i] - prev_pos[i]) / dt).clamp(-5.0, 5.0);
                        self.contact_vel[i] += (v - self.contact_vel[i]) * k;
                    }
                }
            }
            self.contact_prev = Some((time, pos));
            break; // one contact hull per scene
        }
        if self.contact.is_none() {
            self.contact_prev = None;
            self.contact_vel = [0.0; 3];
        }
    }

    /// Write this frame's ocean uniforms (wave phases in f64, grid follows
    /// the camera), the sea's body block and every body's. Water bodies
    /// read them too (ADR 0094), sea or no sea.
    pub(crate) fn upload(
        &self,
        queue: &wgpu::Queue,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
        frame: &WaterFrame<'_>,
    ) {
        if let (Some(spectrum), Some(ubuf), Some(tbuf)) =
            (&self.spectrum, &self.uniform_buffer, &self.transform_buffer)
        {
            let transform = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(tbuf, 0, bytemuck::cast_slice(&[transform]));

            let v = &self.visuals;
            // Snap the grid to inner-cell multiples so wave sampling
            // positions stay world-anchored while the mesh follows us.
            let q = v.snap_quantum();
            let grid_offset = [
                (camera_pos[0] / q).floor() * q,
                (camera_pos[2] / q).floor() * q,
            ];
            let uniforms = crate::ocean_pipeline::OceanUniformsGpu {
                waves: spectrum.to_gpu(frame.time),
                deep_color: v.deep_color,
                shallow_color: v.shallow_color,
                foam_color: v.foam_color,
                sss_color: v.sss_color,
                grid_offset,
                num_waves: spectrum.waves.len() as u32,
                enable_tonemapping: frame.tonemapping,
                grid_scale: v.grid_scale,
                fade_start: v.fade_start,
                fade_end: v.fade_end,
                foam_threshold: v.foam_threshold,
                foam_noise_scale: v.foam_noise_scale,
                ramp_steps: v.ramp_steps,
                specular_strength: v.specular_strength,
                time: (frame.time % 100_000.0) as f32,
                absorption_color: v.absorption_color,
                turbidity: v.turbidity,
                refraction_strength: v.refraction_strength,
                camera_near: self.camera_near_far.0,
                camera_far: self.camera_near_far.1,
                grab_enabled: u32::from(self.grab_this_frame),
                sky_zenith: frame.sky.zenith_color,
                sky_horizon: frame.sky.horizon_color,
                sky_haze: frame.sky.haze_color,
                // Reflections need a procedural sky to reflect; scenes
                // with a texture skybox (or none) keep the old look.
                sky_reflection_strength: if frame.sky_active {
                    v.sky_reflection_strength
                } else {
                    0.0
                },
                // Contact foam is off (strength 0) without a hull.
                splash_strength: if self.contact.is_some() {
                    v.splash_strength
                } else {
                    0.0
                },
                splash_width: v.splash_width,
                splash_flicker_speed: v.splash_flicker_speed,
                hull_a: self.contact.map(|(a, _)| a).unwrap_or([0.0, 0.0, 1.0, 0.0]),
                hull_b: {
                    let h = self.contact.map(|(_, h)| h).unwrap_or([1.0, 1.0]);
                    [h[0], h[1], v.splash_baseline, v.splash_noise_scale]
                },
                hull_c: {
                    let hv = self.contact_vel;
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
                mode: frame.render_mode,
                mode_mix: frame.mode_mix,
                // Shoreline foam reads the grab pass's depth: off
                // without it, whatever the component says.
                shore_foam_width: if self.grab_this_frame {
                    v.shore_foam_width
                } else {
                    0.0
                },
                shore_foam_strength: v.shore_foam_strength,
                shore_foam_color: v.shore_foam_color,
                shore_foam_speed: v.shore_foam_speed,
                shore_foam_reach: v.shore_foam_reach,
                _pad_shore: [0.0; 2],
            };
            queue.write_buffer(ubuf, 0, bytemuck::cast_slice(&[uniforms]));

            // Each body's look, its missing colours the sea's as they
            // stand this frame (so a time-of-day palette tints them too).
            if let Some(sea_buf) = &self.sea_body_buffer {
                let sea = crate::water_body::WaterBodyUniformsGpu::sea(v);
                queue.write_buffer(sea_buf, 0, bytemuck::cast_slice(&[sea]));
            }
            for b in self.bodies_to_draw() {
                let u = b.look.to_gpu(v, b.flowing);
                queue.write_buffer(&b.uniform_buffer, 0, bytemuck::cast_slice(&[u]));
            }
        }
    }

    /// Draw the sea and then the bodies (after all opaque: writes depth so
    /// fog applies; group 3 holds the grab-pass snapshots, or dummies when
    /// the grab didn't run).
    pub(crate) fn draw<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
        pipeline: Option<&'a OceanPipeline>,
        light_bind_group: &'a wgpu::BindGroup,
    ) {
        let grab_bg = if self.grab_this_frame {
            self.grab.bind_group.as_ref()
        } else {
            self.grab_dummy_bind_group.as_ref()
        };
        if let (Some(op), Some(ocean_bg), Some(transform_bg), Some(grab_bg)) = (
            pipeline,
            &self.uniform_bind_group,
            &self.transform_bind_group,
            grab_bg,
        ) {
            render_pass.set_bind_group(0, transform_bg, &[]);
            render_pass.set_bind_group(2, light_bind_group, &[]);
            render_pass.set_bind_group(3, grab_bg, &[]);
            if self.ocean_active {
                render_pass.set_pipeline(&op.pipeline);
                render_pass.set_bind_group(1, ocean_bg, &[]);
                render_pass.set_vertex_buffer(0, op.grid_vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(op.grid_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..op.grid_index_count, 0, 0..1);
            }
            // Lakes and rivers (ADR 0094): after the sea, depth-tested,
            // so one above sea level hides the sea beneath it.
            let mut bodies = self.bodies_to_draw().peekable();
            if bodies.peek().is_some() {
                render_pass.set_pipeline(&op.body_pipeline);
                for b in bodies {
                    render_pass.set_bind_group(1, &b.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, b.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(b.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    render_pass.draw_indexed(0..b.index_count, 0, 0..1);
                }
            }
        }
    }
}
