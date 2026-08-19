//! E2 audio-rung evidence: the LadderDriver applies rung params to the mixer
//! as tweens — texture dropout is audible at the scripted rung entry, motif
//! buses stay untouched, the warble oscillates within its configured depth,
//! and the whole render stays deterministic and click-free.
//!
//! Coherence is scripted here (a value trajectory, not real judgment) — this
//! test pins the ladder→mixer path in isolation. The full reactive loop
//! (judge → coherence → ladder → sequencer) is Milestone 3's test.

use flint_music::analysis::{find_tone_transitions, max_step, tone_envelope, TransitionKind};
use flint_music::event_script::EventScript;
use flint_music::ladder::{Ladder, LadderConfig, LadderDriver, DROP_DB};
use flint_music::manifest::BusDecl;
use flint_music::mixer::LPF_OPEN_HZ;
use flint_music::offline::{render_offline_with, OfflineRenderConfig};
use flint_music::session::StemResolver;
use flint_music::SuiteManifest;
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
const WINDOW: usize = 64;
/// 120 BPM 4/4: one bar = 96_000 samples. Render 8 bars.
const BAR: i64 = 96_000;
const DURATION: i64 = 8 * BAR;
/// Coherence trajectory: clean for 2 bars, then a value inside rung 3
/// (dropout) for 4 bars, then clean recovery.
const FALL_AT: i64 = 2 * BAR;
const RECOVER_AT: i64 = 6 * BAR;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

struct SynthStems;

impl StemResolver for SynthStems {
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
                let t = i as f64 / SR as f64;
                Frame::from_mono((std::f64::consts::TAU * freq * t).sin() as f32 * amp)
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

fn scripted_coherence(sample: i64) -> f64 {
    if (FALL_AT..RECOVER_AT).contains(&sample) {
        0.20 // inside rung 3 (dropout enters below 0.34), above full-fail
    } else {
        0.95
    }
}

struct Observed {
    audio: Vec<f32>,
    /// (suite_sample, level, texture_gain_db, texture_detune, home_lpf, foundation_lpf)
    states: Vec<(i64, usize, f32, f64, f64, f64)>,
}

fn render() -> Observed {
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
    let mut ladder = Ladder::new(LadderConfig::default());
    let mut driver = LadderDriver::new();
    let mut states = Vec::new();
    let result = render_offline_with(&manifest, &SynthStems, &script, &cfg, |pos, session| {
        ladder.observe(scripted_coherence(pos.sample));
        let params = ladder.params();
        driver.apply(
            &params,
            &flint_music::GradientOffsets::default(),
            pos.seconds,
            params.ramp_ms,
            Some((pos.sample, 250.0)),
            &mut session.mixer,
        );
        let get = |name: &str| {
            session
                .mixer
                .buses()
                .find(|b| b.name == name)
                .map(|b| b.state)
                .unwrap()
        };
        states.push((
            pos.sample,
            ladder.level(),
            get("texture").gain_db,
            get("texture").detune_semitones,
            get("home_theme").lpf_hz,
            get("foundation").lpf_hz,
        ));
    })
    .expect("offline render");
    Observed {
        audio: result.samples,
        states,
    }
}

#[test]
fn dropout_rung_silences_texture_and_spares_motifs() {
    let o = render();

    // Ladder level follows the scripted trajectory (a single big fall crosses
    // all three rungs at once; recovery walks back to clean).
    assert!(o.states.iter().filter(|s| s.0 < FALL_AT).all(|s| s.1 == 0));
    assert!(o
        .states
        .iter()
        .filter(|s| (FALL_AT + CHUNK as i64..RECOVER_AT).contains(&s.0))
        .all(|s| s.1 == 3));
    assert_eq!(o.states.last().unwrap().1, 0, "recovers to clean");

    // Shadow state: texture commanded to DROP_DB at the rung, restored after;
    // home_theme's LPF never moves; foundation's does (whole-world haze).
    let at = |sample: i64| o.states.iter().find(|s| s.0 >= sample).unwrap();
    assert_eq!(at(FALL_AT + BAR).2, DROP_DB);
    assert!(at(RECOVER_AT + BAR).2.abs() < 0.01, "texture restored");
    assert!(o.states.iter().all(|s| s.4 == LPF_OPEN_HZ), "motif LPF untouched");
    assert!(at(FALL_AT + BAR).5 < 1_000.0, "foundation LPF engaged");
    assert_eq!(at(RECOVER_AT + BAR).5, LPF_OPEN_HZ, "foundation LPF reopened");

    // Warble: texture detune oscillates within ±depth while the rung holds,
    // and returns to 0 after recovery (re-locking pitch).
    let depth = LadderConfig::default().rungs[2].audio.warble_depth_semitones;
    let detunes: Vec<f64> = o
        .states
        .iter()
        .filter(|s| (FALL_AT + BAR..RECOVER_AT).contains(&s.0))
        .map(|s| s.3)
        .collect();
    assert!(detunes.iter().all(|d| d.abs() <= depth + 1e-9));
    assert!(
        detunes.iter().any(|d| *d > depth * 0.8) && detunes.iter().any(|d| *d < -depth * 0.8),
        "LFO must swing both ways"
    );
    assert_eq!(o.states.last().unwrap().3, 0.0, "detune re-zeroed");

    // Audio evidence: 8 kHz texture drops at the rung entry and rises at
    // recovery, within tween tolerance; 880 Hz motif never transitions.
    let env = tone_envelope(&o.audio, SR, 8_000.0, WINDOW);
    let transitions = find_tone_transitions(&env, WINDOW);
    let tol = (2 * WINDOW + CHUNK) as i64 + 24_000; // 500 ms rung-3 ramp
    assert_eq!(transitions.len(), 2, "one drop, one rise: {transitions:?}");
    assert_eq!(transitions[0].1, TransitionKind::Drop);
    assert!((transitions[0].0 - FALL_AT).abs() <= tol);
    assert_eq!(transitions[1].1, TransitionKind::Rise);
    assert!((transitions[1].0 - RECOVER_AT).abs() <= tol);
    let motif_env = tone_envelope(&o.audio, SR, 880.0, WINDOW);
    assert!(
        find_tone_transitions(&motif_env, WINDOW).is_empty(),
        "home_theme must be untouched by the ladder"
    );

    // No clicks: every move is a tween.
    let step = max_step(&o.audio);
    assert!(step < 0.25, "discontinuity: max step {step}");
}

#[test]
fn ladder_driver_render_is_deterministic() {
    let a = render();
    let b = render();
    assert_eq!(a.audio, b.audio, "bit-identical renders");
}
