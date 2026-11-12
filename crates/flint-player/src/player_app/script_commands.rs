//! The player as a script host (`impl ScriptHost for PlayerApp`, #697) and
//! the PlayerApp-side music-session entry points — code-motion sibling of `mod.rs` (player_app decomposition; see
//! the decomposition ADR). `music_session.rs` (the session integration
//! itself) is deliberately separate and unchanged.

#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
use super::music_session;
use super::scene_loading;
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::PlayerApp;
use super::TransitionPhase;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::quality::QualitySettings;
use flint_core::Vec3 as FlintVec3;
use flint_runtime::pad::Gilrs;
use flint_script::{CameraOverrides, ScriptHost};

impl PlayerApp {
    /// A script's `quit_game()` (#659): the run ends as closing the window
    /// ends it. Natively the event loop exits after this frame's redraw, and
    /// `exiting` flushes the persistent store, so the code is 0; a run with
    /// `--exit-after` prints its report first, as it would at its time. A
    /// page has no window to close: it lets go of the pointer and full
    /// screen and goes back to its doorway (`web_page::quit`).
    pub(super) fn quit_game(&mut self) {
        if self.outcome.exit_requested {
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
            self.outcome.exit_requested = true;
            if self.capture.exit_after.is_some() {
                self.print_exit_report();
            }
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
                .core
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
        if !self.audio.start(&self.core.world) {
            return false;
        }
        if !had_sound && self.music.session.is_none() {
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
        for entity in self.core.world.all_entities() {
            let comp = self
                .core
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
            self.core
                .project
                .root_holding("music_session", manifest.trim(), &self.scene_dir());
        match music_session::MusicSession::start(&base_dir, &comp, &mut self.audio.engine) {
            Ok(Some(session)) => {
                self.music.session = Some(session);
                // The scene's authored post values are the ladder merge's
                // base and the post-teardown restore target (ADR 0021).
                let authored = self
                    .scene
                    .post_process
                    .as_ref()
                    .map(scene_loading::post_process_config_from_def)
                    .unwrap_or_default();
                self.music.pp_base = Some(music_session::LadderPostBase {
                    radial_blur: authored.radial_blur,
                    chromatic_aberration: authored.chromatic_aberration,
                    desaturate: authored.desaturate,
                });
                // The capture thread's rumble engine owns the motors now.
                self.pad_rumble.stop_all();
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
                        if let Some(ms) = &self.music.session {
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
        if let Some(session) = self.music.session.take() {
            session.stop();
            // Queue the one-shot authored-values restore for the next merge
            // pass (setting overrides here would be wiped by the drain).
            self.music.pp_restore = self.music.pp_base.take();
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

/// The player's part of the script commands (#697): the presentation's
/// (`game_core::for_the_host`), which `game_core::dispatch` hands here.
/// Every other command is the game core's, the same for every host.
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

    fn fire_event(&mut self, name: String, _data: toml::Value) {
        // The one event the core hands on: a transition's script saying
        // it's done (`game_core::for_the_host`)
        debug_assert_eq!(name, TRANSITION_COMPLETE);
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
    }

    fn reset_post_style(&mut self, style: String) {
        // The scene's own settings, read only for a reset
        let authored = self.scene_post_config();
        super::game_core::reset_post_style(
            &mut self.core,
            self.scene_renderer.as_mut(),
            &authored,
            &style,
        );
    }

    fn set_camera_cut_detection(&mut self, enabled: bool) {
        self.style.camera_cut.enabled = enabled;
    }

    fn rumble(&mut self, pad: i64, strength: f64, seconds: f64) {
        // No backend (headless build, a failed `Gilrs::new()`), or a music
        // session has the pads: nothing to rumble, as with no pad (#748).
        let Some(gilrs) = &mut self.gilrs else {
            return;
        };
        let target = flint_runtime::PadTarget::from_script(pad);
        let request = flint_runtime::RumbleRequest::new(target, strength, seconds);
        if let Err(e) = self
            .pad_rumble
            .rumble(gilrs, request, web_time::Instant::now())
        {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| tracing::warn!("rumble: {e} (said once)"));
        }
    }

    fn set_look(&mut self, name: Option<String>) {
        if let Err(e) = PlayerApp::set_look(self, name.as_deref()) {
            tracing::warn!("set_look: {e}");
        }
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.quality.script_request = Some(settings);
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
            let path = self.core.scene_path.clone();
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

    fn load_chunk(&mut self, path: String, offset_x: f64, offset_y: f64, chunk_id: String) {
        PlayerApp::load_chunk(self, &path, offset_x as f32, offset_y as f32, &chunk_id);
    }

    fn unload_chunk(&mut self, chunk_id: String) {
        PlayerApp::unload_chunk(self, &chunk_id);
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

pub(super) fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} isn't a map of knobs"))
}

/// A script's `rumble()` in the player, with no music session (#748).
#[cfg(test)]
mod rumble_tests {
    use super::PlayerApp;
    use flint_ecs::FlintWorld;
    use flint_runtime::pad::Gilrs;
    use flint_script::context::ScriptCommand;

    fn buzz() -> ScriptCommand {
        ScriptCommand::Rumble {
            pad: -1,
            strength: 0.7,
            seconds: 0.3,
        }
    }

    #[test]
    fn a_rumble_with_no_backend_or_no_pad_does_nothing() {
        let mut app = PlayerApp::new(
            FlintWorld::new(),
            "rumble_test.scene.toml".into(),
            false,
            None,
            None,
        );
        // Before the window (or in a build without `gamepad`) there's no
        // backend: the command lands and nothing rumbles.
        assert!(app.gilrs.is_none());
        flint_script::host::apply(&mut app, buzz());
        assert_eq!(app.pad_rumble.playing(), 0);

        // A backend with no pad (CI, this machine without a controller).
        app.gilrs = Gilrs::new().ok();
        let no_pad = app
            .gilrs
            .as_ref()
            .is_some_and(|g| g.gamepads().next().is_none());
        flint_script::host::apply(&mut app, buzz());
        if no_pad {
            assert_eq!(app.pad_rumble.playing(), 0);
        }
        app.poll_gamepad_events();
    }
}
