//! Haptics decision-layer pins (ADR 0026): the pure evaluator's contract,
//! driven synthetically against the tempo-change fixture — no audio, no
//! hardware, no `ChartSession`. Determinism here is what lets the driver
//! sit inside live sessions without ever touching the replay/render paths
//! (which never construct one).

use flint_music::chart_session::PulseKind;
use flint_music::haptics::{HapticEvent, HapticsConfig, HapticsDriver};
use flint_music::reintegration::{ReintegrationEvent, SeqPhase};
use flint_music::{Conductor, Grid, SuiteManifest};
use std::path::Path;

fn conductor() -> Conductor {
    let manifest = SuiteManifest::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tempo_change.suite.toml"),
    )
    .expect("fixture manifest");
    Conductor::new(&manifest, None)
}

fn active_cfg() -> HapticsConfig {
    HapticsConfig::parse(
        r#"
schema_version = 0
[timing]
lead_ms = 90.0
late_drop_ms = 30.0
[tick]
grid = "beat"
weak = 0.35
duration_ms = 25.0
[thump]
strong = 0.55
weak = 0.25
duration_ms = 60.0
[grind]
strong = 0.45
[pickup]
weak = 0.4
strong = 0.15
duration_ms = 30.0
count = 2
"#,
    )
    .unwrap()
}

/// Run the driver over a span at a fixed hop, Playing phase, no pulses or
/// sequencer events — pure grid behavior.
fn run_grid(driver: &mut HapticsDriver, c: &Conductor, from: i64, to: i64, hop: i64) -> Vec<HapticEvent> {
    let mut out = Vec::new();
    let mut now = from;
    while now < to {
        driver.evaluate(c, now, SeqPhase::Playing, 0.0, 2.0, &[], &[], &mut out);
        now += hop;
    }
    out
}

#[test]
fn inert_config_emits_nothing_ever() {
    let c = conductor();
    let mut d = HapticsDriver::new(HapticsConfig::default());
    let pulses = vec![(1000, 5.0, PulseKind::Hit)];
    let seam = [ReintegrationEvent::Seam {
        raw_sample: 48_000,
        timeline_offset: 24_000,
        re_entry_sample: 24_000,
    }];
    let mut out = Vec::new();
    for i in 0..500 {
        d.evaluate(&c, i * 960, SeqPhase::Playing, 0.5, 2.0, &pulses, &seam, &mut out);
    }
    assert!(out.is_empty(), "inert built-ins must never emit: {out:?}");
}

#[test]
fn ticks_land_on_the_beat_grid_across_the_tempo_change() {
    let c = conductor();
    let mut d = HapticsDriver::new(active_cfg());
    // The fixture changes 120 BPM 4/4 → 90 BPM 3/4 mid-way; run well across
    // it (~30 s at 48 kHz) with a coarse 30 ms hop (worse than any front
    // end) and require every burst to sit exactly on a conductor beat.
    let sr = 48_000i64;
    let out = run_grid(&mut d, &c, 0, 30 * sr, (0.03 * sr as f64) as i64);
    let bursts: Vec<i64> = out
        .iter()
        .filter_map(|e| match e {
            HapticEvent::Burst { at_suite_sample, .. } => Some(*at_suite_sample),
            _ => None,
        })
        .collect();
    assert!(bursts.len() > 40, "expected a tick per beat over 30 s: {}", bursts.len());
    // Strictly increasing, no duplicates (the lookahead window slides over
    // each tick exactly once).
    for w in bursts.windows(2) {
        assert!(w[1] > w[0], "duplicate or reordered tick: {w:?}");
    }
    // Each burst is a grid point: next_grid_sample from just before it
    // returns exactly it.
    for &b in &bursts {
        assert_eq!(
            c.next_grid_sample(b - 1, Grid::Beat),
            b,
            "tick off-grid at {b}"
        );
    }
}

#[test]
fn thump_fires_on_hits_only() {
    let c = conductor();
    let mut cfg = active_cfg();
    cfg.tick_grid = flint_music::TickGrid::Off; // isolate the thump
    let mut d = HapticsDriver::new(cfg);
    let pulses = vec![
        (1000, 5.0, PulseKind::Hit),
        (1100, 0.0, PulseKind::Miss),
        (1200, 0.0, PulseKind::Spurious),
        (1300, -3.0, PulseKind::Hit),
    ];
    let mut out = Vec::new();
    d.evaluate(&c, 2000, SeqPhase::Playing, 0.0, 2.0, &pulses, &[], &mut out);
    let thumps: Vec<_> = out
        .iter()
        .filter(|e| matches!(e, HapticEvent::Immediate { .. }))
        .collect();
    assert_eq!(thumps.len(), 2, "two hits → two thumps, silence for miss/spurious: {out:?}");
    assert!(
        !out.iter().any(|e| matches!(e, HapticEvent::Burst { .. })),
        "thumps must be Immediate (no lead, no late-drop — a scheduled 'now' \
         is ~latency late in raw-clock terms and would be dropped): {out:?}"
    );
    for t in thumps {
        if let HapticEvent::Immediate { strong, .. } = t {
            assert!(*strong > 0.5);
        }
    }
}

#[test]
fn grind_follows_rewind_and_seam_flushes() {
    let c = conductor();
    let mut d = HapticsDriver::new(active_cfg());
    let mut out = Vec::new();
    // Playing: no grind.
    d.evaluate(&c, 0, SeqPhase::Playing, 0.0, 2.0, &[], &[], &mut out);
    assert!(!out.iter().any(|e| matches!(e, HapticEvent::Continuous { .. })));
    // Failing with rising rewind: continuous levels rise (delta-gated).
    let mut levels = Vec::new();
    for i in 0..10 {
        let mut step = Vec::new();
        d.evaluate(&c, 1000 * i, SeqPhase::Failing, i as f64 / 10.0, 2.0, &[], &[], &mut step);
        for e in &step {
            if let HapticEvent::Continuous { strong, .. } = e {
                levels.push(*strong);
            }
        }
    }
    assert!(levels.len() >= 3, "grind updates as the rewind deepens: {levels:?}");
    assert!(
        levels.windows(2).all(|w| w[1] > w[0]),
        "grind grows with the spin-down: {levels:?}"
    );
    // Seam: flush, grind released, grid re-seeds.
    let mut step = Vec::new();
    let seam = [ReintegrationEvent::Seam {
        raw_sample: 100_000,
        timeline_offset: 50_000,
        re_entry_sample: 50_000,
    }];
    d.evaluate(&c, 50_000, SeqPhase::Reassembling, 0.0, 2.0, &[], &seam, &mut step);
    assert!(
        step.iter().any(|e| matches!(e, HapticEvent::Flush)),
        "seam must flush: {step:?}"
    );
}

#[test]
fn pickup_ticks_land_on_the_beats_before_the_seam() {
    let c = conductor();
    let mut cfg = active_cfg();
    cfg.tick_grid = flint_music::TickGrid::Off;
    cfg.thump.strong = 0.0;
    cfg.thump.weak = 0.0;
    cfg.grind.strong = 0.0;
    let mut d = HapticsDriver::new(cfg);
    let seam = 8 * 48_000i64; // fixture: still in the 120 BPM span
    let re_entry = 4 * 48_000i64;
    let full_fail = [ReintegrationEvent::FullFail {
        at_suite_sample: seam - 96_000,
        re_entry_sample: re_entry,
        seam_suite_sample: seam,
    }];
    let mut out = Vec::new();
    d.evaluate(&c, seam - 96_000, SeqPhase::Failing, 0.0, 2.0, &[], &full_fail, &mut out);
    let bursts: Vec<i64> = out
        .iter()
        .filter_map(|e| match e {
            HapticEvent::Burst { at_suite_sample, .. } => Some(*at_suite_sample),
            _ => None,
        })
        .collect();
    // 120 BPM at 48 kHz = 24 000 samples per beat: "and-a—" at seam − 1 and
    // seam − 2 beats.
    assert_eq!(bursts.len(), 2, "pickup count 2: {bursts:?}");
    assert!(bursts.contains(&(seam - 24_000)), "{bursts:?}");
    assert!(bursts.contains(&(seam - 48_000)), "{bursts:?}");
}

#[test]
fn evaluation_is_bit_identical_across_runs() {
    let c = conductor();
    let run = || {
        let mut d = HapticsDriver::new(active_cfg());
        let mut trace = Vec::new();
        let sr = 48_000i64;
        let mut now = 0i64;
        while now < 20 * sr {
            // A deterministic little scenario: a hit every ~2 s, a fail
            // window mid-run with rising rewind.
            let pulses = if now % (2 * sr) < 960 {
                vec![(now - 500, 4.0, PulseKind::Hit)]
            } else {
                vec![]
            };
            let (phase, rewind) = if (8 * sr..10 * sr).contains(&now) {
                (SeqPhase::Failing, (now - 8 * sr) as f64 / (2 * sr) as f64)
            } else {
                (SeqPhase::Playing, 0.0)
            };
            let mut out = Vec::new();
            d.evaluate(&c, now, phase, rewind, 2.0, &pulses, &[], &mut out);
            for e in out {
                trace.push(format!("{e:?}"));
            }
            now += 960;
        }
        trace
    };
    assert_eq!(run(), run(), "same inputs, same events, bit for bit");
}

#[test]
fn config_event_converts_ms_to_samples() {
    let d = HapticsDriver::new(active_cfg());
    match d.config_event(48_000) {
        HapticEvent::Config {
            lead_samples,
            late_drop_samples,
            gain,
        } => {
            assert_eq!(lead_samples, 4320); // 90 ms at 48 kHz
            assert_eq!(late_drop_samples, 1440); // 30 ms
            assert_eq!(gain, 1.0);
        }
        other => panic!("expected Config, got {other:?}"),
    }
}
