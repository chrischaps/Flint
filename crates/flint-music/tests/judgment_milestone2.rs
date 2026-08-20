//! Milestone 2 automated evidence — the [A] half only.
//!
//! Runs synthetic input sessions through the full chart-eval → judgment →
//! coherence pipeline, headless, over the synthetic tempo/meter-change
//! manifest (120 BPM 4/4 → 90 BPM 3/4). Asserts, deterministically:
//! perfect play drives coherence above 0.9 within four bars and keeps it
//! there across the change; neglect decays smoothly (no cliff steps);
//! a single dropped pulse is nearly invisible (GDD); two runs are
//! bit-identical.
//!
//! Milestone 2 itself is [A+H]: these tests passing is NECESSARY, NOT
//! SUFFICIENT. The milestone closes only on Chris's controller feel check
//! (coherence visibly rising with attention, decaying with neglect), which
//! no test — and no agent — may certify.

use flint_music::chart_eval::ChartEval;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::input_stream::InputEvent;
use flint_music::judgment::{Judge, JudgmentConfig, JudgmentRecord};
use flint_music::replay::{synthesize, SyntheticProfile};
use flint_music::{Chart, SuiteManifest};
use std::path::Path;

fn manifest() -> SuiteManifest {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tempo_change.suite.toml");
    SuiteManifest::load(&path).expect("synthetic manifest")
}

/// Lean sways through both meters; pulses every 2 beats from beat 4 to 60,
/// crossing the tempo/meter change at beat 40 (sample 960_000).
fn chart() -> Chart {
    let mut toml = String::from("schema_version = 0\nsuite = \"tempo-change-test\"\n");
    for (beat, x, y) in [
        (0.0, 0.0, 0.0),
        (8.0, 0.5, -0.2),
        (16.0, -0.4, 0.3),
        (24.0, 0.3, 0.4),
        (32.0, -0.2, -0.4),
        (40.0, 0.4, 0.1), // the change lands here
        (48.0, -0.3, -0.3),
        (56.0, 0.2, 0.3),
        (60.0, 0.0, 0.0),
    ] {
        toml.push_str(&format!(
            "[[curves]]\nchannel = \"lean\"\nbeat = {beat:.1}\nvalue = [{x:.1}, {y:.1}]\ninterp = \"smooth\"\n"
        ));
    }
    let mut beat = 4.0;
    while beat <= 60.0 {
        toml.push_str(&format!("[[pulses]]\nbeat = {beat:.1}\n"));
        beat += 2.0;
    }
    Chart::parse(&toml).expect("test chart")
}

struct RunResult {
    /// (bar, coherence value) sampled after each processed event.
    trace: Vec<(i64, f64)>,
    /// Every value transition for smoothness checks.
    values: Vec<f64>,
    records_debug: String,
    hits: usize,
    misses: usize,
    final_value: f64,
}

fn run(events: &[InputEvent]) -> RunResult {
    let manifest = manifest();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart(), &conductor).unwrap();
    let cfg = JudgmentConfig::default();
    let mut judge = Judge::new(
        ChartEval::new(&chart(), &conductor).unwrap(),
        Conductor::new(&manifest, None),
        cfg,
    );
    let mut coherence = Coherence::new(CoherenceConfig::default());
    let _ = eval;

    let mut out = Vec::new();
    let mut result = RunResult {
        trace: Vec::new(),
        values: vec![coherence.value()],
        records_debug: String::new(),
        hits: 0,
        misses: 0,
        final_value: 0.0,
    };
    let fold = |records: &mut Vec<JudgmentRecord>,
                at_sample: i64,
                coherence: &mut Coherence,
                result: &mut RunResult,
                conductor: &Conductor| {
        let beats_per_bar = conductor
            .tempo()
            .anchor_at(at_sample.max(0))
            .map(|a| a.beats_per_bar as f64)
            .unwrap_or(4.0);
        // Record per-record transitions for the smoothness assertion.
        for rec in records.iter() {
            match rec {
                JudgmentRecord::Pulse { .. } => result.hits += 1,
                JudgmentRecord::Miss { .. } => result.misses += 1,
                _ => {}
            }
            coherence.step(std::slice::from_ref(rec), cfg.grid_beats, beats_per_bar);
            result.values.push(coherence.value());
        }
        for rec in records.drain(..) {
            result.records_debug.push_str(&rec.to_json().to_string());
            result.records_debug.push('\n');
        }
        let bar = conductor.position_at_sample(at_sample).bar;
        result.trace.push((bar, coherence.value()));
    };

    for ev in events {
        judge.ingest(ev, &mut out);
        fold(
            &mut out,
            ev.sample(),
            &mut coherence,
            &mut result,
            &conductor,
        );
    }
    let last = events.last().map(|e| e.sample()).unwrap_or(0);
    judge.finish(&mut out);
    fold(&mut out, last, &mut coherence, &mut result, &conductor);
    result.final_value = coherence.value();
    result
}

fn events_for(profile: SyntheticProfile) -> Vec<InputEvent> {
    let manifest = manifest();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart(), &conductor).unwrap();
    synthesize(&eval, &conductor, profile)
}

#[test]
fn perfect_play_exceeds_090_by_bar_four_and_holds_across_the_change() {
    let result = run(&events_for(SyntheticProfile::Perfect));
    assert_eq!(result.misses, 0, "perfect play must miss nothing");
    assert_eq!(result.hits, 29, "every window judged (beats 4..=60 step 2)");

    // Debug view: last trace value per bar.
    let mut per_bar: Vec<(i64, f64)> = Vec::new();
    for &(bar, v) in &result.trace {
        match per_bar.last_mut() {
            Some((b, val)) if *b == bar => *val = v,
            _ => per_bar.push((bar, v)),
        }
    }
    eprintln!("per-bar coherence: {per_bar:?}");

    let first_idx = result
        .trace
        .iter()
        .position(|(_, v)| *v > 0.9)
        .expect("coherence must exceed 0.9");
    let first_bar = result.trace[first_idx].0;
    assert!(first_bar <= 3, "0.9 reached by bar {first_bar} (0-based)");

    // From the moment it crosses 0.9 it stays there — including across the
    // tempo/meter change at bar 10 — to the end of the chart.
    let after = &result.trace[first_idx..];
    assert!(
        after.iter().any(|(bar, _)| *bar >= 12),
        "trace crosses the change"
    );
    for (bar, v) in after {
        assert!(*v > 0.9, "coherence {v:.3} sagged at bar {bar}");
    }
}

#[test]
fn neglect_after_attention_decays_smoothly_with_no_cliffs() {
    // The GDD's decay claim is about ATTENTION WITHDRAWN: play well, then
    // go neutral. (Pure neglect from a cold start just never rises — the
    // gentle chart's tracking floor sits near the initial value.)
    let manifest = manifest();
    let conductor = Conductor::new(&manifest, None);
    let cutoff = conductor.sample_at_beat(30.0);
    let mut events: Vec<InputEvent> = events_for(SyntheticProfile::Perfect)
        .into_iter()
        .filter(|e| e.sample() <= cutoff)
        .collect();
    events.extend(
        events_for(SyntheticProfile::Neglect)
            .into_iter()
            .filter(|e| e.sample() > cutoff),
    );

    let result = run(&events);
    let at_cutoff = result
        .trace
        .iter()
        .take_while(|(bar, _)| *bar <= 6) // beat 30 = mid-bar 7 (0-based)
        .last()
        .map(|(_, v)| *v)
        .unwrap();
    let min_after = result
        .trace
        .iter()
        .filter(|(bar, _)| *bar >= 7)
        .map(|(_, v)| *v)
        .fold(f64::INFINITY, f64::min);
    eprintln!(
        "attention->neglect: at cutoff {at_cutoff:.3}, min after {min_after:.3}, final {:.3}",
        result.final_value
    );
    assert!(at_cutoff > 0.9, "attentive play must be high before cutoff");
    assert!(
        result.final_value < at_cutoff - 0.2,
        "withdrawing attention must visibly slide coherence ({:.3} from {at_cutoff:.3})",
        result.final_value
    );

    // Smoothness: no single record moves the value more than the largest
    // legal per-record step (miss impulse at defaults = w_pulse *
    // impulse_gain = 0.06, always >= the tracking-grid alpha step).
    let cfg = CoherenceConfig::default();
    let max_step = cfg.w_pulse * cfg.impulse_gain * cfg.miss_penalty + 1e-9;
    for w in result.values.windows(2) {
        let delta = (w[1] - w[0]).abs();
        assert!(delta <= max_step, "cliff step of {delta:.4}");
    }
}

#[test]
fn a_single_dropped_pulse_is_nearly_invisible() {
    let perfect = events_for(SyntheticProfile::Perfect);
    // Drop exactly one mid-piece pulse (the one nearest beat 20).
    let manifest = manifest();
    let conductor = Conductor::new(&manifest, None);
    let dropped_sample = conductor.sample_at_beat(20.0);
    let mut one_miss: Vec<InputEvent> = perfect.clone();
    let idx = one_miss
        .iter()
        .position(|e| matches!(e, InputEvent::Pulse(p) if p.sample == dropped_sample))
        .expect("pulse at beat 20 exists");
    one_miss.remove(idx);

    let a = run(&perfect);
    let b = run(&one_miss);
    assert_eq!(b.misses, 1);

    // The dip is bounded and the endings converge: a single mistake is
    // nearly invisible (GDD), erased well before the piece ends.
    let max_gap = a
        .values
        .iter()
        .zip(&b.values)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f64, f64::max);
    assert!(max_gap < 0.1, "single miss dipped {max_gap:.3}");
    assert!(
        (a.final_value - b.final_value).abs() < 0.01,
        "endings diverged: {:.3} vs {:.3}",
        a.final_value,
        b.final_value
    );
}

#[test]
fn pulse_windows_are_exact_on_both_sides_of_the_change() {
    let result = run(&events_for(SyntheticProfile::Perfect));
    // Every hit in the debug stream must carry err_ms 0.0 — dead center
    // under 120 BPM 4/4 and 90 BPM 3/4 alike.
    let mut checked = 0;
    for line in result.records_debug.lines() {
        if line.contains("\"t\":\"pulse\"") {
            assert!(line.contains("\"err_ms\":0.0"), "off-center hit: {line}");
            checked += 1;
        }
    }
    assert_eq!(checked, 29);
}

#[test]
fn the_pipeline_is_deterministic_bit_for_bit() {
    let a = run(&events_for(SyntheticProfile::Perfect));
    let b = run(&events_for(SyntheticProfile::Perfect));
    assert_eq!(a.records_debug, b.records_debug);
    assert_eq!(a.values, b.values);

    let c = run(&events_for(SyntheticProfile::LateMs(25.0)));
    let d = run(&events_for(SyntheticProfile::LateMs(25.0)));
    assert_eq!(c.records_debug, d.records_debug);
}
