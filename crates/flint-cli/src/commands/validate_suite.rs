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
//! A build without the `rhythm` package (#1333) checks a music suite and
//! refuses a rhythm one, exit 2, rather than pass it half-checked.

use anyhow::{Context, Result};
use flint_music::{validate_manifest_assets, Issue, SuiteManifest};
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
    let rhythm = args.rhythm || args.chart.is_some() || rhythm::declared(&manifest);
    record(
        "manifest",
        if rhythm {
            rhythm::validate_manifest(&manifest)?
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
        record(
            "chart",
            rhythm::validate_chart(Path::new(chart_path), &manifest)?,
        );
    }

    crate::output::when_json(|| {
        serde_json::json!({
            "suite": manifest.id,
            "valid": issues.is_empty(),
            "rhythm": rhythm,
            "sections": manifest.sections.len(),
            "tempo_anchors": manifest.tempo.len(),
            "buses": manifest.buses.len(),
            "issues": issues.iter().map(|(scope, issue)| {
                serde_json::json!({ "scope": scope, "message": issue.to_string() })
            }).collect::<Vec<_>>(),
        })
    });
    if issues.is_empty() {
        crate::output::say!(
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
        eprintln!("{} validation issue(s)", issues.len());
        Err(crate::output::failed())
    }
}

/// The rhythm layer's half of the check: its contract and the chart.
#[cfg(feature = "rhythm")]
mod rhythm {
    use anyhow::{Context, Result};
    use flint_music::{Issue, SuiteManifest};
    use std::path::Path;

    pub fn declared(manifest: &SuiteManifest) -> bool {
        flint_rhythm::Reintegration::declared(manifest)
    }

    pub fn validate_manifest(manifest: &SuiteManifest) -> Result<Vec<Issue>> {
        Ok(flint_rhythm::validate_manifest(manifest))
    }

    pub fn validate_chart(path: &Path, manifest: &SuiteManifest) -> Result<Vec<Issue>> {
        let chart = flint_rhythm::Chart::load(path)
            .with_context(|| format!("loading chart {}", path.display()))?;
        Ok(flint_rhythm::validate_chart(&chart, manifest))
    }
}

/// Built without the rhythm package: a rhythm suite, `--rhythm` and
/// `--chart` are refused, naming it.
#[cfg(not(feature = "rhythm"))]
mod rhythm {
    use crate::commands::absent::missing;
    use anyhow::Result;
    use flint_music::{Issue, SuiteManifest};
    use std::path::Path;

    /// flint-rhythm's `Reintegration::TABLE`: the table that makes a suite
    /// a rhythm suite.
    const TABLE: &str = "reintegration";

    pub fn declared(manifest: &SuiteManifest) -> bool {
        manifest.tables.contains_key(TABLE)
    }

    pub fn validate_manifest(_: &SuiteManifest) -> Result<Vec<Issue>> {
        Err(missing(
            "flint music validate on a rhythm suite ([reintegration], --rhythm or --chart)",
            "rhythm",
        ))
    }

    pub fn validate_chart(_: &Path, _: &SuiteManifest) -> Result<Vec<Issue>> {
        Err(missing("flint music validate --chart", "rhythm"))
    }
}
