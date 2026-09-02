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
use std::path::Path;

mod common;
use common::repo_root;

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

    // Debug guide (ADR 0035), read during pre-roll: every window is ahead,
    // none open, prototype content is pulse-only with no sway/pressure
    // channels, and the horizon actually bounds the list.
    let guide = session.guide_frame(1e9);
    assert!(!guide.windows.is_empty(), "the chart's windows are visible");
    for w in &guide.windows {
        assert!(w.beats_until > 0.0, "pre-roll: all windows upcoming");
        assert!(!w.open_now);
        assert_eq!(w.kind, "pulse");
        assert_eq!((w.strength, w.direction), (None, None));
    }
    assert!(
        guide.windows.windows(2).all(|p| p[0].beat <= p[1].beat),
        "windows sorted by beat"
    );
    assert_eq!(
        (
            guide.sway_target,
            guide.pressure_l_target,
            guide.pressure_r_target
        ),
        (None, None, None)
    );
    let near = session.guide_frame(guide.windows[0].beats_until + 0.01);
    assert!(
        near.windows.len() < guide.windows.len(),
        "horizon bounds the list ({} vs {})",
        near.windows.len(),
        guide.windows.len()
    );

    // Manifest Map (debug timeline): the static map mirrors the manifest.
    let manifest =
        flint_music::SuiteManifest::load(&cfg.manifest).expect("manifest loads for comparison");
    let map = session.timeline_map();
    assert_eq!(map.sample_rate, manifest.sample_rate);
    assert_eq!(
        map.sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
        manifest
            .sections
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>(),
        "sections in manifest order"
    );
    for (ts, ms) in map.sections.iter().zip(&manifest.sections) {
        assert_eq!(ts.start_sample, ms.start_sample);
        assert_eq!(
            ts.re_entry,
            manifest.reintegration.re_entry_sections.contains(&ms.name),
            "re-entry flag matches the manifest for '{}'",
            ms.name
        );
    }
    for pair in map.sections.windows(2) {
        assert_eq!(
            pair[0].end_sample, pair[1].start_sample,
            "each section runs to the next's start"
        );
    }
    let last_section_start = manifest
        .sections
        .iter()
        .map(|s| s.start_sample)
        .max()
        .unwrap();
    assert!(
        map.end_sample >= last_section_start,
        "map spans every section"
    );
    assert_eq!(
        map.sections.last().unwrap().end_sample,
        map.end_sample,
        "last section runs to the map's end"
    );
    assert!(!map.bars.is_empty() && map.bars[0].sample == 0 && map.bars[0].bar == 0);
    assert!(
        map.bars.windows(2).all(|p| p[0].sample < p[1].sample),
        "bar lines strictly increasing"
    );
    assert!(
        map.bars.iter().all(|b| b.sample <= map.end_sample),
        "bar lines stay inside the map"
    );
    assert_eq!(map.tempo_marks.len(), manifest.tempo.len());
    assert_eq!(map.tempo_marks[0].sample, manifest.tempo[0].sample);

    // The per-frame timeline: pre-roll playhead sits before zero, the
    // timeline offset is untouched (no seam), no history yet.
    let tf = session.timeline_frame();
    assert!(tf.preroll && tf.playhead_sample < 0, "pre-roll playhead");
    assert_eq!(tf.timeline_offset, 0, "no seam has happened");
    assert!(
        tf.seams.is_empty() && tf.pulses.is_empty(),
        "no history yet"
    );

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
            direction: None,
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
