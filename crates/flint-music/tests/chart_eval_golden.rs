//! Chart-evaluator golden tests: pin `ChartEval` to the game repo's
//! reference fixtures (ground truth over code). Skips silently when the
//! engine is built standalone, same as golden_fixtures.rs.
//!
//! Fixture facts these assert (fixtures/valid/*.toml):
//! - 120 BPM 4/4 at 48 kHz until sample 3_840_000 → 24_000 samples/beat.
//! - lean keys: beat 0 [0,0] smooth; beat 8 [0.6,-0.2] smooth; beat 16
//!   [-0.4,0.1] linear.
//! - sections: arrival (start 0, 90 ms) / call (start 1_920_000, 70 ms).
//! - pulses at beats 4, 6 (120 ms override), 8, 10 (press), 12 (flick).

use flint_music::{ChannelValue, Chart, ChartEval, Conductor, SuiteManifest};
use std::path::Path;

mod common;
use common::fixtures_dir;

fn load(fixtures: &Path) -> (ChartEval, Conductor) {
    let manifest =
        SuiteManifest::load(&fixtures.join("valid/prototype.suite.toml")).expect("manifest");
    let chart = Chart::load(&fixtures.join("valid/prototype.chart.toml")).expect("chart");
    let conductor = Conductor::new(&manifest, None);
    (
        ChartEval::new(&chart, &conductor).expect("eval builds from golden fixtures"),
        conductor,
    )
}

fn lean(eval: &ChartEval, beat: f64) -> [f64; 2] {
    match eval.sample_channel("lean", beat) {
        Some(ChannelValue::Vec2(v)) => v,
        other => panic!("lean at {beat}: {other:?}"),
    }
}

#[test]
fn lean_curve_matches_fixture_keys_and_interps() {
    let Some(fixtures) = fixtures_dir() else {
        eprintln!("skipping: no game-repo fixtures directory");
        return;
    };
    let (eval, _) = load(&fixtures);

    // Exact values at the keys.
    assert_eq!(lean(&eval, 0.0), [0.0, 0.0]);
    assert_eq!(lean(&eval, 8.0), [0.6, -0.2]);
    assert_eq!(lean(&eval, 16.0), [-0.4, 0.1]);

    // Smooth (cosine) segment out of beat 0: quarter point pins the shape.
    let f = (1.0 - (std::f64::consts::PI * 0.25).cos()) / 2.0;
    let q = lean(&eval, 2.0);
    assert!((q[0] - 0.6 * f).abs() < 1e-12 && (q[1] - (-0.2) * f).abs() < 1e-12);
    // Cosine midpoint equals the linear midpoint.
    let m = lean(&eval, 4.0);
    assert!((m[0] - 0.3).abs() < 1e-12 && (m[1] - (-0.1)).abs() < 1e-12);

    // Segment 8→16 is governed by key 8's interp (smooth): midpoint agrees
    // with lerp, quarter point does not.
    let m = lean(&eval, 12.0);
    assert!((m[0] - 0.1).abs() < 1e-12 && (m[1] - (-0.05)).abs() < 1e-12);
    let q = lean(&eval, 10.0);
    let smooth_q = 0.6 + (-0.4 - 0.6) * f;
    assert!(
        (q[0] - smooth_q).abs() < 1e-12,
        "8→16 must use key 8's smooth interp"
    );

    // Clamp beyond the ends.
    assert_eq!(lean(&eval, -1.0), [0.0, 0.0]);
    assert_eq!(lean(&eval, 999.0), [-0.4, 0.1]);

    // pressure_r has a single key; scalar shape, constant everywhere.
    assert_eq!(
        eval.sample_channel("pressure_r", 5.0),
        Some(ChannelValue::Scalar(0.0))
    );
    // sway has no keys in the fixture chart.
    assert_eq!(eval.sample_channel("sway", 0.0), None);
}

#[test]
fn pulse_windows_resolve_from_sections_and_overrides() {
    let Some(fixtures) = fixtures_dir() else {
        eprintln!("skipping: no game-repo fixtures directory");
        return;
    };
    let (eval, conductor) = load(&fixtures);
    let windows = eval.pulse_windows();
    assert_eq!(windows.len(), 5);

    // All fixture pulses sit in "arrival" (ends at sample 1_920_000 = beat 80).
    // 24_000 samples per beat at the first anchor.
    let per_beat = 24_000i64;
    let ms = |m: f64| (m / 1000.0 * 48_000.0).round() as i64;

    let expect = [
        (4.0, "pulse", ms(90.0)),  // section half-width
        (6.0, "pulse", ms(120.0)), // per-pulse override
        (8.0, "pulse", ms(90.0)),
        (10.0, "press", ms(90.0)),
        (12.0, "flick", ms(90.0)),
    ];
    for (w, (beat, kind, half)) in windows.iter().zip(expect) {
        assert_eq!(w.beat, beat);
        assert_eq!(w.kind, kind);
        assert_eq!(w.center_sample, (beat as i64) * per_beat);
        assert_eq!(w.half_width_samples, half, "beat {beat} half-width");
        // Round-trip through the conductor agrees.
        assert_eq!(w.center_sample, conductor.sample_at_beat(beat));
    }

    // Kind-specific payloads survive resolution.
    assert_eq!(windows[3].strength, Some(0.8));
    assert_eq!(windows[4].direction, Some([0.0, 1.0]));
}
