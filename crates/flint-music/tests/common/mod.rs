//! Shared test-support module for flint-music's integration tests.
//!
//! Holds only the pieces that were genuinely identical across test files:
//! directory helpers, the common sample-rate/chunk constants, the synthetic
//! tempo-change manifest loader, and the sine-tone stem resolvers. Per-test
//! render harnesses and `Observed` structs stay in their own files — they
//! encode different observations and are not shared on purpose.
//!
//! Not every test uses every helper, hence the module-wide dead_code allow.
#![allow(dead_code)]

use flint_music::manifest::BusDecl;
use flint_music::session::StemResolver;
use flint_music::SuiteManifest;
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Sample rate shared by every synthetic fixture.
pub const SR: u32 = 48_000;
/// Offline render chunk size (frames).
pub const CHUNK: usize = 128;
/// Analysis window size (frames).
pub const WINDOW: usize = 64;
/// 120 BPM 4/4 at 48 kHz: one bar = 96_000 samples (the synthetic
/// manifest's first tempo region). Some tests name this `BAR_A`.
pub const BAR: i64 = 96_000;

/// `tests/data` — synthetic manifests/scripts private to this crate's tests.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// The repo's golden `fixtures/` directory, or None when the crate is built
/// outside the game repo (tests skip in that case).
pub fn fixtures_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures");
    dir.canonicalize().ok().filter(|d| d.join("valid").exists())
}

/// The game repo root (three levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The synthetic tempo-change suite: 120 BPM 4/4 until suite sample
/// 960_000, then 90 BPM 3/4.
pub fn tempo_change_manifest() -> SuiteManifest {
    SuiteManifest::load(&data_dir().join("tempo_change.suite.toml")).expect("synthetic manifest")
}

/// One steady mono sine, `duration + 1 s` of frames so end-of-sound never
/// reads as a level transition inside an analyzed render window.
pub fn sine_sound(freq: f64, amp: f32, duration: i64) -> StaticSoundData {
    let frames: Arc<[Frame]> = (0..(duration + SR as i64) as usize)
        .map(|i| {
            let t = i as f64 / SR as f64;
            Frame::from_mono((std::f64::consts::TAU * freq * t).sin() as f32 * amp)
        })
        .collect();
    StaticSoundData {
        sample_rate: SR,
        frames,
        settings: Default::default(),
        slice: None,
    }
}

/// Canonical sine-tone `StemResolver` over the synthetic manifest's bus
/// table — frequencies far apart so Goertzel attribution is unambiguous.
/// File paths in the manifest are labels only (never opened).
///
/// `plain` plays every bus steadily; `theme_gated` silences `home_theme`
/// outside the given `[start, len)` suite-sample spans (position evidence
/// for the seam tests). Ungated buses produce bit-identical frames in both
/// modes.
pub struct SineStems {
    duration: i64,
    theme_gates: Option<Vec<(i64, i64)>>,
}

impl SineStems {
    pub fn plain(duration: i64) -> Self {
        Self {
            duration,
            theme_gates: None,
        }
    }

    pub fn theme_gated(duration: i64, gates: Vec<(i64, i64)>) -> Self {
        Self {
            duration,
            theme_gates: Some(gates),
        }
    }
}

impl StemResolver for SineStems {
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
        let gates = if bus == "home_theme" {
            self.theme_gates.as_deref()
        } else {
            None
        };
        let Some(gates) = gates else {
            return Ok(Some(sine_sound(freq, amp, self.duration)));
        };
        let frames: Arc<[Frame]> = (0..(self.duration + SR as i64) as usize)
            .map(|i| {
                let s = i as i64;
                let gate = gates
                    .iter()
                    .any(|(start, len)| (*start..start + len).contains(&s));
                let t = i as f64 / SR as f64;
                let v = (std::f64::consts::TAU * freq * t).sin() as f32 * amp;
                Frame::from_mono(if gate { v } else { 0.0 })
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
