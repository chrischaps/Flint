//! Milestone 1 evidence, automated: "while the track plays, a scripted
//! sequence drops and restores buses exactly on beat, and the console shows
//! correct bar/beat throughout — including across a tempo change in a
//! synthetic test manifest."
//!
//! Rendered offline (deterministic, no audio device) with synthesized stems
//! at distinct frequencies so a single mixed render attributes every
//! transition to its bus. All tolerances trace to kira's chunk-granular
//! clock advancement (one internal buffer = CHUNK frames) plus the scripted
//! ramp and the analysis window.

use flint_music::analysis::{
    find_tone_transitions, goertzel_power, max_step, peak, tone_envelope, TransitionKind,
};
use flint_music::event_script::EventScript;
use flint_music::offline::{render_offline, OfflineRenderConfig, RenderResult};
use flint_music::scheduler::BusAction;
use flint_music::{validate_manifest, Conductor, MusicalPosition, SuiteManifest};

mod common;
use common::{data_dir, repo_root, tempo_change_manifest, SineStems, CHUNK, SR, WINDOW};

/// Total render: 16 internal bars (10 of 4/4 @ 120, 6 of 3/4 @ 90) = 32 s.
const DURATION: i64 = 1_536_000;

struct Rendered {
    result: RenderResult,
    positions: Vec<MusicalPosition>,
    conductor: Conductor,
}

fn render() -> Rendered {
    let manifest = tempo_change_manifest();
    let issues = validate_manifest(&manifest);
    assert!(
        issues.is_empty(),
        "synthetic manifest must validate: {issues:?}"
    );
    let script =
        EventScript::load(&data_dir().join("milestone1.events.toml")).expect("load event script");

    let cfg = OfflineRenderConfig {
        duration_samples: DURATION,
        chunk_frames: CHUNK,
    };
    let mut positions = Vec::new();
    let result = render_offline(
        &manifest,
        &SineStems::plain(DURATION),
        &script,
        &cfg,
        |pos, _mixer| {
            positions.push(*pos);
        },
    )
    .expect("offline render");
    Rendered {
        result,
        positions,
        conductor: Conductor::new(&manifest, None),
    }
}

#[test]
fn milestone1_scripted_bus_changes_land_on_beat_across_tempo_change() {
    let r = render();
    let audio = &r.result.samples;
    assert_eq!(audio.len(), DURATION as usize * 2);
    assert!(peak(audio) > 0.2, "render must not be silent");

    // --- the scripted samples, from the Conductor (internal 0-based bars) ---
    let drop_at = r.conductor.sample_at_bar(8, 0.0); // script bar:9
    let rise_at = r.conductor.sample_at_bar(12, 0.0); // script bar:13
    let lpf_at = r.conductor.sample_at_bar(10, 0.0); // script bar:11 = anchor
    assert_eq!(drop_at, 768_000);
    assert_eq!(lpf_at, 960_000, "bar 11 is the tempo anchor");
    assert_eq!(rise_at, 1_152_000, "two 3/4 bars past the anchor");

    // --- home_theme (880 Hz) drops and rises exactly on beat ----------------
    // Tolerance: analysis window + one kira chunk + the 10 ms ramp reaching
    // the -20 dB detection floor. All ≪ one beat (24000/32000 samples).
    let tol = (2 * WINDOW + CHUNK + 480) as i64;
    let env = tone_envelope(audio, SR, 880.0, WINDOW);
    let transitions = find_tone_transitions(&env, WINDOW);
    assert_eq!(
        transitions.len(),
        2,
        "exactly one drop and one rise: {transitions:?}"
    );
    let (t_drop, k_drop) = transitions[0];
    let (t_rise, k_rise) = transitions[1];
    assert_eq!(k_drop, TransitionKind::Drop);
    assert_eq!(k_rise, TransitionKind::Rise);
    assert!(
        (t_drop - drop_at).abs() <= tol,
        "drop landed at {t_drop}, scripted {drop_at} (tol {tol})"
    );
    assert!(
        (t_rise - rise_at).abs() <= tol,
        "rise landed at {t_rise}, scripted {rise_at} (tol {tol}) — this event is \
         scheduled AFTER the tempo change"
    );

    // --- texture low-pass engages at the anchor, not before -----------------
    let p8k = |from: i64, to: i64| goertzel_power(audio, SR, 8_000.0, from as usize, to as usize);
    let before_early = p8k(200_000, 500_000);
    let before_late = p8k(600_000, 900_000);
    let after = p8k(1_200_000, 1_500_000);
    assert!(
        after < before_late / 100.0,
        "8 kHz must drop >= 20 dB after the LPF: before {before_late:e}, after {after:e}"
    );
    assert!(
        before_late > before_early / 4.0 && before_late < before_early * 4.0,
        "no filter engagement before the scripted sample: {before_early:e} vs {before_late:e}"
    );

    // --- no clicks: scripted changes ramp; steps stay at content slew -------
    let step = max_step(audio);
    assert!(step < 0.25, "discontinuity detected: max step {step}");

    // --- marker fired, nothing late -----------------------------------------
    assert!(
        r.result
            .fired
            .iter()
            .any(|f| matches!(&f.action, BusAction::Marker(l) if l == "anchor")),
        "marker must fire: {:?}",
        r.result.fired
    );
    assert!(
        r.result.fired.iter().all(|f| !f.late),
        "no event may fire late: {:?}",
        r.result.fired
    );
    assert_eq!(r.result.fired.len(), 4);
}

#[test]
fn milestone1_console_bar_beat_correct_throughout() {
    let r = render();

    // Positions are logged once per chunk. They must be monotone, and every
    // bar 0..16 must begin within one chunk of the Conductor's bar sample —
    // bars 10..16 exercising the 3/4 @ 90 BPM region after the anchor.
    let pos = &r.positions;
    assert!(pos.windows(2).all(|w| w[1].sample >= w[0].sample));
    assert!(pos.windows(2).all(|w| w[1].beat >= w[0].beat));
    for bar in 0..16i64 {
        let expected = r.conductor.sample_at_bar(bar, 0.0);
        let first = pos
            .iter()
            .find(|p| p.bar >= bar)
            .unwrap_or_else(|| panic!("bar {bar} never reached"));
        assert_eq!(first.bar, bar, "bars must not be skipped");
        assert!(
            (first.sample - expected).abs() <= CHUNK as i64,
            "bar {bar} first observed at {}, expected {expected}",
            first.sample
        );
    }
    // Meter is 3/4 after the anchor: beat_in_bar stays < 3.
    assert!(
        pos.iter()
            .filter(|p| p.sample >= 960_000)
            .all(|p| p.beat_in_bar < 3.0),
        "3/4 bars after the anchor"
    );
    // And 4/4 before it: some positions reach beat 3.x.
    assert!(
        pos.iter()
            .any(|p| (0..960_000).contains(&p.sample) && p.beat_in_bar >= 3.0),
        "4/4 bars before the anchor"
    );
}

#[test]
fn milestone1_render_is_deterministic() {
    let a = render();
    let b = render();
    assert_eq!(
        a.result.samples, b.result.samples,
        "two renders of the same inputs must be bit-identical"
    );
    assert_eq!(a.result.fired.len(), b.result.fired.len());
}

/// Offline render of the real prototype suite (golden fixture + FLAC stems),
/// skipped when the game repo's fixtures aren't present (engine CI).
#[test]
fn prototype_fixture_renders_offline() {
    let repo_root = repo_root();
    let manifest_path = repo_root.join("assets/manifests/prototype.suite.toml");
    if !manifest_path.exists() {
        eprintln!("skipping: {} not present", manifest_path.display());
        return;
    }
    let manifest = SuiteManifest::load(&manifest_path).expect("load prototype manifest");
    let stems = flint_music::session::FileStems::new(&repo_root);
    let conductor = Conductor::new(&manifest, None);
    let cfg = OfflineRenderConfig {
        duration_samples: conductor.sample_at_bar(2, 0.0),
        chunk_frames: CHUNK,
    };
    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    let result = render_offline(&manifest, &stems, &script, &cfg, |_, _| {})
        .expect("prototype offline render");
    let audio = &result.samples;
    assert!(peak(audio) > 0.05, "prototype render must not be silent");
    assert!(audio.iter().all(|s| s.is_finite()));
}
