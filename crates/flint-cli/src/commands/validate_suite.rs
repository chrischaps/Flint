//! Musical suite manifest + beatmap chart validation command.
//!
//! Cross-checks a suite manifest against the pinned schema (structural pass),
//! optionally a beatmap chart against the manifest, and — unless `--no-assets`
//! — the stem files on disk (existence, sample rate, equal duration).

use anyhow::{Context, Result};
use flint_music::{
    validate_chart, validate_manifest, validate_manifest_assets, Chart, Issue, SuiteManifest,
};
use std::path::{Path, PathBuf};

pub struct ValidateSuiteArgs {
    pub manifest: String,
    pub chart: Option<String>,
    pub no_assets: bool,
    pub base_dir: Option<String>,
}

pub fn run(args: ValidateSuiteArgs) -> Result<()> {
    let manifest_path = Path::new(&args.manifest);
    let manifest = SuiteManifest::load(manifest_path)
        .with_context(|| format!("loading suite manifest {}", args.manifest))?;

    let mut issues: Vec<(String, Issue)> = Vec::new();
    let mut record = |scope: &str, found: Vec<Issue>| {
        issues.extend(found.into_iter().map(|i| (scope.to_string(), i)));
    };

    record("manifest", validate_manifest(&manifest));

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
            "OK: suite '{}' ({} sections, {} tempo anchors{})",
            manifest.id,
            manifest.sections.len(),
            manifest.tempo.len(),
            if args.chart.is_some() { ", chart valid" } else { "" }
        );
        Ok(())
    } else {
        for (scope, issue) in &issues {
            eprintln!("{scope}: {issue}");
        }
        anyhow::bail!("{} validation issue(s)", issues.len());
    }
}
