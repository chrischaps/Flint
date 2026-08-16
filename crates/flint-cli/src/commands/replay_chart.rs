//! Headless judgment: run a recorded or synthetic input session through the
//! identical chart-eval → judgment → coherence pipeline with no controller,
//! no clock, and no audio device. Deterministic: the same session and
//! config produce a bit-identical judgment log. With `--render` it also
//! runs the offline audio render over the same span, so listening evidence
//! and the coherence trace come from one invocation (coherence→mixer
//! coupling is Phase 3; the render here is the plain suite).

use anyhow::{bail, Context, Result};
use flint_music::chart_eval::ChartEval;
use flint_music::coherence::{Coherence, CoherenceConfig};
use flint_music::conductor::Conductor;
use flint_music::judgment::{Judge, JudgmentConfig, JudgmentRecord, JsonlWriter};
use flint_music::replay::{synthesize, SyntheticProfile};
use flint_music::session::FileStems;
use flint_music::status::coherence_meter;
use flint_music::{
    validate_chart, validate_manifest, validate_manifest_assets, Chart, EventScript,
    OfflineRenderConfig, SuiteManifest,
};
use std::path::{Path, PathBuf};

pub struct ReplayChartArgs {
    pub manifest: String,
    pub chart: String,
    pub session: Option<String>,
    pub synthetic: Option<String>,
    pub config: Option<String>,
    pub out: Option<String>,
    /// Materialize the replayed (usually synthetic) event stream as a
    /// session file — for committing evidence or sharing a repro.
    pub save_session: Option<String>,
    pub render: Option<String>,
    pub base_dir: Option<String>,
    /// Lean judgment mode ("arrival" or "track") — must match the live run
    /// being reproduced; the judgment-log header records which was used.
    pub lean_mode: String,
    /// Disintegration ladder config. With `--render`, its presence (explicit
    /// or `config/ladder.toml`) switches the render to the full reactive
    /// loop: judge → coherence → ladder → reintegration sequencer → mixer,
    /// so a fall-and-reintegration renders to WAV. Without `--render` the
    /// ladder is inactive (Milestone-2 judgment semantics preserved).
    pub ladder: Option<String>,
}

pub fn run(args: ReplayChartArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // --- validate ------------------------------------------------------------
    let manifest = SuiteManifest::load(Path::new(&args.manifest))
        .with_context(|| format!("loading {}", args.manifest))?;
    let chart = Chart::load(Path::new(&args.chart))
        .with_context(|| format!("loading {}", args.chart))?;
    let mut issues = validate_manifest(&manifest);
    issues.extend(validate_chart(&chart, &manifest));
    if args.render.is_some() {
        issues.extend(validate_manifest_assets(&manifest, &base_dir));
    }
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("{i}");
        }
        bail!("validation failed ({} issue(s))", issues.len());
    }

    // --- events --------------------------------------------------------------
    // Offline is exact: no latency compensation anywhere (session samples
    // are already compensated by whoever recorded them).
    let conductor = Conductor::new(&manifest, None);
    let eval = ChartEval::new(&chart, &conductor).map_err(|e| anyhow::anyhow!("{e}"))?;

    // `pairs` keeps each event's raw clock sample alongside its
    // suite-stamped event (identical until a session contains reintegration
    // seams). Synthetic streams carry suite stamps; the reactive path maps
    // them through its own timeline (`synthetic_stamps`).
    let (pairs, session_header, synthetic_stamps) = match (&args.session, &args.synthetic) {
        (Some(path), None) => {
            let (header, pairs) = flint_music::replay::read_session_raw(Path::new(path))
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| format!("reading {path}"))?;
            if header.suite != manifest.id {
                bail!(
                    "session was recorded against suite '{}', manifest is '{}'",
                    header.suite,
                    manifest.id
                );
            }
            println!(
                "session: {path} ({} event(s); recorded latency {:.1} ms, calibration {:+.1} ms)",
                pairs.len(),
                header.latency_ms,
                header.calibration_ms
            );
            (pairs, Some(header), false)
        }
        (None, Some(profile)) => {
            let profile = SyntheticProfile::parse(profile).map_err(|e| anyhow::anyhow!("{e}"))?;
            let events = synthesize(&eval, &conductor, profile);
            println!("synthetic session: {profile:?} ({} event(s))", events.len());
            let pairs = events.into_iter().map(|ev| (ev.sample(), ev)).collect();
            (pairs, None, true)
        }
        _ => bail!("exactly one of --session or --synthetic is required"),
    };
    let events: Vec<flint_music::input_stream::InputEvent> =
        pairs.iter().map(|(_, ev)| ev.clone()).collect();

    if let Some(path) = &args.save_session {
        let header = flint_music::replay::SessionHeader {
            schema: 0,
            suite: manifest.id.clone(),
            chart: args.chart.clone(),
            sample_rate: manifest.sample_rate,
            latency_ms: session_header.as_ref().map(|h| h.latency_ms).unwrap_or(0.0),
            calibration_ms: session_header
                .as_ref()
                .map(|h| h.calibration_ms)
                .unwrap_or(0.0),
            coherence_config: session_header
                .as_ref()
                .and_then(|h| h.coherence_config.clone()),
            capture: args
                .synthetic
                .as_ref()
                .map(|p| serde_json::json!({ "synthetic": p })),
        };
        let mut w = flint_music::replay::SessionWriter::create(Path::new(path), &header)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for ev in &events {
            w.write(ev, manifest.sample_rate)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        w.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("session saved: {path} ({} event(s))", w.written());
    }

    // --- coherence config: explicit flag > session header snapshot >
    // repo default file > built-in defaults ----------------------------------
    let coherence_cfg = if let Some(path) = &args.config {
        let cfg = CoherenceConfig::load(Path::new(path))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("loading {path}"))?;
        println!("coherence config: {path}");
        if session_header
            .as_ref()
            .and_then(|h| h.coherence_config.as_ref())
            .is_some_and(|snap| *snap != cfg.to_json())
        {
            println!("  (differs from the session's recorded config — offline retune)");
        }
        cfg
    } else if let Some(snap) = session_header
        .as_ref()
        .and_then(|h| h.coherence_config.as_ref())
    {
        println!("coherence config: session header snapshot");
        CoherenceConfig::from_json(snap)
    } else {
        let default_path = base_dir.join("config/coherence.toml");
        if default_path.exists() {
            println!("coherence config: {}", default_path.display());
            CoherenceConfig::load(&default_path).map_err(|e| anyhow::anyhow!("{e}"))?
        } else {
            println!("coherence config: built-in defaults");
            CoherenceConfig::default()
        }
    };

    // --- run the pipeline ----------------------------------------------------
    let judgment_cfg = JudgmentConfig {
        lean_mode: super::play_chart::parse_lean_mode(&args.lean_mode)?,
        ..Default::default()
    };
    println!(
        "lean mode: {}",
        super::play_chart::lean_mode_name(judgment_cfg.lean_mode)
    );
    let eval_for_judge =
        ChartEval::new(&chart, &conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut judge = Judge::new(
        eval_for_judge,
        Conductor::new(&manifest, None),
        judgment_cfg,
    );
    let lean_step_beats = judge.lean_step_beats();
    let mut coherence = Coherence::new(coherence_cfg);

    let out_path = args
        .out
        .map(PathBuf::from)
        .unwrap_or_else(|| base_dir.join("logs/judgment/replay.jsonl"));
    let header = serde_json::json!({
        "t": "header", "schema": 0,
        "suite": manifest.id, "chart": args.chart,
        "session": args.session, "synthetic": args.synthetic,
        "latency_ms": session_header.as_ref().map(|h| h.latency_ms).unwrap_or(0.0),
        "calibration_ms": session_header.as_ref().map(|h| h.calibration_ms).unwrap_or(0.0),
        "grid_beats": judgment_cfg.grid_beats,
        "lean_mode": super::play_chart::lean_mode_name(judgment_cfg.lean_mode),
        "arrival_half_beats": judgment_cfg.arrival_half_beats,
        "coherence_config": coherence_cfg.to_json(),
    });
    let mut log = JsonlWriter::create(&out_path, &header).map_err(|e| anyhow::anyhow!("{e}"))?;

    // --- ladder: with --render, its presence switches to the reactive loop ----
    let ladder_cfg = if let Some(path) = &args.ladder {
        let cfg = flint_music::ladder::LadderConfig::load(Path::new(path))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("loading {path}"))?;
        println!("ladder config: {path}");
        Some(cfg)
    } else {
        let default_path = base_dir.join("config/ladder.toml");
        if default_path.exists() {
            println!("ladder config: {}", default_path.display());
            Some(
                flint_music::ladder::LadderConfig::load(&default_path)
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
            )
        } else {
            None
        }
    };
    match (&args.render, ladder_cfg) {
        (Some(wav), Some(lcfg)) => {
            return run_reactive(
                &manifest,
                &base_dir,
                wav,
                lcfg,
                &pairs,
                synthetic_stamps,
                judge,
                coherence,
                lean_step_beats,
                log,
                &out_path,
            );
        }
        (None, Some(_)) => {
            println!("(ladder inactive without --render; judgment replay is Milestone-2 semantics)")
        }
        _ => {}
    }

    let (mut hits, mut misses, mut spurious) = (0u64, 0u64, 0u64);
    let mut abs_err_sum_ms = 0.0f64;
    let mut records = Vec::new();
    let mut last_bar = i64::MIN;
    let mut process =
        |records: &mut Vec<JudgmentRecord>,
         at_sample: i64,
         coherence: &mut Coherence,
         log: &mut JsonlWriter|
         -> Result<()> {
            if records.is_empty() {
                return Ok(());
            }
            let beats_per_bar = conductor
                .tempo()
                .anchor_at(at_sample.max(0))
                .map(|a| a.beats_per_bar as f64)
                .unwrap_or(4.0);
            let value = coherence.step(records, lean_step_beats, beats_per_bar);
            log.write_value(&serde_json::json!({
                "t": "coherence", "sample": at_sample, "value": value,
            }))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            for rec in records.drain(..) {
                match &rec {
                    JudgmentRecord::Pulse { err_ms, .. } => {
                        hits += 1;
                        abs_err_sum_ms += err_ms.abs();
                    }
                    JudgmentRecord::Miss { .. } => misses += 1,
                    JudgmentRecord::Spurious { .. } => spurious += 1,
                    JudgmentRecord::Track { .. } => {}
                }
                log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            Ok(())
        };

    for ev in &events {
        judge.ingest(ev, &mut records);
        process(&mut records, ev.sample(), &mut coherence, &mut log)?;
        let bar = conductor.position_at_sample(ev.sample()).bar;
        if bar != last_bar {
            last_bar = bar;
            println!("bar {:>3} | {}", bar + 1, coherence_meter(coherence.value()));
        }
    }
    let final_sample = events.last().map(|e| e.sample()).unwrap_or(0);
    judge.finish(&mut records);
    process(&mut records, final_sample, &mut coherence, &mut log)?;
    log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;

    let mean_ms = if hits > 0 {
        abs_err_sum_ms / hits as f64
    } else {
        0.0
    };
    println!(
        "done. pulses hit {hits} (mean |err| {mean_ms:.1} ms), missed {misses}, spurious {spurious} | final {}",
        coherence_meter(coherence.value())
    );
    println!("judgment log: {}", out_path.display());

    // --- optional audio render over the same span (plain: no ladder) ----------
    if let Some(wav) = args.render {
        let duration_samples = final_sample.max(1);
        let cfg = OfflineRenderConfig {
            duration_samples,
            ..Default::default()
        };
        let script = EventScript {
            schema_version: 0,
            events: vec![],
        };
        let result = flint_music::render_offline(
            &manifest,
            &FileStems::new(&base_dir),
            &script,
            &cfg,
            |_, _| {},
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        flint_music::write_wav(Path::new(&wav), &result.samples, manifest.sample_rate)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!(
            "rendered {} ({:.1} s) alongside the coherence trace",
            wav,
            duration_samples as f64 / manifest.sample_rate as f64
        );
    }
    Ok(())
}

/// The reactive render: the same judge → coherence → ladder → reintegration
/// sequencer → mixer loop the live harness runs, inside the deterministic
/// offline renderer (ADR 0006/0014). A recorded or synthetic fall renders
/// its full-fail seams and reassemblies to WAV, bit-identically.
#[allow(clippy::too_many_arguments)]
fn run_reactive(
    manifest: &SuiteManifest,
    base_dir: &Path,
    wav: &str,
    ladder_cfg: flint_music::ladder::LadderConfig,
    pairs: &[(i64, flint_music::input_stream::InputEvent)],
    synthetic_stamps: bool,
    mut judge: Judge,
    mut coherence: Coherence,
    lean_step_beats: f64,
    mut log: JsonlWriter,
    out_path: &Path,
) -> Result<()> {
    use flint_music::ladder::{Ladder, LadderDriver};
    use flint_music::reintegration::{ReintegrationEvent, Reintegrator};

    let sample_rate = manifest.sample_rate;
    let last_raw = pairs.last().map(|(r, _)| *r).unwrap_or(0);
    // Generous trailing margin: a fall that begins at the last event still
    // needs to decay, hold, seam, and reassemble — and a fully absent player
    // loops, which is worth hearing a few times.
    let cfg = OfflineRenderConfig {
        duration_samples: last_raw.max(1) + 30 * sample_rate as i64,
        ..Default::default()
    };
    let mut ladder = Ladder::new(ladder_cfg);
    let mut driver = LadderDriver::new();
    let mut reintegrator = Reintegrator::new(manifest.reintegration.clone());
    reintegrator.fade_ms = ladder.config().seam_fade_ms;
    reintegrator.rewind_bars = ladder.config().seam_rewind_bars;
    reintegrator.rewind_drop_st = ladder.config().seam_rewind_drop_st;
    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };

    let (mut hits, mut misses, mut spurious) = (0u64, 0u64, 0u64);
    let mut abs_err_sum_ms = 0.0f64;
    let mut fails = 0u64;
    let mut records: Vec<JudgmentRecord> = Vec::new();
    let mut seq_events: Vec<ReintegrationEvent> = Vec::new();
    let mut cursor = 0usize;
    let mut last_bar = i64::MIN;
    let mut failure: Option<anyhow::Error> = None;

    let result = flint_music::render_offline_with(
        manifest,
        &FileStems::new(base_dir),
        &script,
        &cfg,
        |pos, session| {
            if failure.is_some() {
                return;
            }
            let raw = session.raw_clock_sample();
            if raw < 0 {
                return;
            }
            let mut fallible = || -> Result<()> {
                let offset = session.timeline_offset();
                while cursor < pairs.len() && pairs[cursor].0 <= raw {
                    let (r, ev) = &pairs[cursor];
                    cursor += 1;
                    let suite_ev = if synthetic_stamps {
                        ev.with_sample(r - offset)
                    } else {
                        ev.clone()
                    };
                    if suite_ev.sample() >= 0 {
                        judge.ingest(&suite_ev, &mut records);
                    }
                }
                let now_suite = session.clock_sample();
                judge.advance_to(now_suite, &mut records);

                if !records.is_empty() {
                    let beats_per_bar = session
                        .conductor
                        .tempo()
                        .anchor_at(now_suite.max(0))
                        .map(|a| a.beats_per_bar as f64)
                        .unwrap_or(4.0);
                    let value = coherence.step(&records, lean_step_beats, beats_per_bar);
                    log.write_value(&serde_json::json!({
                        "t": "coherence", "sample": now_suite, "value": value,
                    }))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                    for rec in records.drain(..) {
                        match &rec {
                            JudgmentRecord::Pulse { err_ms, .. } => {
                                hits += 1;
                                abs_err_sum_ms += err_ms.abs();
                            }
                            JudgmentRecord::Miss { .. } => misses += 1,
                            JudgmentRecord::Spurious { .. } => spurious += 1,
                            JudgmentRecord::Track { .. } => {}
                        }
                        log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }

                reintegrator
                    .tick(
                        coherence.value(),
                        &mut ladder,
                        &mut driver,
                        session,
                        &mut seq_events,
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                for ev in seq_events.drain(..) {
                    match &ev {
                        ReintegrationEvent::FullFail {
                            at_suite_sample,
                            re_entry_sample,
                            seam_suite_sample,
                        } => {
                            fails += 1;
                            println!(
                                "FULL FAIL at sample {at_suite_sample}: reintegrating to \
                                 {re_entry_sample} (seam at {seam_suite_sample})"
                            );
                            log.write_value(&serde_json::json!({
                                "t": "full_fail", "sample": at_suite_sample,
                                "re_entry_sample": re_entry_sample,
                                "seam_suite_sample": seam_suite_sample,
                            }))
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                        }
                        ReintegrationEvent::Seam {
                            raw_sample,
                            timeline_offset,
                            re_entry_sample,
                        } => {
                            judge.rewind_to(*re_entry_sample);
                            log.write_value(&serde_json::json!({
                                "t": "seam", "raw_sample": raw_sample,
                                "timeline_offset": timeline_offset,
                                "re_entry_sample": re_entry_sample,
                            }))
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                        }
                        ReintegrationEvent::ReassemblyComplete { at_suite_sample } => {
                            log.write_value(&serde_json::json!({
                                "t": "reassembly_complete", "sample": at_suite_sample,
                            }))
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                        }
                    }
                }

                if pos.bar != last_bar && pos.sample >= 0 {
                    last_bar = pos.bar;
                    let rung = match ladder.rung() {
                        Some(r) => format!(" | rung {} ({})", ladder.level(), r.name),
                        None => String::new(),
                    };
                    println!(
                        "bar {:>3} | {}{rung}",
                        pos.bar + 1,
                        coherence_meter(coherence.value())
                    );
                }
                Ok(())
            };
            if let Err(e) = fallible() {
                failure = Some(e);
            }
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(e) = failure {
        return Err(e);
    }

    judge.finish(&mut records);
    for rec in records.drain(..) {
        match &rec {
            JudgmentRecord::Miss { .. } => misses += 1,
            JudgmentRecord::Spurious { .. } => spurious += 1,
            _ => {}
        }
        log.write(&rec).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    log.flush().map_err(|e| anyhow::anyhow!("{e}"))?;

    let mean_ms = if hits > 0 {
        abs_err_sum_ms / hits as f64
    } else {
        0.0
    };
    println!(
        "done. pulses hit {hits} (mean |err| {mean_ms:.1} ms), missed {misses}, spurious \
         {spurious} | {fails} full-fail(s) | final {}",
        coherence_meter(coherence.value())
    );
    println!("judgment log: {}", out_path.display());

    flint_music::write_wav(Path::new(wav), &result.samples, sample_rate)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "rendered {} ({:.1} s, reactive: ladder + reintegration live)",
        wav,
        cfg.duration_samples as f64 / sample_rate as f64
    );
    Ok(())
}
