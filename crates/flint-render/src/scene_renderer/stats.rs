//! Stats and debug views (ADR 0118, #736): `collect_stats`, the GPU timer,
//! the debug mode and overlays (normals, skeleton, grid), tonemapping, and
//! the archetype visuals a `bounds` box is coloured by.

use super::*;

impl SceneRenderer {
    /// Read-only access to the current debug state
    pub fn debug_state(&self) -> &DebugState {
        &self.debug_state
    }

    /// Mutable access to the debug state
    pub fn debug_state_mut(&mut self) -> &mut DebugState {
        &mut self.debug_state
    }

    /// Set the shading debug mode
    pub fn set_debug_mode(&mut self, mode: DebugMode) {
        self.debug_state.mode = mode;
    }

    /// Collect rendering statistics for the current frame.
    ///
    /// Timing (`fps`, `frame_time_ms`) and `resolution` are left at zero —
    /// the caller fills these from their own timing and context.
    /// `scene_resolution` is the renderer's own.
    pub fn collect_stats(&self) -> crate::render_stats::RenderStats {
        use crate::grass_pipeline::BLADE_INDEX_COUNT;

        // One walk over the list: what each kind counts as
        let (mut entity_draws, mut skinned_draws, mut echo_draws) = (0u32, 0u32, 0u32);
        let (mut blended_draws, mut billboard_draws, mut culled_draws) = (0u32, 0u32, 0u32);
        let mut triangles: u32 = 0;
        // What casts into the shadow cascades (ADR 0100), as an estimate:
        // the opaque statics and the skinned meshes the shadow pass takes
        let (mut shadow_casters, mut shadow_caster_triangles) = (0u32, 0u32);
        for item in self.draws.iter() {
            let opaque = item.blend == Blend::Opaque;
            match &item.draw {
                Draw::Static(_) if !item.drawn() => culled_draws += 1,
                Draw::Static(d) => {
                    triangles += d.index_count / 3;
                    if opaque {
                        entity_draws += 1;
                        if item.casts_shadow {
                            shadow_casters += 1;
                            shadow_caster_triangles += d.index_count / 3;
                        }
                    } else {
                        blended_draws += 1;
                    }
                }
                Draw::Skinned(d) => {
                    triangles += d.index_count / 3;
                    if opaque {
                        skinned_draws += 1;
                    } else {
                        blended_draws += 1;
                    }
                    if item.casts_shadow && (opaque || d.soft_edge_only) {
                        shadow_casters += 1;
                        shadow_caster_triangles += d.index_count / 3;
                    }
                }
                Draw::Echo(d) => {
                    triangles += d.index_count / 3;
                    echo_draws += 1;
                    blended_draws += 1;
                }
                // Each a fixed quad (2 triangles)
                Draw::Billboard(_) => {
                    billboard_draws += 1;
                    triangles += 2;
                }
                // Every chunk, and the visible ones as draw calls below
                Draw::Terrain(n) => {
                    let chunk = self.terrain.draws.get(*n as usize);
                    let t = chunk.map_or(0, |d| d.index_count / 3);
                    triangles += t;
                    shadow_caster_triangles += t;
                }
                // Counted from the upload below
                Draw::Particles(_) => {}
            }
        }
        let terrain_draws = self.terrain.visible_chunks;
        let transparent_draws = blended_draws;
        let particle_draws = self.particle_draws.len() as u32;
        let sprite_batches = self.sprite2d_batches.len() as u32;
        let grass_draw_calls = if self.grass.drawing { 1u32 } else { 0 };

        let draw_calls = entity_draws
            + skinned_draws
            + terrain_draws
            + transparent_draws
            + billboard_draws
            + particle_draws
            + sprite_batches
            + grass_draw_calls;

        // Particles: instanced quads (2 triangles per instance)
        let particle_instances: u32 = self.particle_draws.iter().map(|d| d.instance_count).sum();
        triangles += particle_instances * 2;
        // Sprites: instanced quads (2 triangles per instance)
        let sprite_instances: u32 = self.sprite2d_batches.iter().map(|b| b.instance_count).sum();
        triangles += sprite_instances * 2;
        // Grass: BLADE_INDEX_COUNT indices per instance
        triangles += self.grass.instance_count * BLADE_INDEX_COUNT / 3;

        // Shadow stats: estimate as main pass × the layers drawn (the
        // cascades, and a spot's, #504), counting only the draws that cast
        // (ADR 0100)
        let cascade_count = self.shadow_layers_drawn();
        let shadow_entity_draws = shadow_casters + terrain_draws;
        let shadow_draw_calls = shadow_entity_draws * cascade_count;
        let shadow_triangles = shadow_caster_triangles * cascade_count;

        let style_memory = self.style_target_summary();
        let fxaa_target = self
            .postprocess_resources
            .as_ref()
            .is_some_and(|r| r.fxaa.is_some()) as u32;
        crate::render_stats::RenderStats {
            draw_calls,
            triangles,
            echo_draws,
            entity_draws,
            culled_draws,
            lod_culled_draws: self.lod_culled,
            skinned_draws,
            terrain_draws,
            terrain_total_chunks: self.terrain.total_chunks,
            transparent_draws,
            billboard_draws,
            particle_draws,
            particle_instances,
            sprite_batches,
            grass_instances: self.grass.instance_count,
            grass_draw_calls,
            shadow_draw_calls,
            shadow_triangles,
            gpu_passes: self
                .gpu_timer
                .as_ref()
                .map(|t| t.timings().passes.clone())
                .unwrap_or_default(),
            gpu_frame_ms: self
                .gpu_timer
                .as_ref()
                .map_or(0.0, |t| t.timings().frame_ms),
            scene_resolution: self.scene_size().into(),
            style_targets: style_memory.count,
            style_target_bytes: style_memory.bytes,
            full_size_targets: style_memory.full_size + fxaa_target,
            ..Default::default()
        }
    }

    /// Time the frame's phases on the GPU, or stop (ADR 0108): the grass
    /// compute, the shadows, the scene, the post-process chain, its last
    /// pass (the composite, or FXAA when it's on) and the world lines, each
    /// a scope, into [`RenderStats::gpu_passes`]. Off by default. The
    /// device must have been created with `Features::TIMESTAMP_QUERY`
    /// (Flint's contexts ask for it where the adapter has it); without it
    /// timing stays off. Returns whether timing is on.
    ///
    /// The host ends each frame with
    /// [`finish_gpu_timings`](Self::finish_gpu_timings), after its last
    /// timed submission.
    ///
    /// [`RenderStats::gpu_passes`]: crate::RenderStats::gpu_passes
    pub fn set_gpu_timing(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, on: bool) -> bool {
        if !on {
            self.gpu_timer = None;
        } else if self.gpu_timer.is_none() {
            self.gpu_timer = crate::gpu_timer::GpuTimer::new(device, queue);
        }
        self.gpu_timer.is_some()
    }

    /// Whether the frame's phases are being timed.
    pub fn gpu_timing(&self) -> bool {
        self.gpu_timer.is_some()
    }

    /// The newest frame's GPU busy time read back since the last call, ms,
    /// unsmoothed ([`GpuTimer::take_latest_frame_ms`]); `None` while timing
    /// is off or when no read-back came in.
    ///
    /// [`GpuTimer::take_latest_frame_ms`]: crate::GpuTimer::take_latest_frame_ms
    pub fn take_gpu_frame_ms(&mut self) -> Option<f32> {
        self.gpu_timer.as_mut()?.take_latest_frame_ms()
    }

    /// The timer, while timing is on, for a host to time its own work (its
    /// UI, say) in the same frame: [`GpuTimer::begin`] and
    /// [`GpuTimer::end`] round it.
    ///
    /// [`GpuTimer::begin`]: crate::GpuTimer::begin
    /// [`GpuTimer::end`]: crate::GpuTimer::end
    pub fn gpu_timer_mut(&mut self) -> Option<&mut crate::gpu_timer::GpuTimer> {
        self.gpu_timer.as_mut()
    }

    /// End the frame's GPU timing: pick up the times read back so far and
    /// send this frame's back. Call once a frame after the last timed
    /// submission, before presenting. Does nothing while timing is off.
    pub fn finish_gpu_timings(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(timer) = &mut self.gpu_timer {
            timer.finish_frame(device, queue);
        }
    }

    /// [`finish_gpu_timings`](Self::finish_gpu_timings) in the frame's own
    /// encoder, after its last timed scope: the resolve rides the frame's
    /// one submission, and [`frame_submitted`](Self::frame_submitted)
    /// starts the read-back (#139).
    pub fn encode_gpu_timings(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(timer) = &mut self.gpu_timer {
            timer.resolve_into(device, encoder);
        }
    }

    /// Toggle normal direction arrows on/off, returns the new state
    pub fn toggle_normal_arrows(&mut self) -> bool {
        self.debug_state.show_normals = !self.debug_state.show_normals;
        self.debug_state.show_normals
    }

    /// Toggle skeleton overlay on/off, returns the new state
    pub fn toggle_skeleton_overlay(&mut self) -> bool {
        self.debug_state.show_skeleton = !self.debug_state.show_skeleton;
        self.debug_state.show_skeleton
    }

    /// Set the skeleton overlay mesh (line-list geometry for bone visualization)
    pub fn set_skeleton_overlay(&mut self, device: &wgpu::Device, mesh: &crate::primitives::Mesh) {
        self.skeleton_overlay_draws.clear();
        if mesh.indices.is_empty() {
            return;
        }
        let tex_cache = self.texture_cache.as_ref().expect("texture cache");
        let draw = Self::create_draw_call(
            device,
            &self.layouts,
            mesh,
            true,
            TransformUniforms::new(),
            MaterialUniforms::procedural(),
            tex_cache,
        );
        self.skeleton_overlay_draws.push(draw);
    }

    /// Clear skeleton overlay draw calls
    pub fn clear_skeleton_overlay(&mut self) {
        self.skeleton_overlay_draws.clear();
    }

    /// Show or hide the ground-plane grid at runtime
    pub fn set_show_grid(&mut self, device: &wgpu::Device, show: bool) {
        if show && self.grid_draw.is_none() {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            let tc = self
                .texture_cache
                .as_ref()
                .expect("texture cache must exist");
            self.grid_draw = Some(Self::create_draw_call(
                device,
                &self.layouts,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                tc,
            ));
        } else if !show {
            self.grid_draw = None;
        }
    }

    /// Whether the ground-plane grid is currently visible
    pub fn show_grid(&self) -> bool {
        self.grid_draw.is_some()
    }

    /// Enable or disable tone mapping
    pub fn set_tonemapping(&mut self, enabled: bool) {
        self.tonemapping_enabled = enabled;
    }
}
