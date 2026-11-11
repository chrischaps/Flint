//! Script-command processing and the PlayerApp-side music-session entry
//! points — code-motion sibling of `mod.rs` (player_app decomposition; see
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
use flint_core::Vec3 as FlintVec3;
use flint_input_capture::pad::Gilrs;
use flint_runtime::GameEvent;
use flint_script::context::{LogLevel, ScriptCommand};
use std::path::Path;

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

    pub(super) fn process_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            if style_commands::is_post_style_command(&cmd) {
                self.apply_post_style(&cmd);
                continue;
            }
            // Style commands, and tweens (#370), which start from the
            // scene's style table where an entity has no value of its own
            let applied = {
                let fallback;
                let config = match self.scene_renderer.as_ref() {
                    Some(r) => r.post_process_config(),
                    None => {
                        fallback = flint_render::PostProcessConfig::default();
                        &fallback
                    }
                };
                self.style_tweens.apply(&mut self.world, config, &cmd)
            };
            match applied {
                StyleApplied::NotAStyleCommand => {}
                StyleApplied::Done(event) => {
                    if let Some(event) = event {
                        self.physics.push_event(event);
                    }
                    continue;
                }
                StyleApplied::Failed(message) => {
                    tracing::warn!(target: "script", "{message}");
                    continue;
                }
            }
            match cmd {
                // Applied above
                ScriptCommand::SetEntityStyle { .. }
                | ScriptCommand::SetStyleParam { .. }
                | ScriptCommand::TweenStyleParam { .. }
                | ScriptCommand::ClearEntityStyle { .. }
                | ScriptCommand::SetPostStyle { .. }
                | ScriptCommand::SetPostStyleParam { .. }
                | ScriptCommand::ResetPostStyle { .. } => {}
                ScriptCommand::PlaySound {
                    name,
                    volume,
                    pitch,
                } => {
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
                ScriptCommand::PlaySoundAt {
                    name,
                    position,
                    volume,
                    pitch,
                    distances,
                    rolloff,
                } => {
                    let pos =
                        FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
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
                ScriptCommand::StopSound { name: _ } => {
                    // One-shot sounds play to completion (same as AudioCommand::Stop)
                }
                ScriptCommand::FireEvent { name, data } => {
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
                        continue;
                    }
                    self.physics.push_event(GameEvent::Custom { name, data });
                }
                ScriptCommand::Log { level, message } => match level {
                    LogLevel::Info => tracing::info!(target: "script", "{}", message),
                    LogLevel::Warn => tracing::warn!(target: "script", "{}", message),
                    LogLevel::Error => tracing::error!(target: "script", "{}", message),
                },
                ScriptCommand::EmitBurst { entity_id, count } => {
                    let eid = flint_core::EntityId(entity_id as u64);
                    self.particles.sync.queue_burst(eid, count.max(0) as u32);
                }
                ScriptCommand::PlayEffect {
                    handle,
                    name,
                    position,
                } => {
                    let pos = [position.0 as f32, position.1 as f32, position.2 as f32];
                    if !self.particles.sync.spawn_effect(handle, &name, pos) {
                        tracing::warn!(
                            target: "script",
                            "play_effect: {}",
                            self.particles.sync.missing_effect_report(&name)
                        );
                    }
                }
                ScriptCommand::StopEffect { handle } => {
                    self.particles.sync.stop_effect(handle);
                }
                ScriptCommand::SetEffectParam {
                    handle,
                    param,
                    value,
                } => {
                    if !self.particles.sync.set_effect_param(handle, &param, value) {
                        tracing::warn!(
                            target: "script",
                            "set_effect_param: unknown handle {} or param '{}'",
                            handle,
                            param
                        );
                    }
                }
                ScriptCommand::SetWooze { knobs, seconds } => {
                    self.change_wooze(|field, time| {
                        let table = table_of(&knobs)?;
                        field.set_global(table, time, seconds)
                    });
                }
                ScriptCommand::SetWoozeZone {
                    name,
                    fields,
                    seconds,
                } => {
                    self.change_wooze(|field, time| {
                        let table = table_of(&fields)?;
                        field
                            .set_zone(&name, table, time, seconds)
                            .map_err(|e| format!("zone '{name}': {e}"))
                    });
                }
                ScriptCommand::RemoveWoozeZone { name, seconds } => {
                    self.change_wooze(|field, time| {
                        if field.remove_zone(&name, time, seconds) {
                            Ok(())
                        } else {
                            Err(format!("there's no zone named '{name}' to remove"))
                        }
                    });
                }
                ScriptCommand::TuneLiving { knobs } => {
                    if let Err(problem) = self.tune_living_from_script(knobs) {
                        tracing::warn!(target: "script", "tune_living: {problem}");
                    }
                }
                ScriptCommand::ClearLiving { species, keys } => {
                    if let Err(problem) = self.clear_living_species(&species, &keys) {
                        tracing::warn!(target: "script", "clear_living: {problem}");
                    }
                }
                ScriptCommand::FreezeLiving { frozen } => self.living_tune.frozen = frozen,
                ScriptCommand::StepLiving => self.step_living(),
                ScriptCommand::SaveLivingPreset { name } => match self.save_living_preset(&name) {
                    Ok(path) => println!("[living] saved {}", path.display()),
                    Err(problem) => {
                        tracing::warn!(target: "script", "save_living_preset: {problem}");
                    }
                },
                ScriptCommand::CommitLiving => match self.commit_living() {
                    Ok(said) => println!("[living] {said}"),
                    Err(problem) => tracing::warn!(target: "script", "commit_living: {problem}"),
                },
                ScriptCommand::LoadLivingPreset { name } => {
                    if let Err(problem) = self.load_living_preset(&name) {
                        tracing::warn!(target: "script", "load_living_preset: {problem}");
                    }
                }
                ScriptCommand::CompareLiving { a, b } => {
                    if let Err(problem) = self.compare_living(&a, &b) {
                        tracing::warn!(target: "script", "compare_living: {problem}");
                    }
                }
                ScriptCommand::FlipLiving => {
                    if !self.flip_living() {
                        tracing::warn!(target: "script", "flip_living: nothing is being compared");
                    }
                }
                ScriptCommand::SplitLiving { split } => {
                    if !self.split_living(split) {
                        tracing::warn!(target: "script", "split_living: nothing is being compared");
                    }
                }
                ScriptCommand::StopComparingLiving => self.stop_comparing_living(),
                ScriptCommand::LoadScene { path } => {
                    if self.transition_phase.is_idle() {
                        println!("[transition] Starting exit transition → {}", path);
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::ReloadScene => {
                    if self.transition_phase.is_idle() {
                        let path = self.scene_path.clone();
                        println!("[transition] Reloading current scene");
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::QuitGame => self.quit_game(),
                ScriptCommand::PushState { name } => {
                    if self.state_machine.push_state(&name) {
                        println!("[state] Pushed '{}'", name);
                    } else {
                        tracing::warn!("Unknown state template: '{}'", name);
                    }
                }
                ScriptCommand::PopState => {
                    if let Some(popped) = self.state_machine.pop_state() {
                        println!("[state] Popped '{}'", popped.name);
                    }
                }
                ScriptCommand::ReplaceState { name } => {
                    if self.state_machine.replace_state(&name) {
                        println!("[state] Replaced top with '{}'", name);
                    } else {
                        tracing::warn!("Unknown state template: '{}'", name);
                    }
                }
                ScriptCommand::SetVelocity2D { entity_id, vx, vy } => {
                    let eid = flint_core::EntityId::from_raw(entity_id as u64);
                    self.physics.set_velocity_2d(eid, vx as f32, vy as f32);
                }
                ScriptCommand::LoadChunk {
                    path,
                    offset_x,
                    offset_y,
                    chunk_id,
                } => {
                    self.load_chunk(&path, offset_x as f32, offset_y as f32, &chunk_id);
                }
                ScriptCommand::UnloadChunk { chunk_id } => {
                    self.unload_chunk(&chunk_id);
                }
                ScriptCommand::ResetStyleHistory => {
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.reset_style_history();
                    }
                }
                ScriptCommand::SetCameraCutDetection { enabled } => {
                    self.camera_cut.enabled = enabled;
                }
                ScriptCommand::SetLook { name } => {
                    if let Err(e) = self.set_look(name.as_deref()) {
                        tracing::warn!("set_look: {e}");
                    }
                }
            }
        }
    }

    /// A whole-frame style command (#369): edit the renderer's config and
    /// hand it back; the next frame builds a style newly switched on.
    /// Without a renderer there is no frame to style.
    fn apply_post_style(&mut self, cmd: &ScriptCommand) {
        // The scene's own settings, read only for a reset
        let authored = matches!(cmd, ScriptCommand::ResetPostStyle { .. })
            .then(|| self.scene_post_config().unwrap_or_default());
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };
        let mut config = renderer.post_process_config().clone();
        match style_commands::apply_post_style_command(
            &mut config,
            || authored.unwrap_or_default(),
            cmd,
        ) {
            StyleApplied::Done(event) => {
                renderer.set_post_process_config(config);
                if let Some(event) = event {
                    self.physics.push_event(event);
                }
            }
            StyleApplied::Failed(message) => tracing::warn!(target: "script", "{message}"),
            StyleApplied::NotAStyleCommand => {}
        }
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
        let Some(base_dir) = Path::new(&self.scene_path)
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
        else {
            tracing::warn!(
                "music_session: cannot derive base dir from scene path `{}`",
                self.scene_path
            );
            return;
        };
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

fn table_of(value: &toml::Value) -> Result<&toml::Table, String> {
    value
        .as_table()
        .ok_or_else(|| format!("{value} isn't a map of knobs"))
}
