//! Milestone 3 automated evidence — the [A] half only.
//!
//! The FULL reactive loop, offline and deterministic: synthetic input →
//! judge → coherence → ladder → reintegration sequencer → mixer, inside one
//! `render_offline_with` director. An attention model plays perfectly, then
//! neglects until the world full-fails and reintegrates, recovers *during*
//! reassembly (input-liveness evidence), then neglects again for a second
//! full loop.
//!
//! Assertions: (1) rungs engage in order at their thresholds; (2) full-fail
//! fires only after the hold; (3) the re-entry lands at the checkpoint —
//! the lead bus's position-gated tone reappears at the seam; (4) lead-only
//! at the seam — non-lead stems enter deep and rise to full within
//! `reassembly_bars`; (5) no clicks anywhere; (6) pulses during reassembly
//! are judged (input stays live); (7) two renders are bit-identical; (8) a
//! second fail→reintegration passes the same checks.
//!
//! Milestone 3 itself is [A+H]: these tests passing is NECESSARY, NOT
//! SUFFICIENT. The milestone closes only on Chris's controller session —
//! "grace, not glitch" — which no test, and no agent, may certify.

use flint_music::analysis::{find_tone_transitions, goertzel_power, max_step, peak, tone_envelope, TransitionKind};
use flint_music::chart_eval::ChartEval;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::event_script::EventScript;
use flint_music::judgment::{Judge, JudgmentConfig, JudgmentRecord};
use flint_music::ladder::{Ladder, LadderConfig, LadderDriver};
use flint_music::manifest::BusDecl;
use flint_music::offline::{render_offline_with, OfflineRenderConfig};
use flint_music::reintegration::{ReintegrationEvent, Reintegrator};
use flint_music::replay::{synthesize, SyntheticProfile};
use flint_music::session::StemResolver;
use flint_music::{Chart, SuiteManifest};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
const WINDOW: usize = 64;
/// 120 BPM 4/4 until suite sample 960_000, then 90 BPM 3/4.
const BAR_A: i64 = 96_000;
/// Raw render length: 30 bars of the first tempo's clock time.
const DURATION: i64 = 30 * BAR_A;
/// Attention model: perfect play, then neglect from raw bar 3 until the
/// first seam; perfect again until 4 bars after the first reassembly
/// completes; neglect until the second seam; perfect to the end.
const FALL1_RAW: i64 = 3 * BAR_A;
const RECOVER_BARS: i64 = 4;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn manifest() -> SuiteManifest {
    SuiteManifest::load(&data_dir().join("tempo_change.suite.toml")).expect("synthetic manifest")
}

/// The Milestone 2 evidence chart: lean sways through both meters, pulses
/// every 2 beats from beat 4 to 60.
fn chart() -> Chart {
    let mut toml = String::from("schema_version = 0\nsuite = \"tempo-change-test\"\n");
    for (beat, x, y) in [
        (0.0, 0.0, 0.0),
        (8.0, 0.5, -0.2),
        (16.0, -0.4, 0.3),
        (24.0, 0.3, 0.4),
        (32.0, -0.2, -0.4),
        (40.0, 0.4, 0.1),
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

/// foundation/texture: steady tones (entry-envelope + click material).
/// home_theme (the lead bus): tone gated to the first two bars of EACH
/// re-entry section, so its reappearance is position evidence.
struct SeamStems;

impl StemResolver for SeamStems {
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
        // Section starts: 0 (4/4 @ 120 → bar 96k) and 960_000 (3/4 @ 90 →
        // bar 64k). Two bars of lead tone after each.
        let gates = [(0i64, 2 * 96_000i64), (960_000, 2 * 64_000)];
        let frames: Arc<[Frame]> = (0..(DURATION + SR as i64) as usize)
            .map(|i| {
                let s = i as i64;
                let gate = bus != "home_theme"
                    || gates.iter().any(|(start, len)| (*start..start + len).contains(&s));
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

#[derive(Debug, Clone, PartialEq)]
struct FailRecord {
    at_suite: i64,
    re_entry: i64,
    seam_suite: i64,
    seam_raw: i64,
    span: i64,
    /// Raw sample when coherence first went below enter_below for this hold.
    first_below_raw: i64,
}

struct M3Run {
    audio: Vec<f32>,
    /// (raw, coherence) per chunk.
    coherence_trace: Vec<(i64, f64)>,
    /// (raw, level) per chunk.
    level_trace: Vec<(i64, usize)>,
    fails: Vec<FailRecord>,
    completes_raw: Vec<i64>,
    /// Pulse hits judged while the sequencer was reassembling.
    reassembly_hits: usize,
    /// Flat record log for determinism comparison.
    record_log: String,
}

fn run() -> M3Run {
    run_model(FALL1_RAW, DURATION)
}

/// The reactive loop with a configurable first-fall time and render length —
/// the E7 continuity sweep re-runs it at several fail timings.
fn run_model(fall1_raw: i64, duration: i64) -> M3Run {
    let manifest = manifest();
    let chart = chart();
    let cfg = JudgmentConfig::default();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &conductor).unwrap();
    let script_events = synthesize(&eval, &conductor, SyntheticProfile::Perfect);

    let mut judge = Judge::new(
        ChartEval::new(&chart, &conductor).unwrap(),
        Conductor::new(&manifest, None),
        cfg,
    );
    let mut coherence = Coherence::new(CoherenceConfig::default());
    let ladder_cfg = LadderConfig::default();
    let ff_enter = ladder_cfg.full_fail.enter_below;
    let mut ladder = Ladder::new(ladder_cfg);
    let mut driver = LadderDriver::new();
    let mut reintegrator = Reintegrator::new(manifest.reintegration.clone());

    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    let render_cfg = OfflineRenderConfig {
        duration_samples: duration,
        chunk_frames: CHUNK,
    };

    let mut out = M3Run {
        audio: Vec::new(),
        coherence_trace: Vec::new(),
        level_trace: Vec::new(),
        fails: Vec::new(),
        completes_raw: Vec::new(),
        reassembly_hits: 0,
        record_log: String::new(),
    };

    // Attention model state.
    let mut attentive = true;
    let mut fall2_raw: Option<i64> = None; // set after the first complete
    let mut cursor = 0usize; // index into script_events (sorted by sample)
    let mut prev_suite = i64::MIN;
    let mut first_below: Option<i64> = None;
    let mut records: Vec<JudgmentRecord> = Vec::new();
    let mut seq_events: Vec<ReintegrationEvent> = Vec::new();
    let mut chunk_index = 0i64;
    let preroll = {
        // PREROLL_BEATS at the first anchor: 2 beats at 120 BPM.
        (2.0 * SR as f64 * 60.0 / 120.0).round() as i64
    };

    let result = render_offline_with(&manifest, &SeamStems, &script, &render_cfg, |pos, session| {
        let raw = chunk_index * CHUNK as i64 - preroll;
        chunk_index += 1;
        if pos.sample < 0 {
            return;
        }

        // Attention transitions on raw time.
        if attentive && raw >= fall1_raw && out.fails.is_empty() {
            attentive = false;
        }
        if let Some(f2) = fall2_raw {
            if attentive && raw >= f2 && out.fails.len() < 2 {
                attentive = false;
            }
        }

        // Emit synthetic events whose suite sample falls in (prev, now].
        let now_suite = pos.sample;
        if prev_suite == i64::MIN {
            prev_suite = now_suite - 1;
        }
        while cursor < script_events.len() && script_events[cursor].sample() <= now_suite {
            let ev = &script_events[cursor];
            cursor += 1;
            if ev.sample() <= prev_suite {
                continue;
            }
            if attentive {
                judge.ingest(ev, &mut records);
            }
        }
        prev_suite = now_suite;
        judge.advance_to(now_suite, &mut records);

        // Coherence.
        let beats_per_bar = session
            .conductor
            .tempo()
            .anchor_at(now_suite.max(0))
            .map(|a| a.beats_per_bar as f64)
            .unwrap_or(4.0);
        let reassembling =
            reintegrator.phase() == flint_music::reintegration::SeqPhase::Reassembling;
        for rec in records.drain(..) {
            if reassembling {
                if let JudgmentRecord::Pulse { .. } = rec {
                    out.reassembly_hits += 1;
                }
            }
            coherence.step(std::slice::from_ref(&rec), cfg.grid_beats, beats_per_bar);
            out.record_log.push_str(&rec.to_json().to_string());
            out.record_log.push('\n');
        }
        let value = coherence.value();
        out.coherence_trace.push((raw, value));
        // Mirror the detector: the hold can only start once the ladder has
        // armed (sessions open below the threshold by construction).
        if ladder.armed() && value < ff_enter && first_below.is_none() {
            first_below = Some(raw);
        }

        // Sequencer.
        let seq = reintegrator
            .tick(value, &mut ladder, &mut driver, session, &mut seq_events)
            .expect("sequencer tick");
        out.level_trace.push((raw, seq.params.level));
        for ev in seq_events.drain(..) {
            match ev {
                ReintegrationEvent::FullFail {
                    at_suite_sample,
                    re_entry_sample,
                    seam_suite_sample,
                } => {
                    out.fails.push(FailRecord {
                        at_suite: at_suite_sample,
                        re_entry: re_entry_sample,
                        seam_suite: seam_suite_sample,
                        seam_raw: seam_suite_sample + session.timeline_offset(),
                        span: 0,
                        first_below_raw: first_below.take().unwrap_or(i64::MIN),
                    });
                }
                ReintegrationEvent::Seam {
                    re_entry_sample, ..
                } => {
                    judge.rewind_to(re_entry_sample);
                    // Replay the synthetic stream from the re-entry point.
                    cursor = script_events
                        .partition_point(|e| e.sample() < re_entry_sample);
                    prev_suite = re_entry_sample - 1;
                    attentive = true; // recovery starts DURING reassembly
                }
                ReintegrationEvent::ReassemblyComplete { .. } => {
                    out.completes_raw.push(raw);
                    if fall2_raw.is_none() {
                        fall2_raw = Some(raw + RECOVER_BARS * BAR_A);
                    }
                }
            }
        }
        if let Some(f) = out.fails.last_mut() {
            if f.span == 0 && reintegrator.phase() != flint_music::reintegration::SeqPhase::Playing
            {
                // Span becomes observable once reassembling; recompute from
                // the manifest instead: 2 bars from the re-entry bar.
                let entry_bar = session.conductor.position_at_sample(f.re_entry).bar;
                f.span = session.conductor.sample_at_bar(entry_bar + 2, 0.0) - f.re_entry;
            }
        }
    })
    .expect("offline render");

    out.audio = result.samples;
    out
}

fn expected_full_level(audio: &[f32], freq: f64, from: i64, to: i64) -> f64 {
    goertzel_power(audio, SR as u32, freq, from as usize, to as usize)
}

#[test]
fn milestone3_fall_seam_reassembly_twice_with_no_clicks() {
    let r = run();
    let audio = &r.audio;
    // Debug view: coherence + level once per bar of raw time.
    for bar in 0..(DURATION / BAR_A) {
        let raw = bar * BAR_A;
        let c = r
            .coherence_trace
            .iter()
            .find(|(s, _)| *s >= raw)
            .map(|(_, v)| *v)
            .unwrap_or(f64::NAN);
        let l = r
            .level_trace
            .iter()
            .find(|(s, _)| *s >= raw)
            .map(|(_, v)| *v)
            .unwrap_or(99);
        eprintln!("raw bar {bar:2}: coherence {c:.3}, level {l}");
    }
    let min_c = r
        .coherence_trace
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::INFINITY, f64::min);
    eprintln!("min coherence {min_c:.3}; fails {:?}", r.fails);
    assert_eq!(audio.len(), DURATION as usize * 2);
    assert!(peak(audio) > 0.2);

    // --- two full fail→reintegration loops ---------------------------------
    assert_eq!(
        r.fails.len(),
        2,
        "expected exactly two full-fails: {:?}",
        r.fails
    );
    assert_eq!(r.completes_raw.len(), 2, "both reassemblies completed");

    let ladder_cfg = LadderConfig::default();

    // --- (1) rungs engage in configured order ------------------------------
    // During the first fall (raw in [FALL1, first seam]), the sequence of
    // distinct levels must be strictly increasing 0→…→3 (no skips backwards).
    let seam1 = r.fails[0].seam_raw;
    let mut seen = Vec::new();
    for &(raw, level) in &r.level_trace {
        if (FALL1_RAW..seam1).contains(&raw) && seen.last() != Some(&level) {
            seen.push(level);
        }
    }
    assert!(
        seen.windows(2).all(|w| w[1] > w[0]),
        "levels must engage in order during the fall: {seen:?}"
    );
    assert_eq!(*seen.last().unwrap(), 3, "the fall reaches the deepest rung");

    // Each engagement happens at (or just past) its configured threshold. A
    // miss impulse can legitimately cross two narrow rungs in one step, so
    // look for the first trace entry at or past each level.
    for (i, rung) in ladder_cfg.rungs.iter().enumerate() {
        let at = r
            .level_trace
            .iter()
            .find(|(raw, l)| (FALL1_RAW..seam1).contains(raw) && *l >= i + 1)
            .map(|(raw, _)| *raw)
            .expect("rung engaged");
        let coh = r
            .coherence_trace
            .iter()
            .find(|(raw, _)| *raw == at)
            .map(|(_, v)| *v)
            .unwrap();
        assert!(
            coh < rung.enter_below + 0.02,
            "rung {} engaged at coherence {coh:.3}, threshold {}",
            rung.name,
            rung.enter_below
        );
    }

    // --- (2) full-fail fires only after the hold ---------------------------
    for f in &r.fails {
        assert!(f.first_below_raw > i64::MIN, "hold start recorded");
    }
    let hold_samples = (ladder_cfg.full_fail.hold_ms / 1000.0 * SR as f64) as i64;
    let fail1_raw = r
        .level_trace
        .iter()
        .zip(&r.coherence_trace)
        .find(|((raw, _), _)| *raw >= r.fails[0].seam_raw)
        .map(|((raw, _), _)| *raw)
        .unwrap_or(r.fails[0].seam_raw);
    assert!(
        fail1_raw - r.fails[0].first_below_raw >= hold_samples,
        "full-fail fired {} samples after first dip; hold is {hold_samples}",
        fail1_raw - r.fails[0].first_below_raw
    );

    // --- (3) re-entry lands at the checkpoint ------------------------------
    // The lead bus's tone is gated to each section's first two bars, so a
    // Rise must land near each seam. With the pickup cue the lead's energy
    // legitimately begins up to `pickup_beats` early (the spin-up sweeping
    // into the tone's band), so the window opens that far back — but never
    // earlier.
    let conductor = Conductor::new(&manifest(), None);
    let env = tone_envelope(audio, SR, 880.0, WINDOW);
    let transitions = find_tone_transitions(&env, WINDOW);
    let tol = (2 * WINDOW + CHUNK) as i64 + 2_000;
    for f in &r.fails {
        assert_eq!(
            f.re_entry,
            if f.at_suite < 960_000 { 0 } else { 960_000 },
            "checkpoint is the latest re-entry section at the fail"
        );
        let entry_beat = conductor.position_at_sample(f.re_entry).beat;
        let beat_samples = conductor.sample_at_beat(entry_beat + 1.0) - f.re_entry;
        let pickup = (ladder_cfg.seam_pickup_beats * beat_samples as f64) as i64;
        let rise = transitions.iter().find(|(s, k)| {
            *k == TransitionKind::Rise
                && (f.seam_raw - pickup - tol..=f.seam_raw + tol).contains(s)
        });
        assert!(
            rise.is_some(),
            "no lead-bus rise in the pickup window of seam raw {} (transitions {transitions:?})",
            f.seam_raw
        );
    }

    // --- (4) lead-only at the seam; others rise to full within the span ----
    for f in &r.fails {
        assert!(f.span > 0);
        let probe = 12_000i64; // 250 ms probe windows
        let full = expected_full_level(audio, 220.0, BAR_A, BAR_A + probe);
        let at_seam = expected_full_level(audio, 220.0, f.seam_raw, f.seam_raw + probe);
        let at_end = expected_full_level(
            audio,
            220.0,
            f.seam_raw + f.span,
            f.seam_raw + f.span + probe,
        );
        assert!(
            at_seam < full / 100.0,
            "foundation must enter ≥40 dB down at the seam: {at_seam:e} vs full {full:e}"
        );
        assert!(
            at_end > full / 4.0,
            "foundation must be near full after reassembly: {at_end:e} vs {full:e}"
        );
        let mid = expected_full_level(
            audio,
            220.0,
            f.seam_raw + f.span / 2,
            f.seam_raw + f.span / 2 + probe,
        );
        assert!(
            at_seam < mid && mid < at_end * 1.5,
            "entry envelope must rise monotonically: {at_seam:e} → {mid:e} → {at_end:e}"
        );
    }

    // --- (5) no clicks anywhere -------------------------------------------
    let step = max_step(audio);
    assert!(step < 0.25, "discontinuity detected: max step {step}");

    // --- (6) input stays live through reassembly ---------------------------
    assert!(
        r.reassembly_hits > 0,
        "pulses during reassembly must be judged (got {})",
        r.reassembly_hits
    );
}

/// E7 continuity sweep: the seam stays click-free and lands the lead-bus
/// tone wherever the fail happens — early, mid, and late in the content
/// (the late fall crosses suite sample 960_000, so its checkpoint is the
/// SECOND re-entry section, exercising the other manifest path).
#[test]
fn seam_sweep_fail_timings_stay_click_free() {
    for fall1 in [2 * BAR_A + BAR_A / 2, 4 * BAR_A, 7 * BAR_A] {
        let r = run_model(fall1, 20 * BAR_A);
        for bar in 0..20 {
            let raw = bar * BAR_A;
            if let Some((_, v)) = r.coherence_trace.iter().find(|(s, _)| *s >= raw) {
                eprintln!("  fall1={} raw bar {bar:2}: coherence {v:.3}", fall1 / BAR_A);
            }
        }
        assert!(
            !r.fails.is_empty(),
            "fall at raw {fall1} never full-failed (min coherence {:?})",
            r.coherence_trace
                .iter()
                .map(|(_, v)| *v)
                .fold(f64::INFINITY, f64::min)
        );
        let f = &r.fails[0];
        let expected_re_entry = if f.at_suite < 960_000 { 0 } else { 960_000 };
        assert_eq!(f.re_entry, expected_re_entry, "checkpoint for fall at {fall1}");

        let step = max_step(&r.audio);
        assert!(step < 0.25, "fall at raw {fall1}: max step {step}");

        let env = tone_envelope(&r.audio, SR, 880.0, WINDOW);
        let transitions = find_tone_transitions(&env, WINDOW);
        let tol = (2 * WINDOW + CHUNK) as i64 + 2_000;
        let conductor = Conductor::new(&manifest(), None);
        let entry_beat = conductor.position_at_sample(f.re_entry).beat;
        let beat_samples = conductor.sample_at_beat(entry_beat + 1.0) - f.re_entry;
        let pickup =
            (LadderConfig::default().seam_pickup_beats * beat_samples as f64) as i64;
        assert!(
            transitions.iter().any(|(s, k)| *k == TransitionKind::Rise
                && (f.seam_raw - pickup - tol..=f.seam_raw + tol).contains(s)),
            "fall at raw {fall1}: no lead rise in the pickup window of seam {} ({transitions:?})",
            f.seam_raw
        );
        eprintln!(
            "sweep fall1={} bars: fail at suite {}, re-entry {}, seam raw {}, max step {step:.3}",
            fall1 / BAR_A,
            f.at_suite,
            f.re_entry,
            f.seam_raw
        );
    }
}

#[test]
fn milestone3_is_deterministic_bit_for_bit() {
    let a = run();
    let b = run();
    assert_eq!(a.audio, b.audio, "renders must be bit-identical");
    assert_eq!(a.record_log, b.record_log, "judgment must be bit-identical");
    assert_eq!(a.fails, b.fails);
}
