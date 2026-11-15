//! A suite that only plays music, through the real binary (#745).
//!
//! Two stems on buses the suite names itself (`drums`, `pad`), no
//! `[reintegration]`, no judgment windows: `flint validate-suite` passes it
//! and `flint render-suite` mixes it offline, both stems sample-locked. The
//! rhythm layer's contract is still there for a suite that asks for it:
//! `--rhythm` names what such a suite lacks.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");
const RATE: u32 = 8_000;
/// Two bars at 120 bpm in 4/4: four seconds.
const FRAMES: usize = 4 * RATE as usize;

const SUITE: &str = r#"
schema_version = 0

[suite]
id = "two-buses"
title = "Two buses, no game"

[audio]
sample_rate = 8000

[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]

[[sections]]
name = "a"
start_sample = 0

[[sections]]
name = "b"
start_sample = 16000

[buses.pad]
file = "stems/pad.wav"

[buses.drums]
file = "stems/drums.wav"
"#;

/// The pad's gain drops to silence at bar 2 (1-based, as the console
/// counts), so the render shows the event script reaching a bus the manifest
/// named.
const EVENTS: &str = r#"
schema_version = 0

[[events]]
at = "bar:2"
bus = "pad"
action = "set_gain"
db = -80.0
ramp_ms = 10.0
"#;

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("suite_cli_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("stems")).unwrap();
    std::fs::write(dir.join("two.suite.toml"), SUITE).unwrap();
    std::fs::write(dir.join("two.events.toml"), EVENTS).unwrap();
    // A low tone on the pad, a high one on the drums: mono, as stereo frames
    let tone = |freq: f32, amp: f32| -> Vec<f32> {
        (0..FRAMES)
            .flat_map(|i| {
                let v = (std::f32::consts::TAU * freq * i as f32 / RATE as f32).sin() * amp;
                [v, v]
            })
            .collect()
    };
    flint_music::write_wav(&dir.join("stems/pad.wav"), &tone(220.0, 0.3), RATE).unwrap();
    flint_music::write_wav(&dir.join("stems/drums.wav"), &tone(1_760.0, 0.2), RATE).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("flint runs")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_suite_with_two_named_buses_and_no_reintegration_validates() {
    let dir = project("validate");
    let out = flint(&dir, &["validate-suite", "two.suite.toml"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("OK: suite 'two-buses'"), "{said}");
    assert!(said.contains("2 buses"), "{said}");
    assert!(!said.contains("rhythm contract"), "{said}");
}

#[test]
fn the_rhythm_contract_names_what_a_music_suite_lacks() {
    let dir = project("rhythm");
    let out = flint(&dir, &["validate-suite", "two.suite.toml", "--rhythm"]);
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    for code in [
        "MissingBus",
        "UnknownBus",
        "MissingReintegration",
        "PulseWindowMissing",
    ] {
        assert!(said.contains(code), "no {code} in:\n{said}");
    }
}

#[test]
fn a_suite_with_two_named_buses_renders_offline() {
    let dir = project("render");
    let out = flint(
        &dir,
        &[
            "render-suite",
            "two.suite.toml",
            "--script",
            "two.events.toml",
            "--output",
            "mix.wav",
            "--duration-bars",
            "2",
        ],
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    // The status line lists the buses in the order the suite declares them
    let first_status = said
        .lines()
        .find(|l| l.starts_with("bar "))
        .unwrap_or_else(|| panic!("no status line in:\n{said}"));
    let pad = first_status.find("| pad ").expect("pad in the status line");
    let drums = first_status
        .find("| drums ")
        .expect("drums in the status line");
    assert!(pad < drums, "declared order is pad, drums: {first_status}");
    assert!(said.contains("fired"), "the pad's gain event fired: {said}");

    let mix = hound::WavReader::open(dir.join("mix.wav")).expect("a WAV");
    assert_eq!(mix.spec().sample_rate, RATE);
    let samples: Vec<f32> = mix.into_samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples.len(), FRAMES * 2, "two bars, stereo");
    // Both stems sound in bar 1; the pad is gone in bar 2, the drums stay
    let rms = |from: usize, to: usize| {
        let s = &samples[from * 2..to * 2];
        (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
    };
    let bar = FRAMES / 2;
    let (one, two) = (rms(bar / 4, bar * 3 / 4), rms(bar + bar / 4, bar * 7 / 4));
    assert!(one > 0.1, "bar 1 has both stems: rms {one}");
    assert!(
        two > 0.05 && two < one,
        "bar 2 has the drums alone: rms {two} vs {one}"
    );
}
