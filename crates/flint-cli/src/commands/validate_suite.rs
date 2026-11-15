//! Musical suite manifest + beatmap chart validation command.
//!
//! Cross-checks a suite manifest against the schema (structural pass),
//! optionally a beatmap chart against the manifest, and — unless `--no-assets`
//! — the stem files on disk (existence, sample rate, equal duration).
//!
//! A suite that only plays music (any buses, no `[reintegration]`) is held
//! to flint-music's contract. A rhythm suite, one that declares
//! `[reintegration]` or is given a `--chart` (or `--rhythm`), is also held to
//! flint-rhythm's: the six buses, the seam, a judgment window per section.

use anyhow::{Context, Result};
use flint_music::{validate_manifest_assets, Issue, SuiteManifest};
use flint_rhythm::{validate_chart, Chart};
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct ValidateSuiteArgs {
    /// Path to the suite manifest (.suite.toml)
    pub manifest: String,

    /// Beatmap chart (.chart.toml) to cross-check against the manifest
    #[arg(long)]
    pub chart: Option<String>,

    /// Skip the asset pass (stem file existence/sample-rate/duration)
    #[arg(long)]
    pub no_assets: bool,

    /// Directory the manifest's file paths are relative to (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,

    /// Hold the suite to the rhythm layer's contract even without a
    /// `[reintegration]` table or a chart (a rhythm game's fixtures)
    #[arg(long)]
    pub rhythm: bool,
}

pub fn run(args: ValidateSuiteArgs) -> Result<()> {
    let manifest_path = Path::new(&args.manifest);
    let manifest = SuiteManifest::load(manifest_path)
        .with_context(|| format!("loading suite manifest {}", args.manifest))?;

    let mut issues: Vec<(String, Issue)> = Vec::new();
    let mut record = |scope: &str, found: Vec<Issue>| {
        issues.extend(found.into_iter().map(|i| (scope.to_string(), i)));
    };

    // The rhythm contract runs the general pass first
    let rhythm = args.rhythm || args.chart.is_some() || manifest.reintegration.is_some();
    record(
        "manifest",
        if rhythm {
            flint_rhythm::validate_manifest(&manifest)
        } else {
            flint_music::validate_manifest(&manifest)
        },
    );

    if !args.no_assets {
        // Manifest file paths are relative to the game repo root; default the
        // base dir to the current working directory.
        let base_dir = args
            .base_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        record("assets", validate_manifest_assets(&manifest, &base_dir));
    }

    if let Some(chart_path) = &args.chart {
        let chart = Chart::load(Path::new(chart_path))
            .with_context(|| format!("loading chart {chart_path}"))?;
        record("chart", validate_chart(&chart, &manifest));
    }

    if issues.is_empty() {
        println!(
            "OK: suite '{}' ({} sections, {} tempo anchors, {} buses{}{})",
            manifest.id,
            manifest.sections.len(),
            manifest.tempo.len(),
            manifest.buses.len(),
            if rhythm { ", rhythm contract" } else { "" },
            if args.chart.is_some() {
                ", chart valid"
            } else {
                ""
            }
        );
        Ok(())
    } else {
        for (scope, issue) in &issues {
            eprintln!("{scope}: {issue}");
        }
        anyhow::bail!("{} validation issue(s)", issues.len());
    }
}
