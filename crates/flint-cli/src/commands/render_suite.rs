//! Render a scripted suite session to WAV, offline and deterministic — the
//! agent-facing "programmatic ears". Validates the manifest, runs the event
//! script through the same `SuiteSession` used live, and writes 32-bit float
//! stereo. Prints the same status lines as `play-suite` so offline and
//! realtime evidence are textually comparable.

use anyhow::{bail, Context, Result};
use flint_music::event_script::EventScript;
use flint_music::offline::{render_offline, write_wav, OfflineRenderConfig};
use flint_music::session::FileStems;
use flint_music::status::status_line;
use flint_music::{validate_manifest, validate_manifest_assets, Conductor, SuiteManifest};
use std::path::{Path, PathBuf};

pub struct RenderSuiteArgs {
    pub manifest: String,
    pub script: Option<String>,
    pub output: String,
    pub base_dir: Option<String>,
    pub duration_bars: Option<i64>,
    pub duration_seconds: Option<f64>,
    pub status_every: Option<String>,
    pub chunk_frames: usize,
}

pub fn run(args: RenderSuiteArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // --- validate first, same contract as play-suite -------------------------
    let manifest = SuiteManifest::load(Path::new(&args.manifest))
        .with_context(|| format!("loading {}", args.manifest))?;
    let mut issues = validate_manifest(&manifest);
    issues.extend(validate_manifest_assets(&manifest, &base_dir));
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("{i}");
        }
        bail!(
            "manifest failed validation ({} issue(s)); not rendering",
            issues.len()
        );
    }

    let script = match &args.script {
        Some(path) => EventScript::load(Path::new(path))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("loading event script {path}"))?,
        None => EventScript {
            schema_version: 0,
            events: vec![],
        },
    };

    // --- duration: explicit bars/seconds, else the longest stem --------------
    let conductor = Conductor::new(&manifest, None);
    let duration_samples = if let Some(bars) = args.duration_bars {
        if bars < 1 {
            bail!("--duration-bars must be >= 1");
        }
        conductor.sample_at_bar(bars, 0.0)
    } else if let Some(secs) = args.duration_seconds {
        (secs * manifest.sample_rate as f64).round() as i64
    } else {
        stem_duration_samples(&manifest, &base_dir)
            .context("no --duration-bars/--duration-seconds and no probeable stems")?
    };

    let status_bars = match args.status_every.as_deref() {
        None | Some("bar") => true,
        Some("beat") => false,
        Some(other) => bail!("--status-every must be 'bar' or 'beat', got '{other}'"),
    };

    println!(
        "rendering suite '{}': {} samples ({:.1} s) at {} Hz, {} scripted events, chunk {} frames",
        manifest.id,
        duration_samples,
        duration_samples as f64 / manifest.sample_rate as f64,
        manifest.sample_rate,
        script.events.len(),
        args.chunk_frames,
    );

    let cfg = OfflineRenderConfig {
        duration_samples,
        chunk_frames: args.chunk_frames,
    };
    let mut last_line = (i64::MIN, i64::MIN); // (bar, whole beat)
    let stems = FileStems::new(&base_dir);
    let result = render_offline(&manifest, &stems, &script, &cfg, |pos, mixer| {
        if pos.sample < 0 {
            return;
        }
        let key = (pos.bar, if status_bars { 0 } else { pos.beat_in_bar as i64 });
        if key != last_line {
            last_line = key;
            let section = conductor.section_at_sample(pos.sample);
            println!("{}", status_line(pos, section, mixer));
        }
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    for f in &result.fired {
        println!(
            "fired @ sample {:>9}{}: {:?} {:?}",
            f.sample,
            if f.late { " (LATE)" } else { "" },
            f.bus.as_deref().unwrap_or("-"),
            f.action
        );
    }
    let out_path = Path::new(&args.output);
    write_wav(out_path, &result.samples, manifest.sample_rate)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "wrote {} ({} frames, {:.1} s)",
        out_path.display(),
        result.samples.len() / 2,
        (result.samples.len() / 2) as f64 / manifest.sample_rate as f64
    );
    Ok(())
}

/// Duration of the longest playable stem, via the probe (also what the
/// validator uses, so this agrees with the validated durations).
fn stem_duration_samples(manifest: &SuiteManifest, base_dir: &Path) -> Option<i64> {
    manifest
        .buses
        .values()
        .filter_map(|decl| decl.file.as_ref())
        .filter_map(|file| flint_music::probe::probe_audio(&base_dir.join(file)).ok())
        .map(|info| info.frames as i64)
        .max()
}
