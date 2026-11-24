//! The player as a script host (`impl ScriptHost for PlayerApp`, #697), and
//! the start of sound and quitting.

use super::PlayerApp;
use super::TransitionPhase;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::quality::QualitySettings;
use flint_core::Vec3 as FlintVec3;
use flint_script::{CameraOverrides, ScriptHost, SoundPlay, SoundRef};
use std::time::Duration;

/// A script's sound as the engine plays it, on the bus it names: any the
/// game declares (#730); an unknown one is warned about once and plays on
/// sfx.
fn keyed(
    engine: &mut flint_audio::engine::AudioEngine,
    play: &SoundPlay,
) -> flint_audio::engine::KeyedPlay {
    flint_audio::engine::KeyedPlay {
        volume: play.volume,
        pitch: play.pitch,
        looping: play.looping,
        bus: engine.bus_or_sfx(&play.bus),
    }
}

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
            tracing::info!(target: "script", "a script asked to quit");
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
    /// scene start, and a subsystem that waited for a device (the scene's
    /// music) starts at the next frame's first stage, in the browser's
    /// worklet like every other sound (#1216). Says whether there is sound.
    pub fn start_audio(&mut self) -> bool {
        self.audio.start(&self.core.world)
    }
}

/// The player's part of the script commands (#697): the presentation's
/// (`game_core::for_the_host`), which `game_core::dispatch` hands here.
/// Every other command is the game core's, the same for every host.
impl ScriptHost for PlayerApp {
    fn host_name(&self) -> &'static str {
        "the player"
    }

    // A script's sounds are the engine's keyed ones (#699): the handle
    // `play_sound` returned is the key `stop_sound` and `set_sound_volume`
    // find it by, on kira here or in the browser's worklet alike.
    fn play_sound(&mut self, handle: u64, name: String, play: SoundPlay) {
        if self.audio.engine.is_available() {
            let play = keyed(&mut self.audio.engine, &play);
            if let Err(e) = self.audio.engine.play_keyed(handle, &name, play) {
                tracing::warn!(target: "script", "play_sound error: {:?}", e);
            }
        }
    }

    fn play_sound_at(
        &mut self,
        handle: u64,
        name: String,
        position: (f64, f64, f64),
        play: SoundPlay,
        distances: Option<(f32, f32)>,
        rolloff: Option<f32>,
    ) {
        // As `play_sound`: with no device there's nothing to say (#714
        // found it warning on every call in a silent run)
        if !self.audio.engine.is_available() {
            return;
        }
        let pos = FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
        let play = keyed(&mut self.audio.engine, &play);
        if let Err(e) = self.audio.engine.play_keyed_at(
            handle,
            &name,
            pos,
            distances.unwrap_or(flint_audio::engine::DEFAULT_DISTANCES),
            rolloff.unwrap_or(flint_audio::engine::DEFAULT_ROLLOFF),
            play,
        ) {
            tracing::warn!(target: "script", "play_sound_at error: {:?}", e);
        }
    }

    fn stop_sound(&mut self, sound: SoundRef, fade: f64) {
        let fade = Duration::from_secs_f64(fade.max(0.0));
        match sound {
            SoundRef::Handle(h) => {
                self.audio.engine.stop_keyed(h, fade);
            }
            SoundRef::Name(name) => {
                self.audio.engine.stop_keyed_named(&name, fade);
            }
        }
    }

    fn set_sound_volume(&mut self, handle: u64, volume: f64, fade: f64) {
        let fade = Duration::from_secs_f64(fade.max(0.0));
        self.audio.engine.set_keyed_volume(handle, volume, fade);
    }

    // Kept while silent too, so the mixer opens at it (the web's Begin). A
    // name that is no bus is warned about once, by the engine.
    fn set_bus_volume(&mut self, bus: String, volume: f64) {
        let _ = self.audio.engine.set_bus_volume_named(&bus, volume);
    }

    // The player's own, apart from the game's and saved (#1217, user_audio.rs)
    fn set_user_bus_volume(&mut self, bus: String, volume: f64) {
        PlayerApp::set_user_bus_volume(self, &bus, volume);
    }

    fn set_bus_lowpass(&mut self, bus: String, hz: f64) {
        let _ = self.audio.engine.set_bus_lowpass_named(&bus, hz);
    }

    fn fire_event(&mut self, name: String, data: toml::Value) {
        // The core hands on only a transition's script saying it's done
        // (`game_core::for_the_host`); any other event called here goes on
        // the frame's bus, as the core's dispatcher puts it
        if name != TRANSITION_COMPLETE {
            self.core
                .events
                .push(flint_runtime::GameEvent::Custom { name, data });
            return;
        }
        match &self.transition_phase {
            TransitionPhase::Exiting { target_scene, .. } => {
                let target = target_scene.clone();
                // The script says its cover is drawn (#1125)
                self.transition_phase = TransitionPhase::Loading {
                    target_scene: target,
                    covered: true,
                };
            }
            TransitionPhase::Entering { .. } => {
                self.transition_phase = TransitionPhase::Idle;
                tracing::info!(target: "script", "Transition complete");
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
        // No backend (headless build, a failed `Gilrs::new()`), or a
        // subsystem holds the pads (the rhythm session's capture): nothing
        // to rumble, as with no pad (#748).
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

    fn set_look(&mut self, name: Option<String>, seconds: f64) {
        if let Err(e) = PlayerApp::fade_to_look(self, name.as_deref(), seconds) {
            tracing::warn!("set_look: {e}");
        }
    }

    fn prepare_look(&mut self, name: String) {
        if let Err(e) = PlayerApp::prepare_look(self, &name) {
            tracing::warn!("prepare_look: {e}");
        }
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.quality.script_request = Some(settings);
    }

    fn load_scene(&mut self, path: String) {
        if self.transition_phase.is_idle() {
            tracing::info!(target: "script", "Starting exit transition → {}", path);
            self.transition_phase = TransitionPhase::Exiting {
                target_scene: path,
                elapsed: 0.0,
            };
        }
    }

    fn reload_scene(&mut self) {
        if self.transition_phase.is_idle() {
            let path = self.core.scene_path.clone();
            tracing::info!(target: "script", "Reloading current scene");
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

/// A script's `rumble()` in the player, with the pads its own (#748).
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
