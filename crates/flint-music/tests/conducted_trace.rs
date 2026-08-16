//! F4 automated evidence (ADR 0020): the conducted-parameters frame is
//! deterministic and faithful.
//!
//! Drives `ChartCore` directly — the first test to do so — through the full
//! reactive loop offline (`render_offline_with`), replaying a synthetic
//! session modeled on seam_milestone3's attention script: perfect play →
//! neglect → full-fail → recovery *during* reassembly. Each chunk's
//! `conducted_frame` is serialized bit-exactly (`to_bits`, never float
//! formatting); two runs must produce byte-identical traces, and one run's
//! trace must show every conducted field doing its job (pulses with kinds
//! and errors, target motion, the reassembly/rewind ramps, section names,
//! monotonic bars between seams).
//!
//! Hermetic by design: synthetic manifest + tone stems from `tests/data`,
//! never the game repo's fixtures. The committed real-session replay
//! belongs to F12's `scene_milestone4`.

use flint_music::chart_eval::ChartEval;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::event_script::EventScript;
use flint_music::judgment::{JsonlWriter, Judge, JudgmentConfig};
use flint_music::ladder::{Ladder, LadderConfig};
use flint_music::manifest::BusDecl;
use flint_music::offline::{render_offline_with, OfflineRenderConfig};
use flint_music::reintegration::{ReintegrationEvent, Reintegrator};
use flint_music::replay::{synthesize, SyntheticProfile};
use flint_music::session::StemResolver;
use flint_music::{Chart, ChartCore, ConductedFrame, PulseKind, SuiteManifest};
use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SR: u32 = 48_000;
const CHUNK: usize = 128;
/// 120 BPM 4/4 until suite sample 960_000, then 90 BPM 3/4.
const BAR_A: i64 = 96_000;
const DURATION: i64 = 24 * BAR_A;
/// Neglect begins at this raw sample; recovery starts at the seam.
const FALL_RAW: i64 = 3 * BAR_A;

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn manifest() -> SuiteManifest {
    SuiteManifest::load(&data_dir().join("tempo_change.suite.toml")).expect("synthetic manifest")
}

/// The seam_milestone3 evidence chart: lean sways through both meters,
/// pulses every 2 beats from beat 4 to 60. (Duplicated per the tests'
/// convention — the milestone harness helpers are private.)
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

/// Steady tones per bus (labels in the manifest are never opened).
struct ToneStems;

impl StemResolver for ToneStems {
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
        let frames: Arc<[Frame]> = (0..(DURATION + SR as i64) as usize)
            .map(|i| {
                let t = i as f64 / SR as f64;
                Frame::from_mono((std::f64::consts::TAU * freq * t).sin() as f32 * amp)
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

/// Bit-exact serialization: `to_bits` for every float, ints and strings
/// verbatim. Any formatting ambiguity would hide a real nondeterminism.
fn encode_frame(raw: i64, f: &ConductedFrame, out: &mut String) {
    use std::fmt::Write;
    write!(
        out,
        "{raw};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{};{}",
        f.beat_phase.to_bits(),
        f.bar_phase.to_bits(),
        f.lean[0].to_bits(),
        f.lean[1].to_bits(),
        f.target[0].to_bits(),
        f.target[1].to_bits(),
        f.coherence.to_bits(),
        f.pulse_age_s.to_bits(),
        f.preroll,
        f.desaturate.to_bits(),
        f.blur.to_bits(),
        f.chromatic.to_bits(),
        f.reassembly.to_bits(),
        f.no_input,
        f.rewind.to_bits(),
        f.bar,
        f.beat.to_bits(),
    )
    .unwrap();
    write!(out, ";{}", f.section).unwrap();
    for p in &f.pulses {
        write!(
            out,
            "|{},{},{},{}",
            p.sample,
            p.age_s.to_bits(),
            p.err_ms.to_bits(),
            p.kind.as_str()
        )
        .unwrap();
    }
    out.push('\n');
}

struct TraceRun {
    trace: String,
    frames: Vec<(i64, ConductedFrame)>,
    seams: Vec<i64>,
}

fn run(log_tag: &str) -> TraceRun {
    let manifest = manifest();
    let chart = chart();
    let judgment_cfg = JudgmentConfig::default();
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &conductor).unwrap();
    let script_events = synthesize(&eval, &conductor, SyntheticProfile::Perfect);

    let judge = Judge::new(
        ChartEval::new(&chart, &conductor).unwrap(),
        Conductor::new(&manifest, None),
        judgment_cfg,
    );
    let log_path = std::env::temp_dir().join(format!(
        "conducted-trace-{log_tag}-{}.jsonl",
        std::process::id()
    ));
    let log = JsonlWriter::create(&log_path, &serde_json::json!({"t": "header", "schema": 0}))
        .expect("temp judgment log");
    let visual_eval = ChartEval::new(&chart, &conductor).unwrap();
    let mut core = ChartCore::new(
        judge,
        Coherence::new(CoherenceConfig::default()),
        log,
        Ladder::new(LadderConfig::default()),
        Reintegrator::new(manifest.reintegration.clone()),
        Conductor::new(&manifest, None),
        PathBuf::from("unused-coherence.toml"),
        PathBuf::from("unused-ladder.toml"),
        visual_eval,
    );
    core.sync_seam_params();

    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };
    let render_cfg = OfflineRenderConfig {
        duration_samples: DURATION,
        chunk_frames: CHUNK,
    };

    let mut out = TraceRun {
        trace: String::new(),
        frames: Vec::new(),
        seams: Vec::new(),
    };
    let mut attentive = true;
    let mut cursor = 0usize;
    let mut prev_suite = i64::MIN;
    let mut chunk_index = 0i64;
    let mut err: Option<flint_core::FlintError> = None;
    // PREROLL_BEATS at the first anchor: 2 beats at 120 BPM.
    let preroll = (2.0 * SR as f64 * 60.0 / 120.0).round() as i64;

    render_offline_with(
        &manifest,
        &ToneStems,
        &script,
        &render_cfg,
        |pos, session| {
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
                match core.step_seq(session) {
                    Ok((_seq, seq_events)) => {
                        for ev in seq_events {
                            if let ReintegrationEvent::Seam {
                                re_entry_sample, ..
                            } = ev
                            {
                                out.seams.push(raw);
                                // Recovery starts DURING reassembly: replay the
                                // synthetic stream from the re-entry point.
                                cursor =
                                    script_events.partition_point(|e| e.sample() < re_entry_sample);
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

            let frame = core.conducted_frame(*pos, false);
            encode_frame(raw, &frame, &mut out.trace);
            out.frames.push((raw, frame));
        },
    )
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
fn conducted_trace_is_deterministic_bit_for_bit() {
    let a = run("a");
    let b = run("b");
    assert_eq!(a.trace, b.trace, "conducted traces must be bit-identical");
    assert_eq!(a.seams, b.seams);
}

#[test]
fn conducted_trace_reflects_session_shape() {
    let r = run("shape");
    assert!(!r.seams.is_empty(), "the neglect must reach a seam");

    // Preroll frames come first, then never again (raw time only moves on).
    let first_live = r
        .frames
        .iter()
        .position(|(_, f)| !f.preroll)
        .expect("live frames");
    assert!(first_live > 0, "preroll frames precede live frames");
    assert!(
        r.frames[first_live..].iter().all(|(_, f)| !f.preroll),
        "preroll never returns"
    );
    for (_, f) in &r.frames[..first_live] {
        assert_eq!(f.target, [0.0, 0.0], "target is zero during preroll");
        assert!(f.pulses.is_empty(), "no judged pulses during preroll");
    }

    // Section names are live from the first non-preroll frame, and both
    // manifest sections appear across the run.
    assert!(
        r.frames[first_live..]
            .iter()
            .all(|(_, f)| !f.section.is_empty()),
        "section is always named past preroll"
    );
    assert!(r.frames.iter().any(|(_, f)| f.section == "before"));
    assert!(r.frames.iter().any(|(_, f)| f.section == "after"));

    // Judged pulses carry kinds and sane ages/errors: hits while attentive
    // (finite, in-window errors), misses during the neglect (err 0.0).
    let pulses: Vec<_> = r.frames.iter().flat_map(|(_, f)| f.pulses.iter()).collect();
    let hits = pulses.iter().filter(|p| p.kind == PulseKind::Hit).count();
    let misses = pulses.iter().filter(|p| p.kind == PulseKind::Miss).count();
    assert!(hits > 0, "attentive play must land hits");
    assert!(misses > 0, "neglect must expire windows into misses");
    for p in &pulses {
        assert!(p.age_s >= 0.0, "pulse ages are never negative: {p:?}");
        assert!(
            p.age_s < 5.0,
            "a pulse judged this frame cannot be ancient: {p:?}"
        );
        assert!(p.err_ms.is_finite());
        if p.kind != PulseKind::Hit {
            assert_eq!(p.err_ms, 0.0, "err_ms is hits-only: {p:?}");
        }
    }

    // The chart's lean target moves mid-content.
    assert!(
        r.frames
            .iter()
            .any(|(_, f)| f.target[0].abs() > 0.1 || f.target[1].abs() > 0.1),
        "lean target must move mid-chart"
    );

    // The fail loop shows in the conducted ramps: reassembly leaves 1.0 and
    // returns to it; the rewind gesture registers before the seam.
    assert!(
        r.frames.iter().any(|(_, f)| f.reassembly < 0.5),
        "reassembly ramp must engage after the fail"
    );
    assert_eq!(
        r.frames.last().unwrap().1.reassembly,
        1.0,
        "the world re-gathers fully by the end"
    );
    assert!(
        r.frames.iter().any(|(_, f)| f.rewind > 0.0),
        "the rewind gesture must register"
    );
    // Ladder visuals engage on the way down and clear by the end.
    assert!(r.frames.iter().any(|(_, f)| f.desaturate > 0.0));
    let last = &r.frames.last().unwrap().1;
    assert_eq!(last.desaturate, 0.0);
    assert_eq!(last.blur, 0.0);

    // Bars are non-decreasing between seams (suite time only rewinds at one).
    let mut prev_bar = i64::MIN;
    let mut seam_iter = r.seams.iter().peekable();
    for (raw, f) in r.frames[first_live..].iter() {
        if seam_iter.peek().is_some_and(|s| raw >= *s) {
            seam_iter.next();
            prev_bar = i64::MIN;
        }
        assert!(
            f.bar >= prev_bar,
            "bar went backwards without a seam at raw {raw}: {} < {prev_bar}",
            f.bar
        );
        prev_bar = f.bar;
    }
}
