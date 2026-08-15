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
use flint_music::replay::{read_session, synthesize, SyntheticProfile};
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
    pub render: Option<String>,
    pub base_dir: Option<String>,
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

    let (events, session_header) = match (&args.session, &args.synthetic) {
        (Some(path), None) => {
            let (header, events) = read_session(Path::new(path))
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
                events.len(),
                header.latency_ms,
                header.calibration_ms
            );
            (events, Some(header))
        }
        (None, Some(profile)) => {
            let profile = SyntheticProfile::parse(profile).map_err(|e| anyhow::anyhow!("{e}"))?;
            let events = synthesize(&eval, &conductor, profile);
            println!("synthetic session: {profile:?} ({} event(s))", events.len());
            (events, None)
        }
        _ => bail!("exactly one of --session or --synthetic is required"),
    };

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
    let judgment_cfg = JudgmentConfig::default();
    let eval_for_judge =
        ChartEval::new(&chart, &conductor).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut judge = Judge::new(
        eval_for_judge,
        Conductor::new(&manifest, None),
        judgment_cfg,
    );
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
        "coherence_config": coherence_cfg.to_json(),
    });
    let mut log = JsonlWriter::create(&out_path, &header).map_err(|e| anyhow::anyhow!("{e}"))?;

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
            let value = coherence.step(records, judgment_cfg.grid_beats, beats_per_bar);
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

    // --- optional audio render over the same span ----------------------------
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
