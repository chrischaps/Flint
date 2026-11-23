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

    /// Skip or repeat parts of every frame from now on, by label (#1317,
    /// [`crate::pass_knobs`]): what `--skip-pass` / `--repeat-pass` and the
    /// web's `?skip_pass=` / `?repeat_pass=` hand over. Empty knobs, or
    /// `None`, draw every part once, as a renderer that was never told.
    pub fn set_pass_knobs(&mut self, knobs: Option<crate::pass_knobs::PassKnobs>) {
        self.pass_knobs = knobs
            .filter(|k| !k.is_empty())
            .map(|k| Box::new(crate::pass_knobs::KnobRun::new(k)));
    }

    /// The knobs set, if any.
    pub fn pass_knobs(&self) -> Option<&crate::pass_knobs::PassKnobs> {
        self.pass_knobs.as_deref().map(|run| &run.knobs)
    }

    /// How many times a host's own part named `label` (the player's `UI`)
    /// should record its work this frame: 0 skipped, else 1, or the
    /// repeat asked unless `refuse` says why it can't be (#1317).
    pub fn pass_times(&mut self, label: &'static str, refuse: Option<&str>) -> u32 {
        match self.pass_knobs.as_deref_mut() {
            None => 1,
            Some(run) => run.times(label, refuse),
        }
    }

    /// Collect rendering statistics for the current frame.
    ///
    /// Timing (`fps`, `frame_time_ms`) and `resolution` are left at zero —
    /// the caller fills these from their own timing and context.
    /// `scene_resolution` is the renderer's own.
    /// Whether the last frame drew any of `entities` (#1191, a playtest's
    /// `frame draws`, which asks of an entity and its descendants: a
    /// model's nodes): `None` when the draw list holds nothing of them,
    /// `Some(false)` when every draw they have was culled (frustum, level
    /// of detail, draw distance, hidden). Read after a frame is encoded,
    /// before the next `prepare` rebuilds the list. A draw with no bounds
    /// (a procedural box, a wireframe) is never culled (ADR 0074).
    pub fn entities_drawn(&self, entities: &[flint_core::EntityId]) -> Option<bool> {
        let mut found = None;
        let ours = |i: &&DrawItem| i.entity.is_some_and(|e| entities.contains(&e));
        for item in self.draws.iter().filter(ours) {
            if item.drawn() {
                return Some(true);
            }
            found = Some(false);
        }
        found
    }

    /// Whether style `name` reached the last frame (#1191, a playtest's
    /// `frame style`): post-processing on, the style on over the frame or
    /// for an entity asking for it, and a blend and reveal that leave some
    /// of it in the picture. `None` for a name no registered style has.
    pub fn style_drawn(&self, name: &str) -> Option<bool> {
        let style = self.styles().find(name)?;
        let config = self.effective_post_process_config();
        Some(
            config.enabled
                && style.is_on(config, &self.style_mask.ids)
                && config.styles.shows_in(name, config),
        )
    }

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

        // Shadow stats: estimate as main pass × the views drawn (the
        // cascades, a spot's, #504, and a point light's six faces, #1112),
        // counting only the draws that cast (ADR 0100)
        let cascade_count = self.shadow_views_drawn();
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
            gpu_timestamps: self.gpu_timestamps(),
            scene_resolution: self.scene_size().into(),
            style_targets: style_memory.count,
            style_target_bytes: style_memory.bytes,
            full_size_targets: style_memory.full_size + fxaa_target,
            gpu_calls: self.frame_gpu_calls,
            gpu_memory: self.gpu_memory(),
            ..Default::default()
        }
    }

    /// What the last frame created and wrote on the GPU (#1091): the
    /// process's counted calls between the last two
    /// [`frame_submitted`](Self::frame_submitted)s.
    pub fn frame_gpu_calls(&self) -> crate::gpu_counts::GpuCalls {
        self.frame_gpu_calls
    }

    /// The process's running totals as of the last
    /// [`frame_submitted`](Self::frame_submitted), the frames this renderer
    /// submitted, and how many of them created any GPU object (#1091): a
    /// readout's window is the difference of two.
    pub fn gpu_calls_reading(&self) -> (crate::gpu_counts::GpuCalls, u64, u64) {
        (self.gpu_calls_at, self.gpu_frames.0, self.gpu_frames.1)
    }

    /// The texture memory this renderer knows it holds, by class, and its
    /// mesh buffers (#1091). A walk of its own fields, no GPU call and no
    /// allocation: cheap enough to ask every frame F2 shows. What it
    /// doesn't know (uniform buffers, a style's own small textures, egui's)
    /// is in [`gpu_counts::device_memory`](crate::gpu_counts::device_memory).
    pub fn gpu_memory(&self) -> crate::gpu_counts::GpuMemory {
        use crate::gpu_counts::bytes_of;
        let post = self.postprocess_resources.as_ref();
        let scene_targets = post.map_or(0, |r| r.scene_target_bytes())
            + self.msaa.bytes()
            + self
                .scaled_depth
                .as_ref()
                .map_or(0, |d| bytes_of(&d.texture))
            + self.water.grab_bytes();
        let mesh_buffers = self.mesh_cache.buffer_bytes()
            + self
                .box_mesh_cache
                .values()
                .map(|b| b.vertex_buffer.size() + b.index_buffer.size())
                .sum::<u64>();
        crate::gpu_counts::GpuMemory {
            scene_targets,
            style_targets: self.style_target_summary().bytes,
            shadow_maps: self
                .shadow_pass
                .as_ref()
                .map_or(0, |s| bytes_of(&s.shadow_texture)),
            model_textures: self.texture_cache.as_ref().map_or(0, |t| t.bytes()),
            glyph_atlases: self
                .postprocess_pipeline
                .as_ref()
                .map_or(0, |p| p.styles.glyph_atlas_bytes()),
            mesh_buffers,
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

    /// Whether the timer's timestamps read as pass times (#1316);
    /// `Unchecked` while timing is off.
    pub fn gpu_timestamps(&self) -> crate::gpu_timer::TimestampHealth {
        self.gpu_timer
            .as_ref()
            .map(|t| t.timestamps())
            .unwrap_or_default()
    }

    /// Each frame's GPU busy time read back since the last call, ms,
    /// unsmoothed, with the index it was resolved under
    /// ([`GpuTimer::take_frame_times`]); empty while timing is off or when
    /// no read-back came in.
    ///
    /// [`GpuTimer::take_frame_times`]: crate::GpuTimer::take_frame_times
    pub fn take_gpu_frame_times(&mut self) -> impl Iterator<Item = crate::GpuFrameTime> + '_ {
        self.gpu_timer
            .as_mut()
            .into_iter()
            .flat_map(|t| t.take_frame_times())
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

    /// The scene pass's draws one frame times at most
    /// ([`DrawControls::time_draws`](super::DrawControls::time_draws), #1203).
    pub const DRAW_TIME_BATCH: usize = super::draw_times::BATCH;

    /// The device features per-draw times need: timestamps, and timestamps
    /// inside a pass.
    pub const DRAW_TIME_FEATURES: wgpu::Features = super::draw_times::FEATURES;

    /// The draws a pixel at which the overdraw heat map reaches its last
    /// colour (#1203); each colour before it is half the draws.
    pub const OVERDRAW_HEAT_TOP: u32 = super::overdraw::HEAT_TOP;

    /// Read the draw times the last submitted frame took (#1203), waiting
    /// for its GPU, into the times gathered since the last
    /// [`reset_draw_times`](Self::reset_draw_times), which the next
    /// described frame lays on its scene draws. `None` when that frame timed
    /// nothing; it says how many draws the scene pass listed, so a host can
    /// time the rest a batch a frame.
    pub fn read_draw_times(&mut self, device: &wgpu::Device) -> Option<crate::DrawTimesRead> {
        self.draw_times.read(device)
    }

    /// Why the last frame's asked-for draw times timed nothing (a device
    /// without `TIMESTAMP_QUERY_INSIDE_PASSES`).
    pub fn draw_times_refused(&self) -> Option<&'static str> {
        self.draw_times.refused()
    }

    /// Forget the gathered draw times.
    pub fn reset_draw_times(&mut self) {
        self.draw_times.reset();
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
