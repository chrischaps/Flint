//! W2-13 automated evidence (plan-world2-scene): the World II surface —
//! meter/tempo changes in playable content, the full verb space (sway,
//! pressure, press depth, flick direction), params-carrying cues, and
//! composed degraded alternates — works end to end, deterministically.
//!
//! Hermetic by convention: a synthetic manifest mirroring world2's
//! tempo/meter structure (5/4@96 → 3/4@96 → 3/4@120 → 4/4@72) at reduced
//! length, tone stems (including alternate stems via `load_alternate`),
//! never the game repo's audio. The real-asset checks live in the CLI
//! validation + anatomy runs (`logs/tuning/world2-anatomy-*`).

use flint_music::chart_eval::ChartEval;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::event_script::EventScript;
use flint_music::judgment::{JsonlWriter, Judge, JudgmentConfig, JudgmentRecord, LeanMode};
use flint_music::ladder::{Ladder, LadderConfig};
use flint_music::manifest::BusDecl;
use flint_music::offline::{render_offline_with, OfflineRenderConfig};
use flint_music::reintegration::{ReintegrationEvent, Reintegrator};
use flint_music::replay::{synthesize, SyntheticProfile};
use flint_music::session::StemResolver;
use flint_music::{Chart, ChartCore, ConductedFrame, SuiteManifest};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::PathBuf;
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
/// Compressed world2 structure, integer-sample bars throughout:
/// arrival 4×5/4@96 (150k/bar) → call 8×3/4@96 (90k) → trial 8×3/4@120
/// (72k) → departure 4×4/4@72 (160k, last bar is tail).
const SEC_CALL_S: i64 = 600_000; // beat 20
const SEC_TRIAL_S: i64 = 1_320_000; // beat 44
const SEC_DEPART_S: i64 = 1_896_000; // beat 68
const DURATION: i64 = 2_536_000; // beat 84
/// Neglect begins at this raw sample (mid-call).
const FALL_RAW: i64 = 900_000;

fn manifest() -> SuiteManifest {
    SuiteManifest::parse(&format!(
        r#"
schema_version = 0
[suite]
id = "world2-test"
title = "World II synthetic"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 96.0
time_signature = [5, 4]
[[tempo]]
sample = {SEC_CALL_S}
bpm = 96.0
time_signature = [3, 4]
[[tempo]]
sample = {SEC_TRIAL_S}
bpm = 120.0
time_signature = [3, 4]
[[tempo]]
sample = {SEC_DEPART_S}
bpm = 72.0
time_signature = [4, 4]
[[sections]]
name = "arrival"
start_sample = 0
pulse_window_ms = 140
[[sections]]
name = "call"
start_sample = {SEC_CALL_S}
pulse_window_ms = 110
[[sections]]
name = "trial"
start_sample = {SEC_TRIAL_S}
pulse_window_ms = 80
[[sections]]
name = "departure"
start_sample = {SEC_DEPART_S}
pulse_window_ms = 120
[reintegration]
re_entry_sections = ["arrival", "call", "trial"]
lead_bus = "home_theme"
reassembly_bars = 2
[buses.foundation]
file = "tone-foundation"
[buses.harmony]
file = "tone-harmony"
[buses.world_voice]
file = "tone-world-voice"
[buses.home_theme]
file = "tone-home-theme"
[buses.child_motif]
silent = true
[buses.texture]
file = "tone-texture"
[[degraded_alternates]]
bus = "harmony"
file = "tone-harmony-thin"
from_sample = {SEC_CALL_S}
to_sample = {SEC_DEPART_S}
[[degraded_alternates]]
bus = "world_voice"
file = "tone-world-voice-thin"
from_sample = {SEC_TRIAL_S}
to_sample = {SEC_DEPART_S}
"#
    ))
    .expect("synthetic world2 manifest")
}

/// The full-verb-space chart: lean through every section, sway from the
/// call on, pressure swells in the trial, press/flick answers, params cues.
fn chart() -> Chart {
    let mut t = String::from("schema_version = 0\nsuite = \"world2-test\"\n");
    let mut lean = |beat: f64, x: f64, y: f64| {
        t.push_str(&format!(
            "[[curves]]\nchannel = \"lean\"\nbeat = {beat:.2}\nvalue = [{x:.2}, {y:.2}]\ninterp = \"smooth\"\n"
        ));
    };
    // arrival: one big target per 5/4 bar.
    lean(0.0, 0.0, 0.0);
    lean(5.0, 0.75, 0.0);
    lean(10.0, -0.75, 0.0);
    lean(15.0, 0.0, 0.75);
    // call: gentle lap.
    lean(20.0, 0.7, 0.0);
    lean(26.0, 0.0, -0.7);
    lean(32.0, -0.7, 0.0);
    lean(38.0, 0.0, 0.7);
    // trial: faster lap, wider.
    lean(44.0, 0.85, 0.0);
    lean(50.0, 0.0, -0.85);
    lean(56.0, -0.85, 0.0);
    lean(62.0, 0.0, 0.85);
    // departure: settle.
    lean(68.0, 0.5, 0.0);
    lean(76.0, 0.0, 0.0);
    drop(lean);
    for (beat, x, y) in [
        (20.0, -0.5, 0.0),
        (26.0, 0.0, 0.5),
        (32.0, 0.5, 0.0),
        (38.0, 0.0, -0.5),
        (44.0, -0.6, 0.0),
        (50.0, 0.0, 0.6),
        (56.0, 0.6, 0.0),
        (62.0, 0.0, -0.6),
        (68.0, 0.0, 0.0),
    ] {
        t.push_str(&format!(
            "[[curves]]\nchannel = \"sway\"\nbeat = {beat:.2}\nvalue = [{x:.2}, {y:.2}]\ninterp = \"smooth\"\n"
        ));
    }
    for (beat, v) in [
        (45.0, 0.0),
        (48.0, 0.7),
        (53.0, 0.0),
        (57.0, 0.0),
        (60.0, 0.7),
        (65.0, 0.0),
    ] {
        t.push_str(&format!(
            "[[curves]]\nchannel = \"pressure_r\"\nbeat = {beat:.2}\nvalue = [{v:.2}]\ninterp = \"smooth\"\n"
        ));
    }
    // Pulses: plain (arrival/departure + trial off-beats), press answers,
    // flick accents. Same-kind windows sit far beyond 2× window apart.
    for beat in [10.0, 15.0, 47.0, 53.0, 59.0, 65.0, 68.0, 72.0, 76.0] {
        t.push_str(&format!("[[pulses]]\nbeat = {beat:.2}\n"));
    }
    for (beat, strength) in [(24.0, 0.6), (26.0, 0.6), (36.0, 0.7), (44.0, 0.9), (56.0, 0.9)] {
        t.push_str(&format!(
            "[[pulses]]\nbeat = {beat:.2}\nkind = \"press\"\nstrength = {strength:.2}\n"
        ));
    }
    for (beat, dx, dy) in [(28.0, 1.0, 0.0), (40.0, 0.0, 1.0), (50.0, -1.0, 0.0)] {
        t.push_str(&format!(
            "[[pulses]]\nbeat = {beat:.2}\nkind = \"flick\"\ndirection = [{dx:.1}, {dy:.1}]\n"
        ));
    }
    // Params-carrying cues (ADR 0033).
    t.push_str("[[cues]]\nbeat = 5.0\ncue = \"tumble\"\nparams = { roll = 0.8 }\n");
    t.push_str("[[cues]]\nbeat = 20.0\ncue = \"call\"\nparams = { cell = \"a\", index = 1 }\n");
    t.push_str("[[cues]]\nbeat = 44.0\ncue = \"surge\"\nparams = { depth = 0.6 }\n");
    t.push_str("[[cues]]\nbeat = 56.0\ncue = \"surge\"\nparams = { depth = 1.0 }\n");
    t.push_str("[[cues]]\nbeat = 76.0\ncue = \"final_call\"\n");
    t.push_str("[[intensity]]\nbeat = 0.0\nvalue = 0.25\n[[intensity]]\nbeat = 44.0\nvalue = 0.8\n");
    Chart::parse(&t).expect("test chart")
}

struct ToneStems;

impl ToneStems {
    fn tone(freq: f64, amp: f32) -> StaticSoundData {
        let frames: Arc<[Frame]> = (0..(DURATION + SR as i64) as usize)
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
}

impl StemResolver for ToneStems {
    fn load(&self, bus: &str, decl: &BusDecl) -> flint_core::Result<Option<StaticSoundData>> {
        if decl.file.is_none() {
            return Ok(None);
        }
        let (freq, amp) = match bus {
            "foundation" => (110.0, 0.20f32),
            "harmony" => (330.0, 0.15),
            "world_voice" => (523.0, 0.15),
            "home_theme" => (880.0, 0.20),
            "texture" => (6_000.0, 0.08),
            other => panic!("playable bus without a tone: {other}"),
        };
        Ok(Some(Self::tone(freq, amp)))
    }

    fn load_alternate(&self, bus: &str, _file: &str) -> flint_core::Result<Option<StaticSoundData>> {
        // Distinct pitches so the crossfade is audibly (and analytically) a
        // different voice, quieter — the "authored thinning".
        let (freq, amp) = match bus {
            "harmony" => (165.0, 0.08f32),
            "world_voice" => (262.0, 0.08),
            other => panic!("unexpected alternate bus: {other}"),
        };
        Ok(Some(Self::tone(freq, amp)))
    }
}

fn arrival_judge(coh_weights: bool) -> (Judge, Coherence, Vec<flint_music::InputEvent>, usize) {
    let manifest = manifest();
    let chart = chart();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &conductor).unwrap();
    let n_windows = eval.pulse_windows().len();
    let events = synthesize(&eval, &conductor, SyntheticProfile::Perfect);
    let judge = Judge::new(
        eval,
        Conductor::new(&manifest, None),
        JudgmentConfig {
            lean_mode: LeanMode::Arrival,
            ..Default::default()
        },
    );
    let cfg = if coh_weights {
        CoherenceConfig::parse(
            "schema_version = 0\n[weights]\nsway = 0.3\npressure_l = 0.2\npressure_r = 0.2\n",
        )
        .unwrap()
    } else {
        CoherenceConfig::default()
    };
    (judge, Coherence::new(cfg), events, n_windows)
}

/// Perfect play hits every window of every kind with clean depth/direction,
/// and coherence holds near 1.0 across both tempo changes and both meter
/// changes — with the W2 channel weights live.
#[test]
fn perfect_play_holds_across_meter_changes() {
    let (mut judge, mut coherence, events, n_windows) = arrival_judge(true);
    let step_beats = judge.lean_step_beats();
    let manifest = manifest();
    let conductor = Conductor::new(&manifest, None);

    let mut records = Vec::new();
    let mut boundary_values = Vec::new();
    let mut next_boundary = 0usize;
    let boundaries = [SEC_CALL_S, SEC_TRIAL_S, SEC_DEPART_S, DURATION];
    for ev in &events {
        judge.ingest(ev, &mut records);
        if !records.is_empty() {
            let pos = conductor.position_at_sample(ev.sample().max(0));
            let bpb = conductor
                .tempo()
                .anchor_at(ev.sample().max(0))
                .map(|a| a.beats_per_bar as f64)
                .unwrap_or(4.0);
            let _ = pos;
            coherence.step(&records, step_beats, bpb);
            records.clear();
        }
        while next_boundary < boundaries.len() && ev.sample() >= boundaries[next_boundary] {
            boundary_values.push(coherence.value());
            next_boundary += 1;
        }
    }
    judge.finish(&mut records);

    // Every window consumed as a hit — none expire, none spurious — and the
    // W2 quality dimensions are clean.
    let mut hits = 0;
    for r in &records {
        assert!(
            !matches!(r, JudgmentRecord::Miss { .. }),
            "perfect play must not miss: {r:?}"
        );
    }
    // Hits were consumed during the loop; recount from a fresh run for the
    // full record stream.
    let (mut judge2, _, events2, _) = arrival_judge(true);
    let mut all = Vec::new();
    for ev in &events2 {
        judge2.ingest(ev, &mut all);
    }
    judge2.finish(&mut all);
    for r in &all {
        match r {
            JudgmentRecord::Pulse {
                kind,
                err_ms,
                depth_err,
                dir_err,
                ..
            } => {
                hits += 1;
                assert!(err_ms.abs() < 1.0, "timing ≈0: {r:?}");
                match kind.as_str() {
                    "press" => {
                        assert!(
                            depth_err.expect("press carries depth_err") < 1e-9,
                            "clean depth: {r:?}"
                        );
                        assert!(dir_err.is_none());
                    }
                    "flick" => {
                        assert!(
                            dir_err.expect("flick carries dir_err") < 1e-6,
                            "clean direction: {r:?}"
                        );
                        assert!(depth_err.is_none());
                    }
                    _ => {
                        assert!(depth_err.is_none() && dir_err.is_none());
                    }
                }
            }
            JudgmentRecord::Miss { .. } => panic!("perfect play missed: {r:?}"),
            JudgmentRecord::Spurious { .. } => panic!("perfect play spurious: {r:?}"),
            JudgmentRecord::Track { err, channel, .. } => {
                assert!(
                    *err < 0.05,
                    "perfect tracking errs ≈0 on `{channel}`: {err}"
                );
            }
        }
    }
    assert_eq!(hits, n_windows, "every window of every kind is hit");
    // Sway and pressure channels were actually judged.
    let channels: std::collections::BTreeSet<_> = all
        .iter()
        .filter_map(|r| match r {
            JudgmentRecord::Track { channel, .. } => Some(channel.clone()),
            _ => None,
        })
        .collect();
    assert!(channels.contains("lean"));
    assert!(channels.contains("sway"));
    assert!(channels.contains("pressure_r"));

    // Coherence ≈1.0 by the call boundary and held across 96→120→72 and
    // 5/4→3/4→4/4.
    assert!(
        boundary_values.len() >= 3,
        "boundary sampling: {boundary_values:?}"
    );
    for (i, v) in boundary_values.iter().enumerate() {
        assert!(
            *v > 0.9,
            "coherence must hold across boundary {i}: {v} ({boundary_values:?})"
        );
    }
}

// ── The reactive loop: cues, alternates, seam — deterministic ──

fn encode_frame(raw: i64, f: &ConductedFrame, out: &mut String) {
    use std::fmt::Write;
    write!(
        out,
        "{raw};{};{};{};{};{};{};{};{};{};{};{};{}",
        f.beat.to_bits(),
        f.coherence.to_bits(),
        f.lean[0].to_bits(),
        f.sway[0].to_bits(),
        f.sway[1].to_bits(),
        f.pressure_l.to_bits(),
        f.pressure_r.to_bits(),
        f.reassembly.to_bits(),
        f.rewind.to_bits(),
        f.desaturate.to_bits(),
        f.bar,
        f.section,
    )
    .unwrap();
    for c in &f.cues {
        write!(out, "|{},{}", c.name, c.age_s.to_bits()).unwrap();
        for (k, v) in &c.params {
            write!(out, ",{k}={v}").unwrap();
        }
    }
    out.push('\n');
}

struct ReactiveRun {
    trace: String,
    seams: Vec<i64>,
    /// (raw, cue name) for every conducted cue firing.
    cue_firings: Vec<(i64, String)>,
    /// Max commanded alternate mix per bus across the run.
    max_alt_mix: std::collections::BTreeMap<String, f64>,
    /// Alternate mixes observed on the first frame after each seam.
    post_seam_mix: Vec<f64>,
}

fn reactive_run(tag: &str) -> ReactiveRun {
    let manifest = manifest();
    let chart = chart();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &conductor).unwrap();
    let script_events = synthesize(&eval, &conductor, SyntheticProfile::Perfect);

    let judge = Judge::new(
        ChartEval::new(&chart, &conductor).unwrap(),
        Conductor::new(&manifest, None),
        JudgmentConfig {
            lean_mode: LeanMode::Arrival,
            ..Default::default()
        },
    );
    // Default ladder + alternates on the deeper rungs; full-fail band lifted
    // so this chart's neglect anatomy (sparser pulses than the prototype)
    // still reaches the seam.
    let ladder_cfg = LadderConfig::parse(
        r#"
schema_version = 0
arm_above = 0.80
[full_fail]
enter_below = 0.66
exit_above = 0.80
hold_ms = 1000.0
[alternates]
xfade_ms = 200.0
[[rungs]]
name = "haze"
enter_below = 0.82
exit_above = 0.88
[rungs.audio]
lpf_hz = 2400.0
thin_db = { texture = -6.0 }
[rungs.visual]
desaturate = 0.35
[[rungs]]
name = "warble"
enter_below = 0.72
exit_above = 0.79
[rungs.audio]
lpf_hz = 1400.0
thin_db = { texture = -12.0 }
alternates = ["harmony"]
warble_depth_semitones = 0.3
warble_rate_hz = 0.8
[rungs.visual]
desaturate = 0.6
blur = 0.5
[[rungs]]
name = "dropout"
enter_below = 0.68
exit_above = 0.74
[rungs.audio]
lpf_hz = 900.0
drop = ["texture"]
alternates = ["harmony", "world_voice"]
warble_depth_semitones = 0.45
warble_rate_hz = 0.7
[rungs.visual]
desaturate = 0.85
blur = 0.8
chromatic = 0.35
"#,
    )
    .expect("test ladder");

    let log_path = std::env::temp_dir().join(format!(
        "world2-milestone-{tag}-{}.jsonl",
        std::process::id()
    ));
    let log = JsonlWriter::create(&log_path, &serde_json::json!({"t": "header", "schema": 0}))
        .expect("temp log");
    let visual_eval = ChartEval::new(&chart, &conductor).unwrap();
    let mut core = ChartCore::new(
        judge,
        Coherence::new(CoherenceConfig::default()),
        log,
        Ladder::new(ladder_cfg),
        Reintegrator::new(manifest.reintegration.clone()),
        Conductor::new(&manifest, None),
        PathBuf::from("unused-coherence.toml"),
        PathBuf::from("unused-ladder.toml"),
        flint_music::GradientDriver::new(flint_music::GradientConfig::default()),
        PathBuf::from("unused-gradient.toml"),
        visual_eval,
    );
    core.sync_seam_params();
    core.set_cues(flint_music::chart_session::resolve_cues(
        &chart,
        &Conductor::new(&manifest, None),
    ));

    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    // Raw render length: the seam rewinds suite time (fail mid-call →
    // checkpoint re-entry), so the raw timeline needs the replayed span on
    // top of the suite length for the departure content (final_call) to be
    // reached again.
    let render_cfg = OfflineRenderConfig {
        duration_samples: DURATION + 1_400_000,
        chunk_frames: CHUNK,
    };

    let mut out = ReactiveRun {
        trace: String::new(),
        seams: Vec::new(),
        cue_firings: Vec::new(),
        max_alt_mix: Default::default(),
        post_seam_mix: Vec::new(),
    };
    let mut attentive = true;
    let mut cursor = 0usize;
    let mut prev_suite = i64::MIN;
    let mut chunk_index = 0i64;
    let mut just_seamed = false;
    let mut err: Option<flint_core::FlintError> = None;
    // PREROLL_BEATS at the first anchor: 2 beats at 96 BPM.
    let preroll = (2.0 * SR as f64 * 60.0 / 96.0).round() as i64;

    render_offline_with(&manifest, &ToneStems, &script, &render_cfg, |pos, session| {
        let raw = chunk_index * CHUNK as i64 - preroll;
        chunk_index += 1;
        if err.is_some() {
            return;
        }

        if pos.sample >= 0 {
            if attentive && raw >= FALL_RAW && out.seams.is_empty() {
                attentive = false;
            }
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
                    core.observe_input(ev);
                    if ev.sample() >= 0 {
                        core.ingest(ev);
                    }
                }
            }
            prev_suite = now_suite;
            core.advance_to(now_suite);
            if let Err(e) = core.process(now_suite) {
                err = Some(e);
                return;
            }
            match core.step_seq(session, pos) {
                Ok((_seq, seq_events)) => {
                    for ev in seq_events {
                        if let ReintegrationEvent::Seam { re_entry_sample, .. } = ev {
                            out.seams.push(raw);
                            just_seamed = true;
                            cursor = script_events
                                .partition_point(|e| e.sample() < re_entry_sample);
                            prev_suite = re_entry_sample - 1;
                            attentive = true;
                        }
                    }
                }
                Err(e) => {
                    err = Some(e);
                    return;
                }
            }
        }

        // Mixer evidence: alternate crossfade engagement + post-seam reset.
        for (name, state) in session.mixer.states() {
            let entry = out.max_alt_mix.entry(name.to_string()).or_insert(0.0);
            if state.alternate_mix > *entry {
                *entry = state.alternate_mix;
            }
            if just_seamed && name == "harmony" {
                out.post_seam_mix.push(state.alternate_mix);
            }
        }
        just_seamed = false;

        let frame = core.conducted_frame(*pos, false);
        for c in &frame.cues {
            out.cue_firings.push((raw, c.name.clone()));
        }
        encode_frame(raw, &frame, &mut out.trace);
    })
    .expect("offline render");
    if let Some(e) = err {
        panic!("core error during render: {e}");
    }
    core.flush_log().unwrap();
    drop(core);
    let _ = std::fs::remove_file(&log_path);
    out
}

#[test]
fn reactive_loop_fires_cues_and_alternates_deterministically() {
    let a = reactive_run("a");
    let b = reactive_run("b");
    assert_eq!(a.trace, b.trace, "reactive traces must be bit-identical");
    assert_eq!(a.seams, b.seams);
    assert_eq!(a.cue_firings, b.cue_firings);

    let r = a;
    assert!(!r.seams.is_empty(), "the neglect must reach a seam");

    // Every authored cue name fired, with params riding along in the trace.
    let names: std::collections::BTreeSet<_> =
        r.cue_firings.iter().map(|(_, n)| n.as_str()).collect();
    for expected in ["tumble", "call", "surge", "final_call"] {
        assert!(names.contains(expected), "cue `{expected}` never fired");
    }
    assert!(
        r.trace.contains("depth=0.6") && r.trace.contains("roll=0.8"),
        "cue params must reach the conducted frame"
    );
    // The seam re-arms cues at/past the re-entry point: with a mid-run fail
    // and checkpoint re-entry, at least one cue fires more than once.
    let mut counts = std::collections::BTreeMap::new();
    for (_, n) in &r.cue_firings {
        *counts.entry(n.clone()).or_insert(0) += 1;
    }
    assert!(
        counts.values().any(|c| *c > 1),
        "some cue must re-fire after the seam: {counts:?}"
    );

    // The composed alternates engaged (harmony rides the warble rung's
    // crossfade inside its span) and reset to bus-active at the seam.
    assert!(
        r.max_alt_mix.get("harmony").copied().unwrap_or(0.0) > 0.9,
        "harmony alternate must engage under neglect: {:?}",
        r.max_alt_mix
    );
    assert!(
        !r.post_seam_mix.is_empty() && r.post_seam_mix.iter().all(|m| *m == 0.0),
        "the seam re-play resets the crossfade to bus-active: {:?}",
        r.post_seam_mix
    );
    // Protected/alternate-free buses never crossfade.
    for bus in ["foundation", "home_theme", "texture"] {
        assert_eq!(
            r.max_alt_mix.get(bus).copied().unwrap_or(0.0),
            0.0,
            "{bus} must never crossfade"
        );
    }
}
