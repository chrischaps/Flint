//! Grass (ADR 0089, 0092), owned by [`GrassRenderer`] (ADR 0118, #736):
//! the compute pass that scatters each camera's blades into an instance
//! buffer and counts them into indirect draw arguments, the draw's own
//! uniforms, the ground it grows on (a terrain's splat map or a game's
//! ground and mask), and the count read back for stats a frame or so late.
//! The methods that build and drive it stay `SceneRenderer`'s (they read
//! its pipelines, layouts and shadow pass), here as an `impl` block with
//! their bodies as written. The host's clock is still `grass_time`, and
//! the woozy comb `grass_wooze`, until #739.

use super::*;
use crate::grass_pipeline::{
    check_grass_mask_patch, grass_cull_padding, grass_cull_planes, grass_dispatch_window,
    GrassComputeUniforms, GrassCountReadback, GrassDrawArgs, GrassRenderUniforms,
};

/// What the grass draws from: every field the renderer had as `grass_*`.
#[derive(Default)]
pub(crate) struct GrassRenderer {
    pub(crate) instance_buffer: Option<wgpu::Buffer>,
    /// Blades drawn, for stats: read back without waiting, so a frame or
    /// more behind (ADR 0092). The draws themselves read the GPU's count.
    pub(crate) instance_count: u32,
    /// The grass compute ran for this render_to, so the draws have arguments
    pub(crate) drawing: bool,
    pub(crate) max_instances: u32,
    pub(crate) draw_args_buffer: Option<wgpu::Buffer>,
    pub(crate) count_readback: Option<GrassCountReadback>,
    /// The frame being encoded dispatched the grass: read its count back
    /// once it is submitted (`frame_submitted`).
    pub(crate) readback_due: bool,
    /// The grass's view of each shadow cascade it casts into: a buffer and
    /// its bind group, kept across frames (#139).
    pub(crate) shadow_binds: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    pub(crate) compute_uniform_buffer: Option<wgpu::Buffer>,
    pub(crate) compute_uniform_bind_group: Option<wgpu::BindGroup>,
    pub(crate) compute_texture_bind_group: Option<wgpu::BindGroup>,
    pub(crate) compute_storage_bind_group: Option<wgpu::BindGroup>,
    /// Each placement workgroup's blade count, then its first slot, and how
    /// many workgroups it holds (#341). Grown by the dispatch that needs more.
    pub(crate) group_slots: Option<(wgpu::Buffer, u32)>,
    pub(crate) render_uniform_buffer: Option<wgpu::Buffer>,
    pub(crate) render_uniform_bind_group: Option<wgpu::BindGroup>,
    pub(crate) render_instance_bind_group: Option<wgpu::BindGroup>,
    pub(crate) entity_buffer: Option<wgpu::Buffer>,
    pub(crate) entity_count: u32,
    pub(crate) config: Option<flint_terrain::GrassConfig>,
    pub(crate) terrain_offset: [f32; 3],
    pub(crate) terrain_width: f32,
    pub(crate) terrain_depth: f32,
    pub(crate) terrain_height_scale: f32,
    /// Grass grows on a game's ground (ADR 0089), not a terrain's splat map.
    pub(crate) on_ground: bool,
    /// A mask weight thins the field in proportion (ground only).
    pub(crate) scale_by_density: bool,
    /// The ground's mask and its size in texels, kept so part of it can be
    /// repainted in place (ADR 0093). `None` for a terrain's grass.
    pub(crate) mask: Option<(wgpu::Texture, u32, u32)>,
    /// The config grew past the instance buffer; reallocate before the next dispatch.
    pub(crate) needs_realloc: bool,
    /// The grass draw's own view uniforms, so it needs no terrain to borrow them from.
    pub(crate) transform_buffer: Option<wgpu::Buffer>,
    pub(crate) transform_bind_group: Option<wgpu::BindGroup>,
}

impl SceneRenderer {
    /// Initialize grass GPU resources for the loaded terrain.
    /// Call after load_terrain() when grass is enabled.
    #[allow(clippy::too_many_arguments)]
    pub fn load_grass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        heightmap_data: &[f32],
        heightmap_width: u32,
        heightmap_depth: u32,
        splat_data: &[u8],
        splat_width: u32,
        splat_height: u32,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
    ) {
        if self.pipelines.grass.is_none() {
            return;
        }
        let (_, hm_view) = upload_grass_texture(
            device,
            queue,
            "Grass Heightmap Texture",
            wgpu::TextureFormat::R32Float,
            bytemuck::cast_slice(heightmap_data),
            4,
            heightmap_width,
            heightmap_depth,
        );
        let (_, splat_view) = upload_grass_texture(
            device,
            queue,
            "Grass Splat Texture",
            wgpu::TextureFormat::Rgba8Unorm,
            splat_data,
            4,
            splat_width,
            splat_height,
        );
        self.install_grass(
            device,
            config,
            &hm_view,
            &splat_view,
            terrain_offset,
            terrain_width,
            terrain_depth,
            height_scale,
            None,
        );
    }

    /// Grow grass on a game's own ground instead of a Flint terrain
    /// (ADR 0089): a density mask and a height over a world rectangle.
    ///
    /// Replaces whatever grass is loaded, a terrain's included, and can be
    /// called again whenever the ground changes. `config` is used as for a
    /// terrain, except that `density_layer` and `density_source` don't
    /// apply: the mask is the density. Does nothing, and returns `Ok`, when
    /// the device has no compute grass (see `has_grass_pipeline`).
    pub fn set_grass_ground(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        ground: &GrassGround,
    ) -> Result<(), String> {
        ground.validate()?;
        if self.pipelines.grass.is_none() {
            return Ok(());
        }
        let flat;
        let (heights, hm_w, hm_d) = match ground.height {
            GrassHeight::Flat(y) => {
                flat = [y];
                (&flat[..], 1, 1)
            }
            GrassHeight::Map {
                heights,
                width,
                depth,
            } => (heights, width, depth),
        };
        let (_, hm_view) = upload_grass_texture(
            device,
            queue,
            "Grass Ground Height Texture",
            wgpu::TextureFormat::R32Float,
            bytemuck::cast_slice(heights),
            4,
            hm_w,
            hm_d,
        );
        let (mask, mask_view) = upload_grass_texture(
            device,
            queue,
            "Grass Ground Mask Texture",
            wgpu::TextureFormat::R8Unorm,
            ground.mask,
            1,
            ground.mask_width,
            ground.mask_height,
        );
        // The heights are world y: offset 0, scale 1.
        self.install_grass(
            device,
            config,
            &hm_view,
            &mask_view,
            [ground.origin[0], 0.0, ground.origin[1]],
            ground.size[0],
            ground.size[1],
            1.0,
            Some(ground.scale_by_mask),
        );
        self.grass.mask = Some((mask, ground.mask_width, ground.mask_height));
        Ok(())
    }

    /// Repaint part of the grass ground's mask in place (ADR 0093): the
    /// `width × height` texels at (`x`, `y`), from `bytes` (one a texel,
    /// row by row toward +Z). The buffers and bind groups stay as they
    /// are, so a small clearing costs a small upload, not a reinstall.
    ///
    /// Errs when no ground was set with `set_grass_ground` (a terrain's
    /// grass has no mask to repaint), or when the rectangle runs off the
    /// mask or `bytes` is the wrong length. On a device with no compute
    /// grass it does nothing, as `set_grass_ground` does.
    pub fn update_grass_mask(
        &mut self,
        queue: &wgpu::Queue,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        if self.pipelines.grass.is_none() {
            return Ok(());
        }
        let Some((mask, mask_width, mask_height)) = &self.grass.mask else {
            return Err("no grass ground to repaint: set one with set_grass_ground".into());
        };
        check_grass_mask_patch(*mask_width, *mask_height, x, y, width, height, bytes.len())?;
        if width == 0 || height == 0 {
            return Ok(());
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: mask,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Whether this device runs the compute grass at all.
    pub fn has_grass_pipeline(&self) -> bool {
        self.pipelines.grass.is_some()
    }

    /// Build the grass buffers and bind groups over ground textures already
    /// uploaded. `ground` is `None` for a terrain (its splat layer is the
    /// density), or `Some(scale_by_mask)` for a game's R8 mask.
    #[allow(clippy::too_many_arguments)]
    fn install_grass(
        &mut self,
        device: &wgpu::Device,
        config: &flint_terrain::GrassConfig,
        hm_view: &wgpu::TextureView,
        splat_view: &wgpu::TextureView,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
        ground: Option<bool>,
    ) {
        let grass_pipeline = match &self.pipelines.grass {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.instance_capacity(terrain_width, terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Instance storage buffer (compute writes, render reads)
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Indirect draw arguments; the compute pass counts blades into them (ADR 0092)
        let counter_buffer = create_grass_draw_args_buffer(device);

        // Compute uniform buffer
        let compute_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Compute Uniform Buffer"),
            size: std::mem::size_of::<GrassComputeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: compute_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Compute Uniform Bind Group"),
        });

        // Heightmap uses nearest (non-filtering) sampler since R32Float is not filterable
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Nearest Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let compute_texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(hm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&nearest_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(splat_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
            ],
            label: Some("Grass Compute Texture Bind Group"),
        });

        // Render uniform buffer
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Render Uniform Buffer"),
            size: std::mem::size_of::<GrassRenderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_grass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: render_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Render Uniform Bind Group"),
        });

        // Entity positions buffer
        let entity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Entity Buffer"),
            size: (MAX_GRASS_ENTITIES * std::mem::size_of::<GrassEntityPosition>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: entity_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Store everything
        self.grass.instance_buffer = Some(instance_buffer);
        self.grass.instance_count = 0;
        self.grass.max_instances = max_instances;
        self.grass.draw_args_buffer = Some(counter_buffer);
        self.grass.count_readback = Some(GrassCountReadback::new(device));
        self.grass.compute_uniform_buffer = Some(compute_uniform_buffer);
        self.grass.compute_uniform_bind_group = Some(compute_uniform_bind_group);
        self.grass.compute_texture_bind_group = Some(compute_texture_bind_group);
        // Made by the first dispatch, which knows how many workgroups it runs
        self.grass.compute_storage_bind_group = None;
        self.grass.group_slots = None;
        self.grass.render_uniform_buffer = Some(render_uniform_buffer);
        self.grass.render_uniform_bind_group = Some(render_uniform_bind_group);
        self.grass.render_instance_bind_group = Some(render_instance_bind_group);
        self.grass.entity_buffer = Some(entity_buffer);
        self.grass.config = Some(config.clone());
        self.grass.terrain_offset = terrain_offset;
        self.grass.terrain_width = terrain_width;
        self.grass.terrain_depth = terrain_depth;
        self.grass.terrain_height_scale = height_scale;
        self.grass.on_ground = ground.is_some();
        self.grass.scale_by_density = ground.unwrap_or(false);
        self.grass.mask = None;
        self.grass.needs_realloc = false;
        if self.grass.transform_bind_group.is_none() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Transform Buffer"),
                size: std::mem::size_of::<TransformUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.layouts.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
                label: Some("Grass Transform Bind Group"),
            });
            self.grass.transform_buffer = Some(buffer);
            self.grass.transform_bind_group = Some(bind_group);
        }

        tracing::info!(
            "Grass loaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Clear all grass GPU resources.
    pub fn unload_grass(&mut self) {
        self.grass.instance_buffer = None;
        self.grass.instance_count = 0;
        self.grass.max_instances = 0;
        self.grass.draw_args_buffer = None;
        self.grass.count_readback = None;
        self.grass.compute_uniform_buffer = None;
        self.grass.compute_uniform_bind_group = None;
        self.grass.compute_texture_bind_group = None;
        self.grass.compute_storage_bind_group = None;
        self.grass.group_slots = None;
        self.grass.render_uniform_buffer = None;
        self.grass.render_uniform_bind_group = None;
        self.grass.render_instance_bind_group = None;
        self.grass.entity_buffer = None;
        self.grass.entity_count = 0;
        self.grass.config = None;
        self.grass.on_ground = false;
        self.grass.scale_by_density = false;
        self.grass.mask = None;
        self.grass.needs_realloc = false;
    }

    /// The blades the grass's last compute pass kept, read back from the
    /// GPU (ADR 0098). It waits for the GPU, so it's for tests and tools,
    /// never for a frame. Empty with no grass loaded.
    pub fn read_grass_blades(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<GrassInstanceGpu> {
        let (Some(instances), Some(args)) =
            (&self.grass.instance_buffer, &self.grass.draw_args_buffer)
        else {
            return Vec::new();
        };
        let read = |buffer: &wgpu::Buffer, offset: u64, size: u64| -> Vec<u8> {
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Blade Read-back"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(buffer, offset, &staging, 0, size);
            queue.submit([encoder.finish()]);
            staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let bytes = staging
                .slice(..)
                .get_mapped_range()
                .expect("mapped buffer range")
                .to_vec();
            staging.unmap();
            bytes
        };
        let count = read(args, GrassDrawArgs::INSTANCE_COUNT_OFFSET, 4);
        let count = u32::from_le_bytes([count[0], count[1], count[2], count[3]])
            .min(self.grass.max_instances);
        if count == 0 {
            return Vec::new();
        }
        let stride = std::mem::size_of::<GrassInstanceGpu>();
        read(instances, 0, count as u64 * stride as u64)
            .chunks_exact(stride)
            .map(bytemuck::pod_read_unaligned)
            .collect()
    }

    /// Update grass config without buffer reallocation.
    /// Compute and render passes read `grass_config` fresh each frame to build
    /// uniforms, so changes take effect on the next `render()` call.
    /// If the new config needs more instances than the buffer holds (a
    /// higher density or a longer `max_distance`), the buffer is
    /// reallocated before the next dispatch.
    pub fn set_grass_config(&mut self, config: flint_terrain::GrassConfig) {
        let capacity = config.instance_capacity(self.grass.terrain_width, self.grass.terrain_depth);
        if self.grass.instance_buffer.is_some() && capacity > self.grass.max_instances {
            self.grass.needs_realloc = true;
        }
        self.grass.config = Some(config);
    }

    /// Reallocate the grass instance buffer for a new density value,
    /// reusing existing heightmap/splat GPU textures.
    /// Call this when `GrassConfig.density` changes (affects buffer capacity).
    pub fn reload_grass_config(
        &mut self,
        device: &wgpu::Device,
        config: flint_terrain::GrassConfig,
    ) {
        let grass_pipeline = match &self.pipelines.grass {
            Some(p) => p,
            None => return,
        };

        let max_instances =
            config.instance_capacity(self.grass.terrain_width, self.grass.terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Reallocate instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Indirect draw arguments; the compute pass counts blades into them (ADR 0092)
        let counter_buffer = create_grass_draw_args_buffer(device);

        // Recreate render instance bind group (binds instance buffer at binding 0)
        let entity_buffer = self
            .grass
            .entity_buffer
            .as_ref()
            .expect("grass entity buffer");
        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: entity_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Update stored state
        // Note: grass_compute_texture_bind_group, compute_uniform_bind_group,
        // render_uniform_bind_group, and entity_buffer are intentionally NOT
        // recreated — they don't reference the instance buffer.
        self.grass.instance_buffer = Some(instance_buffer);
        self.grass.instance_count = 0;
        self.grass.max_instances = max_instances;
        self.grass.draw_args_buffer = Some(counter_buffer);
        self.grass.count_readback = Some(GrassCountReadback::new(device));
        // The next dispatch binds the new buffers
        self.grass.compute_storage_bind_group = None;
        self.grass.render_instance_bind_group = Some(render_instance_bind_group);
        self.grass.config = Some(config);
        self.grass.needs_realloc = false;

        tracing::info!(
            "Grass reloaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Make sure the grass compute's storage bind group exists and its
    /// per-workgroup slots hold `groups` workgroups (#341). The slots grow
    /// to the next power of two, so a window that changes size as the
    /// camera nears the ground's edge rebuilds them a few times at most.
    fn ensure_grass_group_slots(&mut self, device: &wgpu::Device, groups: u32) {
        let groups = groups.max(1);
        let fits = self
            .grass
            .group_slots
            .as_ref()
            .is_some_and(|(_, capacity)| *capacity >= groups);
        if fits && self.grass.compute_storage_bind_group.is_some() {
            return;
        }
        let (Some(grass_pipeline), Some(instances), Some(draw_args)) = (
            &self.pipelines.grass,
            &self.grass.instance_buffer,
            &self.grass.draw_args_buffer,
        ) else {
            return;
        };
        if !fits {
            let capacity = groups.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Grass Group Slots"),
                size: capacity as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            self.grass.group_slots = Some((buffer, capacity));
        }
        let (slots, _) = self.grass.group_slots.as_ref().expect("grass group slots");
        self.grass.compute_storage_bind_group =
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.compute_storage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instances.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: draw_args.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: slots.as_entire_binding(),
                    },
                ],
                label: Some("Grass Compute Storage Bind Group"),
            }));
    }

    /// Read-only access to the current grass config (if loaded).
    pub fn grass_config(&self) -> Option<&flint_terrain::GrassConfig> {
        self.grass.config.as_ref()
    }

    /// Update entity positions for grass bend-on-contact.
    /// Also updates entity_count in the render uniform buffer.
    pub fn update_grass_entities(
        &mut self,
        queue: &wgpu::Queue,
        positions: &[GrassEntityPosition],
    ) {
        let count = positions.len().min(MAX_GRASS_ENTITIES);
        self.grass.entity_count = count as u32;
        if let Some(buf) = &self.grass.entity_buffer {
            if count > 0 {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&positions[..count]));
            }
        }
        // Note: entity_count is written to the render uniform buffer in dispatch_grass_compute,
        // which uses self.grass.entity_count to preserve the value set here.
    }

    /// Dispatch the grass compute shader to scatter this camera's blades,
    /// culled to its view, and count them into the indirect draw arguments
    /// (ADR 0092). Call before the shadow and main passes, and call
    /// [`GrassCountReadback::start_map`] after submitting `encoder`.
    ///
    /// [`GrassCountReadback::start_map`]: crate::grass_pipeline::GrassCountReadback::start_map
    pub fn dispatch_grass_compute(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        time: f32,
    ) {
        let config = match &self.grass.config {
            Some(c) if c.enabled => c.clone(),
            _ => return,
        };

        // The last count that has come back, for stats
        if let Some(count) = self
            .grass
            .count_readback
            .as_mut()
            .and_then(|readback| readback.poll(device))
        {
            self.grass.instance_count = count.min(self.grass.max_instances);
        }

        // Only the cells within reach of the camera are dispatched (ADR 0089)
        let spacing = 1.0 / config.density.sqrt();
        let window = grass_dispatch_window(
            [self.grass.terrain_offset[0], self.grass.terrain_offset[2]],
            [self.grass.terrain_width, self.grass.terrain_depth],
            spacing,
            [camera.position.x, camera.position.z],
            config.max_distance,
        );
        let ([start_x, start_z], [count_x, count_z]) = window.unwrap_or(([0, 0], [0, 0]));
        let workgroups_x = count_x.div_ceil(8);
        let workgroups_z = count_z.div_ceil(8);
        let group_count = workgroups_x * workgroups_z;
        self.ensure_grass_group_slots(device, group_count);

        let grass_pipeline = match &self.pipelines.grass {
            Some(p) => p,
            None => return,
        };

        let compute_uniform_bg = match &self.grass.compute_uniform_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_texture_bg = match &self.grass.compute_texture_bind_group {
            Some(bg) => bg,
            None => return,
        };
        let compute_storage_bg = match &self.grass.compute_storage_bind_group {
            Some(bg) => bg,
            None => return,
        };

        // Reset the draw arguments to no blades; the scan pass counts them in
        let Some(draw_args) = &self.grass.draw_args_buffer else {
            return;
        };
        queue.write_buffer(draw_args, 0, bytemuck::bytes_of(&GrassDrawArgs::RESET));

        // Only the blades in view are drawn. The planes are pushed out by the
        // tallest blade, and with shadows on by how far its shadow reaches,
        // so nothing that leans or casts into the view is lost at its edges.
        let sun = self
            .shadows_drawn()
            .then(|| self.lights.uniforms.directional_lights[0].direction);
        let tallest = config.blade_height * (1.0 + config.height_variation.max(0.0));
        let frustum_planes = grass_cull_planes(
            &camera.view_projection_matrix(),
            grass_cull_padding(tallest, sun),
        );

        // Update compute uniforms
        let uniforms = GrassComputeUniforms {
            camera_pos: [camera.position.x, camera.position.y, camera.position.z],
            time,
            terrain_offset: self.grass.terrain_offset,
            density: config.density,
            terrain_width: self.grass.terrain_width,
            terrain_depth: self.grass.terrain_depth,
            height_scale: self.grass.terrain_height_scale,
            max_distance: config.max_distance,
            fade_start: config.fade_start,
            density_threshold: config.density_threshold,
            // A ground's mask is a single channel
            density_layer: if self.grass.on_ground {
                0
            } else {
                config.density_layer
            },
            blade_height: config.blade_height,
            height_variation: config.height_variation,
            scale_by_density: self.grass.scale_by_density as u32,
            grid_start_x: start_x,
            grid_start_z: start_z,
            // A terrain's samples sit on its mesh's vertices; a game's
            // ground map's in its cells (ADR 0098)
            height_at_corners: !self.grass.on_ground as u32,
            group_count,
            _pad1: 0,
            _pad2: 0,
            frustum_planes,
        };

        if let Some(buf) = &self.grass.compute_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update render uniforms (wind, colors, etc.)
        let wind_dir = {
            let d = config.wind_direction;
            let len = (d[0] * d[0] + d[2] * d[2]).sqrt().max(0.001);
            [d[0] / len, d[1], d[2] / len]
        };

        // A comb with no swing is none at all, and the shader skips it.
        let wooze = self
            .grass_wooze
            .filter(|draw| draw.amplitude > 0.0)
            .map(|draw| draw.uniforms())
            .unwrap_or_else(bytemuck::Zeroable::zeroed);
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
            entity_count: self.grass.entity_count,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
            wooze_leaving: wooze.leaving,
            wooze_arriving: wooze.arriving,
            wooze_params: wooze.params,
        };

        if let Some(buf) = &self.grass.render_uniform_buffer {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&[render_uniforms]));
        }

        // Compute dispatch over the window (none when the camera is out of reach;
        // the counter, reset above, then reads zero). Three passes put the
        // blades in grid order, the same every run (#341): an atomic append
        // ordered them by which GPU thread got there first, and the blade
        // drawn first wins a depth tie, so captures moved by a level.
        if group_count > 0 {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Grass Compute Pass"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, compute_uniform_bg, &[]);
            pass.set_bind_group(1, compute_texture_bg, &[]);
            pass.set_bind_group(2, compute_storage_bg, &[]);
            pass.set_pipeline(&grass_pipeline.count_pipeline);
            pass.dispatch_workgroups(workgroups_x, workgroups_z, 1);
            pass.set_pipeline(&grass_pipeline.scan_pipeline);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&grass_pipeline.emit_pipeline);
            pass.dispatch_workgroups(workgroups_x, workgroups_z, 1);
        }

        // Copy the count out for stats, unless the last copy is still coming back
        if let Some(readback) = &mut self.grass.count_readback {
            readback.record_copy(encoder, draw_args);
        }
        self.grass.drawing = true;
    }
}

/// The grass draw's indirect arguments (ADR 0092): the compute pass writes
/// the instance count, and the draws read the buffer as `INDIRECT`.
fn create_grass_draw_args_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Grass Draw Args"),
        contents: bytemuck::bytes_of(&GrassDrawArgs::RESET),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
    })
}

/// Upload one grass ground texture (heightmap, splat map or mask) and
/// return it with its view. `bytes_per_texel` is 4 for R32Float and
/// Rgba8Unorm, 1 for R8Unorm.
#[allow(clippy::too_many_arguments)]
fn upload_grass_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    data: &[u8],
    bytes_per_texel: u32,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes_per_texel),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}
