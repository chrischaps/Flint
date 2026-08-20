//! Play a suite live: validate the manifest, start every bus sample-locked on
//! the session's sample-tick clock, and print the Conductor's musical-time
//! readout (latency-compensated using the newest measurement in
//! `logs/latency/`).

use anyhow::{bail, Context, Result};
use flint_music::session::RealtimePlayer;
use flint_music::status::status_line;
use flint_music::{validate_manifest, validate_manifest_assets, SuiteManifest};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flint_music::chart_session::latest_latency_ms;

#[derive(clap::Args)]
pub struct PlaySuiteArgs {
    /// Path to the suite manifest (.suite.toml)
    pub manifest: String,

    /// Directory the manifest's file paths are relative to (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,

    /// Stop after this many bars (default: play to the end)
    #[arg(long)]
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
        bail!(
            "manifest failed validation ({} issue(s)); not playing",
            issues.len()
        );
    }

    // --- latency report ------------------------------------------------------
    let latency_ms = match latest_latency_ms(&base_dir) {
        Some((file, ms)) => {
            println!("measured output latency: {ms:.1} ms (compensating; from {file})");
            Some(ms)
        }
        None => {
            println!(
                "measured output latency: NONE ON RECORD — run spikes/latency_harness \
                 and commit its log to logs/latency/"
            );
            None
        }
    };

    // --- play -----------------------------------------------------------------
    let anchor = &manifest.tempo[0];
    println!(
        "suite '{}': {} playable buses, {} BPM {}/{} at anchor 0 ({} tempo anchors), {} sections",
        manifest.id,
        manifest.buses.values().filter(|b| b.file.is_some()).count(),
        anchor.bpm,
        anchor.beats_per_bar,
        anchor.beat_unit,
        manifest.tempo.len(),
        manifest.sections.len()
    );

    let mut player = RealtimePlayer::start(&manifest, &base_dir, latency_ms)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let end_sample = args
        .bars
        .map(|b| player.session.conductor.sample_at_bar(b as i64, 0.0))
        .unwrap_or(i64::MAX);

    let mut max_skew = 0.0f64;
    let mut last_bar = i64::MIN;
    loop {
        std::thread::sleep(Duration::from_millis(100));
        player.session.pump();
        let pos = player.session.now();
        if pos.sample < 0 {
            continue; // pre-roll
        }
        if pos.sample >= end_sample {
            break;
        }
        max_skew = max_skew.max(player.session.mixer.max_stem_skew());

        if pos.bar != last_bar {
            last_bar = pos.bar;
            let section = player.session.conductor.section_at_sample(pos.sample);
            println!(
                "{} | max stem skew {:.3} ms",
                status_line(&pos, section, &player.session.mixer),
                max_skew * 1000.0
            );
        }

        if player.session.mixer.all_stopped() {
            break;
        }
    }

    println!(
        "done. max pairwise stem skew observed: {:.3} ms",
        max_skew * 1000.0
    );
    Ok(())
}
