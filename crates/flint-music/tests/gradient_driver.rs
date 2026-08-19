//! ADR 0024 evidence: the error-driven audio gradient maps a scripted lean
//! trajectory onto the mixer through the single ladder-driver writer — the
//! tune bus wobbles off-lean and settles on it, the neutral sink thins its
//! buses at stick-neutral and releases on re-engage, protected buses never
//! move, and the whole render stays deterministic and click-free.
//!
//! The lean error is scripted here (a value trajectory, not real judgment) —
//! this pins the gradient→mixer path in isolation, the way `ladder_driver`
//! pins the rung path. The full loop (ChartCore, phase gating, seam reset)
//! is exercised by `conducted_trace` and the reactive replay.

use flint_music::analysis::max_step;
use flint_music::event_script::EventScript;
use flint_music::gradient::{GradientConfig, GradientDriver};
use flint_music::ladder::{LadderDriver, LadderParams};
use flint_music::manifest::BusDecl;
use flint_music::mixer::LPF_OPEN_HZ;
use flint_music::offline::{render_offline_with, OfflineRenderConfig};
use flint_music::session::StemResolver;
use flint_music::{GradientOffsets, SuiteManifest};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
/// 120 BPM 4/4: one bar = 96_000 samples. Render 9 bars, inside the
/// synthetic manifest's first tempo region.
const BAR: i64 = 96_000;
const DURATION: i64 = 9 * BAR;
/// Scripted lean trajectory: on-target for 3 bars, badly off-target for 3,
/// then stick-neutral for 3.
const OFF_AT: i64 = 3 * BAR;
const NEUTRAL_AT: i64 = 6 * BAR;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// The synthetic manifest's only playing degradable bus is `texture`, so it
/// plays the designated tune-bus role here (`world_voice` is silent — detune
/// on a silent bus is a no-op by design). Sink trims target `harmony`
/// (silent, but track gain is addressable and the shadow state is the pin).
fn gradient_cfg() -> GradientConfig {
    GradientConfig::parse(
        r#"
schema_version = 0
[tune]
bus = "texture"
max_depth_semitones = 0.25
rate_hz = 0.8
err_full = 0.9
gain_trim_db = -2.5
[sink]
threshold = 0.15
db = { harmony = -6.0 }
[smoothing]
attack_ms = 120.0
release_ms = 450.0
"#,
    )
    .expect("test gradient config")
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

/// (lean error, |lean|) per suite sample.
fn scripted_lean(sample: i64) -> (f64, f64) {
    if sample < OFF_AT {
        (0.0, 0.8) // carving right on the target
    } else if sample < NEUTRAL_AT {
        (2.0, 0.8) // engaged but way off the lean
    } else {
        (0.0, 0.0) // hands off the stick
    }
}

struct Observed {
    audio: Vec<f32>,
    /// (suite_sample, texture_gain_db, texture_detune, harmony_gain_db,
    ///  home_gain_db, home_detune, home_lpf, foundation_gain_db)
    states: Vec<(i64, f32, f64, f32, f32, f64, f64, f32)>,
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
    let mut gradient = GradientDriver::new(gradient_cfg());
    let mut driver = LadderDriver::new();
    let clean = LadderParams::clean();
    let mut states = Vec::new();
    let result = render_offline_with(&manifest, &SynthStems, &script, &cfg, |pos, session| {
        let (err, mag) = scripted_lean(pos.sample);
        let offsets = gradient.evaluate(err, mag, pos.seconds);
        driver.apply(&clean, &offsets, pos.seconds, clean.ramp_ms, &mut session.mixer);
        let get = |name: &str| {
            session
                .mixer
                .buses()
                .find(|b| b.name == name)
                .map(|b| b.state)
                .unwrap()
        };
        let (tex, har, home, found) =
            (get("texture"), get("harmony"), get("home_theme"), get("foundation"));
        states.push((
            pos.sample,
            tex.gain_db,
            tex.detune_semitones,
            har.gain_db,
            home.gain_db,
            home.detune_semitones,
            home.lpf_hz,
            found.gain_db,
        ));
    })
    .expect("offline render");
    Observed {
        audio: result.samples,
        states,
    }
}

#[test]
fn gradient_wobbles_off_lean_and_sinks_at_neutral() {
    let o = render();
    let depth = gradient_cfg().tune.max_depth_semitones;

    // On the lean (settled, after the first bar): no wobble, no dulling,
    // no sink.
    let settled_on: Vec<_> = o
        .states
        .iter()
        .filter(|s| (BAR..OFF_AT).contains(&s.0))
        .collect();
    assert!(settled_on.iter().all(|s| s.2.abs() < 0.01), "in tune on the lean");
    assert!(settled_on.iter().all(|s| s.1.abs() < 0.05), "no dulling on the lean");
    assert!(settled_on.iter().all(|s| s.3.abs() < 0.05), "no sink while engaged");

    // Off the lean (settled): the tune bus wobbles — a zero-mean LFO
    // swinging both ways near full depth — and dulls toward the trim.
    let off: Vec<_> = o
        .states
        .iter()
        .filter(|s| (OFF_AT + BAR..NEUTRAL_AT).contains(&s.0))
        .collect();
    assert!(off.iter().all(|s| s.2.abs() <= depth + 1e-9), "wobble bounded by depth");
    assert!(
        off.iter().any(|s| s.2 > depth * 0.8) && off.iter().any(|s| s.2 < -depth * 0.8),
        "LFO must swing both ways"
    );
    assert!(
        off.iter().all(|s| (s.1 - -2.5).abs() < 0.3),
        "tune bus dulled at full error"
    );
    assert!(off.iter().all(|s| s.3.abs() < 0.05), "sink stays out while engaged");

    // Stick-neutral (settled — release is 450 ms, give it a bar): the sink
    // thins harmony, and with the error gone the wobble releases.
    let neutral: Vec<_> = o
        .states
        .iter()
        .filter(|s| s.0 >= NEUTRAL_AT + BAR)
        .collect();
    assert!(
        neutral.iter().all(|s| (s.3 - -6.0).abs() < 0.3),
        "harmony thinned at neutral"
    );
    let last = o.states.last().unwrap();
    assert!(last.2.abs() < 0.02, "wobble settles once the error clears: {}", last.2);
    assert!(last.1 > -0.2, "tune-bus dulling releases: {}", last.1);

    // Protected buses never move: motif and foundation gains stay at 0,
    // motif detune at 0, motif LPF open. (LPF here is clean everywhere —
    // the gradient owns no LPF — but the motif slot is the load-bearing one.)
    for s in &o.states {
        assert_eq!(s.4, 0.0, "home_theme gain untouched (sample {})", s.0);
        assert_eq!(s.5, 0.0, "home_theme detune untouched (sample {})", s.0);
        assert_eq!(s.6, LPF_OPEN_HZ, "home_theme LPF open (sample {})", s.0);
        assert_eq!(s.7, 0.0, "foundation gain untouched (sample {})", s.0);
    }

    // No clicks: every move is a tween riding a slewed target.
    let step = max_step(&o.audio);
    assert!(step < 0.25, "discontinuity: max step {step}");
}

#[test]
fn inert_config_never_touches_the_mixer() {
    // The built-in default must leave the shadow state exactly at rest —
    // this is what keeps gradient-free repos byte-identical.
    let manifest = SuiteManifest::load(&data_dir().join("tempo_change.suite.toml"))
        .expect("load synthetic manifest");
    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    let cfg = OfflineRenderConfig {
        duration_samples: 2 * BAR,
        chunk_frames: CHUNK,
    };
    let mut gradient = GradientDriver::new(GradientConfig::default());
    let mut all_default = true;
    render_offline_with(&manifest, &SynthStems, &script, &cfg, |pos, _session| {
        let (err, mag) = scripted_lean(pos.sample + OFF_AT); // worst case: full error
        let offsets = gradient.evaluate(err, mag, pos.seconds);
        if offsets != GradientOffsets::default() {
            all_default = false;
        }
    })
    .expect("offline render");
    assert!(all_default, "inert config must emit empty offsets forever");
}

#[test]
fn gradient_render_is_deterministic() {
    let a = render();
    let b = render();
    assert_eq!(a.audio, b.audio, "bit-identical renders");
}
