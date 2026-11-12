//! One host interface for what scripts ask (ADR 0116 decision 4, #697).
//!
//! A script's effect on anything but the world it is lent (a sound, a
//! style, a scene change, the camera...) is a [`ScriptCommand`] the call
//! queues. A host drains them ([`crate::ScriptSystem::drain_commands`]) and
//! hands them to [`apply`], which is the one `match` over `ScriptCommand`:
//! each variant becomes a call to its [`ScriptHost`] method. The player and
//! the headless runner (`flint render --scripts`) are both hosts, so they
//! read a command the same way and differ only in what they can do.
//!
//! Every method has a default that reports the command as not supported in
//! this host ([`ScriptHost::unsupported`]), so a host implements what it
//! can do and says so for the rest, rather than dropping it in silence.
//!
//! Adding a command (`crates/flint-script/CLAUDE.md` has the recipe): a
//! variant on `ScriptCommand`, the script function that queues it (in its
//! `api/` module), and a method here with its default and its arm in
//! [`apply`], which the compiler insists on. Then each host that can do it
//! overrides the method; a headless capture that can't names it on its
//! presentation-only list (`flint_player::PRESENTATION_ONLY`).

use crate::context::{CameraOverrides, LogLevel, PostProcessOverrides, ScriptCommand};
use flint_core::quality::QualitySettings;
use flint_core::EntityId;
use std::collections::BTreeSet;
use std::sync::Mutex;

/// What can carry out scripts' commands: the player, a headless capture, a
/// test. One method per [`ScriptCommand`] variant, named for the script
/// function that queues it; each default reports it unsupported.
#[allow(unused_variables)]
pub trait ScriptHost {
    /// How this host is named in what it says ("the player", "a headless
    /// capture").
    fn host_name(&self) -> &'static str {
        "this host"
    }

    /// A script asked for `command` (its script function), which this host
    /// doesn't do. The default warns once per command and host name in the
    /// process; a host with its own record (a capture's notes) overrides it.
    fn unsupported(&mut self, command: &'static str) {
        warn_once(self.host_name(), command);
    }

    // ── Sound ──
    fn play_sound(&mut self, name: String, volume: f64, pitch: f64) {
        self.unsupported("play_sound");
    }
    /// `distances` is `(min, max)` metres and `rolloff` the curve between,
    /// `None` for the engine's defaults.
    fn play_sound_at(
        &mut self,
        name: String,
        position: (f64, f64, f64),
        volume: f64,
        pitch: f64,
        distances: Option<(f32, f32)>,
        rolloff: Option<f32>,
    ) {
        self.unsupported("play_sound_at");
    }
    fn stop_sound(&mut self, name: String) {
        self.unsupported("stop_sound");
    }
    fn set_audio_lowpass(&mut self, cutoff_hz: f32) {
        self.unsupported("set_audio_lowpass");
    }

    // ── Events and the log ──
    /// A custom event for scripts' `on_event` (and the engine's own, such as
    /// `transition_complete`).
    fn fire_event(&mut self, name: String, data: toml::Value) {
        self.unsupported("fire_event");
    }
    /// A script's `log`, `log_warn` or `log_error`. Every host can: the
    /// default writes it to the log under the `script` target.
    fn log(&mut self, level: LogLevel, message: String) {
        match level {
            LogLevel::Info => tracing::info!(target: "script", "{}", message),
            LogLevel::Warn => tracing::warn!(target: "script", "{}", message),
            LogLevel::Error => tracing::error!(target: "script", "{}", message),
        }
    }

    // ── Particles ──
    fn emit_burst(&mut self, entity: EntityId, count: i64) {
        self.unsupported("emit_burst");
    }
    fn play_effect(&mut self, handle: u64, name: String, position: (f64, f64, f64)) {
        self.unsupported("play_effect");
    }
    fn stop_effect(&mut self, handle: u64) {
        self.unsupported("stop_effect");
    }
    fn set_effect_param(&mut self, handle: u64, param: String, value: f64) {
        self.unsupported("set_effect_param");
    }

    // ── Styles ──
    fn set_entity_style(&mut self, entity: EntityId, style: String) {
        self.unsupported("set_entity_style");
    }
    fn set_style_param(&mut self, entity: EntityId, param: String, value: toml::Value) {
        self.unsupported("set_style_param");
    }
    fn tween_style_param(
        &mut self,
        entity: EntityId,
        param: String,
        target: toml::Value,
        seconds: f64,
        easing: String,
    ) {
        self.unsupported("tween_style_param");
    }
    fn clear_entity_style(&mut self, entity: EntityId) {
        self.unsupported("clear_entity_style");
    }
    fn set_post_style(&mut self, style: String, enabled: bool) {
        self.unsupported("set_post_style");
    }
    fn set_post_style_param(&mut self, style: String, param: String, value: toml::Value) {
        self.unsupported("set_post_style_param");
    }
    fn reset_post_style(&mut self, style: String) {
        self.unsupported("reset_post_style");
    }
    fn reset_style_history(&mut self) {
        self.unsupported("reset_style_history");
    }
    fn set_camera_cut_detection(&mut self, enabled: bool) {
        self.unsupported("set_camera_cut_detection");
    }
    /// `set_look(name)`, or `clear_look()` with `None`.
    fn set_look(&mut self, name: Option<String>) {
        self.unsupported("set_look");
    }

    // ── The frame: camera, post-processing, render settings ──
    fn set_camera(&mut self, camera: CameraOverrides) {
        self.unsupported("set_camera_*");
    }
    fn set_post_process(&mut self, post: PostProcessOverrides) {
        self.unsupported("set_vignette and the post-process overrides");
    }
    fn set_fog_color(&mut self, color: [f32; 3]) {
        self.unsupported("set_fog_color");
    }
    fn set_render_mode(&mut self, mode: Option<(u32, f32)>, params: Option<[f32; 4]>) {
        self.unsupported("set_render_mode");
    }
    fn set_lod_scales(&mut self, lod_scale: Option<f32>, draw_distance_scale: Option<f32>) {
        self.unsupported("set_lod_scale");
    }
    fn set_quality(&mut self, settings: QualitySettings) {
        self.unsupported("set_quality");
    }
    fn set_time_scale(&mut self, scale: f64) {
        self.unsupported("set_time_scale");
    }

    // ── The window ──
    fn set_touch_controls(&mut self, enabled: bool) {
        self.unsupported("set_touch_controls");
    }
    fn set_cursor_captured(&mut self, captured: bool) {
        self.unsupported("set_cursor_captured");
    }

    // ── The wooze field and Living Props ──
    fn set_wooze(&mut self, knobs: toml::Value, seconds: f64) {
        self.unsupported("set_wooze");
    }
    fn set_wooze_zone(&mut self, name: String, fields: toml::Value, seconds: f64) {
        self.unsupported("set_wooze_zone");
    }
    fn remove_wooze_zone(&mut self, name: String, seconds: f64) {
        self.unsupported("remove_wooze_zone");
    }
    fn tune_living(&mut self, knobs: toml::Value) {
        self.unsupported("tune_living");
    }
    fn clear_living(&mut self, species: String, keys: Vec<String>) {
        self.unsupported("clear_living");
    }
    fn freeze_living(&mut self, frozen: bool) {
        self.unsupported("freeze_living");
    }
    fn step_living(&mut self) {
        self.unsupported("step_living");
    }
    fn save_living_preset(&mut self, name: String) {
        self.unsupported("save_living_preset");
    }
    fn load_living_preset(&mut self, name: String) {
        self.unsupported("load_living_preset");
    }
    fn commit_living(&mut self) {
        self.unsupported("commit_living");
    }
    fn compare_living(&mut self, a: String, b: String) {
        self.unsupported("compare_living");
    }
    fn flip_living(&mut self) {
        self.unsupported("flip_living");
    }
    fn split_living(&mut self, split: bool) {
        self.unsupported("split_living");
    }
    fn stop_comparing_living(&mut self) {
        self.unsupported("stop_comparing_living");
    }

    // ── Scenes, states, the run ──
    fn load_scene(&mut self, path: String) {
        self.unsupported("load_scene");
    }
    fn reload_scene(&mut self) {
        self.unsupported("reload_scene");
    }
    fn quit_game(&mut self) {
        self.unsupported("quit_game");
    }
    fn push_state(&mut self, name: String) {
        self.unsupported("push_state");
    }
    fn pop_state(&mut self) {
        self.unsupported("pop_state");
    }
    fn replace_state(&mut self, name: String) {
        self.unsupported("replace_state");
    }
    fn load_chunk(&mut self, path: String, offset_x: f64, offset_y: f64, chunk_id: String) {
        self.unsupported("load_chunk");
    }
    fn unload_chunk(&mut self, chunk_id: String) {
        self.unsupported("unload_chunk");
    }

    // ── Physics ──
    fn set_velocity_2d(&mut self, entity: EntityId, vx: f64, vy: f64) {
        self.unsupported("set_velocity_2d");
    }
}

/// Carry out `cmd` on `host`: the one place a `ScriptCommand` is taken
/// apart.
pub fn apply<H: ScriptHost + ?Sized>(host: &mut H, cmd: ScriptCommand) {
    use ScriptCommand as C;
    match cmd {
        C::PlaySound {
            name,
            volume,
            pitch,
        } => host.play_sound(name, volume, pitch),
        C::PlaySoundAt {
            name,
            position,
            volume,
            pitch,
            distances,
            rolloff,
        } => host.play_sound_at(name, position, volume, pitch, distances, rolloff),
        C::StopSound { name } => host.stop_sound(name),
        C::SetAudioLowpass { cutoff_hz } => host.set_audio_lowpass(cutoff_hz),
        C::FireEvent { name, data } => host.fire_event(name, data),
        C::Log { level, message } => host.log(level, message),
        C::EmitBurst { entity, count } => host.emit_burst(entity, count),
        C::PlayEffect {
            handle,
            name,
            position,
        } => host.play_effect(handle, name, position),
        C::StopEffect { handle } => host.stop_effect(handle),
        C::SetEffectParam {
            handle,
            param,
            value,
        } => host.set_effect_param(handle, param, value),
        C::SetEntityStyle { entity, style } => host.set_entity_style(entity, style),
        C::SetStyleParam {
            entity,
            param,
            value,
        } => host.set_style_param(entity, param, value),
        C::TweenStyleParam {
            entity,
            param,
            target,
            seconds,
            easing,
        } => host.tween_style_param(entity, param, target, seconds, easing),
        C::ClearEntityStyle { entity } => host.clear_entity_style(entity),
        C::SetPostStyle { style, enabled } => host.set_post_style(style, enabled),
        C::SetPostStyleParam {
            style,
            param,
            value,
        } => host.set_post_style_param(style, param, value),
        C::ResetPostStyle { style } => host.reset_post_style(style),
        C::ResetStyleHistory => host.reset_style_history(),
        C::SetCameraCutDetection { enabled } => host.set_camera_cut_detection(enabled),
        C::SetLook { name } => host.set_look(name),
        C::SetCamera { camera } => host.set_camera(camera),
        C::SetPostProcess { post } => host.set_post_process(post),
        C::SetFogColor { color } => host.set_fog_color(color),
        C::SetRenderMode { mode, params } => host.set_render_mode(mode, params),
        C::SetLodScales {
            lod_scale,
            draw_distance_scale,
        } => host.set_lod_scales(lod_scale, draw_distance_scale),
        C::SetQuality { settings } => host.set_quality(settings),
        C::SetTimeScale { scale } => host.set_time_scale(scale),
        C::SetTouchControls { enabled } => host.set_touch_controls(enabled),
        C::SetCursorCaptured { captured } => host.set_cursor_captured(captured),
        C::SetWooze { knobs, seconds } => host.set_wooze(knobs, seconds),
        C::SetWoozeZone {
            name,
            fields,
            seconds,
        } => host.set_wooze_zone(name, fields, seconds),
        C::RemoveWoozeZone { name, seconds } => host.remove_wooze_zone(name, seconds),
        C::TuneLiving { knobs } => host.tune_living(knobs),
        C::ClearLiving { species, keys } => host.clear_living(species, keys),
        C::FreezeLiving { frozen } => host.freeze_living(frozen),
        C::StepLiving => host.step_living(),
        C::SaveLivingPreset { name } => host.save_living_preset(name),
        C::LoadLivingPreset { name } => host.load_living_preset(name),
        C::CommitLiving => host.commit_living(),
        C::CompareLiving { a, b } => host.compare_living(a, b),
        C::FlipLiving => host.flip_living(),
        C::SplitLiving { split } => host.split_living(split),
        C::StopComparingLiving => host.stop_comparing_living(),
        C::LoadScene { path } => host.load_scene(path),
        C::ReloadScene => host.reload_scene(),
        C::QuitGame => host.quit_game(),
        C::PushState { name } => host.push_state(name),
        C::PopState => host.pop_state(),
        C::ReplaceState { name } => host.replace_state(name),
        C::LoadChunk {
            path,
            offset_x,
            offset_y,
            chunk_id,
        } => host.load_chunk(path, offset_x, offset_y, chunk_id),
        C::UnloadChunk { chunk_id } => host.unload_chunk(chunk_id),
        C::SetVelocity2D { entity, vx, vy } => host.set_velocity_2d(entity, vx, vy),
    }
}

/// [`apply`] each of `commands`, in order.
pub fn apply_all<H: ScriptHost + ?Sized>(
    host: &mut H,
    commands: impl IntoIterator<Item = ScriptCommand>,
) {
    for cmd in commands {
        apply(host, cmd);
    }
}

/// The (host, command) pairs that have warned, so each warns once.
static WARNED: Mutex<BTreeSet<(&'static str, &'static str)>> = Mutex::new(BTreeSet::new());

fn warn_once(host: &'static str, command: &'static str) {
    let first = crate::lock_or_recover(&WARNED).insert((host, command));
    if first {
        tracing::warn!(
            target: "script",
            "{command}: a script asked for it, and {host} doesn't do it (warned once)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host that does nothing but count what it was asked to do and what
    /// it said it couldn't.
    #[derive(Default)]
    struct Counting {
        sounds: Vec<String>,
        unsupported: Vec<&'static str>,
    }

    impl ScriptHost for Counting {
        fn unsupported(&mut self, command: &'static str) {
            self.unsupported.push(command);
        }
        fn play_sound(&mut self, name: String, _volume: f64, _pitch: f64) {
            self.sounds.push(name);
        }
    }

    #[test]
    fn a_command_reaches_its_method_and_the_rest_report_unsupported() {
        let mut host = Counting::default();
        apply_all(
            &mut host,
            [
                ScriptCommand::PlaySound {
                    name: "ding".into(),
                    volume: 1.0,
                    pitch: 1.0,
                },
                ScriptCommand::QuitGame,
                ScriptCommand::SetTimeScale { scale: 0.5 },
                ScriptCommand::Log {
                    level: LogLevel::Info,
                    message: "logged by the default".into(),
                },
            ],
        );
        assert_eq!(host.sounds, ["ding"]);
        assert_eq!(host.unsupported, ["quit_game", "set_time_scale"]);
    }

    #[test]
    fn frame_requests_come_out_of_the_same_drain_after_the_queue() {
        use crate::context::ScriptCallContext;
        let mut c = ScriptCallContext::new();
        c.commands.push(ScriptCommand::QuitGame);
        c.requests.camera.fov = Some(50.0);
        c.requests.post.vignette = Some(0.3);
        c.requests.time_scale = Some(0.25);
        c.requests.lod_scale = Some(0.5);
        let cmds = c.take_commands();
        let names: Vec<String> = cmds
            .iter()
            .map(|c| {
                format!("{c:?}")
                    .split([' ', '{'])
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            [
                "QuitGame",
                "SetCamera",
                "SetPostProcess",
                "SetTimeScale",
                "SetLodScales"
            ]
        );
        // `get_lod_scale()` reads the scale asked for until the host says
        assert_eq!(c.lod_scale, 0.5);
        // and nothing is asked twice
        assert!(c.take_commands().is_empty());
    }
}
