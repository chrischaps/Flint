//! Headless judgment: run a recorded or synthetic input session through the
//! identical chart-eval → judgment → coherence pipeline with no controller,
//! no clock, and no audio device. Deterministic: the same session and
//! config produce a bit-identical judgment log. With `--render` it also
//! runs the offline audio render over the same span, so listening evidence
//! and the coherence trace come from one invocation (coherence→mixer
//! coupling is Phase 3; the render here is the plain suite).
//!
//! The loop body is [`flint_music::ChartCore`] — the same code the live
//! harness runs (ADR 0016); this command is the offline front end.

use anyhow::{bail, Context, Result};
use flint_music::chart_eval::ChartEval;
use flint_music::chart_session::{
    lean_mode_name, parse_lean_mode, resolve_coherence_config, ChartCore, CoherenceSource,
};
use flint_music::coherence::Coherence;
use flint_music::conductor::Conductor;
use flint_music::gradient::{GradientConfig, GradientDriver};
use flint_music::judgment::{Judge, JudgmentConfig, JsonlWriter};
use flint_music::ladder::{Ladder, LadderConfig};
use flint_music::reintegration::{ReintegrationEvent, Reintegrator};
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
    /// Error-gradient config (ADR 0024). Explicit path must load; default is
    /// `config/gradient.toml` when present, else inert built-ins. Only the
    /// reactive render (`--render` + ladder) ever applies it; it does not
    /// gate reactive mode.
    pub gradient: Option<String>,
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
    let (coherence_cfg, coherence_source) = resolve_coherence_config(
        args.config.as_deref().map(Path::new),
        session_header
            .as_ref()
            .and_then(|h| h.coherence_config.as_ref()),
        &base_dir,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let config_path = match &coherence_source {
        CoherenceSource::Explicit(p) => {
            println!("coherence config: {}", p.display());
            if session_header
                .as_ref()
                .and_then(|h| h.coherence_config.as_ref())
                .is_some_and(|snap| *snap != coherence_cfg.to_json())
            {
                println!("  (differs from the session's recorded config — offline retune)");
            }
            p.clone()
        }
        CoherenceSource::Snapshot => {
            println!("coherence config: session header snapshot");
            base_dir.join("config/coherence.toml")
        }
        CoherenceSource::DefaultFile(p) => {
            println!("coherence config: {}", p.display());
            p.clone()
        }
        CoherenceSource::Builtin(p) => {
            println!("coherence config: built-in defaults");
            p.clone()
        }
    };

    // --- run the pipeline ----------------------------------------------------
    let judgment_cfg = JudgmentConfig {
        lean_mode: parse_lean_mode(&args.lean_mode).map_err(|e| anyhow::anyhow!("{e}"))?,
        ..Default::default()
    };
    println!("lean mode: {}", lean_mode_name(judgment_cfg.lean_mode));
    let eval_for_judge =
        ChartEval::new(&chart, &conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
    let judge = Judge::new(
        eval_for_judge,
        Conductor::new(&manifest, None),
        judgment_cfg,
    );
    let coherence = Coherence::new(coherence_cfg);

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
        "lean_mode": lean_mode_name(judgment_cfg.lean_mode),
        "arrival_half_beats": judgment_cfg.arrival_half_beats,
        "coherence_config": coherence_cfg.to_json(),
    });
    let log = JsonlWriter::create(&out_path, &header).map_err(|e| anyhow::anyhow!("{e}"))?;

    // --- ladder: with --render, its presence switches to the reactive loop ----
    let default_ladder_path = base_dir.join("config/ladder.toml");
    let (ladder_cfg, ladder_path) = if let Some(path) = &args.ladder {
        let cfg = LadderConfig::load(Path::new(path))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("loading {path}"))?;
        println!("ladder config: {path}");
        (Some(cfg), PathBuf::from(path))
    } else if default_ladder_path.exists() {
        println!("ladder config: {}", default_ladder_path.display());
        (
            Some(
                LadderConfig::load(&default_ladder_path).map_err(|e| anyhow::anyhow!("{e}"))?,
            ),
            default_ladder_path,
        )
    } else {
        (None, default_ladder_path)
    };

    let mut ladder_cfg = ladder_cfg;
    let reactive = args.render.is_some() && ladder_cfg.is_some();
    if args.render.is_none() && ladder_cfg.is_some() {
        println!("(ladder inactive without --render; judgment replay is Milestone-2 semantics)");
    }
    let core_ladder = if reactive {
        ladder_cfg.take().unwrap()
    } else {
        LadderConfig::default()
    };
    // Gradient config (ADR 0024): explicit path must load, the default file
    // is optional, absent = inert built-ins (byte-identical to pre-gradient
    // renders). Applied only inside the reactive loop.
    let default_gradient_path = base_dir.join("config/gradient.toml");
    let (gradient_cfg, gradient_path) = if let Some(path) = &args.gradient {
        let cfg = GradientConfig::load(Path::new(path))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("loading {path}"))?;
        println!("gradient config: {path} (tune bus: {})", cfg.tune.bus);
        (cfg, PathBuf::from(path))
    } else if default_gradient_path.exists() {
        let cfg = GradientConfig::load(&default_gradient_path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        println!(
            "gradient config: {} (tune bus: {})",
            default_gradient_path.display(),
            cfg.tune.bus
        );
        (cfg, default_gradient_path)
    } else {
        (GradientConfig::default(), default_gradient_path)
    };

    let visual_eval = ChartEval::new(&chart, &conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut core = ChartCore::new(
        judge,
        coherence,
        log,
        Ladder::new(core_ladder),
        Reintegrator::new(manifest.reintegration.clone()),
        Conductor::new(&manifest, None),
        config_path,
        ladder_path,
        GradientDriver::new(gradient_cfg),
        gradient_path,
        visual_eval,
    );
    core.sync_seam_params();
    if reactive {
        let wav = args.render.as_ref().unwrap();
        return run_reactive(
            &manifest,
            &base_dir,
            wav,
            core,
            &pairs,
            synthetic_stamps,
            &out_path,
        );
    }

    let mut last_bar = i64::MIN;
    for ev in &events {
        core.ingest(ev);
        core.process(ev.sample()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let bar = conductor.position_at_sample(ev.sample()).bar;
        if bar != last_bar {
            last_bar = bar;
            println!(
                "bar {:>3} | {}",
                bar + 1,
                coherence_meter(core.coherence().value())
            );
        }
    }
    let final_sample = events.last().map(|e| e.sample()).unwrap_or(0);
    core.judge_finish();
    core.process(final_sample).map_err(|e| anyhow::anyhow!("{e}"))?;
    core.flush_log().map_err(|e| anyhow::anyhow!("{e}"))?;

    let s = core.summary();
    println!(
        "done. pulses hit {} (mean |err| {:.1} ms), missed {}, spurious {} | final {}",
        s.hits,
        s.mean_abs_err_ms,
        s.misses,
        s.spurious,
        coherence_meter(core.coherence().value())
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
/// sequencer → mixer loop the live harness runs — [`ChartCore`], literally —
/// inside the deterministic offline renderer (ADR 0006/0014). A recorded or
/// synthetic fall renders its full-fail seams and reassemblies to WAV,
/// bit-identically.
fn run_reactive(
    manifest: &SuiteManifest,
    base_dir: &Path,
    wav: &str,
    mut core: ChartCore,
    pairs: &[(i64, flint_music::input_stream::InputEvent)],
    synthetic_stamps: bool,
    out_path: &Path,
) -> Result<()> {
    let sample_rate = manifest.sample_rate;
    let last_raw = pairs.last().map(|(r, _)| *r).unwrap_or(0);
    // Generous trailing margin: a fall that begins at the last event still
    // needs to decay, hold, seam, and reassemble — and a fully absent player
    // loops, which is worth hearing a few times.
    let cfg = OfflineRenderConfig {
        duration_samples: last_raw.max(1) + 30 * sample_rate as i64,
        ..Default::default()
    };
    let script = EventScript {
        schema_version: 0,
        events: vec![],
    };

    let mut fails = 0u64;
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
                        core.ingest(&suite_ev);
                    }
                }
                let now_suite = session.clock_sample();
                core.advance_to(now_suite);
                core.process(now_suite).map_err(|e| anyhow::anyhow!("{e}"))?;

                let (_seq, seq_events) =
                    core.step_seq(session, pos).map_err(|e| anyhow::anyhow!("{e}"))?;
                for ev in &seq_events {
                    if let ReintegrationEvent::FullFail {
                        at_suite_sample,
                        re_entry_sample,
                        seam_suite_sample,
                    } = ev
                    {
                        fails += 1;
                        println!(
                            "FULL FAIL at sample {at_suite_sample}: reintegrating to \
                             {re_entry_sample} (seam at {seam_suite_sample})"
                        );
                    }
                }

                if pos.bar != last_bar && pos.sample >= 0 {
                    last_bar = pos.bar;
                    let rung = match core.ladder().rung() {
                        Some(r) => format!(" | rung {} ({})", core.ladder().level(), r.name),
                        None => String::new(),
                    };
                    println!(
                        "bar {:>3} | {}{rung}",
                        pos.bar + 1,
                        coherence_meter(core.coherence().value())
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

    let s = core.finish().map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "done. pulses hit {} (mean |err| {:.1} ms), missed {}, spurious \
         {} | {fails} full-fail(s) | final {}",
        s.hits,
        s.mean_abs_err_ms,
        s.misses,
        s.spurious,
        coherence_meter(core.coherence().value())
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
