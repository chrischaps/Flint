//! Frame loop for `PlayerApp` — code-motion sibling of `mod.rs`
//! (player_app decomposition; see the decomposition ADR). Owns per-frame
//! tick (scripts, subsystems, transitions), render, the egui HUD pass,
//! and the stats overlay.

use super::hud_render::render_draw_commands;
use super::subsystem::{EncodeCtx, RenderCtx};
use super::PlayerApp;
use super::TransitionPhase;
#[cfg(feature = "debug-hud")]
use super::{
    CAMERA_TUNING_COMPONENT, DEAD_CALM_COMPONENT, GRASS_DEBUG_PANEL, RAFT_VISITOR_COMPONENT,
    REALITY_COMPONENT, TIME_OF_DAY_COMPONENT, WEATHER_COMPONENT,
};
#[cfg(feature = "debug-hud")]
#[cfg(feature = "debug-hud")]
use flint_debug_ui::DebugPanel as _;
use flint_render::GrassEntityPosition;
use flint_runtime::state::AUDIO;
use flint_runtime::{RuntimeSystem, StateConfig};
use std::collections::HashSet;

/// Frame time given to decoding sounds while a late audio bundle settles in
/// (#124). A song is 85 to 190 ms of decoding in Chrome; at 2 ms a frame it
/// spreads over a second or two, and Float's 10 ms frames stay inside one
/// 16.7 ms vsync (at 4 ms they slipped to 33 ms now and then).
const AUDIO_DECODE_BUDGET: std::time::Duration = std::time::Duration::from_millis(2);

impl PlayerApp {
    /// The screen in points as scripts see it (`screen_width()` and
    /// `screen_height()`, and the UI documents' layout): egui's screen, once
    /// it agrees with the window. For egui's first passes its `screen_rect()`
    /// is a 10,000-point placeholder (15,000 once a `--pixels-per-point`
    /// zoom lands on a 1.5x display), and a script sizing its HUD by the
    /// screen asked for lettering 14 to 21 times too big, whose glow wanted
    /// a texture past the GPU's limit (#657). Until egui agrees, it's the
    /// window's size at the scale egui is about to use.
    pub(super) fn script_screen_size(&self) -> (f32, f32) {
        let rect = self.hud.egui_ctx.screen_rect();
        let egui_size = (rect.width(), rect.height());
        let Some(window) = &self.window else {
            return egui_size;
        };
        let size = window.inner_size();
        let ppp = self
            .launch
            .pixels_per_point
            .unwrap_or(window.scale_factor() as f32)
            .max(0.01);
        let window_size = (size.width as f32 / ppp, size.height as f32 / ppp);
        let agrees = (egui_size.0 - window_size.0).abs() <= 1.0
            && (egui_size.1 - window_size.1).abs() <= 1.0;
        if agrees || size.width == 0 || size.height == 0 {
            egui_size
        } else {
            window_size
        }
    }

    #[profiling::function]
    pub(super) fn render(&mut self) {
        // On Android, window may be None between suspended() and resumed()
        if self.window.is_none() {
            return;
        }
        self.update_render_scale();
        if let Some(look) = self.style.pending_look.take() {
            if let Err(e) = self.set_look(look.as_deref()) {
                tracing::warn!("look: {e}");
            }
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

        let view = context.frame_view(&output.texture);

        // The shading boil (#214): none, unless a subsystem draws one (the
        // living subsystem, where the viewer stands in its wooze field)
        renderer.set_boil(Default::default());
        if !self.core.subsystems.is_empty() {
            let mut ctx = RenderCtx {
                renderer: &mut *renderer,
                device: &context.device,
                queue: &context.queue,
                camera: &self.camera,
                clock: &self.core.clock,
            };
            self.core.subsystems.pre_render(&mut ctx);
        }

        // Apply script-driven post-processing overrides before rendering.
        // The Rendering & Effects panel's freeze switch (ADR 0053) skips the
        // stamp entirely so panel edits to contended fields stick; the
        // drained overrides are plain Options re-filled each tick, so nothing
        // piles up and un-freezing resumes on the next script write.
        #[cfg(feature = "debug-hud")]
        let pp_stamp_frozen = self.post.debug_freeze;
        #[cfg(not(feature = "debug-hud"))]
        let pp_stamp_frozen = false;
        if !pp_stamp_frozen && self.post.any() {
            let mut config = renderer.post_process_config().clone();
            self.post.stamp(&mut config);
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

        // Particles panel (ADR 0068): live-mirror counts while clean, apply
        // play/stop/burst/budget/draw actions when dirty.
        #[cfg(feature = "debug-hud")]
        for panel in &mut self.debug_panels {
            if panel.name() != flint_debug_ui::PARTICLES_DEBUG_PANEL {
                continue;
            }
            let Some(pp) = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::ParticlesDebugPanel>()
            else {
                continue;
            };
            if flint_debug_ui::DebugPanel::is_dirty(pp) {
                for action in pp.take_actions() {
                    use flint_debug_ui::ParticlePanelAction as A;
                    match action {
                        A::Play(id) => self.core.particles.sync.set_playing(id, true),
                        A::Stop(id) => self.core.particles.sync.set_playing(id, false),
                        A::Burst(id, n) => self.core.particles.sync.queue_burst(id, n),
                        A::StopDetached(h) => self.core.particles.sync.stop_effect(h),
                        A::RestartAll => self.core.particles.sync.restart_all(),
                        A::SetBudget(b) => self.core.particles.sync.set_budget(b as usize),
                        A::SetRender(on) => self.particles_render_enabled = on,
                    }
                }
                flint_debug_ui::DebugPanel::clear_dirty(pp);
            } else {
                let rows: Vec<flint_debug_ui::EmitterRow> = self
                    .core
                    .particles
                    .sync
                    .instances()
                    .map(|(key, inst)| {
                        let (entity_id, handle, label) = match key {
                            flint_particles::InstanceKey::Entity(id) => (
                                Some(id),
                                0,
                                self.core.world.get_name(id).unwrap_or("?").to_string(),
                            ),
                            flint_particles::InstanceKey::Detached(h) => {
                                (None, h, format!("effect #{h}"))
                            }
                        };
                        let effect = match &inst.source {
                            flint_particles::EffectSource::Asset(fx) => Some(fx.name.clone()),
                            flint_particles::EffectSource::Inline => None,
                        };
                        flint_debug_ui::EmitterRow {
                            entity_id,
                            handle,
                            label,
                            effect,
                            emitters: inst.emitters.len(),
                            alive: inst.alive(),
                            capacity: inst.capacity(),
                            playing: inst.any_emitter_playing(),
                        }
                    })
                    .collect();
                pp.refresh(
                    rows,
                    self.core.particles.sync.total_alive(),
                    self.core.particles.sync.budget() as u32,
                );
                pp.render_enabled = self.particles_render_enabled;
            }
        }

        // A style the F4 or F3 editor just wrote into the scene file (#364):
        // the scene's authored settings after it, for the other editor too
        #[cfg(feature = "debug-hud")]
        let mut committed_styles: Option<flint_render::stylize::StyleSettings> = None;

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
            self.post.debug_freeze = rp.freeze_scripts;
            if flint_debug_ui::DebugPanel::is_dirty(rp) {
                let flags = rp.take_flags();
                if flags.look_changed {
                    self.style.pending_look = Some(rp.look.clone());
                }
                if flags.styles_committed {
                    committed_styles = rp.authored_styles().cloned();
                }
                if flags.pp_changed {
                    renderer.set_post_process_config(rp.pp.clone());
                    if flags.style_needs_resources {
                        renderer.ensure_style_resources(&context.device);
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
                    renderer.update_from_world(&self.core.world, &context.device);
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
            rp.set_style_ids(renderer.style_ids());
        }

        // F3 Styles panel (#268): the same write-through as F4, for the
        // style table alone. Entity rows only while it is open.
        #[cfg(feature = "debug-hud")]
        {
            let rows = self
                .debug_panels
                .iter()
                .any(|p| p.name() == flint_debug_ui::STYLES_DEBUG_PANEL && p.is_open())
                .then(|| {
                    super::debug_panels::styled_entity_rows(&self.core.world, renderer.style_ids())
                });
            for panel in &mut self.debug_panels {
                if panel.name() != flint_debug_ui::STYLES_DEBUG_PANEL {
                    continue;
                }
                let Some(sp) = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::StylesDebugPanel>()
                else {
                    continue;
                };
                if flint_debug_ui::DebugPanel::is_dirty(sp) {
                    let edit = sp.take_edit();
                    if edit.committed.is_some() {
                        committed_styles = sp.authored_styles().cloned();
                    }
                    if edit.changed {
                        let mut config = renderer.post_process_config().clone();
                        config.styles = sp.pp.styles.clone();
                        renderer.set_post_process_config(config);
                        if edit.needs_resources {
                            renderer.ensure_style_resources(&context.device);
                        }
                    }
                    flint_debug_ui::DebugPanel::clear_dirty(sp);
                } else {
                    let entities = rows.clone().unwrap_or_else(|| sp.entities().to_vec());
                    sp.refresh(renderer.post_process_config(), entities);
                }
            }
        }
        #[cfg(feature = "debug-hud")]
        if let Some(authored) = committed_styles {
            super::debug_panels::set_panels_authored_styles(&mut self.debug_panels, &authored);
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
                if let Some(entity_id) = self.core.world.get_id(ocean_panel.entity_name()) {
                    if let Some(comps) = self.core.world.get_components_mut(entity_id) {
                        for (field, value) in ocean_panel.config().to_fields() {
                            comps.set_field(flint_geometry::components::OCEAN, field, value);
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
            let Some(entity_id) = self.core.world.get_id(tod_panel.entity_name()) else {
                continue;
            };
            if let Some(day) = tod_panel.take_day_set() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
                    comps.set_field(TIME_OF_DAY_COMPONENT, "day", toml::Value::Float(day as f64));
                }
            }
            if tod_panel.is_dirty() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
                    for (field, value) in tod_panel.config().to_fields() {
                        comps.set_field(TIME_OF_DAY_COMPONENT, field, value);
                    }
                }
                tod_panel.clear_dirty();
            } else if let Some(hours) = self
                .core
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT))
                .and_then(|c| c.get("time_hours"))
                .and_then(flint_core::toml_util::toml_f32)
            {
                tod_panel.sync_time(hours);
            }
            let day = self
                .core
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
            let Some(entity_id) = self.core.world.get_id(weather_panel.entity_name()) else {
                continue;
            };
            let one_shots = weather_panel.take_one_shots();
            if weather_panel.is_dirty() || !one_shots.is_empty() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
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
                .core
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
            let Some(entity_id) = self.core.world.get_id(reality_panel.entity_name()) else {
                continue;
            };
            let one_shots = reality_panel.take_one_shots();
            if reality_panel.is_dirty() || !one_shots.is_empty() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
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
                .core
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
            let Some(entity_id) = self.core.world.get_id(visitor_panel.entity_name()) else {
                continue;
            };
            let one_shots = visitor_panel.take_one_shots();
            if !one_shots.is_empty() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
                    for (field, value) in one_shots {
                        comps.set_field(RAFT_VISITOR_COMPONENT, field, value);
                    }
                }
            }
            if let Some(comp) = self
                .core
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
            let Some(entity_id) = self.core.world.get_id(calm_panel.entity_name()) else {
                continue;
            };
            let one_shots = calm_panel.take_one_shots();
            if !one_shots.is_empty() {
                if let Some(comps) = self.core.world.get_components_mut(entity_id) {
                    for (field, value) in one_shots {
                        comps.set_field(DEAD_CALM_COMPONENT, field, value);
                    }
                }
            }
            if let Some(comp) = self
                .core
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
                if let Some(entity_id) = self.core.world.get_id(cam_panel.entity_name()) {
                    if let Some(comps) = self.core.world.get_components_mut(entity_id) {
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

        // F2's stats show the GPU's time too (ADR 0108), as does the perf
        // readout unless it was asked not to (#120): time the frame while
        // either is up. Timing the game turned on stays on after.
        if self.stats.show || self.stats.perf.as_ref().is_some_and(|p| p.gpu_wanted) {
            if !renderer.gpu_timing() {
                self.stats.gpu_timing =
                    renderer.set_gpu_timing(&context.device, &context.queue, true);
            }
        } else if self.stats.gpu_timing {
            renderer.set_gpu_timing(&context.device, &context.queue, false);
            self.stats.gpu_timing = false;
        }

        // The whole frame (scene, HUD, the GPU timer's resolve) goes into
        // one encoder and one submission (#139): on the web each submission
        // crosses into JS.
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });
        // Styles sized in points (ASCII cells, #265) follow the window's
        // scale factor, which changes when it moves to another monitor
        // `--pixels-per-point` (#435) pins it, and egui's with it, so a
        // capture's glyphs and panels are the same size on any display
        if let Some(window) = &self.window {
            let native = window.scale_factor() as f32;
            renderer.pixels_per_point = self.launch.pixels_per_point.unwrap_or(native);
            if let Some(ppp) = self.launch.pixels_per_point {
                let zoom = ppp / native.max(0.01);
                if (self.hud.egui_ctx.zoom_factor() - zoom).abs() > 1e-4 {
                    self.hud.egui_ctx.set_zoom_factor(zoom);
                }
            }
        }
        renderer.encode_to(
            &context.device,
            &context.queue,
            &mut encoder,
            // With its id, so bind groups over it are kept (#379)
            context.depth_target(),
            &self.camera,
            &view,
        );
        // A second look drawn over part of the frame, before the HUD (the
        // Living Props A/B split, #238)
        if !self.core.subsystems.is_empty() {
            let mut ctx = EncodeCtx {
                encoder: &mut encoder,
                device: &context.device,
                queue: &context.queue,
                output: &output.texture,
                depth_view: &context.depth_view,
                formats: (context.view_format, context.hud_format),
                renderer: &mut *renderer,
                world: &self.core.world,
                camera: &self.camera,
                #[cfg(feature = "procgen")]
                resolver: &mut self.core.procgen_resolver,
            };
            self.core.subsystems.post_encode(&mut ctx);
        }
        // A field borrow, not `perf_mark`: `context` is still held.
        if let Some(perf) = &mut self.stats.perf {
            perf.mark(super::perf::Phase::Render);
        }

        // Render egui HUD overlay on top of the 3D scene
        // HUD draws into a non-sRGB view of the same texture (see
        // RenderContext::hud_format) so egui blends in display space.
        let hud_view = output.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(context.hud_format),
            ..Default::default()
        });
        self.render_hud(&hud_view, &mut encoder);
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            renderer.encode_gpu_timings(&context.device, &mut encoder);
            context.queue.submit(std::iter::once(encoder.finish()));
            renderer.frame_submitted();
        }
        self.perf_mark(super::perf::Phase::Hud);

        // Capture the composited frame (scene + HUD) before it is presented.
        self.capture.last_frame_time = Some(self.core.clock.total_time);
        if let Some(path) = self.capture_path_due() {
            match self.write_screenshot(&output.texture, &path) {
                Ok(()) => self.capture.written += 1,
                Err(reason) => {
                    tracing::warn!("screenshot: {reason}");
                    self.capture.note_failure(reason);
                }
            }
        }

        output.present();
        let presented = web_time::Instant::now();
        self.perf_mark(super::perf::Phase::Present);
        #[cfg(target_arch = "wasm32")]
        self.show_perf();
        #[cfg(not(target_arch = "wasm32"))]
        self.print_perf();

        if let Some(exit_after) = self.capture.exit_after {
            if self.core.clock.total_time >= super::PERF_SKIP_SECS {
                self.capture.perf_frames += 1;
                self.capture.perf_time += self.core.clock.delta_time;
                self.capture
                    .perf_pacing
                    .record(self.core.clock.delta_time, presented);
                if let Some(renderer) = &self.scene_renderer {
                    let stats = renderer.collect_stats();
                    self.capture.perf_draws += stats.draw_calls as u64;
                    self.capture.perf_entity_draws += stats.entity_draws as u64;
                    self.capture.perf_culled += stats.culled_draws as u64;
                    self.capture.perf_lod_culled += stats.lod_culled_draws as u64;
                    self.capture.perf_shadow_triangles += stats.shadow_triangles as u64;
                    self.capture.perf_triangles += stats.triangles as u64;
                    if stats.gpu_frame_ms > 0.0 {
                        self.capture.perf_gpu_ms += stats.gpu_frame_ms as f64;
                        self.capture.perf_gpu_frames += 1;
                    }
                }
            }
            // A shot that's due but failed waits for its retries (#639).
            let now = self.core.clock.total_time;
            // A script's `quit_game()` this frame has already said it (#659).
            if now >= exit_after && !self.capture.shot_pending(now) && !self.outcome.exit_requested
            {
                self.outcome.exit_requested = true;
                self.print_exit_report();
            }
        }
    }

    /// `--exit-after`'s report, at its time or at a script's `quit_game()`
    /// before it (#659).
    pub(super) fn print_exit_report(&self) {
        // Headless perf figure: average frame time over the run, so a
        // scene change can be A/B'd without watching the overlay.
        let frames = self.capture.perf_frames;
        if frames > 0 {
            let avg_ms = self.capture.perf_time * 1000.0 / frames as f64;
            eprintln!(
                "[perf] {frames} frames over {:.2} s after t={:.0}: avg {avg_ms:.2} ms/frame ({:.1} fps)",
                self.capture.perf_time,
                super::PERF_SKIP_SECS,
                1000.0 / avg_ms
            );
            // How even they were: a mean hides bursts (#406).
            if let Some(dt) = self.capture.perf_pacing.dt() {
                eprintln!("[perf] dt: {dt}");
            }
            if let Some(present) = self.capture.perf_pacing.present() {
                eprintln!("[perf] present to present: {present}");
            }
            // The F2 overlay's main figures, averaged, so a capture
            // can compare draw counts and culling headlessly (H9).
            let avg = |sum: u64| sum as f64 / frames as f64;
            eprintln!(
                "[perf] avg per frame: {:.0} draw calls ({:.0} entity draws, {:.0} culled, {:.0} of them by LOD), {:.0} triangles, {:.0} in shadows",
                avg(self.capture.perf_draws),
                avg(self.capture.perf_entity_draws),
                avg(self.capture.perf_culled),
                avg(self.capture.perf_lod_culled),
                avg(self.capture.perf_triangles),
                avg(self.capture.perf_shadow_triangles)
            );
            // GPU time, when the frames were timed (--stats, ADR 0108).
            let gpu_frames = self.capture.perf_gpu_frames;
            if gpu_frames > 0 {
                let gpu_ms = self.capture.perf_gpu_ms / gpu_frames as f64;
                // Under vsync an idle GPU clocks down and reads high (#380).
                let present = self
                    .render_context
                    .as_ref()
                    .map(|c| c.present_mode_in_use());
                eprintln!(
                    "[perf] avg GPU {gpu_ms:.2} ms/frame over {gpu_frames} timed frames{}",
                    super::perf::gpu_vsync_note(present, gpu_ms as f32, avg_ms as f32)
                );
            }
        }
        for line in self.subsystem_report() {
            eprintln!("{line}");
        }
    }

    /// Decide whether this frame should be written to disk, and to where.
    ///
    /// Single-shot: the first frame at or after `capture.at`. Interval mode:
    /// the first frame at or after `capture.at`, then every `every` seconds,
    /// numbered `<stem>_0001.<ext>`, `<stem>_0002.<ext>`, ...
    fn capture_path_due(&mut self) -> Option<std::path::PathBuf> {
        let base = self.capture.path.clone()?;
        let now = self.core.clock.total_time;
        // Tolerance so `--screenshot-at` means the frame at that time: 210 steps of
        // 1/60 add up to a hair under 3.5, and the next frame is a step too late.
        let reached = now + 1e-6 >= self.capture.at;
        match self.capture.every {
            None => {
                if self.capture.taken || !reached {
                    return None;
                }
                self.capture.taken = true;
                Some(base)
            }
            Some(every) => {
                if !self.capture.taken {
                    if !reached {
                        return None;
                    }
                    self.capture.taken = true;
                    self.capture.next_due = now;
                }
                // Tolerance so `--screenshot-every` equal to `--frame-step`
                // writes every frame instead of losing some to rounding.
                if now + 1e-6 < self.capture.next_due {
                    return None;
                }
                self.capture.sequence += 1;
                self.capture.next_due = now + every.max(1.0 / 240.0);
                Some(numbered_capture_path(&base, self.capture.sequence))
            }
        }
    }

    /// Read the swapchain texture back and save it as a PNG; the reason
    /// it couldn't, if it couldn't (#639).
    fn write_screenshot(
        &self,
        texture: &wgpu::Texture,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let Some(context) = &self.render_context else {
            return Err("no render context".into());
        };
        if !context.surface_copyable {
            return Err(format!(
                "skipped {}: the surface has no COPY_SRC usage on this adapter",
                path.display()
            ));
        }
        let pixels = context
            .read_surface_rgba(texture)
            .map_err(|e| format!("read-back of {} failed: {e}", path.display()))?;
        let img = image::RgbaImage::from_raw(texture.width(), texture.height(), pixels)
            .ok_or_else(|| format!("pixel buffer size mismatch for {}", path.display()))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
        }
        img.save(path)
            .map_err(|e| format!("failed to save {}: {e}", path.display()))?;
        tracing::info!(
            "screenshot written: {} ({}x{} at t={:.2}s)",
            path.display(),
            texture.width(),
            texture.height(),
            self.core.clock.total_time
        );
        Ok(())
    }

    #[profiling::function]
    pub(super) fn tick(&mut self) {
        // A frame begins: the perf readout's clock (#120).
        if let Some(perf) = &mut self.stats.perf {
            perf.begin_frame();
        }
        use super::perf::Phase;

        // The page's Begin click and the browser's pointer lock (#102).
        #[cfg(target_arch = "wasm32")]
        self.sync_pointer_lock();

        // Pads are held while a text field has focus (ADR 0110), before
        // they're polled.
        self.gate_gamepads();

        // The touch controls' layout follows the window and page (#133).
        self.sync_touch_controls();

        // Before the clock ticks: a subsystem that reads the pads its own
        // way does it here (the rhythm session's capture drain, ADR 0018),
        // and the player polls them only while none holds them.
        super::game_core::run_stage(self, super::subsystem::Stage::Clock, None);
        if !self.pads_lent {
            self.poll_gamepad_events();
        }

        // Advance game clock
        self.core.clock.tick();
        if self
            .launch
            .hold_at
            .is_some_and(|t| self.core.clock.total_time >= t)
            && self.core.clock.frame_step != Some(0.0)
        {
            self.core.clock.frame_step = Some(0.0);
            tracing::info!("game time held at {:.3} s", self.core.clock.total_time);
        }

        // `--stats`: the overlay starts open, and F2 can still close it.
        if std::mem::take(&mut self.capture.show_stats) {
            self.stats.show = true;
        }

        // Scripted input due by now (`--input-script`), before anything reads it.
        // Keys and the pointer go the way a real keyboard's and mouse's do:
        // egui first (ADR 0082, 0097), then the debug keys (H9) and InputState.
        if let Some(script) = &mut self.capture.input_script {
            let applied = script.apply(self.core.clock.total_time, &mut self.input);
            for event in applied.events {
                self.scripted_event(event);
            }
        }

        // The game's frame (ADR 0117 decision 4): the stages, with this
        // window's part in them (`impl FrameHost for PlayerApp` below)
        super::game_core::step(self);

        // Script audio/cursor frame overrides, then per-frame input clear
        self.finish_frame();
        self.perf_mark(Phase::Sync);
    }

    #[profiling::function]
    fn run_extension_pre_updates(&mut self) {
        if self.extensions.is_empty() {
            return;
        }
        let mut fired_events = Vec::new();
        let rays = self
            .scene_renderer
            .as_ref()
            .map(|r| r.ray_scene())
            .unwrap_or_default();
        for ext in &mut self.extensions {
            let gpu = match (&mut self.scene_renderer, &self.render_context) {
                (Some(renderer), Some(context)) => Some(crate::extension::ExtensionGpu {
                    renderer,
                    device: &context.device,
                    queue: &context.queue,
                }),
                _ => None,
            };
            let mut ctx = crate::extension::ExtensionCtx {
                world: &mut self.core.world,
                clock: &self.core.clock,
                input: &self.input,
                camera: &self.camera,
                gpu,
                persist: &mut self.core.store,
                audio: crate::extension::ExtensionAudio::new(&mut self.audio),
                rays: rays.clone(),
                scene_tables: &self.scene.tables,
                fired_events: &mut fired_events,
                cursor_captured: self.focus.cursor_captured,
            };
            ext.pre_update(&mut ctx);
        }
        // Onto the frame's bus, so collect_game_events hands them to scripts
        // and audio this frame.
        for event in fired_events {
            self.core.events.push(event);
        }
    }

    fn finish_frame(&mut self) {
        // Apply audio low-pass filter override from scripts
        if let Some(cutoff) = self.core.script_requests.audio_lowpass.take() {
            self.audio.set_filter_cutoff(cutoff);
        }

        // A menu put the touch controls away, or play brought them back.
        if let Some(enabled) = self.core.script_requests.touch_controls.take() {
            self.input.touch_controls_mut().set_enabled(enabled);
        }

        // A quality setting moved the distance levers (#56).
        if let Some(renderer) = &mut self.scene_renderer {
            let lod = self.core.script_requests.lod_scale.take();
            let draw = self.core.script_requests.draw_distance_scale.take();
            let mut s = renderer.lod_settings();
            if lod.is_some() || draw.is_some() {
                s.lod_scale = lod.unwrap_or(s.lod_scale);
                s.draw_distance_scale = draw.unwrap_or(s.draw_distance_scale);
                renderer.set_lod_settings(s);
                s = renderer.lod_settings();
            }
            self.core
                .script
                .set_lod_state(s.lod_scale, s.draw_distance_scale);
        }

        // Apply cursor capture request from scripts (skipped while a debug
        // panel is open — the panel needs the mouse).
        if let Some(capture) = self.core.script_requests.cursor_captured.take() {
            #[cfg(feature = "debug-hud")]
            let panel_open = self.debug_panels.iter().any(|p| p.is_open());
            #[cfg(not(feature = "debug-hud"))]
            let panel_open = false;
            if capture && !panel_open && !self.focus.cursor_captured {
                self.capture_cursor();
            } else if !capture && self.focus.cursor_captured {
                self.release_cursor();
            }
        }

        // Debounced save of the persistent store (1 s after the last change,
        // or now if a script called persist_save()).
        self.flush_persistent_store(false, "debounce");

        // Clear per-frame input state
        self.input.end_frame();
    }

    /// The frame-budgeted procgen queue, as the frame runs it (tests).
    #[cfg(all(test, feature = "living"))]
    pub(super) fn tick_procgen(&mut self) {
        use super::game_core::FrameHost;
        let (core, lent) = self.lend();
        core.tick_procgen(lent);
    }

    #[profiling::function]
    /// Hot reload of user styles' descriptions (#275, natively): a few
    /// times a second, look for edited or new `styles/*.shader.toml`, and
    /// read the scene's settings again for each style that changed, since
    /// its tables were checked against the old parameters. The WGSL files
    /// are polled by the styles themselves.
    fn poll_user_styles(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::{AtomicU32, Ordering};
            static FRAMES: AtomicU32 = AtomicU32::new(0);
            if FRAMES.fetch_add(1, Ordering::Relaxed) % 15 != 0 {
                return;
            }
            let reload =
                flint_render::stylize::custom::reload_changed_for(&self.kept_asset_scope());
            for message in &reload.messages {
                tracing::warn!("{message}");
            }
            if !reload.changed() {
                return;
            }
            // What the scene authors, with --style over it, so a reloaded
            // style a word names keeps the word's settings (#328)
            let authored = self.scene_post_config();
            if let Some(renderer) = &mut self.scene_renderer {
                let mut config = renderer.post_process_config().clone();
                flint_render::stylize::custom::refresh_settings(&mut config, &authored, &reload);
                renderer.set_post_process_config(config);
            }
        }
    }

    fn sync_renderer(&mut self) {
        self.poll_user_styles();

        // The camera is final for the frame: a cut (a teleport, a jump to a
        // new shot) means the picture no longer follows on from the last
        // one, so styles that keep history start over (#343)
        let cut = self.style.camera_cut.observe(
            self.camera.position_array(),
            self.camera.forward_vector(),
            self.core.clock.delta_time as f32,
        );
        if cut {
            if let Some(renderer) = &mut self.scene_renderer {
                renderer.reset_style_history();
            }
        }

        // Refresh renderer with updated transforms
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.camera_offset = [self.camera.position.x, self.camera.position.y];
            renderer.ortho_height = if self.camera.ortho_height > 0.0 {
                self.camera.ortho_height
            } else {
                10.0
            };
            renderer.aspect_ratio = self.camera.aspect;
            renderer.update_from_world(&self.core.world, &context.device);
            // Next frame's scripts cast against what this one draws (ADR 0105).
            self.core.script.set_ray_scene(renderer.ray_scene());
        }

        // Pack (camera-sorted) and upload particle instances (ADR 0068)
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            if self.particles_render_enabled {
                self.core.particles.pack(Some(self.camera.position_array()));
                renderer.update_particles_from(
                    &context.device,
                    &context.queue,
                    &self.core.particles.sync,
                );
            } else {
                renderer.update_particles(&context.device, &context.queue, &[]);
            }
        }

        // Update grass time and entity positions for bend-on-contact
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            // The wall clock, unless the grass is on the wooze field's (#252):
            // then the shallow wooze subsystem sets it at Stage::Sync
            renderer.grass_time = self.core.clock.total_time as f32;
            renderer.grass_wooze = None;
            // Ocean waves run on the same clock scripts see via total_time(),
            // keeping script-side ocean_height() queries in sync with the GPU.
            renderer.ocean_time = self.core.clock.total_time;
            // History styles decay by game time (#342), so a --frame-step
            // capture settles alike at any step
            renderer.style_dt = self.core.clock.delta_time as f32;
            // --style-time holds the post time, as `flint render`'s does (#328)
            if let Some(t) = self.launch.style_time {
                renderer.ocean_time = t as f64;
            }

            let cam_pos = self.camera.position;
            let grass_entities = vec![GrassEntityPosition {
                position: [cam_pos.x, cam_pos.y, cam_pos.z],
                _pad: 0.0,
            }];
            renderer.update_grass_entities(&context.queue, &grass_entities);
        }
    }

    fn drain_post_overrides(&mut self) {
        // Script post-processing overrides for this frame (the commands
        // are in by now: both drains run before this), in place of last
        // frame's, with what a subsystem filled in at the procgen stage (the
        // rhythm ladder's values, ADR 0021). DoF is script-owned (ADR 0027);
        // scene loads reset the sticky config from the def.
        self.post.take_from(&mut self.core.script_requests);
    }

    #[profiling::function]
    fn render_hud(&mut self, target_view: &wgpu::TextureView, encoder: &mut wgpu::CommandEncoder) {
        // Lazy-load any sprite textures referenced by draw commands
        // (must happen before egui_winit borrow)
        self.load_pending_sprites();

        let Some(window) = &self.window else { return };
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(egui_winit) = &mut self.hud.egui_winit else {
            return;
        };

        let mut raw_input = egui_winit.take_egui_input(window);
        // Under --frame-step egui runs on the game clock too (#435): its
        // animations (a header opening, a tooltip fading) would otherwise
        // follow the wall clock, and a slow capture frame would land them
        // somewhere else on every run
        if let Some(step) = self.core.clock.frame_step {
            raw_input.time = Some(self.core.clock.total_time);
            raw_input.predicted_dt = step as f32;
        }

        let draw_commands = std::mem::take(&mut self.hud.draw_commands);
        let mut extensions = std::mem::take(&mut self.extensions);
        let mut subsystems = std::mem::take(&mut self.core.subsystems);
        #[cfg(feature = "debug-hud")]
        let mut debug_panels = std::mem::take(&mut self.debug_panels);
        #[cfg(all(feature = "debug-hud", feature = "living"))]
        let view_proj = self.camera.view_projection_matrix();
        let ui_textures = &self.hud.textures;
        let ui_fonts = &self.hud.fonts;
        let ui_fonts_warned = &mut self.hud.fonts_warned;
        let text_halos = &mut self.hud.text_halos;
        text_halos.begin_frame();
        let key_note = &mut self.hud.key_note;

        // World text needs the frame's depth to be drawn against.
        let world_texts = std::mem::take(&mut self.hud.world_texts);
        let text_view = (!world_texts.is_empty()
            && self
                .scene_renderer
                .as_ref()
                .is_some_and(|r| r.frame_depth_ready()))
        .then(|| super::world_text::TextView {
            view_proj: self.camera.view_projection_matrix(),
            eye: self.camera.position_array(),
            screen: [
                context.config.width as f32 / self.hud.egui_ctx.pixels_per_point(),
                context.config.height as f32 / self.hud.egui_ctx.pixels_per_point(),
            ],
            px_per_point: self.hud.egui_ctx.pixels_per_point(),
        });
        let mut laid_text = Vec::new();

        let touch_widgets = self.input.touch_controls().widgets();
        let touch_opacity = self.input.touch_controls().opacity();

        let show_stats = self.stats.show;
        let stats_data = if show_stats {
            self.scene_renderer.as_ref().map(|r| {
                let mut stats = r.collect_stats();
                // FPS smoothing: rolling window of delta_time samples
                self.stats.frame_times.push_back(self.core.clock.delta_time);
                while self.stats.frame_times.len() > 60 {
                    self.stats.frame_times.pop_front();
                }
                let avg_dt: f64 = self.stats.frame_times.iter().sum::<f64>()
                    / self.stats.frame_times.len().max(1) as f64;
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

        // User styles that don't compile (#275), and built-in shaders
        // read from FLINT_STYLE_SHADER_DIR that don't (#418): shown until
        // they do
        #[cfg(feature = "debug-hud")]
        let style_errors = {
            let mut errors = flint_render::stylize::custom::errors();
            errors.extend(flint_render::stylize::dev_shader::errors());
            errors
        };

        let full_output = self.hud.egui_ctx.run(raw_input, |ctx| {
            // Layers, back to front: script UI, game egui, debug panels.
            // Script UI first: it shares the background layer with any game
            // panels, so issuing it first puts those panels in front of it
            // (see render_draw_commands). Items a script pushed over the
            // UI go on their own Middle layer, above the game's panels and
            // windows and below the debug panels (ADR 0085).
            render_draw_commands(
                ctx,
                &draw_commands,
                ui_textures,
                ui_fonts,
                ui_fonts_warned,
                text_halos,
            );

            // World text is laid out here, so the glyphs it needs go up
            // with this pass's font atlas.
            if let Some(view) = &text_view {
                laid_text = super::world_text::lay_out(
                    ctx,
                    &world_texts,
                    view,
                    ui_fonts,
                    ui_fonts_warned,
                    text_halos,
                );
            }

            // The game's own egui (PlayerExtension::ui). Its panels sit on
            // the background layer and its windows on Middle, both below
            // the debug panels, which float on Foreground.
            for ext in extensions.iter_mut() {
                ext.ui(ctx);
            }

            // On-screen touch controls (#133), over the game's UI.
            super::touch_overlay::paint(ctx, &touch_widgets, touch_opacity);

            // The subsystems' own (which preset is where, while two living
            // props presets are compared, #238)
            subsystems.ui(ctx);

            // The pose overlay (#218) and a picked tree's marks and window
            // (#248), in the world and under every panel.
            #[cfg(all(feature = "debug-hud", feature = "living"))]
            for panel in debug_panels.iter_mut() {
                if let Some(living) = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::LivingPropsDebugPanel>()
                {
                    living.paint_overlay(ctx, &view_proj);
                }
            }

            #[cfg(feature = "debug-hud")]
            let debug_width =
                super::debug_panels::show_debug_panels(ctx, &mut debug_panels, &mut |panel, ui| {
                    super::debug_panels::draw_panel(panel, &mut extensions, ui)
                });
            #[cfg(not(feature = "debug-hud"))]
            let debug_width = 0.0;

            if let Some(ref stats) = stats_data {
                render_stats_overlay(ctx, stats, debug_width);
            }

            // On the web, above the page's wordmark in the same corner
            // (index.html's `.mark`, two lines at bottom 20px; #415)
            // A debug key's note (#367), over everything
            key_note.paint(ctx, 16.0);

            #[cfg(feature = "debug-hud")]
            flint_debug_ui::paint_style_errors(
                ctx,
                &style_errors,
                if cfg!(target_arch = "wasm32") {
                    52.0
                } else {
                    0.0
                },
            );
        });

        self.hud.draw_commands = draw_commands;
        self.extensions = extensions;
        self.core.subsystems = subsystems;
        #[cfg(feature = "debug-hud")]
        {
            self.debug_panels = debug_panels;
        }
        // Fonts exist from here on; measure_text may now lay text out for real.
        self.hud
            .egui_fonts_ready
            .store(true, std::sync::atomic::Ordering::Relaxed);

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let paint_jobs = self
            .hud
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let glyphs = text_view
            .map(|view| super::world_text::tessellate(&self.hud.egui_ctx, laid_text, &view))
            .unwrap_or_default();

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.hud.egui_renderer.take().unwrap();

        // The HUD's GPU time, with the renderer's phases (ADR 0108).
        let ui_scope = self
            .scene_renderer
            .as_mut()
            .and_then(|r| r.gpu_timer_mut())
            .map(|t| t.begin(encoder, "UI"));

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
        }

        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        // World text goes under the HUD, against the scene's depth.
        if !glyphs.is_empty() {
            let pipeline = self.hud.world_text_pipeline.get_or_insert_with(|| {
                super::world_text::WorldTextPipeline::new(&context.device, context.hud_format)
            });
            pipeline.draw(
                &context.device,
                &context.queue,
                encoder,
                target_view,
                &context.depth_view,
                &glyphs,
                &egui_renderer,
            );
        }

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

        if let (Some(timer), Some(scope)) = (
            self.scene_renderer.as_mut().and_then(|r| r.gpu_timer_mut()),
            ui_scope,
        ) {
            timer.end(encoder, scope);
        }

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.hud.egui_renderer = Some(egui_renderer);
    }
}
/// The player's part in the game's frame (ADR 0117 decision 4): what
/// `game_core::step` calls back, in the order it does.
impl super::game_core::FrameHost for PlayerApp {
    fn lend(&mut self) -> (&mut super::game_core::GameCore, super::game_core::Lent<'_>) {
        (
            &mut self.core,
            super::game_core::Lent {
                input: &mut self.input,
                camera: &mut self.camera,
                renderer: self.scene_renderer.as_mut(),
                gpu: self.render_context.as_ref().map(|c| (&c.device, &c.queue)),
                pixels_per_point: self.hud.egui_ctx.pixels_per_point(),
                cursor_captured: self.focus.cursor_captured,
                scene_file_writable: self.launch.scene_file_writable,
                note: &mut self.hud.key_note,
                #[cfg(feature = "debug-hud")]
                panels: &mut self.debug_panels,
                audio: Some(&mut self.audio.engine),
                pads: Some(super::subsystem::Pads {
                    gilrs: &mut self.gilrs,
                    rumble: &mut self.pad_rumble,
                    lent: &mut self.pads_lent,
                }),
                quit: Some(&mut self.outcome.exit_requested),
            },
        )
    }

    fn core(&mut self) -> &mut super::game_core::GameCore {
        &mut self.core
    }

    fn before_events(&mut self) {
        // Advance transition phase timing
        self.advance_transition();
        // Game extensions, before anything below reads the world
        self.run_extension_pre_updates();
    }

    fn phase_done(&mut self, phase: super::perf::Phase) {
        self.perf_mark(phase);
        if phase == super::perf::Phase::Scripts {
            if let Some(perf) = &mut self.stats.perf {
                perf.record_scripts(self.core.script.update_times());
            }
        }
    }

    fn window_for_scripts(&mut self) {
        // Transition state for script access
        let script = &mut self.core.script;
        match &self.transition_phase {
            TransitionPhase::Idle => {
                script.set_transition_state(-1.0, "idle");
            }
            TransitionPhase::Exiting { elapsed, .. } => {
                script.set_transition_state(*elapsed as f64, "exiting");
            }
            TransitionPhase::Loading { .. } => {
                script.set_transition_state(1.0, "loading");
            }
            TransitionPhase::Entering { elapsed } => {
                script.set_transition_state(*elapsed as f64, "entering");
            }
        }
        let (screen_w, screen_h) = self.script_screen_size();
        let script = &mut self.core.script;
        script.set_screen_size(screen_w, screen_h);
        // Cursor in the same logical-point space as screen_width()/draw_*:
        // InputState holds physical pixels, egui's ppp is the window scale.
        {
            let ppp = self.hud.egui_ctx.pixels_per_point().max(0.01);
            let (mx, my) = self.input.mouse_position;
            script.set_mouse_position(mx as f32 / ppp, my as f32 / ppp);
        }
        script.set_cursor_captured(self.focus.cursor_captured);

        // Sync loaded chunk IDs so scripts can query is_chunk_loaded()
        let chunk_ids: HashSet<String> = self.loaded_chunks.keys().cloned().collect();
        script.set_loaded_chunk_ids(chunk_ids);

        // Where each audio_source is in its sound (#658), read every frame,
        // paused or not: loops play on under a pause menu.
        self.audio.sample_positions();
        script.set_audio_clock(self.audio.sync.positions(), self.audio.output_latency());
        // The music clock and the conducted snapshot are the music
        // subsystem's, set at `Stage::PreScripts`, just before this
    }

    fn listener(&mut self, at: super::game_core::Listener) {
        match at {
            super::game_core::Listener::FirstPerson(position, yaw, pitch) => {
                self.audio.update_listener(position, yaw, pitch);
            }
            // A script-driven camera (chase cam, etc.)
            super::game_core::Listener::ScriptCamera => {
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
    }

    fn host_command(&mut self, command: flint_script::context::ScriptCommand) {
        flint_script::host::apply(self, command);
    }

    fn script_ui_drawn(&mut self) {
        // Collect draw commands for this frame (scripts + data-driven UI)
        let mut commands = self.core.script.drain_draw_commands();
        let (screen_w, screen_h) = self.script_screen_size();
        let ui_commands = self
            .core
            .script
            .generate_ui_draw_commands(screen_w, screen_h);
        commands.extend(ui_commands);
        self.hud.draw_commands = commands;

        // Depth-tested world lines (ADR 0083) render with this frame's scene.
        let world_lines = self.core.script.drain_world_lines();
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.set_world_lines(world_lines, self.hud.egui_ctx.pixels_per_point());
        }
        // World text (ADR 0104) is laid out in the HUD's egui pass.
        self.hud.world_texts = self.core.script.drain_world_texts();
    }

    #[profiling::function]
    fn sound(&mut self, config: &StateConfig, game_events: &[flint_runtime::GameEvent]) {
        // A late bundle (audio, #98) landed: queue what wasn't there at
        // start. Already-loaded sounds are skipped, and an autoplay source
        // that was waiting for its file starts on the update after its
        // sound is decoded.
        let generation = flint_core::vfs::generation();
        if generation != self.audio_vfs_generation {
            self.audio_vfs_generation = generation;
            super::scene_loading::load_audio_from_world(
                &self.core.world,
                &mut self.audio,
                &self.core.project,
                &self.core.scene_path,
                self.scene.preload_audio,
                true,
            );
        }
        // ...and decode it a few milliseconds a frame (#124).
        self.audio.engine.decode_queued(AUDIO_DECODE_BUDGET);

        // Audio triggers from game events (skip when paused)
        if config.runs(AUDIO) {
            self.audio.process_events(game_events, &self.core.world);
            self.audio
                .update(&mut self.core.world, self.core.clock.delta_time)
                .ok();
        }
    }

    fn bones_posed(&mut self) {
        // Push skeletal bone matrices to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.core.animation.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(
                        &context.device,
                        &context.queue,
                        *entity_id,
                        asset_name,
                        matrices,
                    );
                }
            }
        }
    }

    fn sync(&mut self) {
        // Renderer sync: transforms, particle upload, grass/ocean time
        self.sync_renderer();

        // Script post-override drain (a subsystem's share is in by now)
        self.drain_post_overrides();

        // A script's set_quality (FlintKart#55), applied here, between the
        // frame's scripts and its draw
        self.apply_script_quality();
    }
}

/// `right_inset`: width the debug columns take at the right edge, so the
/// overlay sits beside them rather than over them.
fn render_stats_overlay(ctx: &egui::Context, stats: &flint_render::RenderStats, right_inset: f32) {
    use flint_render::format_count;

    egui::Area::new(egui::Id::new("render_stats_overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0 - right_inset, 8.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
                .stroke(egui::Stroke::new(
                    1.0f32,
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
                    if stats.culled_draws > 0 {
                        ui.label(format!(
                            "Entities: {} ({} culled)",
                            stats.entity_draws, stats.culled_draws
                        ));
                    } else {
                        ui.label(format!("Entities: {}", stats.entity_draws));
                    }
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
                    // Style chain memory (#302): every target, and how
                    // many are full output size (FXAA's included)
                    if stats.style_targets > 0 {
                        ui.label(format!(
                            "Style targets: {} ({:.1} MB, {} full-size)",
                            stats.style_targets,
                            stats.style_target_bytes as f64 / (1024.0 * 1024.0),
                            stats.full_size_targets
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

                    // GPU time per phase (ADR 0108), where the device can time
                    if !stats.gpu_passes.is_empty() {
                        ui.label(format!("GPU: {:.2}ms", stats.gpu_frame_ms));
                        for pass in &stats.gpu_passes {
                            ui.colored_label(
                                egui::Color32::from_gray(170),
                                format!("  {:<13}{:>6.2}", pass.label, pass.ms),
                            );
                        }
                        ui.separator();
                    }

                    // Resolution, and the scene's below render scale 1 (#122)
                    let scene = stats.scene_resolution;
                    let resolution = if scene == stats.resolution || scene == [0, 0] {
                        format!("{}x{}", stats.resolution[0], stats.resolution[1])
                    } else {
                        format!(
                            "{}x{}, scene {}x{}",
                            stats.resolution[0], stats.resolution[1], scene[0], scene[1]
                        )
                    };
                    ui.colored_label(egui::Color32::from_gray(102), resolution);
                });
        });
}

/// `hud.png` + 3 -> `hud_0003.png`; extension-less paths get `.png`.
pub(crate) fn numbered_capture_path(base: &std::path::Path, n: u32) -> std::path::PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".to_string());
    let ext = base
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "png".to_string());
    base.with_file_name(format!("{stem}_{n:04}.{ext}"))
}

#[cfg(test)]
mod capture_tests {
    use super::numbered_capture_path;
    use std::path::Path;

    #[test]
    fn numbered_path_keeps_dir_and_extension() {
        let p = numbered_capture_path(Path::new("renders/hud.png"), 7);
        assert_eq!(p, Path::new("renders/hud_0007.png"));
    }

    #[test]
    fn numbered_path_defaults_extension() {
        let p = numbered_capture_path(Path::new("shot"), 12);
        assert_eq!(p, Path::new("shot_0012.png"));
    }
}
