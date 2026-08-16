//! The music-session lifecycle inside the player app (Phase 4 F3, ADR 0019).
//!
//! A scene declares its suite with a `music_session` component
//! (`schemas/components/music_session.toml`, game-side); the player starts a
//! [`ChartSession`] on the shared kira `AudioManager` (ADR 0017), hands the
//! gamepad to the 1 kHz capture thread for the session's duration, and
//! down-samples the same capture drain into `InputState` so the script/action
//! surface keeps working frame-quantized (ADR 0018). Headless (no audio
//! device) degrades to no-session, matching the engine's Option-audio
//! convention.
//!
//! While a session is active only lean + pulse reach `InputState` — every
//! other pad control is dropped inside the capture crate (accepted gap,
//! ADR 0018). Keyboard stays winit throughout.

use anyhow::{anyhow, Result};
use flint_audio::engine::AudioEngine;
use flint_input_capture::CaptureConfig;
use flint_music::chart_session::{
    judgment_offset_samples, latest_calibration_ms, latest_latency_ms, parse_lean_mode,
    ChartSession, ChartSessionConfig,
};
use flint_music::{InputEvent, Tick, VisualFrame};
use flint_runtime::InputState;
use std::path::Path;
use std::time::Instant;

/// Component name in scene TOML (schema: `schemas/components/music_session.toml`).
pub const MUSIC_SESSION: &str = "music_session";

/// Gamepad slot the down-sampled events claim in `InputState`.
const GAMEPAD_SLOT: u32 = 0;
/// The pulse verb's button name in `InputState`'s gamepad vocabulary.
const PULSE_BUTTON: &str = "South";
/// Teardown fade: long enough to avoid a click, short enough to feel like
/// the world simply lets go.
const STOP_FADE_MS: f64 = 50.0;

/// Ladder visuals → engine post params (ADR 0021; F4/F6 move authorship into
/// scripts via conducted params, superseding this direct path script-wins).
/// The scales are provisional, owned by the Phase 4 [H] look-check: the
/// ladder's `blur` becomes zoom-style radial blur (world-losing-focus),
/// `chromatic` and `desaturate` map 1:1 (rung values are authored 0..1 and
/// the composite shader's ash-grey formula does its own darkening).
pub const MUSIC_BLUR_SCALE: f32 = 0.6;
pub const MUSIC_CHROMATIC_SCALE: f32 = 1.0;
pub const MUSIC_DESATURATE_SCALE: f32 = 1.0;

/// The scene's authored values for the three ladder-driven post params,
/// captured at session start. The merge writes `base + rung × scale` every
/// frame (not only when a rung is active) so a ladder that recovers to clean
/// actually restores the authored look — the renderer config is sticky, and
/// an only-when-degraded write would leave the last degraded values applied
/// forever (ADR 0021).
#[derive(Debug, Clone, Copy, Default)]
pub struct LadderPostBase {
    pub radial_blur: f32,
    pub chromatic_aberration: f32,
    pub desaturate: f32,
}

/// Merge the ladder visuals into this frame's post overrides. Script
/// overrides win (only `None` slots are filled); preroll leaves the world
/// untouched.
pub fn merge_ladder_postprocess(
    radial_blur: &mut Option<f32>,
    chromatic_aberration: &mut Option<f32>,
    desaturation: &mut Option<f32>,
    vf: &VisualFrame,
    base: LadderPostBase,
) {
    if vf.preroll {
        return;
    }
    if radial_blur.is_none() {
        *radial_blur = Some(base.radial_blur + vf.blur * MUSIC_BLUR_SCALE);
    }
    if chromatic_aberration.is_none() {
        *chromatic_aberration =
            Some(base.chromatic_aberration + vf.chromatic * MUSIC_CHROMATIC_SCALE);
    }
    if desaturation.is_none() {
        *desaturation = Some((base.desaturate + vf.desaturate * MUSIC_DESATURATE_SCALE).min(1.0));
    }
}

/// One-shot restore of the authored values after session teardown, so the
/// config does not end stuck mid-ladder. Script overrides still win.
pub fn restore_ladder_postprocess(
    radial_blur: &mut Option<f32>,
    chromatic_aberration: &mut Option<f32>,
    desaturation: &mut Option<f32>,
    base: LadderPostBase,
) {
    if radial_blur.is_none() {
        *radial_blur = Some(base.radial_blur);
    }
    if chromatic_aberration.is_none() {
        *chromatic_aberration = Some(base.chromatic_aberration);
    }
    if desaturation.is_none() {
        *desaturation = Some(base.desaturate);
    }
}

/// One scene's music session: the reactive loop on the shared manager plus
/// the InputState down-sampling state and permanent, near-free evidence
/// counters (the F2 pass criteria, re-checked on every real run).
///
/// `ChartSession`'s internal field order drops the capture guard before the
/// kira handles; `PlayerApp`'s field order drops those handles before the
/// shared manager (ADR 0017).
pub struct MusicSession {
    session: ChartSession,
    /// A down-sampled pulse press is released at the start of the *next*
    /// tick so action edge detection sees a full down/up pair (ADR 0018).
    pulse_release_pending: bool,
    events_total: u64,
    monotonic_violations: u64,
    last_event_sample: i64,
    /// Max stem skew that survived the ADR 0017 re-read discriminator
    /// (a one-callback-buffer reporting transient collapses on re-read).
    max_confirmed_skew_ms: f64,
    last_skew_check: Instant,
}

/// A string field of the component; empty/whitespace counts as absent.
fn comp_str(comp: &toml::Value, key: &str) -> Option<String> {
    comp.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

impl MusicSession {
    /// Parse the component, start the suite on the player's shared manager,
    /// and spawn capture. `Ok(None)` when no audio device exists (headless —
    /// no session, everything else runs).
    pub fn start(
        base_dir: &Path,
        comp: &toml::Value,
        audio: &mut AudioEngine,
    ) -> Result<Option<Self>> {
        let manifest = comp_str(comp, "manifest")
            .ok_or_else(|| anyhow!("music_session component: `manifest` is required"))?;
        let chart = comp_str(comp, "chart")
            .ok_or_else(|| anyhow!("music_session component: `chart` is required"))?;
        // Declared-for-later field (F7): parsed so a populated field is
        // never a silent lie, but this lifecycle does not read it yet.
        if let Some(t) = comp_str(comp, "tuning_config") {
            tracing::debug!("music_session: tuning_config `{t}` declared; not read until F7");
        }
        // `bindings` is declarative (ADR 0020): scripts load through the one
        // script-discovery owner — a `script` component on a scene entity —
        // never through this field. Validate it so a typo is loud.
        if let Some(b) = comp_str(comp, "bindings") {
            let path = base_dir.join(&b);
            if path.exists() {
                tracing::info!(
                    "music_session: bindings `{b}` present (load it via a `script` component)"
                );
            } else {
                tracing::warn!(
                    "music_session: bindings `{b}` does not exist at {} — this field is \
                     declarative; the script loads via a `script` component on a scene entity",
                    path.display()
                );
            }
        }
        let lean_mode = comp_str(comp, "lean_mode").unwrap_or_else(|| "arrival".to_string());
        let lean_mode =
            parse_lean_mode(&lean_mode).map_err(|e| anyhow!("music_session component: {e}"))?;
        // Coercion helper because TOML `16` and `16.0` are different types.
        let bars = comp
            .get("bars")
            .and_then(flint_core::toml_util::toml_f64)
            .map(|b| b.max(0.0).round() as u64)
            .filter(|&b| b > 0);

        // Timing offsets from the committed logs in <base_dir>/logs/latency/.
        let latency_ms = match latest_latency_ms(base_dir) {
            Some((file, ms)) => {
                println!("[music] measured output latency: {ms:.1} ms (from {file})");
                Some(ms)
            }
            None => {
                println!("[music] measured output latency: NONE ON RECORD");
                None
            }
        };
        let calibration_ms = match latest_calibration_ms(base_dir) {
            Some((file, ms)) => {
                println!("[music] tap calibration: {ms:+.1} ms (from {file})");
                ms
            }
            None => {
                println!("[music] tap calibration: none on record");
                0.0
            }
        };

        let cfg = ChartSessionConfig {
            manifest: base_dir.join(&manifest),
            chart: base_dir.join(&chart),
            base_dir: base_dir.to_path_buf(),
            coherence_config: comp_str(comp, "coherence_config").map(|p| base_dir.join(p)),
            ladder_config: comp_str(comp, "ladder_config").map(|p| base_dir.join(p)),
            record: None,
            bars,
            lean_mode,
            latency_ms,
            calibration_ms,
        };

        // The shared manager (ADR 0017): the borrow ends at start — the
        // session keeps only kira handles, the manager stays in the engine.
        let Some(manager) = audio.manager_mut() else {
            println!("[music] no audio device — session skipped");
            return Ok(None);
        };
        let (mut session, notices) =
            ChartSession::open_shared(&cfg, manager).map_err(|e| anyhow!("{e}"))?;
        for n in notices {
            println!("[music] {n}");
        }

        // Gamepad handoff (ADR 0018): the capture thread owns the pad for the
        // run. Capture failure is non-fatal — the session plays without input
        // (and the bar-2 NO INPUT notice will not fire: no receiver attached).
        let offset_samples =
            judgment_offset_samples(latency_ms, calibration_ms, session.sample_rate());
        match flint_input_capture::spawn(
            session.bridge(),
            CaptureConfig {
                offset_samples,
                ..Default::default()
            },
        ) {
            Ok((handle, rx)) => {
                println!("[music] gamepad capture running (left stick = lean, South/RT = pulse)");
                match flint_input_capture::connected_gamepads() {
                    Ok(pads) if pads.is_empty() => println!(
                        "[music] *** WARNING: NO GAMEPADS VISIBLE — the session will receive \
                         no input.\n[music] *** On Windows only XInput-class devices are seen; \
                         check the controller/mapper mode."
                    ),
                    Ok(pads) => println!("[music] gamepad(s): {}", pads.join(", ")),
                    Err(_) => {}
                }
                session.set_input(rx, Box::new(handle));
            }
            Err(e) => {
                eprintln!("[music] gamepad capture unavailable ({e}) — playing without input");
            }
        }

        Ok(Some(Self {
            session,
            pulse_release_pending: false,
            events_total: 0,
            monotonic_violations: 0,
            last_event_sample: i64::MIN,
            max_confirmed_skew_ms: 0.0,
            last_skew_check: Instant::now(),
        }))
    }

    /// One player frame. Returns `true` when the session is over (naturally
    /// or on error) — the caller tears down via [`MusicSession::stop`].
    pub fn tick(&mut self, input: &mut InputState) -> bool {
        if self.pulse_release_pending {
            input.process_gamepad_button_up(GAMEPAD_SLOT, PULSE_BUTTON);
            self.pulse_release_pending = false;
        }

        // Split borrows: the tap closure feeds InputState and the evidence
        // counters while `session` runs the drain (ADR 0018 down-sample).
        let Self {
            session,
            pulse_release_pending,
            events_total,
            monotonic_violations,
            last_event_sample,
            ..
        } = self;
        let outcome = session.tick_with(|ev| {
            *events_total += 1;
            let sample = ev.sample();
            if sample < *last_event_sample {
                *monotonic_violations += 1;
            }
            *last_event_sample = sample;
            match ev {
                InputEvent::Lean(l) => {
                    input.process_gamepad_axis(GAMEPAD_SLOT, "LeftStickX", l.x as f32);
                    input.process_gamepad_axis(GAMEPAD_SLOT, "LeftStickY", l.y as f32);
                }
                InputEvent::Pulse(_) => {
                    input.process_gamepad_button_down(GAMEPAD_SLOT, PULSE_BUTTON);
                    *pulse_release_pending = true;
                }
            }
        });
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("music session tick failed, ending session: {e}");
                return true;
            }
        };
        for n in &outcome.notices {
            println!("[music] {n}");
        }

        // Once per second: stem-skew evidence with the ADR 0017 re-read
        // discriminator (an exactly-one-buffer transient collapses on
        // re-read; real de-sync persists and grows).
        if self.last_skew_check.elapsed().as_secs_f64() >= 1.0 {
            self.last_skew_check = Instant::now();
            let mut skew_ms = self.session.max_stem_skew() * 1000.0;
            if skew_ms > 0.5 {
                skew_ms = skew_ms.min(self.session.max_stem_skew() * 1000.0);
                if skew_ms > 0.5 {
                    tracing::warn!("stem skew {skew_ms:.3} ms survived a re-read");
                }
            }
            self.max_confirmed_skew_ms = self.max_confirmed_skew_ms.max(skew_ms);
        }

        outcome.state == Tick::Finished
    }

    /// Ladder/reintegration visuals for the post-override merge.
    pub fn visual_frame(&self) -> VisualFrame {
        self.session.visual_frame()
    }

    /// The frame → script-POD conversion (ADR 0020): the only place
    /// flint-music types meet flint-script types — flint-script never
    /// depends on flint-music.
    pub fn conducted_snapshot(&self) -> flint_script::ConductedSnapshot {
        let vf = self.session.visual_frame();
        flint_script::ConductedSnapshot {
            lean: [vf.lean[0] as f64, vf.lean[1] as f64],
            target: [vf.target[0] as f64, vf.target[1] as f64],
            coherence: vf.coherence as f64,
            beat_phase: vf.beat_phase as f64,
            bar_phase: vf.bar_phase as f64,
            bar: vf.bar,
            beat: vf.beat,
            section: vf.section,
            pulses: vf
                .pulses
                .iter()
                .map(|p| flint_script::ConductedPulse {
                    age: p.age_s,
                    err_ms: p.err_ms,
                    kind: p.kind.as_str().to_string(),
                })
                .collect(),
            desaturate: vf.desaturate as f64,
            blur: vf.blur as f64,
            chromatic: vf.chromatic as f64,
            reassembly: vf.reassembly as f64,
            rewind: vf.rewind as f64,
            no_input: vf.no_input,
            preroll: vf.preroll,
        }
    }

    /// Teardown: stop the stems with a fade FIRST — on the shared manager,
    /// dropping handles does not stop sounds — then finish (which drops the
    /// capture guard before anything else). Prints the summary plus the
    /// F2-criteria evidence line.
    pub fn stop(mut self) {
        self.session.stop_audio(STOP_FADE_MS);
        let evidence = format!(
            "[music] session evidence: max confirmed skew {:.3} ms | events {} | \
             monotonic violations {}",
            self.max_confirmed_skew_ms, self.events_total, self.monotonic_violations,
        );
        match self.session.finish() {
            Ok(notices) => {
                for n in notices {
                    println!("[music] {n}");
                }
            }
            Err(e) => tracing::warn!("music session finish failed: {e}"),
        }
        println!("{evidence}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(desaturate: f32, blur: f32, chromatic: f32, preroll: bool) -> VisualFrame {
        VisualFrame {
            desaturate,
            blur,
            chromatic,
            preroll,
            ..Default::default()
        }
    }

    #[test]
    fn merge_fills_all_three_from_rung_values() {
        // Rung 3 of the built-in ladder at base 0.
        let (mut rb, mut ca, mut de) = (None, None, None);
        merge_ladder_postprocess(
            &mut rb,
            &mut ca,
            &mut de,
            &frame(0.85, 0.8, 0.35, false),
            LadderPostBase::default(),
        );
        assert!((rb.unwrap() - 0.8 * MUSIC_BLUR_SCALE).abs() < 1e-6);
        assert!((ca.unwrap() - 0.35).abs() < 1e-6);
        assert!((de.unwrap() - 0.85).abs() < 1e-6);
    }

    #[test]
    fn merge_script_override_wins() {
        let (mut rb, mut ca, mut de) = (Some(0.1), None, Some(0.2));
        merge_ladder_postprocess(
            &mut rb,
            &mut ca,
            &mut de,
            &frame(0.85, 0.8, 0.35, false),
            LadderPostBase::default(),
        );
        assert_eq!(rb, Some(0.1));
        assert_eq!(de, Some(0.2));
        assert!(ca.is_some());
    }

    #[test]
    fn merge_preroll_leaves_overrides_untouched() {
        let (mut rb, mut ca, mut de) = (None, None, None);
        merge_ladder_postprocess(
            &mut rb,
            &mut ca,
            &mut de,
            &frame(0.85, 0.8, 0.35, true),
            LadderPostBase::default(),
        );
        assert_eq!((rb, ca, de), (None, None, None));
    }

    #[test]
    fn merge_clean_ladder_writes_back_the_base() {
        // The stickiness fix (ADR 0021): recovery to a clean ladder must
        // actively restore the authored values, not stop writing.
        let base = LadderPostBase {
            radial_blur: 0.05,
            chromatic_aberration: 0.1,
            desaturate: 0.0,
        };
        let (mut rb, mut ca, mut de) = (None, None, None);
        merge_ladder_postprocess(
            &mut rb,
            &mut ca,
            &mut de,
            &frame(0.0, 0.0, 0.0, false),
            base,
        );
        assert_eq!(rb, Some(0.05));
        assert_eq!(ca, Some(0.1));
        assert_eq!(de, Some(0.0));
    }

    #[test]
    fn merge_desaturate_clamps_to_one() {
        let base = LadderPostBase {
            desaturate: 0.5,
            ..Default::default()
        };
        let (mut rb, mut ca, mut de) = (None, None, None);
        merge_ladder_postprocess(
            &mut rb,
            &mut ca,
            &mut de,
            &frame(0.85, 0.0, 0.0, false),
            base,
        );
        assert_eq!(de, Some(1.0));
    }

    #[test]
    fn restore_writes_base_where_scripts_are_silent() {
        let base = LadderPostBase {
            radial_blur: 0.05,
            chromatic_aberration: 0.1,
            desaturate: 0.0,
        };
        let (mut rb, mut ca, mut de) = (None, Some(0.9), None);
        restore_ladder_postprocess(&mut rb, &mut ca, &mut de, base);
        assert_eq!(rb, Some(0.05));
        assert_eq!(ca, Some(0.9));
        assert_eq!(de, Some(0.0));
    }
}
