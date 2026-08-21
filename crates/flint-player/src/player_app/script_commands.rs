//! Script-command processing and the PlayerApp-side music-session entry
//! points — code-motion sibling of `mod.rs` (player_app decomposition; see
//! the decomposition ADR). `music_session.rs` (the session integration
//! itself) is deliberately separate and unchanged.

use super::PlayerApp;
#[cfg(feature = "debug-hud")]
use super::music_guide_panel;
use super::music_session;
#[cfg(feature = "debug-hud")]
use super::timeline_panel;
use super::TransitionPhase;
use super::scene_loading;
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::Vec3 as FlintVec3;
use flint_runtime::GameEvent;
use flint_script::context::{LogLevel, ScriptCommand};
use gilrs::Gilrs;
use std::path::Path;

impl PlayerApp {
    pub(super) fn process_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            match cmd {
                ScriptCommand::PlaySound { name, volume } => {
                    if self.audio.engine.is_available() {
                        if let Err(e) = self.audio.engine.play_non_spatial(
                            &name,
                            volume,
                            1.0,
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
                } => {
                    let pos =
                        FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
                    if let Err(e) = self
                        .audio
                        .engine
                        .play_at_position(&name, pos, volume, pitch)
                    {
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
                    self.particles.sync.queue_burst(eid, count as u32);
                }
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
            }
        }
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
