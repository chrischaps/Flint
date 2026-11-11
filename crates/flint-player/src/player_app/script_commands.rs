//! The player as a script host (`impl ScriptHost for PlayerApp`, #697) and
//! the PlayerApp-side music-session entry points — code-motion sibling of `mod.rs` (player_app decomposition; see
//! the decomposition ADR). `music_session.rs` (the session integration
//! itself) is deliberately separate and unchanged.

#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
use super::music_session;
use super::scene_loading;
use super::style_commands::{self, StyleApplied};
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::PlayerApp;
use super::TransitionPhase;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::quality::QualitySettings;
use flint_core::EntityId;
use flint_core::Vec3 as FlintVec3;
use flint_input_capture::pad::Gilrs;
use flint_runtime::GameEvent;
use flint_script::{CameraOverrides, PostProcessOverrides, ScriptHost};

impl PlayerApp {
    /// A script's `quit_game()` (#659): the run ends as closing the window
    /// ends it. Natively the event loop exits after this frame's redraw, and
    /// `exiting` flushes the persistent store, so the code is 0; a run with
    /// `--exit-after` prints its report first, as it would at its time. A
    /// page has no window to close: it lets go of the pointer and full
    /// screen and goes back to its doorway (`web_page::quit`).
    pub(super) fn quit_game(&mut self) {
        if self.exit_requested {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.release_cursor();
            super::web_page::quit();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            println!("[quit] a script asked to quit");
            self.exit_requested = true;
            if self.capture.exit_after.is_some() {
                self.print_exit_report();
            }
        }
    }

    /// Carry out what the scripts have asked since the last drain (#697),
    /// through the one dispatcher ([`flint_script::host::apply_all`]). The
    /// frame drains twice: after the fixed physics steps, for what
    /// `on_update` (and last frame's late callbacks) asked, so a camera a
    /// script set is the one `on_draw_ui` sees and the frame draws; and
    /// after `on_draw_ui`, for what it asked. What a frame keeps one value
    /// of (the camera, post-processing, the distance and quality levers,
    /// cursor and touch) is held in `script_requests` until the step of the
    /// frame that applies it.
    pub(super) fn dispatch_script_commands(&mut self) {
        let commands = self.script.drain_commands();
        flint_script::host::apply_all(self, commands);
    }

    /// Hand a style command's outcome on: its event to scripts' `on_event`
    /// (next frame, with the physics events), its refusal to the log.
    fn style_applied(&mut self, applied: StyleApplied) {
        match applied {
            StyleApplied::Done(Some(event)) => self.physics.push_event(event),
            StyleApplied::Done(None) => {}
            StyleApplied::Failed(message) => tracing::warn!(target: "script", "{message}"),
        }
    }

    /// A whole-frame style command (#369): edit the renderer's config and
    /// hand it back; the next frame builds a style newly switched on.
    /// Without a renderer there is no frame to style.
    fn edit_post_styles(
        &mut self,
        edit: impl FnOnce(&mut flint_render::PostProcessConfig) -> StyleApplied,
    ) {
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let mut config = renderer.post_process_config().clone();
        let applied = edit(&mut config);
        if let StyleApplied::Done(_) = applied {
            renderer.set_post_process_config(config);
        }
        self.style_applied(applied);
    }

    /// A script's change to the wooze field. A change that's no good is named and
    /// changes nothing. A scene with no `[wooze]` table never asked for a field, so
    /// it doesn't get one; it says so once, since a script may ask every frame.
    fn change_wooze(
        &mut self,
        change: impl FnOnce(&mut flint_core::wooze::WoozeField, f64) -> Result<(), String>,
    ) {
        let time = self.wooze_time;
        match &mut self.wooze {
            Some(field) => {
                if let Err(reason) = change(field, time) {
                    tracing::warn!("A script's change to the wooze field: {reason}");
                }
            }
            None if !self.wooze_missing_warned => {
                self.wooze_missing_warned = true;
                tracing::warn!(
                    "A script tried to change the wooze field, and this scene has none: \
                     give it a [wooze] table, even an empty one, to opt in"
                );
            }
            None => {}
        }
    }

    /// Begin sound, from the user gesture that begins the game (#101). A
    /// browser gives no sound before one, so the web player runs silent
    /// until this is called from its Begin click; on the desktop the device
    /// opened at start and this does nothing new. Loops already in the
    /// scene start, and so does the scene's music session if it was skipped
    /// for want of a device. Says whether there is sound.
    pub fn start_audio(&mut self) -> bool {
        let had_sound = self.audio.engine.is_available();
        // A music session clocks its suite on kira's manager itself, which
        // the browser's worklet mix (#136) keeps on the audio thread; such a
        // scene mixes on the main thread, as before.
        #[cfg(target_arch = "wasm32")]
        if !had_sound
            && !self
                .world
                .entities_with_component(music_session::MUSIC_SESSION)
                .is_empty()
        {
            tracing::info!(
                "[flint-audio] a music_session needs kira here: mixing on the main thread"
            );
            self.audio
                .engine
                .set_web_mixer(flint_audio::engine::WebMixer::MainThread);
        }
        if !self.audio.start(&self.world) {
            return false;
        }
        if !had_sound && self.music_session.is_none() {
            self.start_music_session();
        }
        true
    }

    /// Start the scene-declared music session, if any (F3, ADR 0019): find
    /// the first `music_session` component, resolve its repo-root-relative
    /// paths against the scene's base dir (scene file's parent's parent —
    /// `scenes/` sits at the repo root), start the suite on the shared
    /// manager, and hand the gamepad to the capture thread (ADR 0018).
    /// Headless or component-less scenes: no session, nothing changes.
    pub(super) fn start_music_session(&mut self) {
        let mut found = None;
        for entity in self.world.all_entities() {
            let comp = self
                .world
                .get_components(entity.id)
                .and_then(|c| c.get(music_session::MUSIC_SESSION).cloned());
            if let Some(comp) = comp {
                if found.is_some() {
                    tracing::warn!(
                        "multiple music_session components in scene; using the first \
                         (extra on entity {})",
                        entity.id
                    );
                } else {
                    found = Some(comp);
                }
            }
        }
        let Some(comp) = found else { return };
        // The suite's paths are from the project root (ADR 0114). For one
        // release a manifest that is only where the root used to be guessed
        // is still read from there, with a warning.
        let manifest = comp.get("manifest").and_then(|v| v.as_str()).unwrap_or("");
        let base_dir =
            self.project
                .root_holding("music_session", manifest.trim(), &self.scene_dir());
        match music_session::MusicSession::start(&base_dir, &comp, &mut self.audio.engine) {
            Ok(Some(session)) => {
                self.music_session = Some(session);
                // The scene's authored post values are the ladder merge's
                // base and the post-teardown restore target (ADR 0021).
                let authored = self
                    .scene_post_process
                    .as_ref()
                    .map(scene_loading::post_process_config_from_def)
                    .unwrap_or_default();
                self.music_pp_base = Some(music_session::LadderPostBase {
                    radial_blur: authored.radial_blur,
                    chromatic_aberration: authored.chromatic_aberration,
                    desaturate: authored.desaturate,
                });
                self.gilrs = None;
                println!(
                    "[music] gamepad handed to capture thread \
                     (player polling suspended for the session)"
                );
                // Music Guide debug panel (ADR 0035): registered closed;
                // Backquote summons it. Lives only as long as a session
                // could feed it.
                #[cfg(feature = "debug-hud")]
                {
                    if !self
                        .debug_panels
                        .iter()
                        .any(|p| p.name() == music_guide_panel::MUSIC_GUIDE_PANEL)
                    {
                        self.debug_panels
                            .push(Box::new(music_guide_panel::MusicGuidePanel::new()));
                        println!("[music] debug guide available — press ` (Backquote)");
                    }
                    // Manifest Map timeline strip: registered closed with the
                    // suite's static map baked in; Backslash summons it.
                    if !self
                        .debug_panels
                        .iter()
                        .any(|p| p.name() == timeline_panel::MANIFEST_MAP_PANEL)
                    {
                        let mut panel = timeline_panel::ManifestMapPanel::new();
                        if let Some(ms) = &self.music_session {
                            panel.set_map(ms.timeline_map());
                        }
                        self.debug_panels.push(Box::new(panel));
                        println!("[music] manifest map available — press \\ (Backslash)");
                    }
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[music] session failed to start: {e:#}"),
        }
    }

    /// Tear the music session down (stems stopped with a fade, capture guard
    /// dropped first inside) and give the gamepad back to player polling.
    /// Must run before `audio.clear()` in a scene transition (ADR 0017).
    pub(super) fn stop_music_session(&mut self) {
        if let Some(session) = self.music_session.take() {
            session.stop();
            // Queue the one-shot authored-values restore for the next merge
            // pass (setting overrides here would be wiped by the drain).
            self.music_pp_restore = self.music_pp_base.take();
            self.gilrs = Gilrs::new().ok();
            println!("[music] session ended — gamepad returned to player polling");
            #[cfg(feature = "debug-hud")]
            self.debug_panels.retain(|p| {
                p.name() != music_guide_panel::MUSIC_GUIDE_PANEL
                    && p.name() != timeline_panel::MANIFEST_MAP_PANEL
            });
        }
    }
}

/// The player is a script host (#697): every command does here what it
/// does in a game.
impl ScriptHost for PlayerApp {
    fn host_name(&self) -> &'static str {
        "the player"
    }

    fn play_sound(&mut self, name: String, volume: f64, pitch: f64) {
        if self.audio.engine.is_available() {
            if let Err(e) = self.audio.engine.play_non_spatial(
                &name,
                volume,
                pitch,
                false,
                flint_audio::Bus::Sfx,
            ) {
                tracing::warn!(target: "script", "play_sound error: {:?}", e);
            }
        }
    }

    fn play_sound_at(
        &mut self,
        name: String,
        position: (f64, f64, f64),
        volume: f64,
        pitch: f64,
        distances: Option<(f32, f32)>,
        rolloff: Option<f32>,
    ) {
        let pos = FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
        if let Err(e) = self.audio.engine.play_oneshot_at(
            &name,
            pos,
            volume,
            pitch,
            distances.unwrap_or(flint_audio::engine::DEFAULT_DISTANCES),
            rolloff.unwrap_or(flint_audio::engine::DEFAULT_ROLLOFF),
            flint_audio::Bus::Sfx,
        ) {
            tracing::warn!(target: "script", "play_sound_at error: {:?}", e);
        }
    }

    fn stop_sound(&mut self, _name: String) {
        // One-shot sounds play to completion (same as AudioCommand::Stop)
    }

    fn set_audio_lowpass(&mut self, cutoff_hz: f32) {
        self.script_requests.audio_lowpass = Some(cutoff_hz);
    }

    fn fire_event(&mut self, name: String, data: toml::Value) {
        // Intercept transition completion signal
        if name == TRANSITION_COMPLETE {
            match &self.transition_phase {
                TransitionPhase::Exiting { target_scene, .. } => {
                    let target = target_scene.clone();
                    self.transition_phase = TransitionPhase::Loading {
                        target_scene: target,
                    };
                }
                TransitionPhase::Entering { .. } => {
                    self.transition_phase = TransitionPhase::Idle;
                    println!("[transition] Transition complete");
                }
                _ => {}
            }
            return;
        }
        self.physics.push_event(GameEvent::Custom { name, data });
    }

    fn emit_burst(&mut self, entity: EntityId, count: i64) {
        self.particles.sync.queue_burst(entity, count.max(0) as u32);
    }

    fn play_effect(&mut self, handle: u64, name: String, position: (f64, f64, f64)) {
        let pos = [position.0 as f32, position.1 as f32, position.2 as f32];
        if !self.particles.sync.spawn_effect(handle, &name, pos) {
            tracing::warn!(
                target: "script",
                "play_effect: {}",
                self.particles.sync.missing_effect_report(&name)
            );
        }
    }

    fn stop_effect(&mut self, handle: u64) {
        self.particles.sync.stop_effect(handle);
    }

    fn set_effect_param(&mut self, handle: u64, param: String, value: f64) {
        if !self.particles.sync.set_effect_param(handle, &param, value) {
            tracing::warn!(
                target: "script",
                "set_effect_param: unknown handle {} or param '{}'",
                handle,
                param
            );
        }
    }

    fn set_entity_style(&mut self, entity: EntityId, style: String) {
        let applied = self
            .style_tweens
            .set_entity_style(&mut self.world, entity, &style);
        self.style_applied(applied);
    }

    fn set_style_param(&mut self, entity: EntityId, param: String, value: toml::Value) {
        let applied = self
            .style_tweens
            .set_style_param(&mut self.world, entity, &param, value);
        self.style_applied(applied);
    }

    fn tween_style_param(
        &mut self,
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        easing: String,
    ) {
        // A tween starts from the scene's style table where an entity has
        // no value of its own (#370)
        let applied = {
            let fallback;
            let config = match self.scene_renderer.as_ref() {
                Some(r) => r.post_process_config(),
                None => {
                    fallback = flint_render::PostProcessConfig::default();
                    &fallback
                }
            };
            self.style_tweens.tween(
                &mut self.world,
                config,
                entity,
                &param,
                &target,
                seconds,
                &easing,
            )
        };
        self.style_applied(applied);
    }

    fn clear_entity_style(&mut self, entity: EntityId) {
        let applied = self
            .style_tweens
            .clear_entity_style(&mut self.world, entity);
        self.style_applied(applied);
    }

    fn set_post_style(&mut self, style: String, enabled: bool) {
        self.edit_post_styles(|config| style_commands::set_post_style(config, &style, enabled));
    }

    fn set_post_style_param(&mut self, style: String, param: String, value: toml::Value) {
        self.edit_post_styles(|config| {
            style_commands::set_post_style_param(config, &style, &param, value)
        });
    }

    fn reset_post_style(&mut self, style: String) {
        // The scene's own settings, read only for a reset
        let authored = self.scene_post_config();
        self.edit_post_styles(|config| style_commands::reset_post_style(config, &authored, &style));
    }

    fn reset_style_history(&mut self) {
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.reset_style_history();
        }
    }

    fn set_camera_cut_detection(&mut self, enabled: bool) {
        self.camera_cut.enabled = enabled;
    }

    fn set_look(&mut self, name: Option<String>) {
        if let Err(e) = PlayerApp::set_look(self, name.as_deref()) {
            tracing::warn!("set_look: {e}");
        }
    }

    fn set_camera(&mut self, camera: CameraOverrides) {
        self.script_requests.camera.merge(camera);
    }

    fn set_post_process(&mut self, post: PostProcessOverrides) {
        self.script_requests.post.merge(post);
    }

    fn set_fog_color(&mut self, color: [f32; 3]) {
        self.script_requests.fog_color = Some(color);
    }

    fn set_render_mode(&mut self, mode: Option<(u32, f32)>, params: Option<[f32; 4]>) {
        let r = &mut self.script_requests;
        r.render_mode = mode.or(r.render_mode);
        r.mode_params = params.or(r.mode_params);
    }

    fn set_lod_scales(&mut self, lod_scale: Option<f32>, draw_distance_scale: Option<f32>) {
        let r = &mut self.script_requests;
        r.lod_scale = lod_scale.or(r.lod_scale);
        r.draw_distance_scale = draw_distance_scale.or(r.draw_distance_scale);
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.script_quality = Some(settings);
    }

    fn set_time_scale(&mut self, scale: f64) {
        // This frame's clock has ticked, so the new scale starts with the
        // next frame's
        self.clock.time_scale = scale;
    }

    fn set_touch_controls(&mut self, enabled: bool) {
        self.script_requests.touch_controls = Some(enabled);
    }

    fn set_cursor_captured(&mut self, captured: bool) {
        self.script_requests.cursor_captured = Some(captured);
    }

    fn set_wooze(&mut self, knobs: toml::Value, seconds: f64) {
        self.change_wooze(|field, time| {
            let table = table_of(&knobs)?;
            field.set_global(table, time, seconds)
        });
    }

    fn set_wooze_zone(&mut self, name: String, fields: toml::Value, seconds: f64) {
        self.change_wooze(|field, time| {
            let table = table_of(&fields)?;
            field
                .set_zone(&name, table, time, seconds)
                .map_err(|e| format!("zone '{name}': {e}"))
        });
    }

    fn remove_wooze_zone(&mut self, name: String, seconds: f64) {
        self.change_wooze(|field, time| {
            if field.remove_zone(&name, time, seconds) {
                Ok(())
            } else {
                Err(format!("there's no zone named '{name}' to remove"))
            }
        });
    }

    fn tune_living(&mut self, knobs: toml::Value) {
        if let Err(problem) = self.tune_living_from_script(knobs) {
            tracing::warn!(target: "script", "tune_living: {problem}");
        }
    }

    fn clear_living(&mut self, species: String, keys: Vec<String>) {
        if let Err(problem) = self.clear_living_species(&species, &keys) {
            tracing::warn!(target: "script", "clear_living: {problem}");
        }
    }

    fn freeze_living(&mut self, frozen: bool) {
        self.living_tune.frozen = frozen;
    }

    fn step_living(&mut self) {
        PlayerApp::step_living(self);
    }

    fn save_living_preset(&mut self, name: String) {
        match PlayerApp::save_living_preset(self, &name) {
            Ok(path) => println!("[living] saved {}", path.display()),
            Err(problem) => {
                tracing::warn!(target: "script", "save_living_preset: {problem}");
            }
        }
    }

    fn load_living_preset(&mut self, name: String) {
        if let Err(problem) = PlayerApp::load_living_preset(self, &name) {
            tracing::warn!(target: "script", "load_living_preset: {problem}");
        }
    }

    fn commit_living(&mut self) {
        match PlayerApp::commit_living(self) {
            Ok(said) => println!("[living] {said}"),
            Err(problem) => tracing::warn!(target: "script", "commit_living: {problem}"),
        }
    }

    fn compare_living(&mut self, a: String, b: String) {
        if let Err(problem) = PlayerApp::compare_living(self, &a, &b) {
            tracing::warn!(target: "script", "compare_living: {problem}");
        }
    }

    fn flip_living(&mut self) {
        if !PlayerApp::flip_living(self) {
            tracing::warn!(target: "script", "flip_living: nothing is being compared");
        }
    }

    fn split_living(&mut self, split: bool) {
        if !PlayerApp::split_living(self, split) {
            tracing::warn!(target: "script", "split_living: nothing is being compared");
        }
    }

    fn stop_comparing_living(&mut self) {
        PlayerApp::stop_comparing_living(self);
    }

    fn load_scene(&mut self, path: String) {
        if self.transition_phase.is_idle() {
            println!("[transition] Starting exit transition → {}", path);
            self.transition_phase = TransitionPhase::Exiting {
                target_scene: path,
                elapsed: 0.0,
            };
        }
    }

    fn reload_scene(&mut self) {
        if self.transition_phase.is_idle() {
            let path = self.scene_path.clone();
            println!("[transition] Reloading current scene");
            self.transition_phase = TransitionPhase::Exiting {
                target_scene: path,
                elapsed: 0.0,
            };
        }
    }

    fn quit_game(&mut self) {
        PlayerApp::quit_game(self);
    }

    fn push_state(&mut self, name: String) {
        if self.state_machine.push_state(&name) {
            println!("[state] Pushed '{}'", name);
        } else {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }

    fn pop_state(&mut self) {
        if let Some(popped) = self.state_machine.pop_state() {
            println!("[state] Popped '{}'", popped.name);
        }
    }

    fn replace_state(&mut self, name: String) {
        if self.state_machine.replace_state(&name) {
            println!("[state] Replaced top with '{}'", name);
        } else {
            tracing::warn!("Unknown state template: '{}'", name);
        }
    }

    fn load_chunk(&mut self, path: String, offset_x: f64, offset_y: f64, chunk_id: String) {
        PlayerApp::load_chunk(self, &path, offset_x as f32, offset_y as f32, &chunk_id);
    }

    fn unload_chunk(&mut self, chunk_id: String) {
        PlayerApp::unload_chunk(self, &chunk_id);
    }

    fn set_velocity_2d(&mut self, entity: EntityId, vx: f64, vy: f64) {
        self.physics.set_velocity_2d(entity, vx as f32, vy as f32);
    }
}

/// Put the camera where a script's `set_camera_*` calls asked (#697), as the
/// player's frame and a headless capture both do: each field asked for, and
/// the roll, which rebuilds the up vector from the view basis each frame it
/// is asked for. With none asked the up vector is world up again (the
/// requests last a frame: without the reset a roll would stick after a
/// script stops asking, e.g. hot-reload into a compile error).
pub fn apply_camera_overrides(camera: &mut flint_render::Camera, cam: &CameraOverrides) {
    if let Some(pos) = cam.position {
        camera.position = FlintVec3::new(pos[0], pos[1], pos[2]);
    }
    if let Some(target) = cam.target {
        camera.target = FlintVec3::new(target[0], target[1], target[2]);
    }
    if let Some(fov) = cam.fov {
        camera.fov = fov;
    }
    if let Some(ortho) = cam.orthographic {
        camera.orthographic = ortho;
    }
    if let Some(height) = cam.ortho_height {
        camera.ortho_height = height;
    }
    match cam.roll {
        Some(roll) => {
            let forward = FlintVec3::new(
                camera.target.x - camera.position.x,
                camera.target.y - camera.position.y,
                camera.target.z - camera.position.z,
            )
            .normalized();
            let right = forward.cross(&FlintVec3::UP);
            let right_len = (right.dot(&right)).sqrt();
            if right_len > 1e-4 {
                let right = FlintVec3::new(
                    right.x / right_len,
                    right.y / right_len,
                    right.z / right_len,
                );
                let base_up = right.cross(&forward);
                let (sin_r, cos_r) = roll.sin_cos();
                camera.up = FlintVec3::new(
                    base_up.x * cos_r + right.x * sin_r,
                    base_up.y * cos_r + right.y * sin_r,
                    base_up.z * cos_r + right.z * sin_r,
                );
            }
            // Degenerate (looking straight up/down): keep the previous up.
        }
        None => camera.up = FlintVec3::UP,
    }
}

fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} isn't a map of knobs"))
}
