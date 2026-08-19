//! Historical note (2026-08-19 hygiene pass): `seam_milestone3.rs` now
//! exercises the full reactive loop on top of this mechanism, but this spike
//! remains the *isolated* proof of the seam primitive (no judge, no ladder)
//! and is cited from the Phase 3 status entry — kept, not superseded.
//!
//! E4 seam-mechanism spike (Phase 3): prove that stopping all stems with a
//! short fade and re-playing them from an earlier position on one shared
//! clock tick (a) lands the re-entry sample-accurately, (b) keeps the stems
//! sample-locked after the seam, (c) produces no click, and (d) moves the
//! suite timeline backwards exactly once via the session's timeline offset —
//! before the reintegration sequencer (E5) is built on top of it.
//!
//! Content encoding: home_theme carries 880 Hz only during the first two
//! bars of the *suite* timeline, so its reappearance in the rendered output
//! is direct evidence that playback returned to suite sample 0.

use flint_music::analysis::{find_tone_transitions, max_step, peak, tone_envelope, TransitionKind};
use flint_music::event_script::EventScript;
use flint_music::manifest::BusDecl;
use flint_music::offline::{render_offline_with, OfflineRenderConfig, RenderResult};
use flint_music::session::StemResolver;
use flint_music::{MusicalPosition, SuiteManifest};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
const WINDOW: usize = 64;
/// 120 BPM 4/4 (the manifest's first region): one bar = 96_000 samples.
const BAR: i64 = 96_000;
/// Render 8 bars of *clock* time.
const DURATION: i64 = 8 * BAR;
/// home_theme sounds for suite samples [0, 2 bars).
const THEME_END: i64 = 2 * BAR;
/// The director triggers the seam once the play head passes bar 4...
const TRIGGER_AT: i64 = 4 * BAR;
/// ...scheduling the re-entry (to suite sample 0) at bar 5.
const SEAM_AT: i64 = 5 * BAR;
const FADE_MS: f64 = 20.0;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// foundation/texture: steady tones (lock + click material across the seam).
/// home_theme: 880 Hz gated to the first two suite bars (position evidence).
struct SeamStems;

impl StemResolver for SeamStems {
    fn load(&self, bus: &str, decl: &BusDecl) -> flint_core::Result<Option<StaticSoundData>> {
        if decl.file.is_none() {
            return Ok(None);
        }
        let (freq, amp) = match bus {
            "foundation" => (220.0, 0.20f32),
            "home_theme" => (880.0, 0.25),
            "texture" => (8_000.0, 0.10),
            other => panic!("playable bus without a tone: {other}"),
        };
        let frames: Arc<[Frame]> = (0..(DURATION + SR as i64) as usize)
            .map(|i| {
                let gate = bus != "home_theme" || (i as i64) < THEME_END;
                let t = i as f64 / SR as f64;
                let s = (std::f64::consts::TAU * freq * t).sin() as f32 * amp;
                Frame::from_mono(if gate { s } else { 0.0 })
            })
            .collect();
        Ok(Some(StaticSoundData {
            sample_rate: SR,
            frames,
            settings: Default::default(),
            slice: None,
        }))
    }
}

struct Rendered {
    result: RenderResult,
    /// Suite positions observed per chunk (raw index = chunk order).
    positions: Vec<MusicalPosition>,
    /// Max pairwise stem skew observed after the seam committed, seconds.
    max_skew_after_seam: f64,
}

fn render() -> Rendered {
    let manifest = SuiteManifest::load(&data_dir().join("tempo_change.suite.toml"))
        .expect("load synthetic manifest");
    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    let cfg = OfflineRenderConfig {
        duration_samples: DURATION,
        chunk_frames: CHUNK,
    };
    let mut positions = Vec::new();
    let mut max_skew_after_seam = 0.0f64;
    let mut triggered = false;
    let result = render_offline_with(&manifest, &SeamStems, &script, &cfg, |pos, session| {
        positions.push(*pos);
        if !triggered && pos.sample >= TRIGGER_AT {
            triggered = true;
            session
                .schedule_seam(0, SEAM_AT, FADE_MS, &|_| 0.0)
                .expect("schedule seam");
        }
        if session.timeline_offset() != 0 {
            max_skew_after_seam = max_skew_after_seam.max(session.mixer.max_stem_skew());
        }
    })
    .expect("offline render");
    assert!(triggered, "seam trigger never reached");
    Rendered {
        result,
        positions,
        max_skew_after_seam,
    }
}

#[test]
fn seam_replays_at_position_sample_locked_without_click() {
    let r = render();
    let audio = &r.result.samples;
    assert_eq!(audio.len(), DURATION as usize * 2);
    assert!(peak(audio) > 0.2, "render must not be silent");

    // --- home_theme reappears exactly at the seam --------------------------
    // Output index = raw clock sample. Expected 880 Hz shape: on during raw
    // [0, 2 bars), off, on again at the seam (suite restarts at 0), off two
    // suite bars later.
    let tol = (2 * WINDOW + CHUNK) as i64 + (FADE_MS / 1000.0 * SR as f64) as i64;
    let env = tone_envelope(audio, SR, 880.0, WINDOW);
    let transitions = find_tone_transitions(&env, WINDOW);
    assert_eq!(
        transitions.len(),
        3,
        "drop at theme end, rise at seam, drop at theme end again: {transitions:?}"
    );
    let (t0, k0) = transitions[0];
    let (t1, k1) = transitions[1];
    let (t2, k2) = transitions[2];
    assert_eq!(k0, TransitionKind::Drop);
    assert_eq!(k1, TransitionKind::Rise);
    assert_eq!(k2, TransitionKind::Drop);
    assert!((t0 - THEME_END).abs() <= tol, "first drop at {t0}");
    assert!(
        (t1 - SEAM_AT).abs() <= tol,
        "re-entry landed at {t1}, scheduled {SEAM_AT} (tol {tol})"
    );
    assert!(
        (t2 - (SEAM_AT + THEME_END)).abs() <= tol,
        "post-seam suite time must run from 0: second drop at {t2}"
    );

    // --- stems stay sample-locked after the seam ---------------------------
    assert!(
        r.max_skew_after_seam < 1e-6,
        "post-seam stem skew {} s",
        r.max_skew_after_seam
    );

    // --- no click anywhere (the fade is the only level move) ---------------
    let step = max_step(audio);
    assert!(step < 0.25, "discontinuity detected: max step {step}");
}

#[test]
fn seam_jumps_suite_time_backwards_exactly_once() {
    let r = render();
    let pos = &r.positions;

    // Exactly one backwards jump, landing within one chunk of suite sample 0.
    let jumps: Vec<usize> = pos
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[1].sample < w[0].sample)
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(jumps.len(), 1, "expected exactly one timeline jump: {jumps:?}");
    let j = jumps[0];
    assert!(
        (0..=CHUNK as i64).contains(&pos[j].sample),
        "post-seam suite position must restart near 0, got {}",
        pos[j].sample
    );
    assert_eq!(pos[j].bar, 0, "post-seam bar must be 0");
    // The jump commits at the seam. Chunk j starts at raw sample
    // j*CHUNK - preroll (2 beats at 120 BPM = 48_000 samples).
    let raw_at_jump = (j * CHUNK) as i64 - 48_000;
    assert!(
        (raw_at_jump - SEAM_AT).abs() <= 2 * CHUNK as i64,
        "jump committed at raw {raw_at_jump}, seam {SEAM_AT}"
    );
    // Monotone on both sides of the single jump.
    assert!(pos[..j].windows(2).all(|w| w[1].sample >= w[0].sample));
    assert!(pos[j..].windows(2).all(|w| w[1].sample >= w[0].sample));
}

#[test]
fn seam_render_is_deterministic() {
    let a = render();
    let b = render();
    assert_eq!(
        a.result.samples, b.result.samples,
        "two renders of the same inputs must be bit-identical"
    );
}
