//! Milestone 0 command: validate a suite manifest, play its stems
//! sample-locked from one clock, print a musical-time readout, and report the
//! measured output latency from the latency harness logs.

use anyhow::{bail, Context, Result};
use flint_music::playback::SuitePlayer;
use flint_music::{validate_manifest, validate_manifest_assets, SuiteManifest};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct PlaySuiteArgs {
    pub manifest: String,
    pub base_dir: Option<String>,
    pub bars: Option<u64>,
}

pub fn run(args: PlaySuiteArgs) -> Result<()> {
    let base_dir = args
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // --- validate first: playback only ever runs on a clean manifest --------
    let manifest = SuiteManifest::load(Path::new(&args.manifest))
        .with_context(|| format!("loading {}", args.manifest))?;
    let mut issues = validate_manifest(&manifest);
    issues.extend(validate_manifest_assets(&manifest, &base_dir));
    if !issues.is_empty() {
        for i in &issues {
            eprintln!("{i}");
        }
        bail!("manifest failed validation ({} issue(s)); not playing", issues.len());
    }

    // --- latency report ------------------------------------------------------
    match latest_latency_ms(&base_dir) {
        Some((file, ms)) => println!(
            "measured output latency: {ms:.1} ms (compensation seed; from {file})"
        ),
        None => println!(
            "measured output latency: NONE ON RECORD — run spikes/latency_harness \
             and commit its log to logs/latency/"
        ),
    }

    // --- play -----------------------------------------------------------------
    let anchor = &manifest.tempo[0];
    let beats_per_bar = anchor.beats_per_bar as f64;
    println!(
        "suite '{}': {} playable buses, {} BPM {}/{}, {} sections",
        manifest.id,
        manifest.buses.values().filter(|b| b.file.is_some()).count(),
        anchor.bpm,
        anchor.beats_per_bar,
        anchor.beat_unit,
        manifest.sections.len()
    );

    let player = SuitePlayer::start(&manifest, &base_dir)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let spb = manifest.sample_rate as f64 * 60.0 / anchor.bpm;
    let total_beats = args
        .bars
        .map(|b| (b as f64) * beats_per_bar)
        .unwrap_or(f64::MAX);

    let mut max_skew = 0.0f64;
    let mut last_bar = 0i64;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let beats = player.beats();
        if beats < 0.0 {
            continue; // pre-roll
        }
        if beats >= total_beats {
            break;
        }
        max_skew = max_skew.max(player.max_stem_skew());

        let bar = (beats / beats_per_bar) as i64 + 1;
        if bar != last_bar {
            last_bar = bar;
            let sample = (beats * spb) as i64;
            let section = manifest
                .sections
                .iter()
                .rev()
                .find(|s| s.start_sample <= sample)
                .map(|s| s.name.as_str())
                .unwrap_or("?");
            let beat_in_bar = (beats % beats_per_bar) as i64 + 1;
            println!(
                "bar {bar:>3} beat {beat_in_bar} | section {section:<10} | sample {sample:>9} | max stem skew {:.3} ms",
                max_skew * 1000.0
            );
        }

        // stop when every stem has finished
        if player.all_stopped() {
            break;
        }
    }

    println!(
        "done. max pairwise stem skew observed: {:.3} ms",
        max_skew * 1000.0
    );
    Ok(())
}

/// Newest measurement in `<base_dir>/logs/latency/` (files are named
/// `latency-<date>-<epoch>.toml`, so lexicographic max = newest). Returns the
/// loopback mean, falling back to the driver-reported mean.
fn latest_latency_ms(base_dir: &Path) -> Option<(String, f64)> {
    let dir = base_dir.join("logs/latency");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("latency-") && n.ends_with(".toml"))
        .collect();
    names.sort();
    let name = names.pop()?;
    let value: toml::Value = std::fs::read_to_string(dir.join(&name)).ok()?.parse().ok()?;
    let mean = |table: &str| {
        value
            .get(table)
            .and_then(|t| t.get("mean"))
            .and_then(flint_core::toml_util::toml_f64)
            .filter(|m| m.is_finite())
    };
    let ms = mean("loopback_ms").or_else(|| mean("driver_reported_ms"))?;
    Some((name, ms))
}
