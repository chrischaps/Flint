//! F3: `ChartSession` on a caller-owned manager (the player-app path,
//! ADR 0017) — proved here on the offline backend, no audio device needed.
//! Uses the committed prototype fixtures (real FLAC stems) because
//! `open_shared` resolves stems from disk; skipped when the game repo's
//! fixtures aren't present (engine CI), like `prototype_fixture_renders_offline`.

use flint_music::chart_session::{judgment_offset_samples, ChartSession, ChartSessionConfig};
use flint_music::input_stream::{InputEvent, LeanSample, PulseEvent};
use flint_music::judgment::LeanMode;
use flint_music::offline::{OfflineBackend, OfflineBackendSettings};
use flint_music::Tick;
use kira::{AudioManager, AudioManagerSettings};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn prototype_cfg(root: &Path) -> ChartSessionConfig {
    ChartSessionConfig {
        manifest: root.join("assets/manifests/prototype.suite.toml"),
        chart: root.join("assets/charts/prototype.chart.toml"),
        base_dir: root.to_path_buf(),
        coherence_config: None,
        ladder_config: None,
        gradient_config: None,
        haptics_config: None,
        record: None,
        bars: Some(4),
        lean_mode: LeanMode::Arrival,
        latency_ms: None,
        calibration_ms: 0.0,
    }
}

/// The judgment log a session creates is a test artifact, not a committed
/// run record — find its path in the startup notices and remove it.
fn remove_judgment_log(notices: &[String]) {
    for n in notices {
        if let Some(path) = n.strip_prefix("judgment log: ") {
            let _ = std::fs::remove_file(path.trim());
        }
    }
}

/// The whole shared-manager lifecycle in one pass: open on a caller-owned
/// offline manager, tick through pre-roll, tap every drained input event in
/// order via `tick_with`, stop the audio explicitly (the shared-manager
/// teardown rule), and finish cleanly.
#[test]
fn open_shared_ticks_taps_and_finishes_on_offline_manager() {
    let root = repo_root();
    let manifest_path = root.join("assets/manifests/prototype.suite.toml");
    if !manifest_path.exists() {
        eprintln!("skipping: {} not present", manifest_path.display());
        return;
    }

    let settings = AudioManagerSettings::<OfflineBackend> {
        capacities: Default::default(),
        main_track_builder: Default::default(),
        internal_buffer_size: 128,
        backend_settings: OfflineBackendSettings { sample_rate: 48000 },
    };
    let mut manager =
        AudioManager::<OfflineBackend>::new(settings).expect("offline backend manager");

    let cfg = prototype_cfg(&root);
    let (mut session, notices) =
        ChartSession::open_shared(&cfg, &mut manager).expect("open_shared on offline manager");
    assert!(
        notices.iter().any(|n| n == "lean mode: arrival"),
        "startup notices carry the lean mode: {notices:?}"
    );
    assert!(
        notices.iter().any(|n| n.starts_with("judgment log: ")),
        "startup notices name the judgment log: {notices:?}"
    );

    // Plain tick during pre-roll (the offline clock never advances — the
    // backend is never processed): session stays Running, no panic.
    let out = session.tick().expect("pre-roll tick");
    assert!(out.state == Tick::Running);

    // Attach input the way the player does: a plain channel and an opaque
    // guard. Events are pre-roll-stamped (clock sits before sample 0), so
    // they are tapped but never ingested into judgment.
    let (tx, rx) = std::sync::mpsc::channel();
    session.set_input(rx, Box::new(()));
    let sent = vec![
        InputEvent::Lean(LeanSample {
            sample: -4000,
            x: 0.5,
            y: -0.25,
        }),
        InputEvent::Pulse(PulseEvent {
            sample: -3000,
            kind: "pulse".to_string(),
        }),
        InputEvent::Lean(LeanSample {
            sample: -2000,
            x: -0.75,
            y: 0.5,
        }),
    ];
    for ev in &sent {
        tx.send(ev.clone()).expect("send input event");
    }
    let mut tapped = Vec::new();
    let out = session
        .tick_with(|ev| tapped.push(ev.clone()))
        .expect("tick_with");
    assert!(out.state == Tick::Running);
    assert_eq!(tapped.len(), sent.len(), "every drained event is tapped");
    for (t, s) in tapped.iter().zip(&sent) {
        assert_eq!(t.sample(), s.sample(), "tap order preserved, raw stamps");
    }

    // Shared-manager teardown: explicit stop (handle drop alone would leave
    // the host's manager playing), then finish — clean summary, zero hits.
    session.stop_audio(0.0);
    let finish_notices = session.finish().expect("finish");
    assert!(
        finish_notices
            .iter()
            .any(|n| n.starts_with("done. pulses hit 0 ")),
        "no window was ever judged: {finish_notices:?}"
    );

    remove_judgment_log(&notices);
}

#[test]
fn judgment_offset_samples_matches_the_capture_contract() {
    // 124 ms latency + (-17.6) ms calibration at 48 kHz.
    assert_eq!(
        judgment_offset_samples(Some(124.0), -17.6, 48000),
        ((124.0 - 17.6) / 1000.0f64 * 48000.0).round() as i64
    );
    // No latency on record: calibration alone.
    assert_eq!(judgment_offset_samples(None, 10.0, 48000), 480);
    // Nothing on record: zero.
    assert_eq!(judgment_offset_samples(None, 0.0, 48000), 0);
    // Rounds to nearest, negative allowed.
    assert_eq!(judgment_offset_samples(Some(0.0), -0.01, 48000), 0);
    assert_eq!(judgment_offset_samples(None, -10.0, 48000), -480);
}
