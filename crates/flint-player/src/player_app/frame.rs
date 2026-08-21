//! Frame loop for `PlayerApp` — code-motion sibling of `mod.rs`
//! (player_app decomposition; see the decomposition ADR). Owns per-frame
//! tick (scripts, music session, transitions), render, the egui HUD pass,
//! and the stats overlay.

use super::PlayerApp;
use super::hud_render::render_draw_commands;
#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
use super::music_session;
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::TransitionPhase;
use super::scene_loading;
#[cfg(feature = "debug-hud")]
use super::{
    CAMERA_TUNING_COMPONENT, DEAD_CALM_COMPONENT, GRASS_DEBUG_PANEL, RAFT_VISITOR_COMPONENT,
    REALITY_COMPONENT, TIME_OF_DAY_COMPONENT, WEATHER_COMPONENT,
};
use flint_core::Vec3 as FlintVec3;
use std::collections::HashSet;
#[cfg(feature = "debug-hud")]
use flint_core::components as comp;
#[cfg(feature = "debug-hud")]
use flint_debug_ui::DebugPanel as _;
use flint_render::{GrassEntityPosition, ParticleDrawData, ParticleInstanceGpu};
use flint_runtime::{RuntimeSystem, StateConfig, SystemPolicy};

impl PlayerApp {
    pub(super) fn render(&mut self) {
        // On Android, window may be None between suspended() and resumed()
        if self.window.is_none() {
            return;
        }
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };

        let output = match context.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                return;
            }
            Err(e) => {
                tracing::warn!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Apply script-driven post-processing overrides before rendering.
        // The Rendering & Effects panel's freeze switch (ADR 0053) skips the
        // stamp entirely so panel edits to contended fields stick; the
        // drained overrides are plain Options re-filled each tick, so nothing
        // piles up and un-freezing resumes on the next script write.
        #[cfg(feature = "debug-hud")]
        let pp_stamp_frozen = self.pp_debug_freeze;
        #[cfg(not(feature = "debug-hud"))]
        let pp_stamp_frozen = false;
        if !pp_stamp_frozen
            && (self.pp_vignette_override.is_some()
                || self.pp_bloom_override.is_some()
                || self.pp_exposure_override.is_some()
                || self.pp_chromatic_aberration_override.is_some()
                || self.pp_radial_blur_override.is_some()
                || self.pp_ssao_intensity_override.is_some()
                || self.pp_fog_density_override.is_some()
                || self.pp_desaturation_override.is_some()
                || self.pp_dof_strength_override.is_some()
                || self.pp_dof_focus_distance_override.is_some()
                || self.pp_dof_focus_range_override.is_some()
                || self.pp_fog_color_override.is_some()
                || self.pp_render_mode_override.is_some()
                || self.pp_mode_params_override.is_some()
                || self.pp_mode_was_active)
        {
            let mut config = renderer.post_process_config().clone();
            if let Some(v) = self.pp_vignette_override {
                config.vignette_enabled = v > 0.001;
                config.vignette_intensity = v;
            }
            if let Some(b) = self.pp_bloom_override {
                config.bloom_intensity = b;
            }
            if let Some(e) = self.pp_exposure_override {
                config.exposure = e;
            }
            if let Some(ca) = self.pp_chromatic_aberration_override {
                config.chromatic_aberration = ca;
            }
            if let Some(rb) = self.pp_radial_blur_override {
                config.radial_blur = rb;
            }
            if let Some(si) = self.pp_ssao_intensity_override {
                config.ssao_intensity = si;
            }
            if let Some(fd) = self.pp_fog_density_override {
                config.fog_density = fd;
            }
            if let Some(d) = self.pp_desaturation_override {
                config.desaturate = d;
            }
            if let Some(s) = self.pp_dof_strength_override {
                config.dof_strength = s;
            }
            if let Some(fd) = self.pp_dof_focus_distance_override {
                config.dof_focus_distance = fd;
            }
            if let Some(fr) = self.pp_dof_focus_range_override {
                config.dof_focus_range = fr;
            }
            if let Some(fc) = self.pp_fog_color_override {
                config.fog_color = fc;
            }
            // Reality tear: transient, unlike the sticky overrides above.
            match self.pp_render_mode_override {
                Some((mode, mix)) => {
                    config.render_mode = mode;
                    config.mode_mix = mix;
                    self.pp_mode_was_active = mode != 0 && mix > 0.0;
                }
                None if self.pp_mode_was_active => {
                    config.render_mode = 0;
                    config.mode_mix = 0.0;
                    self.pp_mode_was_active = false;
                }
                None => {}
            }
            if let Some(mp) = self.pp_mode_params_override {
                config.mode_params = mp;
            }
            renderer.set_post_process_config(config);
        }

        // Push debug panel grass config changes to renderer
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() == GRASS_DEBUG_PANEL && panel.is_dirty() {
                // A name collision must be a miss, not a panic.
                if let Some(grass_panel) = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::GrassDebugPanel>()
                {
                    if grass_panel.density_changed() {
                        renderer.reload_grass_config(&context.device, grass_panel.config().clone());
                        grass_panel.clear_density_changed();
                    } else {
                        renderer.set_grass_config(grass_panel.config().clone());
                    }
                }
                panel.clear_dirty();
            }
        }

        // Rendering & Effects panel (ADR 0053): live-mirror renderer state
        // while the panel is clean; write edits through when dirty, routing
        // expensive operations by the per-group flags.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != flint_debug_ui::RENDER_DEBUG_PANEL {
                continue;
            }
            // A name collision must be a miss, not a panic.
            let Some(rp) = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::RenderDebugPanel>()
            else {
                continue;
            };
            self.pp_debug_freeze = rp.freeze_scripts;
            if flint_debug_ui::DebugPanel::is_dirty(rp) {
                let flags = rp.take_flags();
                if flags.pp_changed {
                    renderer.set_post_process_config(rp.pp.clone());
                    if flags.kuwahara_needs_resources {
                        renderer.ensure_kuwahara_resources(&context.device, &context.queue);
                    }
                    if flags.fxaa_needs_resources {
                        renderer.ensure_fxaa_resources(&context.device);
                    }
                }
                if flags.shadows_changed {
                    renderer.set_shadows(rp.shadows_enabled);
                }
                if flags.shadow_res_changed {
                    renderer.set_shadow_resolution(&context.device, rp.shadow_resolution);
                }
                if flags.lighting_reset {
                    renderer.reset_ambient();
                } else if flags.lighting_changed {
                    renderer.set_ambient(rp.lighting.ambient_sky, rp.lighting.ambient_ground);
                    renderer.set_diffuse_wrap(rp.lighting.diffuse_wrap);
                    renderer.set_oren_nayar(rp.lighting.oren_nayar);
                    renderer.set_sheen(rp.lighting.sheen_color, rp.lighting.sheen_strength);
                }
                if flags.mode_changed {
                    renderer.set_debug_mode(rp.debug_mode);
                    renderer.update_from_world(&self.world, &context.device);
                }
                if flags.fov_changed {
                    self.camera.fov = rp.fov_deg;
                }
                flint_debug_ui::DebugPanel::clear_dirty(rp);
            } else {
                rp.refresh(
                    renderer.post_process_config(),
                    renderer.debug_state().mode,
                    renderer.shadows_enabled(),
                    renderer.shadow_resolution(),
                    renderer.lighting_levers(),
                    self.camera.fov,
                );
            }
        }

        // Push ocean panel edits into the world's `ocean` component — the
        // renderer extraction and the script API both read from there.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() == "Ocean Debug" && panel.is_dirty() {
                let ocean_panel = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::OceanDebugPanel>()
                    .unwrap();
                if let Some(entity_id) = self.world.get_id(ocean_panel.entity_name()) {
                    if let Some(comps) = self.world.get_components_mut(entity_id) {
                        for (field, value) in ocean_panel.config().to_fields() {
                            comps.set_field(comp::OCEAN, field, value);
                        }
                    }
                }
                panel.clear_dirty();
            }
        }

        // Day/time panel: push edits into the component; while auto time
        // advances (the game script owns time_hours), pull it back so the
        // slider tracks the sky instead of going stale. The day counter is
        // script-owned outright, so day edits are one-shot overrides (never
        // part of the persistent config) and the live value is pulled back
        // every frame for display.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Day / Time" {
                continue;
            }
            let tod_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::TimeOfDayDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(tod_panel.entity_name()) else {
                continue;
            };
            if let Some(day) = tod_panel.take_day_set() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    comps.set_field(TIME_OF_DAY_COMPONENT, "day", toml::Value::Float(day as f64));
                }
            }
            if tod_panel.is_dirty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    for (field, value) in tod_panel.config().to_fields() {
                        comps.set_field(TIME_OF_DAY_COMPONENT, field, value);
                    }
                }
                tod_panel.clear_dirty();
            } else if let Some(hours) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT))
                .and_then(|c| c.get("time_hours"))
                .and_then(flint_core::toml_util::toml_f32)
            {
                tod_panel.sync_time(hours);
            }
            let day = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT))
                .and_then(|c| c.get("day"))
                .and_then(flint_core::toml_util::toml_f32);
            tod_panel.sync_day(day);
        }

        // Weather panel: push edits + queued one-shots into the component;
        // the weather script owns state/wind/sea, so pull those back for the
        // read-only status line every frame.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Weather" {
                continue;
            }
            let weather_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::WeatherDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(weather_panel.entity_name()) else {
                continue;
            };
            let one_shots = weather_panel.take_one_shots();
            if weather_panel.is_dirty() || !one_shots.is_empty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    if weather_panel.is_dirty() {
                        for (field, value) in weather_panel.config().to_fields() {
                            comps.set_field(WEATHER_COMPONENT, field, value);
                        }
                    }
                    for (field, value) in one_shots {
                        comps.set_field(WEATHER_COMPONENT, field, value);
                    }
                }
                weather_panel.clear_dirty();
            }
            if let Some(comp) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(WEATHER_COMPONENT))
            {
                let g = |name: &str| {
                    comp.get(name)
                        .and_then(flint_core::toml_util::toml_f32)
                        .unwrap_or(0.0)
                };
                weather_panel.sync_status(g("state"), g("wind"), g("sea"));
            }
        }

        // Reality panel: push edits + queued one-shots into the component;
        // the reality script owns active_mode/mix/next_in_s, so pull those
        // back for the read-only status line every frame.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Reality" {
                continue;
            }
            let reality_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::RealityDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(reality_panel.entity_name()) else {
                continue;
            };
            let one_shots = reality_panel.take_one_shots();
            if reality_panel.is_dirty() || !one_shots.is_empty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    if reality_panel.is_dirty() {
                        for (field, value) in reality_panel.config().to_fields() {
                            comps.set_field(REALITY_COMPONENT, field, value);
                        }
                    }
                    for (field, value) in one_shots {
                        comps.set_field(REALITY_COMPONENT, field, value);
                    }
                }
                reality_panel.clear_dirty();
            }
            if let Some(comp) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(REALITY_COMPONENT))
            {
                let g = |name: &str, dv: f32| {
                    comp.get(name)
                        .and_then(flint_core::toml_util::toml_f32)
                        .unwrap_or(dv)
                };
                reality_panel.sync_status(
                    g("active_mode", 0.0),
                    g("mix", 0.0),
                    g("next_in_s", -1.0),
                );
            }
        }

        // Visitor panel: queue the one-shot trigger into the component; the
        // visitor script owns phase/day, so pull those back for the
        // read-only status line every frame.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Visitor" {
                continue;
            }
            let visitor_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::VisitorDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(visitor_panel.entity_name()) else {
                continue;
            };
            let one_shots = visitor_panel.take_one_shots();
            if !one_shots.is_empty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    for (field, value) in one_shots {
                        comps.set_field(RAFT_VISITOR_COMPONENT, field, value);
                    }
                }
            }
            if let Some(comp) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(RAFT_VISITOR_COMPONENT))
            {
                let g = |name: &str, dv: f32| {
                    comp.get(name)
                        .and_then(flint_core::toml_util::toml_f32)
                        .unwrap_or(dv)
                };
                visitor_panel.sync_status(g("phase", 0.0), g("day", 1.0));
            }
        }

        // Dead Calm panel: queue the one-shot trigger/end into the
        // component; the calm script owns phase/calm/next_in_s, so pull
        // those back for the read-only status line every frame.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Dead Calm" {
                continue;
            }
            let calm_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::DeadCalmDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(calm_panel.entity_name()) else {
                continue;
            };
            let one_shots = calm_panel.take_one_shots();
            if !one_shots.is_empty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    for (field, value) in one_shots {
                        comps.set_field(DEAD_CALM_COMPONENT, field, value);
                    }
                }
            }
            if let Some(comp) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(DEAD_CALM_COMPONENT))
            {
                let g = |name: &str, dv: f32| {
                    comp.get(name)
                        .and_then(flint_core::toml_util::toml_f32)
                        .unwrap_or(dv)
                };
                calm_panel.sync_status(g("phase", 0.0), g("calm", 0.0), g("next_in_s", -1.0));
            }
        }

        // Camera panel: edits drive the live render camera and the component
        // (so Commit to File persists them); while idle, track the camera so
        // script FOV overrides don't leave the slider stale.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != "Camera" {
                continue;
            }
            let cam_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::CameraDebugPanel>()
                .unwrap();
            if cam_panel.is_dirty() {
                self.camera.fov = cam_panel.config().fov_deg;
                if let Some(entity_id) = self.world.get_id(cam_panel.entity_name()) {
                    if let Some(comps) = self.world.get_components_mut(entity_id) {
                        for (field, value) in cam_panel.config().to_fields() {
                            comps.set_field(CAMERA_TUNING_COMPONENT, field, value);
                        }
                    }
                }
                cam_panel.clear_dirty();
            } else {
                cam_panel.sync_fov(self.camera.fov);
            }
        }

        if let Err(e) = renderer.render(context, &self.camera, &view) {
            tracing::warn!("Render error: {:?}", e);
        }

        // Render egui HUD overlay on top of the 3D scene
        self.render_hud(&view);

        output.present();
    }

    pub(super) fn tick(&mut self) {
        // Music session drain + guide/timeline panel feed (F3, ADR 0018).
        self.tick_music_session();

        // Advance game clock
        self.clock.tick();

        // Advance transition phase timing
        self.advance_transition();

        // Read active state config to decide which systems run
        let config = self.state_machine.active_config().clone();

        let has_fps_player = self.physics.has_player_entity();

        // Fixed-timestep physics + FPS camera/listener follow
        self.tick_fixed_physics(&config, has_fps_player);

        // Physics + input events — scripts and audio both consume them
        let game_events = self.collect_game_events();

        // Set state machine + persistent store + physics pointers for script
        // access. The RAII guard must outlive every script call this frame —
        // including the sprite-anim-end callbacks inside tick_av_systems —
        // so it lives here, in tick's own scope, not in a sub-step.
        let _state_scope = self.script.state_scope(
            &mut self.state_machine,
            &mut self.persistent_store,
            &self.physics,
        );
        self.script.set_current_scene(&self.scene_path);

        // Script context (transition state, camera, input/events, conducted
        // parameters — session tick above precedes set_conducted, which
        // precedes provide_context/on_update: F4/ADR 0020 ordering), then
        // on_update.
        self.run_scripts(&config, &game_events);

        // Script camera overrides (chase cam etc.) + roll basis + listener
        self.apply_script_camera_overrides(has_fps_player);

        // on_draw_ui (always runs), script commands, draw-command collection
        self.drive_script_ui();

        // Audio triggers, animations + sprite-end callbacks, bone matrices,
        // bone probes, particles
        self.tick_av_systems(&config, &game_events);

        // Frame-budgeted procgen queue
        self.tick_procgen();

        // Renderer sync: transforms, particle upload, grass/ocean time
        self.sync_renderer();

        // Script post-override drain + music-session ladder merge (ADR 0021)
        self.drain_post_overrides();

        // Script audio/cursor frame overrides, then per-frame input clear
        self.finish_frame();
    }

    fn finish_frame(&mut self) {
        // Apply audio low-pass filter override from scripts
        if let Some(cutoff) = self.script.take_audio_overrides() {
            self.audio.set_filter_cutoff(cutoff);
        }

        // Apply cursor capture request from scripts (skipped while a debug
        // panel is open — the panel needs the mouse).
        if let Some(capture) = self.script.take_cursor_capture_override() {
            #[cfg(feature = "debug-hud")]
            let panel_open = self.debug_panels.iter().any(|p| p.is_open());
            #[cfg(not(feature = "debug-hud"))]
            let panel_open = false;
            if capture && !panel_open && !self.cursor_captured {
                self.capture_cursor();
            } else if !capture && self.cursor_captured {
                self.release_cursor();
            }
        }

        // Clear per-frame input state
        self.input.end_frame();
    }

    fn tick_music_session(&mut self) {
        // Music session (F3, ADR 0018): while a session is active the capture
        // thread owns the pad — its drain replaces the player's own polling,
        // feeding the Judge at full precision and InputState down-sampled.
        let session_finished = if let Some(ms) = &mut self.music_session {
            ms.tick(&mut self.input)
        } else {
            self.poll_gamepad_events();
            false
        };
        if session_finished {
            // Natural mid-scene finish: gilrs back, the scene keeps running —
            // unless the component asked for `quit_on_finish`.
            let quit = self
                .music_session
                .as_ref()
                .is_some_and(music_session::MusicSession::quit_on_finish);
            self.stop_music_session();
            if quit {
                println!("[music] quit_on_finish — exiting player");
                self.music_exit_requested = true;
            }
        }

        // Feed the Music Guide + Manifest Map panels — only while summoned,
        // so a closed panel costs nothing (history accumulates in the core
        // regardless).
        #[cfg(feature = "debug-hud")]
        if let Some(ms) = &self.music_session {
            for panel in &mut self.debug_panels {
                if !panel.is_open() {
                    continue;
                }
                if panel.name() == music_guide_panel::MUSIC_GUIDE_PANEL {
                    if let Some(p) = panel
                        .as_any_mut()
                        .downcast_mut::<music_guide_panel::MusicGuidePanel>()
                    {
                        p.set_data(
                            ms.visual_frame(),
                            ms.guide_frame(music_guide_panel::GUIDE_HORIZON_BEATS),
                        );
                    }
                } else if panel.name() == timeline_panel::MANIFEST_MAP_PANEL {
                    if let Some(p) = panel
                        .as_any_mut()
                        .downcast_mut::<timeline_panel::ManifestMapPanel>()
                    {
                        p.set_frame(ms.timeline_frame());
                    }
                }
            }
        }
    }

    fn tick_fixed_physics(&mut self, config: &StateConfig, has_fps_player: bool) {
        // Fixed-timestep physics loop (skip when paused, but still consume steps to avoid spiral)
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            if config.physics == SystemPolicy::Run {
                if has_fps_player {
                    self.physics
                        .update_character(&self.input, &mut self.world, dt);
                }
                self.physics
                    .fixed_update(&mut self.world, dt)
                    .unwrap_or_else(|e| tracing::warn!("Physics error: {:?}", e));
            }

            self.clock.consume_fixed_step();
        }

        // Update camera from player character position (FPS mode)
        if has_fps_player {
            let cam_pos = self.physics.camera_position(&self.world);
            let cam_target = self.physics.camera_target(cam_pos);
            self.camera.update_first_person(
                cam_pos,
                self.physics.camera_yaw(),
                self.physics.camera_pitch(),
            );
            self.camera.target = cam_target;

            if config.audio == SystemPolicy::Run {
                self.audio.update_listener(
                    cam_pos,
                    self.physics.camera_yaw(),
                    self.physics.camera_pitch(),
                );
            }
        }
    }

    fn collect_game_events(&mut self) -> Vec<flint_runtime::GameEvent> {
        // Process physics events — scripts + audio both consume them
        // Always collect events (input always processed so pause/unpause keybinds work)
        let mut game_events = self.physics.drain_events();
        for action in self.input.actions_just_pressed() {
            game_events.push(flint_runtime::GameEvent::ActionPressed(action));
        }
        for action in self.input.actions_just_released() {
            game_events.push(flint_runtime::GameEvent::ActionReleased(action));
        }
        game_events

    }

    fn run_scripts(&mut self, config: &StateConfig, game_events: &Vec<flint_runtime::GameEvent>) {
        // Set transition state for script access
        match &self.transition_phase {
            TransitionPhase::Idle => {
                self.script.set_transition_state(-1.0, "idle");
            }
            TransitionPhase::Exiting { elapsed, .. } => {
                self.script.set_transition_state(*elapsed as f64, "exiting");
            }
            TransitionPhase::Loading { .. } => {
                self.script.set_transition_state(1.0, "loading");
            }
            TransitionPhase::Entering { elapsed } => {
                self.script
                    .set_transition_state(*elapsed as f64, "entering");
            }
        }

        // Script system: provide camera context, then run updates
        self.script
            .set_camera(self.camera.position_array(), self.camera.forward_vector());
        self.script.provide_context(
            &self.input,
            &game_events,
            self.clock.total_time,
            self.clock.delta_time,
        );
        let screen_rect = self.egui_ctx.screen_rect();
        self.script
            .set_screen_size(screen_rect.width(), screen_rect.height());

        // Sync loaded chunk IDs so scripts can query is_chunk_loaded()
        let chunk_ids: HashSet<String> = self.loaded_chunks.keys().cloned().collect();
        self.script.set_loaded_chunk_ids(chunk_ids);

        // Conducted parameters (F4, ADR 0020): the music session's per-frame
        // state for scene bindings; neutral defaults when no session (also
        // resets the frame after a session ends).
        self.script.set_conducted(
            self.music_session
                .as_ref()
                .map(|ms| ms.conducted_snapshot())
                .unwrap_or_default(),
        );

        // Only run on_update when scripts are not paused
        if config.scripts == SystemPolicy::Run {
            self.script
                .update(&mut self.world, self.clock.delta_time)
                .unwrap_or_else(|e| tracing::warn!("Script error: {:?}", e));
        }
    }

    fn apply_script_camera_overrides(&mut self, has_fps_player: bool) {
        // Apply script camera overrides (for non-FPS camera modes like chase camera)
        let cam = self.script.take_camera_overrides();
        if let Some(pos) = cam.position {
            self.camera.position = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
        }
        if let Some(target) = cam.target {
            self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam.fov {
            self.camera.fov = fov;
        }
        if let Some(ortho) = cam.orthographic {
            self.camera.orthographic = ortho;
        }
        if let Some(height) = cam.ortho_height {
            self.camera.ortho_height = height;
        }
        // Roll rebuilds the up vector from the view basis each frame it is set;
        // when absent the up vector must reset to world up (overrides are
        // one-frame take()s — without the reset, roll would stick after a
        // script stops setting it, e.g. hot-reload into a compile error).
        match cam.roll {
            Some(roll) => {
                let forward = flint_core::Vec3::new(
                    self.camera.target.x - self.camera.position.x,
                    self.camera.target.y - self.camera.position.y,
                    self.camera.target.z - self.camera.position.z,
                )
                .normalized();
                let right = forward.cross(&flint_core::Vec3::UP);
                let right_len = (right.dot(&right)).sqrt();
                if right_len > 1e-4 {
                    let right = FlintVec3::new(
                        right.x / right_len,
                        right.y / right_len,
                        right.z / right_len,
                    );
                    let base_up = right.cross(&forward);
                    let (sin_r, cos_r) = roll.sin_cos();
                    self.camera.up = FlintVec3::new(
                        base_up.x * cos_r + right.x * sin_r,
                        base_up.y * cos_r + right.y * sin_r,
                        base_up.z * cos_r + right.z * sin_r,
                    );
                }
                // Degenerate (looking straight up/down): keep the previous up.
            }
            None => {
                self.camera.up = flint_core::Vec3::UP;
            }
        }

        // Update audio listener for script-driven cameras (chase cam, etc.)
        if !has_fps_player && cam.position.is_some() {
            let cam_pos = self.camera.position;
            let dir = flint_core::Vec3::new(
                self.camera.target.x - cam_pos.x,
                self.camera.target.y - cam_pos.y,
                self.camera.target.z - cam_pos.z,
            );
            let yaw = dir.x.atan2(dir.z);
            let horiz = (dir.x * dir.x + dir.z * dir.z).sqrt();
            let pitch = (-dir.y).atan2(horiz);
            self.audio.update_listener(cam_pos, yaw, pitch);
        }
    }

    fn drive_script_ui(&mut self) {
        // on_draw_ui() ALWAYS runs (pause menus, transition visuals need to draw)
        self.script.call_draw_uis(&mut self.world);

        let script_commands = self.script.drain_commands();
        self.process_script_commands(script_commands);

        // Collect draw commands for this frame (scripts + data-driven UI)
        let mut commands = self.script.drain_draw_commands();
        let screen_rect = self.egui_ctx.screen_rect();
        let ui_commands = self
            .script
            .generate_ui_draw_commands(screen_rect.width(), screen_rect.height());
        commands.extend(ui_commands);
        self.draw_commands = commands;
    }

    fn tick_av_systems(&mut self, config: &StateConfig, game_events: &Vec<flint_runtime::GameEvent>) {
        // Audio triggers from game events (skip when paused)
        if config.audio == SystemPolicy::Run {
            self.audio.process_events(&game_events, &self.world);
            self.audio
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }

        // Advance animations (skip when paused)
        if config.animation == SystemPolicy::Run {
            self.animation
                .update(&mut self.world, self.clock.delta_time)
                .ok();

            // Deliver sprite animation end events to scripts
            let sprite_events = self.animation.drain_sprite_events();
            if !sprite_events.is_empty() {
                self.script
                    .call_sprite_anim_ends(&mut self.world, &sprite_events);
            }
        }

        // Push skeletal bone matrices to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.animation.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(&context.queue, asset_name, matrices);
                }
            }
        }

        // Publish bone_probe joints: model-local joint positions written
        // into the component right after the pose computation, so scripts
        // read this frame's pose (e.g. seat_camera following the eye).
        {
            let probe_ids: Vec<flint_core::EntityId> = self
                .world
                .entities_with_component(flint_core::components::BONE_PROBE)
                .iter()
                .copied()
                .collect();
            for id in probe_ids {
                let joint = self
                    .world
                    .get_components(id)
                    .and_then(|c| c.get_field(flint_core::components::BONE_PROBE, "joint"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if let Some(joint) = joint {
                    if let Some(pos) = self.animation.joint_position(&id, &joint) {
                        for (field, value) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
                            let _ = self.world.set_field(
                                id,
                                flint_core::components::BONE_PROBE,
                                field,
                                toml::Value::Float(value as f64),
                            );
                        }
                    }
                }
            }
        }

        // Advance particle simulation (skip when paused)
        if config.particles == SystemPolicy::Run {
            self.particles
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }
    }

    fn tick_procgen(&mut self) {
        // Process procgen generation queue (frame-budgeted)
        {
            let camera_pos = self.camera.position_array();
            let completed = self.procgen_resolver.process_frame(camera_pos);
            if !completed.is_empty() {
                if let (Some(renderer), Some(ctx)) =
                    (&mut self.scene_renderer, &self.render_context)
                {
                    scene_loading::upload_completed_procgen_assets(
                        &completed,
                        renderer,
                        &ctx.device,
                        &ctx.queue,
                    );
                }
            }
        }
    }

    fn sync_renderer(&mut self) {
        // Refresh renderer with updated transforms
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.camera_offset = [self.camera.position.x, self.camera.position.y];
            renderer.ortho_height = if self.camera.ortho_height > 0.0 {
                self.camera.ortho_height
            } else {
                10.0
            };
            renderer.aspect_ratio = self.camera.aspect;
            renderer.update_from_world(&self.world, &context.device);
        }

        // Upload particle instance data to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            let sync_draw_data = self.particles.sync.draw_data();
            let render_draw_data: Vec<ParticleDrawData<'_>> = sync_draw_data
                .iter()
                .map(|d| {
                    let gpu_instances: &[ParticleInstanceGpu] =
                        bytemuck::cast_slice(bytemuck::cast_slice::<_, u8>(d.instances));
                    ParticleDrawData {
                        instances: gpu_instances,
                        texture: d.texture,
                        additive: d.blend_mode == flint_particles::ParticleBlendMode::Additive,
                    }
                })
                .collect();
            renderer.update_particles(&context.device, render_draw_data);
        }

        // Update grass time and entity positions for bend-on-contact
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.grass_time = self.clock.total_time as f32;
            // Ocean waves run on the same clock scripts see via total_time(),
            // keeping script-side ocean_height() queries in sync with the GPU.
            renderer.ocean_time = self.clock.total_time;

            let cam_pos = self.camera.position;
            let grass_entities = vec![GrassEntityPosition {
                position: [cam_pos.x, cam_pos.y, cam_pos.z],
                _pad: 0.0,
            }];
            renderer.update_grass_entities(&context.queue, &grass_entities);
        }
    }

    fn drain_post_overrides(&mut self) {
        // Drain script post-processing overrides for this frame
        let pp = self.script.take_postprocess_overrides();
        self.pp_vignette_override = pp.vignette;
        self.pp_bloom_override = pp.bloom;
        self.pp_exposure_override = pp.exposure;
        self.pp_chromatic_aberration_override = pp.chromatic_aberration;
        self.pp_radial_blur_override = pp.radial_blur;
        self.pp_ssao_intensity_override = pp.ssao_intensity;
        self.pp_fog_density_override = pp.fog_density;
        self.pp_desaturation_override = pp.desaturation;
        // DoF is script-owned (ADR 0027) — never touched by the ladder merge
        // below; there is no restore machinery because owning scripts write it
        // every frame and scene loads reset the sticky config from the def.
        self.pp_dof_strength_override = pp.dof_strength;
        self.pp_dof_focus_distance_override = pp.dof_focus_distance;
        self.pp_dof_focus_range_override = pp.dof_focus_range;
        self.pp_fog_color_override = self.script.take_fog_color_override();
        let (pp_mode, pp_mode_params) = self.script.take_render_mode_overrides();
        self.pp_render_mode_override = pp_mode;
        self.pp_mode_params_override = pp_mode_params;

        // Music-session ladder/reintegration visuals (ADR 0021). Script
        // overrides win — F4/F6 script-authored rung visuals supersede this
        // direct path with no code change and no double application. The
        // merge writes base + rung every frame so a recovered ladder actively
        // restores the scene's authored post config (the renderer config is
        // sticky); after teardown a one-shot restore does the same.
        if let Some(ms) = &self.music_session {
            let vf = ms.visual_frame();
            music_session::merge_ladder_postprocess(
                &mut self.pp_radial_blur_override,
                &mut self.pp_chromatic_aberration_override,
                &mut self.pp_desaturation_override,
                &vf,
                self.music_pp_base.unwrap_or_default(),
            );
        } else if let Some(base) = self.music_pp_restore.take() {
            music_session::restore_ladder_postprocess(
                &mut self.pp_radial_blur_override,
                &mut self.pp_chromatic_aberration_override,
                &mut self.pp_desaturation_override,
                base,
            );
        }
    }


    fn render_hud(&mut self, target_view: &wgpu::TextureView) {
        // Lazy-load any sprite textures referenced by draw commands
        // (must happen before egui_winit borrow)
        self.load_pending_sprites();

        let Some(window) = &self.window else { return };
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(egui_winit) = &mut self.egui_winit else {
            return;
        };

        let raw_input = egui_winit.take_egui_input(window);

        let draw_commands = std::mem::take(&mut self.draw_commands);
        #[cfg(feature = "debug-hud")]
        let mut debug_panels = std::mem::take(&mut self.debug_panels);
        let ui_textures = &self.ui_textures;

        let show_stats = self.show_stats;
        let stats_data = if show_stats {
            self.scene_renderer.as_ref().map(|r| {
                let mut stats = r.collect_stats();
                // FPS smoothing: rolling window of delta_time samples
                self.stats_frame_times.push_back(self.clock.delta_time);
                while self.stats_frame_times.len() > 60 {
                    self.stats_frame_times.pop_front();
                }
                let avg_dt: f64 = self.stats_frame_times.iter().sum::<f64>()
                    / self.stats_frame_times.len().max(1) as f64;
                stats.fps = (1.0 / avg_dt) as f32;
                stats.frame_time_ms = (avg_dt * 1000.0) as f32;
                if let Some(ctx) = &self.render_context {
                    stats.resolution = [ctx.config.width, ctx.config.height];
                }
                stats
            })
        } else {
            None
        };

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // Script UI first: it shares the panels' background layer, so
            // issuing it before the panels puts the panels in front of it
            // (title card, HUD) — see render_draw_commands.
            render_draw_commands(ctx, &draw_commands, ui_textures);

            #[cfg(feature = "debug-hud")]
            {
                // Full-width bottom strips first (e.g. the timeline panel);
                // the panel draws its own compact labels (a heading would
                // eat the vertical budget).
                for panel in debug_panels.iter_mut() {
                    if panel.is_open()
                        && matches!(panel.layout(), flint_debug_ui::PanelLayout::Bottom)
                    {
                        let panel_name = panel.name().to_owned();
                        egui::TopBottomPanel::bottom(egui::Id::new(&panel_name))
                            .exact_height(112.0)
                            .show(ctx, |ui| {
                                panel.ui(ui);
                            });
                    }
                }

                // Open side-panel indices, in creation order.
                let open: Vec<usize> = debug_panels
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        p.is_open() && matches!(p.layout(), flint_debug_ui::PanelLayout::SideRight)
                    })
                    .map(|(i, _)| i)
                    .collect();

                if !open.is_empty() {
                    // Logical points (screen_rect), never config.width (physical px).
                    let screen_w = ctx.screen_rect().width();
                    // Panels take at most ~half the window; fewer columns when narrow.
                    let max_cols = (((screen_w * 0.5) / 300.0).floor() as usize).clamp(1, 3);
                    let names: Vec<String> = open
                        .iter()
                        .map(|&i| debug_panels[i].name().to_owned())
                        .collect();
                    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let columns = flint_debug_ui::assign_columns(&name_refs, max_cols);

                    for (col_idx, col) in columns.iter().enumerate() {
                        egui::SidePanel::right(egui::Id::new(("debug_col", col_idx)))
                            .default_width(300.0)
                            .min_width(280.0)
                            .show(ctx, |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        for &slot in col {
                                            let panel = &mut debug_panels[open[slot]];
                                            let name = panel.name().to_owned();
                                            egui::CollapsingHeader::new(
                                                egui::RichText::new(name).heading(),
                                            )
                                            .default_open(true)
                                            .show(ui, |ui| panel.ui(ui));
                                            ui.separator();
                                        }
                                    });
                            });
                    }
                }
            }
            if let Some(ref stats) = stats_data {
                render_stats_overlay(ctx, stats);
            }
        });

        self.draw_commands = draw_commands;
        #[cfg(feature = "debug-hud")]
        {
            self.debug_panels = debug_panels;
        }

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("HUD Encoder"),
            });

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
        }

        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HUD Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // overlay on top of 3D
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
        }

        context.queue.submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);
    }
}

fn render_stats_overlay(ctx: &egui::Context, stats: &flint_render::RenderStats) {
    use flint_render::format_count;

    egui::Area::new(egui::Id::new("render_stats_overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
                ))
                .rounding(egui::Rounding::same(4.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                    ui.set_min_width(180.0);

                    // Header
                    ui.colored_label(
                        egui::Color32::from_gray(136),
                        egui::RichText::new("RENDERING STATS").size(9.0),
                    );
                    ui.separator();

                    // Core metrics
                    ui.horizontal(|ui| {
                        ui.label("FPS:");
                        ui.colored_label(
                            egui::Color32::from_rgb(74, 222, 128),
                            format!("{:.0}", stats.fps),
                        );
                        ui.colored_label(
                            egui::Color32::from_gray(102),
                            format!("({:.1}ms)", stats.frame_time_ms),
                        );
                    });
                    ui.label(format!("Draw Calls: {}", stats.draw_calls));
                    ui.label(format!("Triangles: {}", format_count(stats.triangles)));

                    ui.separator();

                    // Breakdown
                    ui.label(format!("Entities: {}", stats.entity_draws));
                    if stats.skinned_draws > 0 {
                        ui.label(format!("Skinned: {}", stats.skinned_draws));
                    }
                    ui.label(format!(
                        "Terrain: {}/{} chunks",
                        stats.terrain_draws, stats.terrain_total_chunks
                    ));
                    if stats.transparent_draws > 0 {
                        ui.label(format!("Transparent: {}", stats.transparent_draws));
                    }
                    if stats.billboard_draws > 0 {
                        ui.label(format!("Billboards: {}", stats.billboard_draws));
                    }
                    if stats.particle_draws > 0 {
                        ui.label(format!(
                            "Particles: {} ({} inst)",
                            stats.particle_draws,
                            format_count(stats.particle_instances)
                        ));
                    }
                    if stats.sprite_batches > 0 {
                        ui.label(format!("Sprites: {}", stats.sprite_batches));
                    }
                    if stats.grass_instances > 0 {
                        ui.label(format!(
                            "Grass: {} inst",
                            format_count(stats.grass_instances)
                        ));
                    }

                    ui.separator();

                    // Shadow pass
                    ui.label(format!("Shadow Calls: {}", stats.shadow_draw_calls));
                    ui.label(format!(
                        "Shadow Tris: {}",
                        format_count(stats.shadow_triangles)
                    ));

                    ui.separator();

                    // Resolution
                    ui.colored_label(
                        egui::Color32::from_gray(102),
                        format!("{}x{}", stats.resolution[0], stats.resolution[1]),
                    );
                });
        });
}
