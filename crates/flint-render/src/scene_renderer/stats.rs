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

        let entity_draws = self.entity_draws.len() as u32;
        let skinned_draws = self.skinned_entity_draws.len() as u32;
        let terrain_draws = self.terrain.visible_chunks;
        let echo_draws = self.echo_draws.len() as u32;
        let transparent_draws = (self.transparent_draws.len()
            + self.transparent_skinned_draws.len()
            + self.echo_draws.len()) as u32;
        let billboard_draws = self.billboard_draws.len() as u32;
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

        // Triangles: sum index_count/3 for types that have it, fixed counts for others
        let mut triangles: u32 = 0;
        for d in &self.entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.skinned_entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_skinned_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.echo_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.terrain.draws {
            triangles += d.index_count / 3;
        }
        // Billboards: each is a fixed quad (2 triangles)
        triangles += billboard_draws * 2;
        // Particles: instanced quads (2 triangles per instance)
        let particle_instances: u32 = self.particle_draws.iter().map(|d| d.instance_count).sum();
        triangles += particle_instances * 2;
        // Sprites: instanced quads (2 triangles per instance)
        let sprite_instances: u32 = self.sprite2d_batches.iter().map(|b| b.instance_count).sum();
        triangles += sprite_instances * 2;
        // Grass: BLADE_INDEX_COUNT indices per instance
        triangles += self.grass.instance_count * BLADE_INDEX_COUNT / 3;

        // Shadow stats: estimate as main pass × the cascades drawn, counting
        // only the draws that cast (ADR 0100)
        let cascade_count = match &self.shadow_pass {
            Some(sp) if self.shadows_drawn() => sp.cascades() as u32,
            _ => 0,
        };
        let casting = self.entity_draws.iter().filter(|d| d.casts_shadow);
        let casting_skinned =
            Self::skinned_shadow_draws(&self.skinned_entity_draws, &self.transparent_skinned_draws);
        let shadow_entity_draws =
            (casting.clone().count() + casting_skinned.clone().count()) as u32 + terrain_draws;
        let shadow_draw_calls = shadow_entity_draws * cascade_count;
        let shadow_triangles = {
            let mut t: u32 = 0;
            for d in casting {
                t += d.index_count / 3;
            }
            for d in casting_skinned {
                t += d.index_count / 3;
            }
            for d in &self.terrain.draws {
                t += d.index_count / 3;
            }
            t * cascade_count
        };

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
            culled_draws: (self.culled_entity_draws.len() + self.culled_transparent_draws.len())
                as u32,
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

    pub(super) fn default_archetype_visuals() -> HashMap<String, ArchetypeVisual> {
        let mut archetype_visuals = HashMap::new();

        archetype_visuals.insert(
            "room".to_string(),
            ArchetypeVisual {
                color: [0.27, 0.53, 1.0, 0.5],
                wireframe: true,
                default_size: [10.0, 4.0, 10.0],
            },
        );

        archetype_visuals.insert(
            "door".to_string(),
            ArchetypeVisual {
                color: [1.0, 0.53, 0.27, 1.0],
                wireframe: false,
                default_size: [1.0, 2.0, 0.1],
            },
        );

        archetype_visuals.insert(
            "furniture".to_string(),
            ArchetypeVisual {
                color: [0.27, 1.0, 0.53, 1.0],
                wireframe: false,
                default_size: [1.0, 1.0, 1.0],
            },
        );

        archetype_visuals.insert(
            "character".to_string(),
            ArchetypeVisual {
                color: [1.0, 1.0, 0.27, 1.0],
                wireframe: false,
                default_size: [0.5, 1.8, 0.5],
            },
        );

        archetype_visuals.insert(
            "wall".to_string(),
            ArchetypeVisual {
                color: [0.76, 0.70, 0.60, 1.0],
                wireframe: false,
                default_size: [10.0, 4.0, 0.3],
            },
        );

        archetype_visuals.insert(
            "floor".to_string(),
            ArchetypeVisual {
                color: [0.55, 0.55, 0.52, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "ceiling".to_string(),
            ArchetypeVisual {
                color: [0.65, 0.62, 0.58, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "pillar".to_string(),
            ArchetypeVisual {
                color: [0.70, 0.65, 0.55, 1.0],
                wireframe: false,
                default_size: [0.5, 4.0, 0.5],
            },
        );

        archetype_visuals
    }

    /// Set visual representation for an archetype
    pub fn set_archetype_visual(&mut self, archetype: &str, visual: ArchetypeVisual) {
        self.archetype_visuals.insert(archetype.to_string(), visual);
    }
}
